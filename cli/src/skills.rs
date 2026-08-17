//! Privacy-bounded, opt-in navigation skill cache.
//!
//! Skills deliberately contain no task text, secrets, typed values, screen
//! references, or coordinates. They cache only guarded, non-consequential
//! semantic navigation programs which are freshly grounded before replay.

use crate::error::{AndroidError, Result};
use crate::model::{next_action_id, ActionV1, SnapshotV1};
use crate::session::SessionStore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

const SCHEMA: &str = "tempera.android.skills/v1";
const MAX_SKILLS: usize = 128;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillV1 {
    pub id: String,
    pub task_sha256: String,
    pub start_guard: GuardV1,
    pub program: Vec<SkillActionV1>,
    pub completion: CompletionV1,
    pub learned_at_ms: u128,
    pub successes: u32,
    pub failures: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuardV1 {
    pub package: String,
    pub activity: String,
    pub contains: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillActionV1 {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionV1 {
    pub package: String,
    pub exact: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SkillFileV1 {
    schema_version: String,
    skills: Vec<SkillV1>,
}

#[derive(Debug, Clone)]
pub struct SkillStore {
    path: PathBuf,
}

impl SkillStore {
    pub fn from_environment() -> Result<Self> {
        let store = SessionStore::from_environment()?;
        Ok(Self {
            path: store.root().join("skills.json"),
        })
    }

    pub fn list(&self) -> Result<Vec<SkillV1>> {
        let mut skills = self.load()?;
        skills.sort_by(|left, right| {
            (
                right.successes as i64 - right.failures as i64,
                right.learned_at_ms,
            )
                .cmp(&(
                    left.successes as i64 - left.failures as i64,
                    left.learned_at_ms,
                ))
        });
        Ok(skills)
    }

    pub fn clear(&self, confirmed: bool) -> Result<bool> {
        if !confirmed {
            return Err(AndroidError::InvalidInput(
                "skills clear removes the local cache; rerun with --yes".to_string(),
            ));
        }
        if self.path.is_file() {
            fs::remove_file(&self.path)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn candidates(&self, task: &str, snapshot: &SnapshotV1) -> Result<Vec<SkillV1>> {
        let digest = task_digest(task);
        Ok(self
            .load()?
            .into_iter()
            .filter(|skill| {
                skill.task_sha256 == digest && guard_matches(&skill.start_guard, snapshot)
            })
            .collect())
    }

    pub fn learn(
        &self,
        task: &str,
        start: &SnapshotV1,
        actions: &[ActionV1],
        completion: CompletionV1,
    ) -> Result<Option<SkillV1>> {
        let program = portable_program(actions, start)?;
        if program.is_empty() || completion.exact.is_empty() || completion.package.is_empty() {
            return Ok(None);
        }
        let first_selector = program.iter().find_map(|action| action.selector.clone());
        let guard = GuardV1 {
            package: start.package.clone(),
            activity: start.activity.clone(),
            contains: first_selector.into_iter().collect(),
        };
        if !guard_matches(&guard, start) {
            return Ok(None);
        }
        let digest = task_digest(task);
        let identity = serde_json::to_vec(&(&digest, &guard, &program, &completion))?;
        let id = hex::encode(Sha256::digest(identity))[..24].to_string();
        let mut all = self.load()?;
        if let Some(existing) = all.iter_mut().find(|skill| skill.id == id) {
            existing.successes = existing.successes.saturating_add(1);
            existing.learned_at_ms = SnapshotV1::now_ms();
            let skill = existing.clone();
            self.save(&all)?;
            return Ok(Some(skill));
        }
        let skill = SkillV1 {
            id,
            task_sha256: digest,
            start_guard: guard,
            program,
            completion,
            learned_at_ms: SnapshotV1::now_ms(),
            successes: 1,
            failures: 0,
        };
        all.push(skill.clone());
        if all.len() > MAX_SKILLS {
            all.sort_by_key(|entry| entry.learned_at_ms);
            all.drain(..all.len() - MAX_SKILLS);
        }
        self.save(&all)?;
        Ok(Some(skill))
    }

    pub fn record_success(&self, id: &str) -> Result<()> {
        self.update(id, true)
    }

    pub fn record_failure(&self, id: &str) -> Result<()> {
        self.update(id, false)
    }

    fn update(&self, id: &str, success: bool) -> Result<()> {
        let mut all = self.load()?;
        if let Some(skill) = all.iter_mut().find(|skill| skill.id == id) {
            if success {
                skill.successes = skill.successes.saturating_add(1);
            } else {
                skill.failures = skill.failures.saturating_add(1);
            }
            skill.learned_at_ms = SnapshotV1::now_ms();
            if skill.failures >= skill.successes.saturating_add(2).max(3) {
                all.retain(|candidate| candidate.id != id);
            }
            self.save(&all)?;
        }
        Ok(())
    }

    fn load(&self) -> Result<Vec<SkillV1>> {
        if !self.path.is_file() {
            return Ok(Vec::new());
        }
        let file: SkillFileV1 = serde_json::from_slice(&fs::read(&self.path)?)?;
        if file.schema_version != SCHEMA {
            return Err(AndroidError::InvalidInput(format!(
                "{} has unsupported schema",
                self.path.display()
            )));
        }
        Ok(file.skills.into_iter().filter(valid).collect())
    }

    fn save(&self, skills: &[SkillV1]) -> Result<()> {
        let file = SkillFileV1 {
            schema_version: SCHEMA.to_string(),
            skills: skills.to_vec(),
        };
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(&file)?)?;
        fs::rename(temporary, &self.path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}

pub fn replay_action(action: &SkillActionV1, snapshot: &SnapshotV1) -> Result<ActionV1> {
    if let Some(selector) = action.selector.as_deref() {
        if snapshot.node(selector).is_none() {
            return Err(AndroidError::InvalidInput(
                "skill start guard no longer matches a current selector".to_string(),
            ));
        }
    }
    Ok(ActionV1 {
        action_id: next_action_id("skill-action"),
        kind: action.kind.clone(),
        selector: action.selector.clone(),
        text: None,
        key: action.key.clone(),
        direction: action.direction.clone(),
        coordinates: None,
        expected_revision: Some(snapshot.revision),
        expected_state_hash: Some(snapshot.state_hash.clone()),
        metadata: BTreeMap::new(),
    })
}

pub fn completion_matches(completion: &CompletionV1, snapshot: &SnapshotV1) -> bool {
    completion.package == snapshot.package
        && completion.exact.iter().all(|expected| {
            snapshot.nodes.iter().any(|node| {
                node.label.eq_ignore_ascii_case(expected)
                    || node
                        .text
                        .as_deref()
                        .is_some_and(|text| text.eq_ignore_ascii_case(expected))
            })
        })
}

pub fn completion_from_evidence(
    package: &str,
    refs: &[String],
    exact: &[String],
    snapshot: &SnapshotV1,
) -> Option<CompletionV1> {
    let mut labels = exact.to_vec();
    for reference in refs {
        if let Some(node) = snapshot.node(reference) {
            if !node.label.is_empty() {
                labels.push(node.label.clone());
            }
        }
    }
    labels.sort();
    labels.dedup();
    (!package.is_empty() && !labels.is_empty()).then(|| CompletionV1 {
        package: package.to_string(),
        exact: labels,
    })
}

fn portable_program(actions: &[ActionV1], snapshot: &SnapshotV1) -> Result<Vec<SkillActionV1>> {
    if actions.is_empty() || actions.len() > 12 {
        return Ok(Vec::new());
    }
    actions
        .iter()
        .map(|action| {
            if !matches!(
                action.kind.as_str(),
                "tap" | "back" | "home" | "scroll" | "wait"
            ) || action.text.is_some()
                || action.coordinates.is_some()
            {
                return Err(AndroidError::InvalidInput(
                    "not a cacheable navigation action".to_string(),
                ));
            }
            let selector = action
                .selector
                .as_deref()
                .map(|selector| {
                    if selector.starts_with('@') {
                        snapshot
                            .node(selector)
                            .map(|node| node.label.clone())
                            .filter(|label| !label.is_empty())
                            .ok_or_else(|| {
                                AndroidError::InvalidInput(
                                    "cannot cache an unresolved semantic reference".to_string(),
                                )
                            })
                    } else {
                        Ok(selector.to_string())
                    }
                })
                .transpose()?;
            if selector.as_deref().is_some_and(is_sensitive) {
                return Err(AndroidError::InvalidInput(
                    "cannot cache a consequential navigation selector".to_string(),
                ));
            }
            Ok(SkillActionV1 {
                kind: action.kind.clone(),
                selector,
                key: action.key.clone(),
                direction: action.direction.clone(),
            })
        })
        .collect()
}

fn valid(skill: &SkillV1) -> bool {
    !skill.id.is_empty()
        && !skill.task_sha256.is_empty()
        && !skill.program.is_empty()
        && !skill.completion.exact.is_empty()
        && skill.program.iter().all(|action| {
            matches!(
                action.kind.as_str(),
                "tap" | "back" | "home" | "scroll" | "wait"
            ) && action
                .selector
                .as_deref()
                .is_none_or(|selector| !selector.starts_with('@') && !is_sensitive(selector))
        })
}
fn task_digest(task: &str) -> String {
    hex::encode(Sha256::digest(
        task.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase(),
    ))
}
fn guard_matches(guard: &GuardV1, snapshot: &SnapshotV1) -> bool {
    guard.package == snapshot.package
        && guard.activity == snapshot.activity
        && guard.contains.iter().all(|expected| {
            snapshot
                .nodes
                .iter()
                .any(|node| node.label.eq_ignore_ascii_case(expected))
        })
}
fn is_sensitive(value: &str) -> bool {
    [
        "send",
        "post",
        "publish",
        "buy",
        "purchase",
        "pay",
        "transfer",
        "delete",
        "subscribe",
        "book",
        "order",
        "submit",
    ]
    .iter()
    .any(|word| value.to_ascii_lowercase().contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NodeV1, RectV1, CONTROL_SCHEMA_V1};
    fn snapshot(label: &str) -> SnapshotV1 {
        SnapshotV1 {
            schema_version: CONTROL_SCHEMA_V1.to_string(),
            session_id: "s".to_string(),
            serial: "e".to_string(),
            target_kind: "emulator".to_string(),
            package: "demo".to_string(),
            activity: ".Main".to_string(),
            screen: [1, 1],
            revision: 1,
            state_hash: "sha256:x".to_string(),
            captured_at_ms: 0,
            nodes: vec![NodeV1 {
                reference: "@e0".to_string(),
                backend_reference: None,
                role: "Button".to_string(),
                label: label.to_string(),
                text: None,
                content_description: None,
                resource_id: None,
                bounds: RectV1 {
                    left: 0,
                    top: 0,
                    right: 1,
                    bottom: 1,
                },
                enabled: true,
                clickable: true,
                editable: false,
                scrollable: false,
                password: false,
                actions: vec![],
            }],
        }
    }
    #[test]
    fn skills_hash_tasks_and_never_store_task_text() {
        let directory = tempfile::tempdir().unwrap();
        let store = SkillStore {
            path: directory.path().join("skills.json"),
        };
        let secret_task = "Open private customer workspace";
        let action = ActionV1 {
            action_id: "a".to_string(),
            kind: "tap".to_string(),
            selector: Some("@e0".to_string()),
            text: None,
            key: None,
            direction: None,
            coordinates: None,
            expected_revision: Some(1),
            expected_state_hash: Some("sha256:x".to_string()),
            metadata: BTreeMap::new(),
        };
        store
            .learn(
                secret_task,
                &snapshot("Settings"),
                &[action],
                CompletionV1 {
                    package: "demo".to_string(),
                    exact: vec!["Settings".to_string()],
                },
            )
            .unwrap();
        let raw = fs::read_to_string(&store.path).unwrap();
        assert!(!raw.contains(secret_task));
        assert!(raw.contains(&task_digest(secret_task)));
    }
    #[test]
    fn sensitive_and_typed_actions_are_not_cacheable() {
        let typed = ActionV1 {
            action_id: "a".to_string(),
            kind: "type".to_string(),
            selector: Some("@e0".to_string()),
            text: Some("secret".to_string()),
            key: None,
            direction: None,
            coordinates: None,
            expected_revision: None,
            expected_state_hash: None,
            metadata: BTreeMap::new(),
        };
        assert!(portable_program(&[typed], &snapshot("Name")).is_err());
        let tap = ActionV1 {
            action_id: "a".to_string(),
            kind: "tap".to_string(),
            selector: Some("@e0".to_string()),
            text: None,
            key: None,
            direction: None,
            coordinates: None,
            expected_revision: None,
            expected_state_hash: None,
            metadata: BTreeMap::new(),
        };
        assert!(portable_program(&[tap], &snapshot("Buy now")).is_err());
    }
}
