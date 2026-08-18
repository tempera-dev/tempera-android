//! Native-bridge fast path for Android browser mutations.
//!
//! The generic command executor remains the compatibility and ADB fallback.
//! When the Accessibility bridge is already authorized, this path performs one
//! guarded observation followed by the bridge's fused `act_observe` operation,
//! eliminating redundant host command dispatch and post-action observation.

use crate::adb;
use crate::android_browser::{self, BrowserRequest, BrowserSnapshotV1, ANDROID_BROWSER_SCHEMA_V1};
use crate::bridge;
use crate::error::{AndroidError, Result};
use crate::model::{ActionReceiptV1, ActionV1, NodeV1, SnapshotV1, CONTROL_SCHEMA_V1};
use crate::session::SessionStore;
use serde_json::{json, Value};
use std::time::Instant;

const MAX_BROWSER_NODES: usize = 512;

pub fn step(request: &BrowserRequest, action: ActionV1) -> Result<Value> {
    require_full_guard(&action)?;
    let serial = android_browser::resolve_serial(request.serial.as_deref())?;
    let store = SessionStore::from_environment()?;

    if let Some(receipt) = store.receipt(&request.session_id, &action.action_id)? {
        let mut replay_request = request.clone();
        replay_request.serial = Some(serial);
        return Ok(json!({
            "schemaVersion": ANDROID_BROWSER_SCHEMA_V1,
            "ok": true,
            "replayed": true,
            "receipt": receipt,
            "snapshot": android_browser::snapshot(&replay_request)?,
        }));
    }

    match request.transport.as_str() {
        "bridge" => {
            let bridge = bridge::BridgeClient::connect(&serial, &store)?;
            step_bridge(request, serial, store, bridge, action)
        }
        "auto" => match bridge::BridgeClient::connect(&serial, &store) {
            Ok(bridge) => step_bridge(request, serial, store, bridge, action),
            Err(_) => {
                let mut fallback = request.clone();
                fallback.serial = Some(serial);
                fallback.transport = "adb".to_string();
                android_browser::step(&fallback, action)
            }
        },
        "adb" => {
            let mut fallback = request.clone();
            fallback.serial = Some(serial);
            android_browser::step(&fallback, action)
        }
        other => Err(AndroidError::InvalidInput(format!(
            "Unknown Android browser transport {other:?}; use auto, bridge, or adb"
        ))),
    }
}

fn step_bridge(
    request: &BrowserRequest,
    serial: String,
    store: SessionStore,
    mut bridge: bridge::BridgeClient,
    action: ActionV1,
) -> Result<Value> {
    let mut session = store.get_or_create(&request.session_id, &serial, "bridge")?;
    session.transport = "bridge".to_string();

    let started = Instant::now();
    let before = bridge.observe(&mut session)?;
    ensure_browser_foreground(request, &before)?;
    adb::validate_guard(&action, &before)?;
    adb::validate_sensitive(&action, &before)?;

    let payload = bridge::action_payload(&action, &before)?;
    let started_at_ms = SnapshotV1::now_ms();
    let (after, results) = bridge.act_observe(&mut session, before.revision, vec![payload])?;
    let result = results.first().ok_or_else(|| {
        AndroidError::Backend("Native bridge action omitted its result".to_string())
    })?;
    if results.len() != 1 {
        return Err(AndroidError::Backend(
            "Native bridge browser step returned an unexpected result count".to_string(),
        ));
    }
    if result.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(AndroidError::Backend(
            result
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("Native bridge browser action failed")
                .to_string(),
        ));
    }

    let receipt = ActionReceiptV1 {
        schema_version: CONTROL_SCHEMA_V1.to_string(),
        action_id: action.action_id,
        kind: action.kind,
        ok: true,
        transport: "native-accessibility-bridge".to_string(),
        started_at_ms,
        completed_at_ms: SnapshotV1::now_ms(),
        before_revision: before.revision,
        after_revision: after.revision,
        before_state_hash: before.state_hash,
        after_state_hash: after.state_hash.clone(),
        detail: result
            .get("detail")
            .and_then(Value::as_str)
            .map(str::to_string),
    };

    store.save(&session)?;
    store.save_snapshot(&session.session_id, &after)?;
    store.save_receipts(&session.session_id, std::slice::from_ref(&receipt))?;
    let snapshot = browser_snapshot(request, after)?;

    Ok(json!({
        "schemaVersion": ANDROID_BROWSER_SCHEMA_V1,
        "ok": true,
        "fastPath": "native-bridge-act-observe",
        "receipt": receipt,
        "snapshot": snapshot,
        "timing": {"actObserveMicros": started.elapsed().as_micros()},
    }))
}

fn require_full_guard(action: &ActionV1) -> Result<()> {
    if action.expected_revision.is_none() || action.expected_state_hash.is_none() {
        return Err(AndroidError::InvalidInput(
            "Android browser actions require expectedRevision and expectedStateHash from the latest browser snapshot"
                .to_string(),
        ));
    }
    Ok(())
}

fn ensure_browser_foreground(request: &BrowserRequest, snapshot: &SnapshotV1) -> Result<()> {
    if snapshot.package == request.package {
        Ok(())
    } else {
        Err(AndroidError::Backend(format!(
            "Expected Android browser package {:?}, but foreground package is {:?}",
            request.package, snapshot.package
        )))
    }
}

fn browser_snapshot(
    request: &BrowserRequest,
    android: SnapshotV1,
) -> Result<BrowserSnapshotV1> {
    ensure_browser_foreground(request, &android)?;
    let nodes = compact_nodes(&android.nodes);
    let url_hint = extract_url_hint(&nodes);
    let title_hint = extract_title_hint(&nodes, url_hint.as_deref());
    Ok(BrowserSnapshotV1 {
        schema_version: ANDROID_BROWSER_SCHEMA_V1.to_string(),
        package: android.package.clone(),
        url_hint,
        title_hint,
        revision: android.revision,
        state_hash: android.state_hash.clone(),
        captured_at_ms: android.captured_at_ms,
        screen: android.screen,
        nodes,
        android,
    })
}

fn compact_nodes(nodes: &[NodeV1]) -> Vec<NodeV1> {
    nodes
        .iter()
        .filter(|node| {
            !node.password
                && (node.clickable
                    || node.editable
                    || node.scrollable
                    || !node.label.trim().is_empty()
                    || node
                        .text
                        .as_deref()
                        .is_some_and(|text| !text.trim().is_empty()))
        })
        .take(MAX_BROWSER_NODES)
        .cloned()
        .collect()
}

fn extract_url_hint(nodes: &[NodeV1]) -> Option<String> {
    nodes.iter().find_map(|node| {
        let url_bar = node
            .resource_id
            .as_deref()
            .is_some_and(|value| value.ends_with("/url_bar") || value.ends_with(":id/url_bar"));
        let candidate = node
            .text
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| (!node.label.trim().is_empty()).then_some(node.label.as_str()))?;
        (url_bar || candidate.starts_with("http://") || candidate.starts_with("https://"))
            .then(|| candidate.to_string())
    })
}

fn extract_title_hint(nodes: &[NodeV1], url: Option<&str>) -> Option<String> {
    nodes.iter().find_map(|node| {
        let label = node.label.trim();
        (!label.is_empty()
            && Some(label) != url
            && !node.editable
            && matches!(
                node.role.as_str(),
                "android.widget.TextView" | "text" | "heading"
            ))
        .then(|| label.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn action(revision: Option<u64>, hash: Option<&str>) -> ActionV1 {
        ActionV1 {
            action_id: "browser-step".to_string(),
            kind: "tap".to_string(),
            selector: Some("@e1".to_string()),
            text: None,
            key: None,
            direction: None,
            coordinates: None,
            expected_revision: revision,
            expected_state_hash: hash.map(str::to_string),
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn fused_step_requires_revision_and_hash() {
        assert!(require_full_guard(&action(Some(1), Some("sha256:state"))).is_ok());
        assert!(require_full_guard(&action(None, Some("sha256:state"))).is_err());
        assert!(require_full_guard(&action(Some(1), None)).is_err());
    }
}
