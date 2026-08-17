//! Direct Android Debug Bridge and UIAutomator backend.
//!
//! This is deliberately independent of the companion service. It is both the
//! zero-install path and the baseline that proves the native bridge improves
//! latency without changing semantics.

use crate::error::{AndroidError, Result};
use crate::model::{
    ActionReceiptV1, ActionV1, NodeV1, RectV1, SessionV1, SnapshotV1, CONTROL_SCHEMA_V1,
};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct AdbBackend {
    serial: String,
    adb: PathBuf,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub serial: String,
    pub state: String,
    pub details: String,
    pub target_kind: String,
}

impl AdbBackend {
    pub fn new(serial: impl Into<String>) -> Result<Self> {
        let adb = if let Some(value) = env::var_os("TEMPERA_ANDROID_ADB") {
            PathBuf::from(value)
        } else if let Some(root) =
            env::var_os("ANDROID_SDK_ROOT").or_else(|| env::var_os("ANDROID_HOME"))
        {
            PathBuf::from(root)
                .join("platform-tools")
                .join(executable("adb"))
        } else {
            PathBuf::from(executable("adb"))
        };
        Ok(Self {
            serial: serial.into(),
            adb,
        })
    }

    pub fn serial(&self) -> &str {
        &self.serial
    }

    pub fn device_list(&self) -> Result<Vec<DeviceInfo>> {
        let output = self.run_global(&["devices", "-l"])?;
        Ok(output
            .lines()
            .skip(1)
            .filter_map(|line| {
                let line = line.trim();
                if line.is_empty() {
                    return None;
                }
                let mut parts = line.splitn(3, char::is_whitespace);
                let serial = parts.next()?.to_string();
                let state = parts.next().unwrap_or("unknown").to_string();
                let details = parts.next().unwrap_or("").trim().to_string();
                Some(DeviceInfo {
                    target_kind: if serial.starts_with("emulator-") {
                        "emulator"
                    } else {
                        "device"
                    }
                    .to_string(),
                    serial,
                    state,
                    details,
                })
            })
            .collect())
    }

    pub fn ensure_ready(&self) -> Result<()> {
        let device = self
            .device_list()?
            .into_iter()
            .find(|device| device.serial == self.serial)
            .ok_or_else(|| {
                AndroidError::Backend(format!("Android target {:?} is not connected", self.serial))
            })?;
        if device.state != "device" {
            return Err(AndroidError::Backend(format!(
                "Android target {:?} is not ready (state: {})",
                self.serial, device.state
            )));
        }
        Ok(())
    }

    pub fn connect(&self, endpoint: &str) -> Result<String> {
        self.run_global(&["connect", endpoint])
    }

    pub fn forward(&self, host_port: u16, device_port: u16) -> Result<()> {
        self.run_target(&[
            "forward",
            &format!("tcp:{host_port}"),
            &format!("tcp:{device_port}"),
        ])
        .map(|_| ())
    }

    pub fn remove_forward(&self, host_port: u16) -> Result<()> {
        self.run_target(&["forward", "--remove", &format!("tcp:{host_port}")])
            .map(|_| ())
    }

    pub fn snapshot(&self, session: &mut SessionV1) -> Result<SnapshotV1> {
        self.ensure_ready()?;
        let (package, activity, screen) = self.metadata()?;
        let nodes = parse_hierarchy(&self.hierarchy()?)?;
        let state_hash = SnapshotV1::state_hash_for(&package, &activity, screen, &nodes);
        if session.last_state_hash.as_deref() != Some(state_hash.as_str()) {
            session.last_revision = session.last_revision.saturating_add(1).max(1);
            session.last_state_hash = Some(state_hash.clone());
        }
        session.updated_at_ms = SnapshotV1::now_ms();
        Ok(SnapshotV1 {
            schema_version: CONTROL_SCHEMA_V1.to_string(),
            session_id: session.session_id.clone(),
            serial: self.serial.clone(),
            target_kind: session.target_kind.clone(),
            package,
            activity,
            screen,
            revision: session.last_revision,
            state_hash,
            captured_at_ms: SnapshotV1::now_ms(),
            nodes,
        })
    }

    pub fn execute_action(
        &self,
        session: &mut SessionV1,
        action: &ActionV1,
    ) -> Result<ActionReceiptV1> {
        let before = self.snapshot(session)?;
        validate_guard(action, &before)?;
        validate_sensitive(action, &before)?;
        let started_at_ms = SnapshotV1::now_ms();
        match action.kind.as_str() {
            "tap" | "long_press" => {
                let (x, y) = resolve_coordinates(action, &before)?;
                if action.kind == "tap" {
                    self.shell(&["input", "tap", &x.to_string(), &y.to_string()])?;
                } else {
                    self.shell(&[
                        "input",
                        "swipe",
                        &x.to_string(),
                        &y.to_string(),
                        &x.to_string(),
                        &y.to_string(),
                        "700",
                    ])?;
                }
            }
            "type" | "fill" => {
                if let Some(selector) = action.selector.as_deref() {
                    let (x, y) = resolve_selector(selector, &before)?.bounds.center();
                    self.shell(&["input", "tap", &x.to_string(), &y.to_string()])?;
                }
                let text = action.text.as_deref().ok_or_else(|| {
                    AndroidError::InvalidInput("type/fill requires text".to_string())
                })?;
                self.shell(&["input", "text", &encode_input_text(text)])?;
            }
            "press" => {
                let key = action
                    .key
                    .as_deref()
                    .ok_or_else(|| AndroidError::InvalidInput("press requires key".to_string()))?;
                self.shell(&["input", "keyevent", &android_keycode(key)])?;
            }
            "back" => {
                self.shell(&["input", "keyevent", "KEYCODE_BACK"])?;
            }
            "home" => {
                self.shell(&["input", "keyevent", "KEYCODE_HOME"])?;
            }
            "recents" => {
                self.shell(&["input", "keyevent", "KEYCODE_APP_SWITCH"])?;
            }
            "swipe" | "scroll" => self.execute_scroll(action, &before)?,
            "launch" => {
                let package = action.selector.as_deref().ok_or_else(|| {
                    AndroidError::InvalidInput("launch requires a package selector".to_string())
                })?;
                self.shell(&[
                    "monkey",
                    "-p",
                    package,
                    "-c",
                    "android.intent.category.LAUNCHER",
                    "1",
                ])?;
            }
            "wait" => {
                let milliseconds = action
                    .metadata
                    .get("milliseconds")
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(250)
                    .min(10_000);
                std::thread::sleep(std::time::Duration::from_millis(milliseconds));
            }
            other => {
                return Err(AndroidError::Unsupported(format!(
                    "Unsupported ADB action {other:?}"
                )))
            }
        }
        let after = self.snapshot(session)?;
        Ok(ActionReceiptV1 {
            schema_version: CONTROL_SCHEMA_V1.to_string(),
            action_id: action.action_id.clone(),
            kind: action.kind.clone(),
            ok: true,
            transport: "adb-uiautomator".to_string(),
            started_at_ms,
            completed_at_ms: SnapshotV1::now_ms(),
            before_revision: before.revision,
            after_revision: after.revision,
            before_state_hash: before.state_hash,
            after_state_hash: after.state_hash,
            detail: None,
        })
    }

    pub fn screenshot(&self, destination: &Path) -> Result<()> {
        self.ensure_ready()?;
        let output = Command::new(&self.adb)
            .args(["-s", &self.serial, "exec-out", "screencap", "-p"])
            .output()?;
        if !output.status.success() {
            return Err(AndroidError::Backend(command_error(&output)));
        }
        fs::write(destination, output.stdout)?;
        Ok(())
    }

    pub fn app_list(&self, include_system: bool) -> Result<Vec<String>> {
        let mut arguments = vec!["pm", "list", "packages"];
        if !include_system {
            arguments.push("-3");
        }
        Ok(self
            .shell(&arguments)?
            .lines()
            .filter_map(|line| line.strip_prefix("package:").map(str::to_string))
            .collect())
    }

    pub fn app_install(&self, paths: &[String]) -> Result<()> {
        if paths.is_empty() {
            return Err(AndroidError::InvalidInput(
                "app install requires at least one APK path".to_string(),
            ));
        }
        let mut arguments = vec!["-s", self.serial.as_str()];
        arguments.push(if paths.len() == 1 {
            "install"
        } else {
            "install-multiple"
        });
        arguments.push("-r");
        arguments.extend(paths.iter().map(String::as_str));
        let output = Command::new(&self.adb).args(arguments).output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(AndroidError::Backend(command_error(&output)))
        }
    }

    pub fn app_manage(&self, operation: &str, package: &str) -> Result<()> {
        match operation {
            "uninstall" => self.run_target(&["uninstall", package]).map(|_| ()),
            "stop" => self.shell(&["am", "force-stop", package]).map(|_| ()),
            "clear" => self.shell(&["pm", "clear", package]).map(|_| ()),
            "open" => self
                .shell(&[
                    "monkey",
                    "-p",
                    package,
                    "-c",
                    "android.intent.category.LAUNCHER",
                    "1",
                ])
                .map(|_| ()),
            _ => Err(AndroidError::Unsupported(format!(
                "Unknown app operation {operation:?}"
            ))),
        }
    }

    pub fn app_deeplink(&self, uri: &str) -> Result<()> {
        self.shell(&[
            "am",
            "start",
            "-W",
            "-a",
            "android.intent.action.VIEW",
            "-d",
            uri,
        ])
        .map(|_| ())
    }

    pub fn logs(&self, lines: u32) -> Result<String> {
        self.shell(&["logcat", "-d", "-t", &lines.clamp(1, 2_000).to_string()])
    }

    pub fn network_status(&self) -> Result<String> {
        self.shell(&["dumpsys", "connectivity"])
    }

    pub fn clipboard_get(&self) -> Result<String> {
        self.shell(&["cmd", "clipboard", "get"])
    }

    pub fn clipboard_set(&self, value: &str) -> Result<()> {
        self.shell(&["cmd", "clipboard", "set", value]).map(|_| ())
    }

    pub fn emulator_location(&self, latitude: f64, longitude: f64) -> Result<()> {
        if !self.serial.starts_with("emulator-") {
            return Err(AndroidError::InvalidInput(
                "location injection is emulator-only in the ADB backend; a physical device location must be changed by its owner or integration provider".to_string(),
            ));
        }
        if !latitude.is_finite()
            || !longitude.is_finite()
            || !(-90.0..=90.0).contains(&latitude)
            || !(-180.0..=180.0).contains(&longitude)
        {
            return Err(AndroidError::InvalidInput(
                "latitude must be -90..90 and longitude must be -180..180".to_string(),
            ));
        }
        self.run_target(&[
            "emu",
            "geo",
            "fix",
            &longitude.to_string(),
            &latitude.to_string(),
        ])
        .map(|_| ())
    }

    pub fn shell(&self, arguments: &[&str]) -> Result<String> {
        let mut values = vec!["shell"];
        values.extend_from_slice(arguments);
        self.run_target(&values)
    }

    fn hierarchy(&self) -> Result<String> {
        let compact = self.shell(&["uiautomator", "dump", "--compressed", "/dev/tty"])?;
        if let Some(index) = compact.find("<hierarchy") {
            return Ok(compact[index..].to_string());
        }
        let full = self.shell(&["uiautomator", "dump", "/dev/tty"])?;
        full.find("<hierarchy")
            .map(|index| full[index..].to_string())
            .ok_or_else(|| AndroidError::Backend("Could not read Android UI hierarchy".to_string()))
    }

    fn metadata(&self) -> Result<(String, String, [u32; 2])> {
        let output = self.shell(&[
            "sh",
            "-c",
            "wm size; echo __TEMPERA_ANDROID_WINDOW__; dumpsys window windows | grep -m 1 -E 'mCurrentFocus|mFocusedApp'",
        ])?;
        let screen = output.lines().find_map(parse_size).unwrap_or([1080, 1920]);
        let component = output.split_whitespace().find(|value| {
            value.contains('/')
                && value.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '$' | '/')
                })
        });
        let (package, activity) = component
            .map(|value| value.split_once('/').unwrap_or((value, "")))
            .map(|(package, activity)| (package.to_string(), activity.to_string()))
            .unwrap_or_default();
        Ok((package, activity, screen))
    }

    fn execute_scroll(&self, action: &ActionV1, snapshot: &SnapshotV1) -> Result<()> {
        let [width, height] = snapshot.screen;
        let direction = action.direction.as_deref().unwrap_or("down");
        let (start_x, start_y, end_x, end_y) = match direction {
            "down" => (width / 2, height * 3 / 4, width / 2, height / 4),
            "up" => (width / 2, height / 4, width / 2, height * 3 / 4),
            "left" => (width * 3 / 4, height / 2, width / 4, height / 2),
            "right" => (width / 4, height / 2, width * 3 / 4, height / 2),
            other => {
                return Err(AndroidError::InvalidInput(format!(
                    "Unknown scroll direction {other:?}"
                )))
            }
        };
        self.shell(&[
            "input",
            "swipe",
            &start_x.to_string(),
            &start_y.to_string(),
            &end_x.to_string(),
            &end_y.to_string(),
            "300",
        ])?;
        Ok(())
    }

    fn run_global(&self, arguments: &[&str]) -> Result<String> {
        let output = Command::new(&self.adb).args(arguments).output()?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(AndroidError::Backend(command_error(&output)))
        }
    }

    fn run_target(&self, arguments: &[&str]) -> Result<String> {
        let mut values = vec!["-s", self.serial.as_str()];
        values.extend_from_slice(arguments);
        self.run_global(&values)
    }
}

fn executable(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn command_error(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stderr.is_empty() {
        stdout
    } else {
        stderr
    }
}

fn parse_size(value: &str) -> Option<[u32; 2]> {
    let candidate = value.split_whitespace().find(|part| part.contains('x'))?;
    let (width, height) = candidate.split_once('x')?;
    Some([width.parse().ok()?, height.parse().ok()?])
}

fn parse_hierarchy(xml: &str) -> Result<Vec<NodeV1>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut nodes = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(event)) | Ok(Event::Empty(event))
                if event.name().as_ref() == b"node" =>
            {
                let attributes: BTreeMap<String, String> = event
                    .attributes()
                    .flatten()
                    .filter_map(|attribute| {
                        let key = std::str::from_utf8(attribute.key.as_ref())
                            .ok()?
                            .to_string();
                        let value = attribute
                            .decode_and_unescape_value(reader.decoder())
                            .ok()?
                            .to_string();
                        Some((key, value))
                    })
                    .collect();
                let password = attributes
                    .get("password")
                    .is_some_and(|value| value == "true");
                let text = attributes
                    .get("text")
                    .filter(|value| !password && !value.is_empty())
                    .cloned();
                let description = attributes
                    .get("content-desc")
                    .filter(|value| !value.is_empty())
                    .cloned();
                let resource_id = attributes
                    .get("resource-id")
                    .filter(|value| !value.is_empty())
                    .cloned();
                let label = text
                    .clone()
                    .or_else(|| description.clone())
                    .or_else(|| {
                        resource_id
                            .as_ref()
                            .and_then(|id| id.rsplit('/').next().map(str::to_string))
                    })
                    .unwrap_or_default();
                let bounds = attributes
                    .get("bounds")
                    .and_then(|value| parse_bounds(value))
                    .unwrap_or(RectV1 {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    });
                let clickable = attributes
                    .get("clickable")
                    .is_some_and(|value| value == "true");
                let editable = attributes
                    .get("editable")
                    .is_some_and(|value| value == "true")
                    || attributes
                        .get("class")
                        .is_some_and(|value| value.contains("EditText"));
                let scrollable = attributes
                    .get("scrollable")
                    .is_some_and(|value| value == "true");
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
                nodes.push(NodeV1 {
                    reference: format!("@e{}", nodes.len()),
                    backend_reference: None,
                    role: attributes
                        .get("class")
                        .and_then(|value| value.rsplit('.').next())
                        .unwrap_or("View")
                        .to_string(),
                    label,
                    text,
                    content_description: description,
                    resource_id,
                    bounds,
                    enabled: attributes
                        .get("enabled")
                        .is_none_or(|value| value == "true"),
                    clickable,
                    editable,
                    scrollable,
                    password,
                    actions,
                });
            }
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(AndroidError::Backend(format!(
                    "Invalid Android UI hierarchy: {error}"
                )))
            }
            _ => {}
        }
        buffer.clear();
    }
    Ok(nodes)
}

fn parse_bounds(value: &str) -> Option<RectV1> {
    let cleaned = value.replace(['[', ']'], " ").replace(',', " ");
    let values: Vec<u32> = cleaned
        .split_whitespace()
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()
        .ok()?;
    (values.len() == 4).then(|| RectV1 {
        left: values[0],
        top: values[1],
        right: values[2],
        bottom: values[3],
    })
}

pub(crate) fn validate_guard(action: &ActionV1, snapshot: &SnapshotV1) -> Result<()> {
    if let Some(expected) = action.expected_revision {
        if expected != snapshot.revision {
            return Err(AndroidError::StaleState {
                expected,
                actual: snapshot.revision,
            });
        }
    }
    if let Some(expected) = action.expected_state_hash.as_deref() {
        if expected != snapshot.state_hash {
            return Err(AndroidError::Backend(
                "Android UI changed before the planned action could execute (state hash mismatch)"
                    .to_string(),
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_sensitive(action: &ActionV1, snapshot: &SnapshotV1) -> Result<()> {
    let Some(selector) = action.selector.as_deref() else {
        return Ok(());
    };
    let Some(node) = snapshot.node(selector) else {
        return Ok(());
    };
    let sensitive = [
        "send",
        "post",
        "buy",
        "pay",
        "transfer",
        "delete",
        "subscribe",
        "book",
        "order",
        "submit",
    ];
    if sensitive
        .iter()
        .any(|word| node.label.to_ascii_lowercase().contains(word))
        && action.metadata.get("approval").map(String::as_str) != Some("granted")
    {
        return Err(AndroidError::InvalidInput(format!(
            "Action targets consequential UI label {:?}; set metadata.approval=granted after explicit user approval", node.label
        )));
    }
    Ok(())
}

fn resolve_selector<'a>(selector: &str, snapshot: &'a SnapshotV1) -> Result<&'a NodeV1> {
    snapshot.node(selector).ok_or_else(|| {
        AndroidError::InvalidInput(format!(
            "No current Android node matches {selector:?}; take a fresh snapshot"
        ))
    })
}

fn resolve_coordinates(action: &ActionV1, snapshot: &SnapshotV1) -> Result<(u32, u32)> {
    if let Some([x, y]) = action.coordinates {
        return Ok((x, y));
    }
    let selector = action.selector.as_deref().ok_or_else(|| {
        AndroidError::InvalidInput("tap/long_press requires selector or coordinates".to_string())
    })?;
    Ok(resolve_selector(selector, snapshot)?.bounds.center())
}

fn encode_input_text(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace(' ', "%s")
        .replace('&', "\\&")
        .replace('<', "\\<")
        .replace('>', "\\>")
        .replace('"', "\\\"")
        .replace('\'', "\\'")
}

fn android_keycode(value: &str) -> String {
    let uppercase = value.trim().to_ascii_uppercase();
    if uppercase.starts_with("KEYCODE_")
        || uppercase
            .chars()
            .all(|character| character.is_ascii_digit())
    {
        uppercase
    } else {
        format!("KEYCODE_{uppercase}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_redacts_android_hierarchy() {
        let nodes = parse_hierarchy(r#"<hierarchy><node text="Search" content-desc="" resource-id="id/search" class="android.widget.EditText" clickable="true" enabled="true" editable="true" scrollable="false" password="false" bounds="[1,2][30,40]"/><node text="secret" class="android.widget.EditText" clickable="false" enabled="true" editable="true" scrollable="false" password="true" bounds="[0,0][1,1]"/></hierarchy>"#).unwrap();
        assert_eq!(nodes[0].reference, "@e0");
        assert_eq!(nodes[0].label, "Search");
        assert_eq!(nodes[1].text, None);
        assert!(nodes[1].password);
    }

    #[test]
    fn state_hash_ignores_capture_time() {
        let nodes = parse_hierarchy(r#"<hierarchy><node text="OK" class="Button" clickable="true" enabled="true" editable="false" scrollable="false" password="false" bounds="[0,0][4,4]"/></hierarchy>"#).unwrap();
        assert_eq!(
            SnapshotV1::state_hash_for("p", "a", [1, 1], &nodes),
            SnapshotV1::state_hash_for("p", "a", [1, 1], &nodes)
        );
    }

    #[test]
    fn text_encoding_keeps_spaces_transport_safe() {
        assert_eq!(encode_input_text("hello world"), "hello%sworld");
    }
}
