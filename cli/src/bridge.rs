//! Optional native Accessibility bridge.
//!
//! The bridge is deliberately a local-only accelerator. Every host request
//! travels over an ADB forward to Android loopback, carries a per-device
//! random token, and is revision guarded on-device. It exposes no shell
//! primitive and callers can always fall back to direct ADB/UIAutomator.

use crate::adb::AdbBackend;
use crate::error::{AndroidError, Result};
use crate::model::{ActionV1, NodeV1, RectV1, SessionV1, SnapshotV1, CONTROL_SCHEMA_V1};
use crate::session::SessionStore;
use getrandom::fill;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub const PACKAGE: &str = "dev.tempera.android.bridge";
pub const SERVICE: &str = "dev.tempera.android.bridge/.BridgeAccessibilityService";
pub const DEVICE_PORT: u16 = 6210;
pub const PROTOCOL_VERSION: u64 = 3;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeStatus {
    pub package: String,
    pub installed: bool,
    pub enabled: bool,
    pub token_configured: bool,
    pub reachable: bool,
    pub protocol: u64,
    pub transport: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

pub struct BridgeClient {
    serial: String,
    token: String,
    host_port: u16,
    client_id: String,
    server_epoch: Option<String>,
    adb: AdbBackend,
}

impl BridgeClient {
    pub fn connect(serial: &str, store: &SessionStore) -> Result<Self> {
        let adb = AdbBackend::new(serial)?;
        let token = read_token(store, serial)?;
        let host_port = unused_port()?;
        adb.forward(host_port, DEVICE_PORT)?;
        let mut bytes = [0_u8; 16];
        fill(&mut bytes).map_err(|error| {
            AndroidError::Backend(format!("Could not create bridge client ID: {error}"))
        })?;
        let mut client = Self {
            serial: serial.to_string(),
            token,
            host_port,
            client_id: hex::encode(bytes),
            server_epoch: None,
            adb,
        };
        let health = client.health()?;
        let protocol = health
            .get("protocol")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        if protocol != PROTOCOL_VERSION {
            return Err(AndroidError::Unsupported(format!(
                "Bridge protocol {protocol} is incompatible with Tempera Android protocol {PROTOCOL_VERSION}; run bridge setup"
            )));
        }
        client.server_epoch = health
            .get("server_epoch")
            .and_then(Value::as_str)
            .map(str::to_string);
        Ok(client)
    }

    pub fn health(&mut self) -> Result<Value> {
        self.request("health", json!({}), false)
    }

    pub fn observe(&mut self, session: &mut SessionV1) -> Result<SnapshotV1> {
        let result = self.request("observe", json!({}), true)?;
        self.snapshot_from_observation(session, &result)
    }

    pub fn act_observe(
        &mut self,
        session: &mut SessionV1,
        expected_revision: u64,
        actions: Vec<Value>,
    ) -> Result<(SnapshotV1, Vec<Value>)> {
        let result = self.request(
            "act_observe",
            json!({
                "expected_revision": expected_revision,
                "actions": actions,
                "timeout_ms": 900,
                "quiet_ms": 120,
                "max_settle_ms": 900
            }),
            true,
        )?;
        if result.get("stale").and_then(Value::as_bool) == Some(true) {
            let actual = result
                .get("revision")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            return Err(AndroidError::StaleState {
                expected: expected_revision,
                actual,
            });
        }
        let observation = result.get("observation").ok_or_else(|| {
            AndroidError::Backend("Bridge act_observe response omitted observation".to_string())
        })?;
        let snapshot = self.snapshot_from_observation(session, observation)?;
        let results = result
            .get("results")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok((snapshot, results))
    }

    fn snapshot_from_observation(
        &self,
        session: &mut SessionV1,
        result: &Value,
    ) -> Result<SnapshotV1> {
        let package = result
            .get("package")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let activity = result
            .get("activity")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let screen = parse_screen(result.get("screen"));
        let nodes: Vec<NodeV1> = result
            .get("nodes")
            .and_then(Value::as_array)
            .map(|nodes| {
                nodes
                    .iter()
                    .enumerate()
                    .filter_map(|(index, node)| parse_node(index, node))
                    .collect::<Vec<NodeV1>>()
            })
            .unwrap_or_default();
        let state_hash = SnapshotV1::state_hash_for(&package, &activity, screen, &nodes);
        let revision = result
            .get("revision")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| session.last_revision.saturating_add(1).max(1));
        session.last_revision = revision;
        session.last_state_hash = Some(state_hash.clone());
        session.updated_at_ms = SnapshotV1::now_ms();
        Ok(SnapshotV1 {
            schema_version: CONTROL_SCHEMA_V1.to_string(),
            session_id: session.session_id.clone(),
            serial: self.serial.clone(),
            target_kind: session.target_kind.clone(),
            package,
            activity,
            screen,
            revision,
            state_hash,
            captured_at_ms: SnapshotV1::now_ms(),
            nodes,
        })
    }

    fn request(&mut self, operation: &str, payload: Value, requires_epoch: bool) -> Result<Value> {
        let mut request = json!({
            "id": format!("{}-{}", self.client_id, SnapshotV1::now_ms()),
            "client_id": self.client_id,
            "token": self.token,
            "op": operation,
        });
        if requires_epoch {
            let epoch = self.server_epoch.as_deref().ok_or_else(|| {
                AndroidError::Backend("Bridge health was not completed".to_string())
            })?;
            request["server_epoch"] = Value::String(epoch.to_string());
        }
        if let Some(values) = payload.as_object() {
            for (key, value) in values {
                request[key] = value.clone();
            }
        }
        let address = format!("127.0.0.1:{}", self.host_port)
            .parse()
            .map_err(|_| AndroidError::Backend("Invalid bridge port".to_string()))?;
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(900))?;
        stream.set_read_timeout(Some(Duration::from_secs(8)))?;
        stream.write_all(serde_json::to_string(&request)?.as_bytes())?;
        stream.write_all(b"\n")?;
        stream.flush()?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        let response: Value = serde_json::from_str(&line)?;
        if response.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(AndroidError::Backend(
                response
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("Bridge request failed")
                    .to_string(),
            ));
        }
        response
            .get("result")
            .cloned()
            .ok_or_else(|| AndroidError::Backend("Bridge response omitted result".to_string()))
    }
}

/// Translate a public action into the limited native bridge vocabulary. The
/// backend ref never appears in a public snapshot; it is valid only at the
/// snapshot revision which selected it.
pub fn action_payload(action: &ActionV1, snapshot: &SnapshotV1) -> Result<Value> {
    let mut value = serde_json::Map::new();
    let kind = match action.kind.as_str() {
        "fill" => "type",
        "press" => match action.key.as_deref().map(str::to_ascii_uppercase).as_deref() {
            Some("BACK") => "back",
            Some("HOME") => "home",
            Some("RECENTS") => "recents",
            Some("ENTER") => "enter",
            _ => {
                return Err(AndroidError::Unsupported(
                    "Native bridge supports BACK, HOME, RECENTS, and ENTER; use --transport adb for other key events".to_string(),
                ))
            }
        },
        "tap" | "long_press" | "type" | "back" | "home" | "recents" | "scroll" | "swipe"
        | "launch" | "wait" => action.kind.as_str(),
        other => return Err(AndroidError::Unsupported(format!("Unsupported native bridge action {other:?}"))),
    };
    value.insert("type".to_string(), Value::String(kind.to_string()));
    if matches!(kind, "tap" | "long_press" | "type") {
        if let Some(selector) = action.selector.as_deref() {
            let node = snapshot.node(selector).ok_or_else(|| {
                AndroidError::InvalidInput(format!(
                    "No current node matches {selector:?}; capture a new snapshot"
                ))
            })?;
            if kind == "type" && node.password {
                return Err(AndroidError::Unsupported(
                    "Password fields cannot be read or modified through the native bridge"
                        .to_string(),
                ));
            }
            let reference = node.backend_reference.as_deref().ok_or_else(|| {
                AndroidError::InvalidInput("Current node is not backed by the native bridge; capture a fresh bridge snapshot".to_string())
            })?;
            value.insert("ref".to_string(), Value::String(reference.to_string()));
        } else if matches!(kind, "tap" | "long_press") {
            let [x, y] = action.coordinates.ok_or_else(|| {
                AndroidError::InvalidInput(
                    "tap/long_press requires a selector or coordinates".to_string(),
                )
            })?;
            value.insert("x".to_string(), Value::from(x));
            value.insert("y".to_string(), Value::from(y));
        }
    }
    if kind == "type" {
        let text = action
            .text
            .as_deref()
            .ok_or_else(|| AndroidError::InvalidInput("type/fill requires text".to_string()))?;
        value.insert("text".to_string(), Value::String(text.to_string()));
    }
    if kind == "scroll" {
        if let Some(selector) = action.selector.as_deref() {
            let node = snapshot.node(selector).ok_or(AndroidError::StaleState {
                expected: snapshot.revision,
                actual: snapshot.revision,
            })?;
            if let Some(reference) = node.backend_reference.as_deref() {
                value.insert("ref".to_string(), Value::String(reference.to_string()));
            }
        }
        value.insert(
            "direction".to_string(),
            Value::String(
                action
                    .direction
                    .clone()
                    .unwrap_or_else(|| "down".to_string()),
            ),
        );
    }
    if kind == "swipe" {
        let [width, height] = snapshot.screen;
        let (x1, y1, x2, y2) = match action.direction.as_deref().unwrap_or("down") {
            "down" => (width / 2, height * 3 / 4, width / 2, height / 4),
            "up" => (width / 2, height / 4, width / 2, height * 3 / 4),
            "left" => (width * 3 / 4, height / 2, width / 4, height / 2),
            "right" => (width / 4, height / 2, width * 3 / 4, height / 2),
            other => {
                return Err(AndroidError::InvalidInput(format!(
                    "Unknown swipe direction {other:?}"
                )))
            }
        };
        value.insert("x1".to_string(), Value::from(x1));
        value.insert("y1".to_string(), Value::from(y1));
        value.insert("x2".to_string(), Value::from(x2));
        value.insert("y2".to_string(), Value::from(y2));
    }
    if kind == "launch" {
        value.insert(
            "package".to_string(),
            Value::String(action.selector.clone().ok_or_else(|| {
                AndroidError::InvalidInput("launch requires a package selector".to_string())
            })?),
        );
    }
    if kind == "wait" {
        let milliseconds = action
            .metadata
            .get("milliseconds")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(250)
            .min(10_000);
        value.insert(
            "seconds".to_string(),
            Value::from(milliseconds as f64 / 1000.0),
        );
    }
    Ok(Value::Object(value))
}

impl Drop for BridgeClient {
    fn drop(&mut self) {
        let _ = self.adb.remove_forward(self.host_port);
    }
}

pub fn status(serial: &str, store: &SessionStore) -> Result<BridgeStatus> {
    let adb = AdbBackend::new(serial)?;
    let installed = adb
        .shell(&["pm", "path", PACKAGE])
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    let enabled = adb
        .shell(&[
            "settings",
            "get",
            "secure",
            "enabled_accessibility_services",
        ])
        .map(|value| value.split(':').any(|service| service == SERVICE))
        .unwrap_or(false);
    let token_configured = token_path(store, serial).is_file();
    let mut value = BridgeStatus {
        package: PACKAGE.to_string(),
        installed,
        enabled,
        token_configured,
        reachable: false,
        protocol: PROTOCOL_VERSION,
        transport: "adb-uiautomator".to_string(),
        detail: None,
    };
    if installed && enabled && token_configured {
        match BridgeClient::connect(serial, store) {
            Ok(_) => {
                value.reachable = true;
                value.transport = "accessibility-bridge".to_string();
            }
            Err(error) => value.detail = Some(error.to_string()),
        }
    }
    Ok(value)
}

pub fn setup(serial: &str, store: &SessionStore, apk: Option<&Path>) -> Result<BridgeStatus> {
    let adb = AdbBackend::new(serial)?;
    adb.ensure_ready()?;
    let apk = match apk {
        Some(path) => path.to_path_buf(),
        None => build_companion()?,
    };
    let path = apk.to_string_lossy().to_string();
    adb.app_install(&[path])?;
    let token = create_token()?;
    write_token(store, serial, &token)?;
    adb.shell(&[
        "run-as",
        PACKAGE,
        "sh",
        "-c",
        &format!("umask 077; printf %s {} > files/bridge.token", token),
    ])?;
    enable(&adb)?;
    status(serial, store)
}

pub fn enable(adb: &AdbBackend) -> Result<()> {
    let existing = adb
        .shell(&[
            "settings",
            "get",
            "secure",
            "enabled_accessibility_services",
        ])
        .unwrap_or_default();
    let mut services: Vec<String> = existing
        .trim()
        .split(':')
        .filter(|value| !value.is_empty() && *value != "null")
        .map(str::to_string)
        .collect();
    if !services.iter().any(|service| service == SERVICE) {
        services.push(SERVICE.to_string());
    }
    let joined = services.join(":");
    adb.shell(&[
        "settings",
        "put",
        "secure",
        "enabled_accessibility_services",
        &joined,
    ])?;
    adb.shell(&["settings", "put", "secure", "accessibility_enabled", "1"])?;
    Ok(())
}

pub fn disable(adb: &AdbBackend) -> Result<()> {
    let existing = adb
        .shell(&[
            "settings",
            "get",
            "secure",
            "enabled_accessibility_services",
        ])
        .unwrap_or_default();
    let joined = existing
        .trim()
        .split(':')
        .filter(|service| *service != SERVICE && *service != "null")
        .collect::<Vec<_>>()
        .join(":");
    adb.shell(&[
        "settings",
        "put",
        "secure",
        "enabled_accessibility_services",
        &joined,
    ])?;
    Ok(())
}

fn build_companion() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("TEMPERA_ANDROID_BRIDGE_APK") {
        return Ok(PathBuf::from(path));
    }
    let output = Command::new("bash")
        .arg("scripts/build-companion.sh")
        .output()?;
    if !output.status.success() {
        return Err(AndroidError::Backend(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    let path = String::from_utf8_lossy(&output.stdout)
        .lines()
        .last()
        .unwrap_or_default()
        .trim()
        .to_string();
    if path.is_empty() {
        return Err(AndroidError::Backend(
            "Companion build did not report an APK".to_string(),
        ));
    }
    Ok(PathBuf::from(path))
}

fn parse_screen(value: Option<&Value>) -> [u32; 2] {
    let Some(values) = value.and_then(Value::as_array) else {
        return [1080, 1920];
    };
    [
        values.first().and_then(Value::as_u64).unwrap_or(1080) as u32,
        values.get(1).and_then(Value::as_u64).unwrap_or(1920) as u32,
    ]
}

fn parse_node(index: usize, value: &Value) -> Option<NodeV1> {
    let bounds = value.get("bounds")?.as_array()?;
    let bounds = RectV1 {
        left: bounds.first()?.as_u64()? as u32,
        top: bounds.get(1)?.as_u64()? as u32,
        right: bounds.get(2)?.as_u64()? as u32,
        bottom: bounds.get(3)?.as_u64()? as u32,
    };
    let backend_reference = value.get("ref").and_then(Value::as_str).map(str::to_string);
    let text = value
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_string);
    let description = value
        .get("desc")
        .and_then(Value::as_str)
        .map(str::to_string);
    let resource_id = value.get("id").and_then(Value::as_str).map(str::to_string);
    let label = value
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let clickable = value
        .get("clickable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let editable = value
        .get("editable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let scrollable = value
        .get("scrollable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut actions = Vec::new();
    if clickable {
        actions.push("tap".to_string());
    }
    if editable {
        actions.push("type".to_string());
    }
    if scrollable {
        actions.push("scroll".to_string());
    }
    Some(NodeV1 {
        reference: format!("@e{index}"),
        backend_reference,
        role: value
            .get("class")
            .and_then(Value::as_str)
            .unwrap_or("View")
            .to_string(),
        label,
        text,
        content_description: description,
        resource_id,
        bounds,
        enabled: value
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        clickable,
        editable,
        scrollable,
        password: value
            .get("password")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        actions,
    })
}

fn unused_port() -> Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

fn token_path(store: &SessionStore, serial: &str) -> PathBuf {
    store
        .root()
        .join("bridge")
        .join(format!("{}.token", safe_serial(serial)))
}

fn safe_serial(serial: &str) -> String {
    serial
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn read_token(store: &SessionStore, serial: &str) -> Result<String> {
    let token = fs::read_to_string(token_path(store, serial))?
        .trim()
        .to_string();
    if token.len() != 64 || !token.chars().all(|character| character.is_ascii_hexdigit()) {
        return Err(AndroidError::InvalidInput(
            "Bridge token is invalid; run tempera-android bridge setup".to_string(),
        ));
    }
    Ok(token)
}

fn write_token(store: &SessionStore, serial: &str, token: &str) -> Result<()> {
    let path = token_path(store, serial);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, token)?;
    Ok(())
}

fn create_token() -> Result<String> {
    let mut value = [0_u8; 32];
    fill(&mut value).map_err(|error| {
        AndroidError::Backend(format!("Could not create bridge token: {error}"))
    })?;
    Ok(hex::encode(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bridge_nodes_keep_public_refs_private_mapping() {
        let node = parse_node(2, &json!({"ref":"babc","class":"Button","label":"Continue","bounds":[1,2,3,4],"clickable":true})).unwrap();
        assert_eq!(node.reference, "@e2");
        assert_eq!(node.backend_reference.as_deref(), Some("babc"));
    }

    #[test]
    fn bridge_token_paths_are_non_traversing() {
        assert_eq!(safe_serial("emulator-5554/../x"), "emulator_5554____x");
    }
}
