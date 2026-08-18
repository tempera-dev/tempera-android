//! Bounded, read-only semantic observation streams and portable recordings.
//!
//! Both surfaces invoke the canonical `snapshot` command for every frame. This
//! keeps backend selection, session revision handling, and snapshot persistence
//! identical to ordinary control use, while ensuring observation streaming never
//! becomes a second control path.

use crate::command::{execute, Command, CommandRequest};
use crate::error::{AndroidError, Result};
use crate::model::{next_action_id, SnapshotV1};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

const STREAM_SCHEMA: &str = "tempera.android.stream/v1";
const RECORD_SCHEMA: &str = "tempera.android.record/v1";

pub fn capture(
    request: &CommandRequest,
    observations: u32,
    interval_ms: u64,
) -> Result<Vec<Value>> {
    validate(observations, interval_ms)?;
    let mut events = Vec::with_capacity(observations as usize);
    for sequence in 0..observations {
        let response = execute(CommandRequest {
            id: next_action_id("stream-observe"),
            session_id: request.session_id.clone(),
            serial: request.serial.clone(),
            transport: request.transport.clone(),
            appium_url: request.appium_url.clone(),
            appium_capabilities: request.appium_capabilities.clone(),
            command: Command::Snapshot { full: false },
        });
        if !response.ok {
            return Err(AndroidError::Backend(response.error.unwrap_or_else(|| {
                "Android observation stream failed".to_string()
            })));
        }
        let snapshot: SnapshotV1 = serde_json::from_value(response.result.unwrap_or(Value::Null))?;
        events.push(json!({
            "schemaVersion": STREAM_SCHEMA,
            "sequence": sequence + 1,
            "capturedAtMs": snapshot.captured_at_ms,
            "snapshot": sanitize(snapshot),
        }));
        if sequence + 1 < observations && interval_ms > 0 {
            std::thread::sleep(Duration::from_millis(interval_ms));
        }
    }
    Ok(events)
}

pub fn record(
    request: &CommandRequest,
    path: &Path,
    observations: u32,
    interval_ms: u64,
    overwrite: bool,
) -> Result<Value> {
    if path.is_dir() {
        return Err(AndroidError::InvalidInput(format!(
            "record path {} is a directory",
            path.display()
        )));
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(AndroidError::InvalidInput(format!(
            "record directory {} does not exist",
            parent.display()
        )));
    }
    if path.exists() && !overwrite {
        return Err(AndroidError::InvalidInput(format!(
            "record path {} already exists; rerun with --overwrite to replace it",
            path.display()
        )));
    }
    let events = capture(request, observations, interval_ms)?;
    let temporary = path.with_extension("jsonl.tmp");
    if temporary.exists() {
        return Err(AndroidError::InvalidInput(format!(
            "temporary record path {} already exists; remove it after inspection",
            temporary.display()
        )));
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let header = json!({
        "schemaVersion": RECORD_SCHEMA,
        "sessionId": request.session_id,
        "transport": request.transport,
        "observations": observations,
        "intervalMs": interval_ms,
    });
    serde_json::to_writer(&mut file, &header)?;
    file.write_all(b"\n")?;
    for event in &events {
        serde_json::to_writer(&mut file, event)?;
        file.write_all(b"\n")?;
    }
    file.flush()?;
    if overwrite {
        fs::rename(&temporary, path)?;
    } else {
        // `hard_link` is an atomic no-replace publication operation. It avoids
        // a check-then-overwrite race for a recording the user did not opt into
        // replacing, and keeps incomplete temporary files out of the target.
        fs::hard_link(&temporary, path)?;
        fs::remove_file(&temporary)?;
    }
    Ok(json!({
        "schemaVersion": RECORD_SCHEMA,
        "path": path,
        "events": events.len(),
        "containsScreenshots": false,
        "note": "JSONL semantic trajectory; password node values are redacted and no actions are performed",
    }))
}

fn validate(observations: u32, interval_ms: u64) -> Result<()> {
    if !(1..=300).contains(&observations) {
        return Err(AndroidError::InvalidInput(
            "observations must be 1..=300".to_string(),
        ));
    }
    if interval_ms > 10_000 {
        return Err(AndroidError::InvalidInput(
            "intervalMs must be 0..=10000".to_string(),
        ));
    }
    Ok(())
}

fn sanitize(mut snapshot: SnapshotV1) -> SnapshotV1 {
    for node in &mut snapshot.nodes {
        if node.password {
            node.text = Some("[REDACTED]".to_string());
            node.content_description = Some("[REDACTED]".to_string());
            node.label = "[REDACTED]".to_string();
        }
    }
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_limits_are_bounded() {
        assert!(validate(1, 0).is_ok());
        assert!(validate(300, 10_000).is_ok());
        assert!(validate(0, 0).is_err());
        assert!(validate(301, 0).is_err());
        assert!(validate(1, 10_001).is_err());
    }
}
