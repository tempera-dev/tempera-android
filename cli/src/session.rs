use crate::error::{AndroidError, Result};
use crate::model::{ActionReceiptV1, SessionV1, SnapshotV1, CONTROL_SCHEMA_V1};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    pub fn from_environment() -> Result<Self> {
        let root = env::var_os("TEMPERA_ANDROID_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("HOME").map(|home| PathBuf::from(home).join(".tempera-android"))
            })
            .ok_or_else(|| {
                AndroidError::InvalidInput("Cannot determine TEMPERA_ANDROID_HOME".to_string())
            })?;
        Self::new(root)
    }

    pub fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(root.join("sessions"))?;
        fs::create_dir_all(root.join("state"))?;
        Ok(Self { root })
    }

    fn safe_id(id: &str) -> Result<&str> {
        if id.is_empty()
            || id.len() > 80
            || !id.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
            })
        {
            return Err(AndroidError::InvalidInput(
                "Session IDs may contain only letters, digits, '.', '_' and '-'".to_string(),
            ));
        }
        Ok(id)
    }

    fn path(&self, id: &str) -> Result<PathBuf> {
        Ok(self
            .root
            .join("sessions")
            .join(format!("{}.json", Self::safe_id(id)?)))
    }

    pub fn load(&self, id: &str) -> Result<Option<SessionV1>> {
        let path = self.path(id)?;
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_slice(&fs::read(path)?)?))
    }

    pub fn get_or_create(&self, id: &str, serial: &str, transport: &str) -> Result<SessionV1> {
        if let Some(existing) = self.load(id)? {
            if existing.serial != serial {
                return Err(AndroidError::InvalidInput(format!(
                    "Session {id:?} is bound to {}, not {serial}; use a different --session",
                    existing.serial
                )));
            }
            return Ok(existing);
        }
        let now = SnapshotV1::now_ms();
        let target_kind = if serial.starts_with("emulator-") {
            "emulator"
        } else {
            "device"
        };
        let session = SessionV1 {
            schema_version: CONTROL_SCHEMA_V1.to_string(),
            session_id: id.to_string(),
            serial: serial.to_string(),
            target_kind: target_kind.to_string(),
            transport: transport.to_string(),
            created_at_ms: now,
            updated_at_ms: now,
            last_revision: 0,
            last_state_hash: None,
        };
        self.save(&session)?;
        Ok(session)
    }

    pub fn save(&self, session: &SessionV1) -> Result<()> {
        let path = self.path(&session.session_id)?;
        atomic_json(&path, session)
    }

    pub fn save_snapshot(&self, session_id: &str, snapshot: &SnapshotV1) -> Result<()> {
        Self::safe_id(session_id)?;
        atomic_json(
            &self
                .root
                .join("state")
                .join(format!("{session_id}.snapshot.json")),
            snapshot,
        )
    }

    pub fn snapshot(&self, session_id: &str) -> Result<Option<SnapshotV1>> {
        Self::safe_id(session_id)?;
        let path = self
            .root
            .join("state")
            .join(format!("{session_id}.snapshot.json"));
        if path.is_file() {
            Ok(Some(serde_json::from_slice(&fs::read(path)?)?))
        } else {
            Ok(None)
        }
    }

    pub fn save_receipts(&self, session_id: &str, receipts: &[ActionReceiptV1]) -> Result<()> {
        Self::safe_id(session_id)?;
        atomic_json(
            &self
                .root
                .join("state")
                .join(format!("{session_id}.receipts.json")),
            receipts,
        )
    }

    pub fn receipts(&self, session_id: &str) -> Result<Vec<ActionReceiptV1>> {
        Self::safe_id(session_id)?;
        let path = self
            .root
            .join("state")
            .join(format!("{session_id}.receipts.json"));
        if path.is_file() {
            Ok(serde_json::from_slice(&fs::read(path)?)?)
        } else {
            Ok(Vec::new())
        }
    }

    pub fn list(&self) -> Result<Vec<SessionV1>> {
        let mut sessions: Vec<SessionV1> = Vec::new();
        for entry in fs::read_dir(self.root.join("sessions"))? {
            let entry = entry?;
            if entry.path().extension().and_then(|value| value.to_str()) == Some("json") {
                sessions.push(serde_json::from_slice(&fs::read(entry.path())?)?);
            }
        }
        sessions.sort_by(|left, right| left.session_id.cmp(&right.session_id));
        Ok(sessions)
    }

    pub fn remove(&self, id: &str) -> Result<bool> {
        let path = self.path(id)?;
        if path.exists() {
            fs::remove_file(path)?;
            for suffix in ["snapshot", "receipts"] {
                let related = self.root.join("state").join(format!("{id}.{suffix}.json"));
                if related.is_file() {
                    fs::remove_file(related)?;
                }
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn atomic_json(path: &Path, value: &(impl serde::Serialize + ?Sized)) -> Result<()> {
    let data = serde_json::to_vec_pretty(value)?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, data)?;
    fs::rename(temporary, path)?;
    Ok(())
}
