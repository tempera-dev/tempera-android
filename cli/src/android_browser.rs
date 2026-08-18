//! Android-specific browser runtime built on Tempera Android's native bridge.
//!
//! The hot path stays semantic and revision-safe. Browser launch/navigation is
//! constrained to validated HTTP(S) intents, while observation and actions go
//! through the canonical Tempera command executor. A bounded, read-only Chrome
//! DevTools target probe is available for diagnostics and future DOM acceleration.

use crate::adb::AdbBackend;
use crate::command::{execute, Command, CommandRequest};
use crate::error::{AndroidError, Result};
use crate::model::{ActionReceiptV1, ActionV1, NodeV1, SnapshotV1};
use serde::Serialize;
use serde_json::{json, Value};
use std::env;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::thread;
use std::time::{Duration, Instant};

pub const ANDROID_BROWSER_SCHEMA_V1: &str = "tempera.android.browser/v1";
const DEFAULT_BROWSER_PACKAGE: &str = "com.android.chrome";
const DEFAULT_CDP_SOCKET: &str = "chrome_devtools_remote";
const MAX_HTTP_BYTES: u64 = 4 * 1024 * 1024;
const MAX_BROWSER_NODES: usize = 512;

#[derive(Debug, Clone)]
pub struct BrowserRequest {
    pub session_id: String,
    pub serial: Option<String>,
    pub transport: String,
    pub package: String,
}

impl Default for BrowserRequest {
    fn default() -> Self {
        Self {
            session_id: "browser".to_string(),
            serial: None,
            transport: "auto".to_string(),
            package: DEFAULT_BROWSER_PACKAGE.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSnapshotV1 {
    pub schema_version: String,
    pub package: String,
    pub url_hint: Option<String>,
    pub title_hint: Option<String>,
    pub revision: u64,
    pub state_hash: String,
    pub captured_at_ms: u128,
    pub screen: [u32; 2],
    pub nodes: Vec<NodeV1>,
    pub android: SnapshotV1,
}

pub fn resolve_serial(serial: Option<&str>) -> Result<String> {
    if let Some(serial) = serial.filter(|value| !value.trim().is_empty()) {
        return Ok(serial.to_string());
    }
    let devices = AdbBackend::new("unused")?
        .device_list()?
        .into_iter()
        .filter(|device| device.state == "device")
        .collect::<Vec<_>>();
    match devices.as_slice() {
        [device] => Ok(device.serial.clone()),
        [] => Err(AndroidError::Backend(
            "No ready Android device is connected; pass --serial after authorizing ADB".to_string(),
        )),
        _ => Err(AndroidError::InvalidInput(
            "Multiple Android devices are ready; pass --serial explicitly".to_string(),
        )),
    }
}

pub fn doctor(request: &BrowserRequest, cdp_socket: Option<&str>) -> Result<Value> {
    validate_package(&request.package)?;
    let serial = resolve_serial(request.serial.as_deref())?;
    let backend = AdbBackend::new(serial.clone())?;
    backend.ensure_ready()?;
    let package_path = backend.shell(&["pm", "path", &request.package])?;
    let installed = package_path.lines().any(|line| line.starts_with("package:"));
    let cdp = match targets(&serial, cdp_socket.unwrap_or(DEFAULT_CDP_SOCKET)) {
        Ok(targets) => json!({"reachable": true, "targets": targets}),
        Err(error) => json!({"reachable": false, "detail": error.to_string()}),
    };
    Ok(json!({
        "schemaVersion": ANDROID_BROWSER_SCHEMA_V1,
        "serial": serial,
        "package": request.package,
        "installed": installed,
        "transport": request.transport,
        "nativeSemanticPath": true,
        "cdp": cdp,
    }))
}

pub fn open(
    request: &BrowserRequest,
    url: &str,
    timeout_ms: u64,
) -> Result<Value> {
    validate_package(&request.package)?;
    validate_url(url)?;
    if !(250..=60_000).contains(&timeout_ms) {
        return Err(AndroidError::InvalidInput(
            "browser open timeoutMs must be 250..=60000".to_string(),
        ));
    }
    let serial = resolve_serial(request.serial.as_deref())?;
    let backend = AdbBackend::new(serial.clone())?;
    backend.ensure_ready()?;
    let started = Instant::now();
    let launch = backend.shell(&[
        "am",
        "start",
        "-W",
        "-a",
        "android.intent.action.VIEW",
        "-d",
        url,
        "-p",
        &request.package,
    ])?;

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        match snapshot(request) {
            Ok(browser) if browser.package == request.package => {
                return Ok(json!({
                    "schemaVersion": ANDROID_BROWSER_SCHEMA_V1,
                    "opened": true,
                    "requestedUrl": url,
                    "launch": launch.trim(),
                    "timing": {"totalMicros": started.elapsed().as_micros()},
                    "snapshot": browser,
                }));
            }
            Ok(_) | Err(_) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(50));
            }
            Ok(browser) => {
                return Err(AndroidError::Backend(format!(
                    "Android browser did not become foreground before timeout (foreground package: {:?})",
                    browser.package
                )));
            }
            Err(error) => return Err(error),
        }
    }
}

pub fn snapshot(request: &BrowserRequest) -> Result<BrowserSnapshotV1> {
    validate_package(&request.package)?;
    let snapshot = observe(request)?;
    if snapshot.package != request.package {
        return Err(AndroidError::Backend(format!(
            "Expected Android browser package {:?}, but foreground package is {:?}",
            request.package, snapshot.package
        )));
    }
    let nodes = compact_nodes(&snapshot.nodes);
    let url_hint = extract_url_hint(&nodes);
    let title_hint = extract_title_hint(&nodes, url_hint.as_deref());
    Ok(BrowserSnapshotV1 {
        schema_version: ANDROID_BROWSER_SCHEMA_V1.to_string(),
        package: snapshot.package.clone(),
        url_hint,
        title_hint,
        revision: snapshot.revision,
        state_hash: snapshot.state_hash.clone(),
        captured_at_ms: snapshot.captured_at_ms,
        screen: snapshot.screen,
        nodes,
        android: snapshot,
    })
}

/// Execute one guarded browser action and return the resulting semantic state.
/// This fuses action + observation into one host invocation and avoids a second
/// CLI startup in agent loops.
pub fn step(request: &BrowserRequest, action: ActionV1) -> Result<Value> {
    if action.expected_revision.is_none() || action.expected_state_hash.is_none() {
        return Err(AndroidError::InvalidInput(
            "Android browser actions require expectedRevision and expectedStateHash from the latest browser snapshot"
                .to_string(),
        ));
    }
    let before = snapshot(request)?;
    if action.expected_revision != Some(before.revision)
        || action.expected_state_hash.as_deref() != Some(before.state_hash.as_str())
    {
        return Err(AndroidError::StaleState {
            expected: action.expected_revision.unwrap_or_default(),
            actual: before.revision,
        });
    }
    let started = Instant::now();
    let response = execute(base_command(request, Command::Action { action }));
    if !response.ok {
        return Err(AndroidError::Backend(
            response
                .error
                .unwrap_or_else(|| "Android browser action failed".to_string()),
        ));
    }
    let receipt: ActionReceiptV1 = serde_json::from_value(response.result.unwrap_or(Value::Null))?;
    let after = snapshot(request)?;
    Ok(json!({
        "schemaVersion": ANDROID_BROWSER_SCHEMA_V1,
        "ok": true,
        "receipt": receipt,
        "snapshot": after,
        "timing": {"actObserveMicros": started.elapsed().as_micros()},
    }))
}

pub fn bench(request: &BrowserRequest, iterations: u32) -> Result<Value> {
    if !(1..=1_000).contains(&iterations) {
        return Err(AndroidError::InvalidInput(
            "browser bench iterations must be 1..=1000".to_string(),
        ));
    }
    let mut samples = Vec::with_capacity(iterations as usize);
    let mut last = None;
    for _ in 0..iterations {
        let started = Instant::now();
        last = Some(snapshot(request)?);
        samples.push(started.elapsed().as_micros());
    }
    samples.sort_unstable();
    let total = samples.iter().copied().sum::<u128>();
    let p95_index = ((samples.len() - 1) * 95) / 100;
    Ok(json!({
        "schemaVersion": ANDROID_BROWSER_SCHEMA_V1,
        "iterations": iterations,
        "transport": request.transport,
        "observation": {
            "minMicros": samples[0],
            "meanMicros": total / samples.len() as u128,
            "p95Micros": samples[p95_index],
            "maxMicros": samples[samples.len() - 1],
        },
        "lastSnapshot": last,
    }))
}

/// Return Chrome/WebView DevTools targets through a temporary loopback-only ADB
/// forward. This is read-only and always removes the forward before returning.
pub fn targets(serial: &str, socket: &str) -> Result<Value> {
    validate_socket(socket)?;
    let adb = adb_path();
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    drop(listener);

    let host = format!("tcp:{port}");
    let remote = format!("localabstract:{socket}");
    let output = ProcessCommand::new(&adb)
        .args(["-s", serial, "forward", &host, &remote])
        .output()?;
    if !output.status.success() {
        return Err(AndroidError::Backend(process_error(&output)));
    }
    let guard = ForwardGuard {
        adb,
        serial: serial.to_string(),
        host,
    };
    let body = http_get(port, "/json/list")?;
    let parsed: Value = serde_json::from_slice(&body)?;
    drop(guard);
    Ok(parsed)
}

fn observe(request: &BrowserRequest) -> Result<SnapshotV1> {
    let response = execute(base_command(request, Command::Snapshot { full: false }));
    if !response.ok {
        return Err(AndroidError::Backend(
            response
                .error
                .unwrap_or_else(|| "Android browser observation failed".to_string()),
        ));
    }
    serde_json::from_value(response.result.unwrap_or(Value::Null)).map_err(AndroidError::from)
}

fn base_command(request: &BrowserRequest, command: Command) -> CommandRequest {
    CommandRequest {
        id: crate::model::next_action_id("android-browser"),
        session_id: request.session_id.clone(),
        serial: request.serial.clone(),
        transport: request.transport.clone(),
        appium_url: None,
        appium_capabilities: None,
        command,
    }
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
                    || node.text.as_deref().is_some_and(|text| !text.trim().is_empty()))
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
            && matches!(node.role.as_str(), "android.widget.TextView" | "text" | "heading"))
        .then(|| label.to_string())
    })
}

fn validate_url(url: &str) -> Result<()> {
    if url.len() > 16_384
        || url.chars().any(|character| character.is_control())
        || url.chars().any(char::is_whitespace)
        || !(url.starts_with("https://") || url.starts_with("http://"))
    {
        return Err(AndroidError::InvalidInput(
            "Android browser URL must be a bounded HTTP(S) URL without whitespace or control characters"
                .to_string(),
        ));
    }
    let authority = url
        .split_once("://")
        .map(|(_, rest)| rest.split('/').next().unwrap_or(rest))
        .unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return Err(AndroidError::InvalidInput(
            "Android browser URLs must include a host and must not embed credentials".to_string(),
        ));
    }
    Ok(())
}

fn validate_package(package: &str) -> Result<()> {
    if package.is_empty()
        || !package.contains('.')
        || package
            .chars()
            .any(|character| !(character.is_ascii_alphanumeric() || matches!(character, '.' | '_')))
    {
        return Err(AndroidError::InvalidInput(
            "browser package must be an Android package identifier".to_string(),
        ));
    }
    Ok(())
}

fn validate_socket(socket: &str) -> Result<()> {
    if socket.is_empty()
        || socket.len() > 128
        || socket.chars().any(|character| {
            !(character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | ':' | '-'))
        })
    {
        return Err(AndroidError::InvalidInput(
            "CDP socket must be a bounded Android localabstract socket name".to_string(),
        ));
    }
    Ok(())
}

fn adb_path() -> PathBuf {
    if let Some(value) = env::var_os("TEMPERA_ANDROID_ADB") {
        return PathBuf::from(value);
    }
    if let Some(root) = env::var_os("ANDROID_SDK_ROOT").or_else(|| env::var_os("ANDROID_HOME")) {
        return PathBuf::from(root)
            .join("platform-tools")
            .join(if cfg!(windows) { "adb.exe" } else { "adb" });
    }
    PathBuf::from(if cfg!(windows) { "adb.exe" } else { "adb" })
}

struct ForwardGuard {
    adb: PathBuf,
    serial: String,
    host: String,
}

impl Drop for ForwardGuard {
    fn drop(&mut self) {
        let _ = ProcessCommand::new(&self.adb)
            .args(["-s", &self.serial, "forward", "--remove", &self.host])
            .output();
    }
}

fn http_get(port: u16, path: &str) -> Result<Vec<u8>> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nAccept: application/json\r\n\r\n"
    )?;
    stream.flush()?;
    let mut response = Vec::new();
    stream
        .take(MAX_HTTP_BYTES + 1)
        .read_to_end(&mut response)?;
    if response.len() as u64 > MAX_HTTP_BYTES {
        return Err(AndroidError::Backend(
            "Chrome DevTools target response exceeded 4 MiB".to_string(),
        ));
    }
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| AndroidError::Backend("Invalid Chrome DevTools HTTP response".to_string()))?;
    let header = String::from_utf8_lossy(&response[..header_end]);
    if !header.lines().next().is_some_and(|line| line.contains(" 200 ")) {
        return Err(AndroidError::Backend(format!(
            "Chrome DevTools target endpoint failed: {}",
            header.lines().next().unwrap_or("unknown response")
        )));
    }
    let body = response[header_end + 4..].to_vec();
    if header.to_ascii_lowercase().contains("transfer-encoding: chunked") {
        decode_chunked(&body)
    } else {
        Ok(body)
    }
}

fn decode_chunked(body: &[u8]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut cursor = 0usize;
    loop {
        let line_end = body[cursor..]
            .windows(2)
            .position(|window| window == b"\r\n")
            .map(|offset| cursor + offset)
            .ok_or_else(|| AndroidError::Backend("Invalid chunked CDP response".to_string()))?;
        let size_text = std::str::from_utf8(&body[cursor..line_end])
            .map_err(|_| AndroidError::Backend("Invalid chunk size encoding".to_string()))?;
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or(""), 16)
            .map_err(|_| AndroidError::Backend("Invalid chunk size".to_string()))?;
        cursor = line_end + 2;
        if size == 0 {
            break;
        }
        let end = cursor.saturating_add(size);
        if end + 2 > body.len() || &body[end..end + 2] != b"\r\n" {
            return Err(AndroidError::Backend(
                "Truncated chunked CDP response".to_string(),
            ));
        }
        output.extend_from_slice(&body[cursor..end]);
        cursor = end + 2;
    }
    Ok(output)
}

fn process_error(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        stderr
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_validation_rejects_unsafe_schemes_and_credentials() {
        assert!(validate_url("https://example.com/path").is_ok());
        assert!(validate_url("http://127.0.0.1:8080").is_ok());
        assert!(validate_url("javascript:alert(1)").is_err());
        assert!(validate_url("https://user:pass@example.com").is_err());
        assert!(validate_url("https://example.com/a b").is_err());
    }

    #[test]
    fn package_and_socket_validation_are_bounded() {
        assert!(validate_package("com.android.chrome").is_ok());
        assert!(validate_package("../chrome").is_err());
        assert!(validate_socket("chrome_devtools_remote").is_ok());
        assert!(validate_socket("../../socket").is_err());
    }

    #[test]
    fn chunked_target_response_decodes() {
        assert_eq!(
            decode_chunked(b"4\r\n[1,2\r\n1\r\n]\r\n0\r\n\r\n").unwrap(),
            b"[1,2]"
        );
    }

    #[test]
    fn browser_nodes_are_compact_and_password_free() {
        let mut password = NodeV1 {
            reference: "@e1".to_string(),
            backend_reference: None,
            role: "input".to_string(),
            label: "Password".to_string(),
            text: None,
            content_description: None,
            resource_id: None,
            bounds: crate::model::RectV1 {
                left: 0,
                top: 0,
                right: 10,
                bottom: 10,
            },
            enabled: true,
            clickable: true,
            editable: true,
            scrollable: false,
            password: true,
            actions: Vec::new(),
        };
        let visible = NodeV1 {
            reference: "@e2".to_string(),
            password: false,
            label: "Continue".to_string(),
            ..password.clone()
        };
        password.password = true;
        let compact = compact_nodes(&[password, visible]);
        assert_eq!(compact.len(), 1);
        assert_eq!(compact[0].reference, "@e2");
    }
}
