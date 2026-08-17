//! Deterministic Android evaluation contracts.
//!
//! Evaluation success is derived from independently observable Android state,
//! never from an agent's self-reported completion message.

use crate::model::SnapshotV1;
use serde::Serialize;
use serde_json::json;

pub const EVAL_SCHEMA_V1: &str = "tempera.android.evals/v1";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalCaseV1 {
    pub id: &'static str,
    pub population: &'static str,
    pub setup: &'static str,
    pub expected_package: &'static str,
    pub exact_present: &'static [&'static str],
    pub exact_absent: &'static [&'static str],
    pub max_steps: u32,
}

const CASES: &[EvalCaseV1] = &[
    EvalCaseV1 {
        id: "fixture.wifi-multiscreen",
        population: "synthetic_fixture",
        setup: "tempera-bridge-fixture",
        expected_package: "dev.tempera.android.bridge",
        exact_present: &["Wi-Fi lab complete", "Fixture state: complete"],
        exact_absent: &[],
        max_steps: 20,
    },
    EvalCaseV1 {
        id: "fixture.profile-text-entry",
        population: "synthetic_fixture",
        setup: "tempera-bridge-fixture",
        expected_package: "dev.tempera.android.bridge",
        exact_present: &["Profile saved", "Lengths: name=3, note=5"],
        exact_absent: &[],
        max_steps: 20,
    },
    EvalCaseV1 {
        id: "fixture.dialog-multiwindow",
        population: "synthetic_fixture",
        setup: "tempera-bridge-fixture",
        expected_package: "dev.tempera.android.bridge",
        exact_present: &["Dialog accepted", "Fixture state: complete"],
        exact_absent: &[],
        max_steps: 20,
    },
    EvalCaseV1 {
        id: "fixture.long-scroll",
        population: "synthetic_fixture",
        setup: "tempera-bridge-fixture",
        expected_package: "dev.tempera.android.bridge",
        exact_present: &["Long list complete", "Fixture state: complete"],
        exact_absent: &[],
        max_steps: 28,
    },
    EvalCaseV1 {
        id: "android.open-settings",
        population: "android_settings",
        setup: "home",
        expected_package: "com.android.settings",
        exact_present: &[],
        exact_absent: &[],
        max_steps: 12,
    },
];

pub fn cases() -> &'static [EvalCaseV1] {
    CASES
}

pub fn case(id: &str) -> Option<&'static EvalCaseV1> {
    CASES.iter().find(|candidate| candidate.id == id)
}

pub fn evaluate(case: &EvalCaseV1, snapshot: &SnapshotV1) -> serde_json::Value {
    let labels = snapshot
        .nodes
        .iter()
        .filter_map(|node| (!node.label.is_empty()).then_some(normalize(&node.label)))
        .collect::<Vec<_>>();
    let mut checks = serde_json::Map::new();
    checks.insert(
        "package".to_string(),
        json!(snapshot.package == case.expected_package),
    );
    for expected in case.exact_present {
        checks.insert(
            format!("present:{expected}"),
            json!(labels.iter().any(|label| label == &normalize(expected))),
        );
    }
    for absent in case.exact_absent {
        checks.insert(
            format!("absent:{absent}"),
            json!(!labels.iter().any(|label| label == &normalize(absent))),
        );
    }
    let success = checks.values().all(|value| value == &json!(true));
    json!({
        "schemaVersion": EVAL_SCHEMA_V1,
        "case": case,
        "success": success,
        "grader": {
            "type": "deterministic_android_end_state",
            "checks": checks,
            "finalStateHash": snapshot.state_hash,
            "finalRevision": snapshot.revision,
            "finalPackage": snapshot.package,
        },
        "claimScope": "local regression and engineering evidence only; not an official capability claim",
    })
}

fn normalize(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NodeV1, RectV1};

    #[test]
    fn verifier_uses_observed_state_not_agent_output() {
        let case = case("fixture.wifi-multiscreen").unwrap();
        let snapshot = SnapshotV1 {
            schema_version: "v".to_string(),
            session_id: "s".to_string(),
            serial: "e".to_string(),
            target_kind: "emulator".to_string(),
            package: "dev.tempera.android.bridge".to_string(),
            activity: "x".to_string(),
            screen: [1, 1],
            revision: 1,
            state_hash: "sha256:x".to_string(),
            captured_at_ms: 0,
            nodes: vec![
                NodeV1 {
                    reference: "@e0".to_string(),
                    backend_reference: None,
                    role: "Text".to_string(),
                    label: "Wi-Fi lab complete".to_string(),
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
                    clickable: false,
                    editable: false,
                    scrollable: false,
                    password: false,
                    actions: vec![],
                },
                NodeV1 {
                    reference: "@e1".to_string(),
                    backend_reference: None,
                    role: "Text".to_string(),
                    label: "Fixture state: complete".to_string(),
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
                    clickable: false,
                    editable: false,
                    scrollable: false,
                    password: false,
                    actions: vec![],
                },
            ],
        };
        assert_eq!(evaluate(case, &snapshot)["success"], true);
    }
}
