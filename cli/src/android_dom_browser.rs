//! Persistent host client for the dedicated Tempera Android WebView browser.
//!
//! This path is intentionally separate from generic Android computer use. It
//! talks only to the browser app's loopback control server through an ADB
//! forward, authenticates with the app-private bearer token, and exposes no
//! arbitrary shell or JavaScript primitive.

use crate::adb::AdbBackend;
use crate::error::{AndroidError, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

pub const PACKAGE: &str = "dev.tempera.android.browser";
pub const ACTIVITY: &str = "dev.tempera.android.browser.TemperaBrowserActivity";
pub const DEVICE_PORT: u16 = 7433;
pub const SCHEMA_VERSION: &str = "tempera.android.dom-browser-client/v1";
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_HEADER_LINE: usize = 16 * 1024;
const MAX_HEADERS: usize = 96;

pub struct DomBrowserClient {
    serial: String,
    token: String,
    host_port: u16,
    adb: AdbBackend,
    connection: Option<BufReader<TcpStream>>,
    sequence: u64,
}

impl DomBrowserClient {
    pub fn connect(serial: &str) -> Result<Self> {
        let adb = AdbBackend::new(serial)?;
        adb.ensure_ready()?;
        adb.shell(&["am", "start", "-n", &format!("{PACKAGE}/{ACTIVITY}")])?;
        let token = read_token(&adb)?;
        let host_port = unused_port()?;
        adb.forward(host_port, DEVICE_PORT)?;
        Ok(Self {
            serial: serial.to_string(),
            token,
            host_port,
            adb,
            connection: None,
            sequence: 0,
        })
    }

    pub fn serial(&self) -> &str {
        &self.serial
    }

    pub fn host_port(&self) -> u16 {
        self.host_port
    }

    pub fn health(&mut self) -> Result<Value> {
        self.request_readonly("GET", "/v1/health", None)
    }

    pub fn snapshot(&mut self) -> Result<Value> {
        self.request_readonly("GET", "/v1/snapshot", None)
    }

    pub fn snapshot_delta(&mut self, previous_state_hash: &str) -> Result<Value> {
        if previous_state_hash.is_empty() || previous_state_hash.len() > 256 {
            return Err(AndroidError::InvalidInput(
                "snapshot delta requires a bounded previousStateHash".to_string(),
            ));
        }
        self.request_readonly(
            "POST",
            "/v1/snapshot-delta",
            Some(json!({"previousStateHash": previous_state_hash})),
        )
    }

    pub fn navigate(&mut self, url: &str) -> Result<Value> {
        validate_url(url)?;
        self.request_mutating("POST", "/v1/navigate", Some(json!({"url": url})))
    }

    pub fn action(&mut self, action: Value) -> Result<Value> {
        validate_action(&action)?;
        self.request_mutating("POST", "/v1/action", Some(action))
    }

    pub fn act_observe(&mut self, action: Value, settle_ms: u64) -> Result<Value> {
        validate_action(&action)?;
        if settle_ms > 2_000 {
            return Err(AndroidError::InvalidInput(
                "Android DOM browser settleMs must be <= 2000".to_string(),
            ));
        }
        self.request_mutating(
            "POST",
            "/v1/act-observe",
            Some(json!({"action": action, "settleMs": settle_ms})),
        )
    }

    pub fn wait_for(
        &mut self,
        previous_state_hash: Option<&str>,
        exact_text: Option<&str>,
        timeout_ms: u64,
    ) -> Result<Value> {
        if !(1..=10_000).contains(&timeout_ms) {
            return Err(AndroidError::InvalidInput(
                "Android DOM browser wait timeoutMs must be 1..=10000".to_string(),
            ));
        }
        self.request_readonly(
            "POST",
            "/v1/wait",
            Some(json!({
                "previousStateHash": previous_state_hash.unwrap_or(""),
                "exactText": exact_text.unwrap_or(""),
                "timeoutMs": timeout_ms,
            })),
        )
    }

    pub fn benchmark_snapshots(&mut self, iterations: u32) -> Result<Value> {
        if !(1..=1_000).contains(&iterations) {
            return Err(AndroidError::InvalidInput(
                "snapshot benchmark iterations must be 1..=1000".to_string(),
            ));
        }
        let mut samples = Vec::with_capacity(iterations as usize);
        let mut bytes = Vec::with_capacity(iterations as usize);
        let mut last = Value::Null;
        for _ in 0..iterations {
            let started = Instant::now();
            last = self.snapshot()?;
            samples.push(started.elapsed().as_micros());
            bytes.push(serde_json::to_vec(&last)?.len() as u64);
        }
        samples.sort_unstable();
        bytes.sort_unstable();
        let percentile = |values: &[u128], percentile: usize| -> u128 {
            values[((values.len() - 1) * percentile) / 100]
        };
        let byte_percentile = |values: &[u64], percentile: usize| -> u64 {
            values[((values.len() - 1) * percentile) / 100]
        };
        Ok(json!({
            "schemaVersion": SCHEMA_VERSION,
            "iterations": iterations,
            "transport": "adb-forward-persistent-http",
            "latencyMicros": {
                "min": samples[0],
                "p50": percentile(&samples, 50),
                "p95": percentile(&samples, 95),
                "p99": percentile(&samples, 99),
                "max": samples[samples.len() - 1],
                "mean": samples.iter().sum::<u128>() / samples.len() as u128,
            },
            "payloadBytes": {
                "p50": byte_percentile(&bytes, 50),
                "p95": byte_percentile(&bytes, 95),
                "max": bytes[bytes.len() - 1],
            },
            "lastSnapshot": last,
        }))
    }

    fn request_readonly(&mut self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        match self.request_once(method, path, body.as_ref()) {
            Ok(value) => Ok(value),
            Err(first) => {
                // Read-only calls may be replayed after an ambiguous transport
                // failure because they cannot create browser-side effects.
                self.connection = None;
                self.request_once(method, path, body.as_ref())
                    .map_err(|second| {
                        AndroidError::Backend(format!(
                        "Android DOM browser read failed ({first}); reconnect failed ({second})"
                    ))
                    })
            }
        }
    }

    fn request_mutating(&mut self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        // Never replay a mutating request after transmission begins. The
        // document-state guard and action receipt are used for reconciliation.
        self.request_once(method, path, body.as_ref()).map_err(|error| {
            self.connection = None;
            AndroidError::Backend(format!(
                "Android DOM browser mutation failed; delivery may be unknown and was not replayed: {error}"
            ))
        })
    }

    fn request_once(&mut self, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
        self.sequence = self.sequence.wrapping_add(1);
        let body_bytes = match body {
            Some(value) => serde_json::to_vec(value)?,
            None => Vec::new(),
        };
        if body_bytes.len() > 1024 * 1024 {
            return Err(AndroidError::InvalidInput(
                "Android DOM browser request exceeds 1 MiB".to_string(),
            ));
        }
        self.ensure_connection()?;
        let request_id = format!("host-{}-{}", std::process::id(), self.sequence);
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nAccept: application/json\r\nX-Tempera-Request-Id: {request_id}\r\nConnection: keep-alive\r\n",
            self.host_port, self.token
        );
        if !body_bytes.is_empty() {
            request.push_str("Content-Type: application/json\r\n");
        }
        request.push_str(&format!("Content-Length: {}\r\n\r\n", body_bytes.len()));

        let connection = self.connection.as_mut().expect("connection established");
        connection.get_mut().write_all(request.as_bytes())?;
        if !body_bytes.is_empty() {
            connection.get_mut().write_all(&body_bytes)?;
        }
        connection.get_mut().flush()?;
        let response = read_http_response(connection)?;
        if response.connection_close {
            self.connection = None;
        }
        if response.status != 200 {
            return Err(AndroidError::Backend(format!(
                "Android DOM browser returned HTTP {}: {}",
                response.status,
                String::from_utf8_lossy(&response.body)
            )));
        }
        let value: Value = serde_json::from_slice(&response.body)?;
        if value.get("ok").and_then(Value::as_bool) == Some(false) {
            return Err(AndroidError::Backend(
                value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("Android DOM browser request failed")
                    .to_string(),
            ));
        }
        Ok(value)
    }

    fn ensure_connection(&mut self) -> Result<()> {
        if self.connection.is_some() {
            return Ok(());
        }
        let address = SocketAddr::from(([127, 0, 0, 1], self.host_port));
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_secs(12)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        self.connection = Some(BufReader::new(stream));
        Ok(())
    }
}

impl Drop for DomBrowserClient {
    fn drop(&mut self) {
        self.connection = None;
        let _ = self.adb.remove_forward(self.host_port);
    }
}

struct HttpResponse {
    status: u16,
    body: Vec<u8>,
    connection_close: bool,
}

fn read_http_response(reader: &mut BufReader<TcpStream>) -> Result<HttpResponse> {
    let status_line = read_bounded_header_line(reader)?;
    let mut parts = status_line.split_whitespace();
    let version = parts.next().unwrap_or_default();
    let status = parts
        .next()
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| AndroidError::Backend("Invalid Android browser HTTP status".to_string()))?;
    if !version.starts_with("HTTP/1.") {
        return Err(AndroidError::Backend(
            "Android browser returned unsupported HTTP version".to_string(),
        ));
    }

    let mut content_length = None;
    let mut connection_close = false;
    for _ in 0..MAX_HEADERS {
        let line = read_bounded_header_line(reader)?;
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').ok_or_else(|| {
            AndroidError::Backend("Invalid Android browser HTTP header".to_string())
        })?;
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => {
                let length = value.trim().parse::<usize>().map_err(|_| {
                    AndroidError::Backend("Invalid Android browser Content-Length".to_string())
                })?;
                if length > MAX_RESPONSE_BYTES {
                    return Err(AndroidError::Backend(
                        "Android browser response exceeds 4 MiB".to_string(),
                    ));
                }
                content_length = Some(length);
            }
            "connection" => {
                connection_close = value.trim().eq_ignore_ascii_case("close");
            }
            _ => {}
        }
    }
    let length = content_length.ok_or_else(|| {
        AndroidError::Backend("Android browser response omitted Content-Length".to_string())
    })?;
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body)?;
    Ok(HttpResponse {
        status,
        body,
        connection_close,
    })
}

fn read_bounded_header_line(reader: &mut BufReader<TcpStream>) -> Result<String> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Err(AndroidError::Backend(
                "Android browser control connection closed".to_string(),
            ));
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if line.len().saturating_add(take) > MAX_HEADER_LINE {
            return Err(AndroidError::Backend(
                "Android browser HTTP header exceeds limit".to_string(),
            ));
        }
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        if line.last() == Some(&b'\n') {
            break;
        }
    }
    while matches!(line.last(), Some(b'\r' | b'\n')) {
        line.pop();
    }
    String::from_utf8(line)
        .map_err(|_| AndroidError::Backend("Android browser HTTP header is not UTF-8".to_string()))
}

fn read_token(adb: &AdbBackend) -> Result<String> {
    let token = adb
        .shell(&["run-as", PACKAGE, "cat", "files/control-token"])?
        .trim()
        .to_string();
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AndroidError::Backend(
            "Dedicated Android browser control token is unavailable; launch the browser app first"
                .to_string(),
        ));
    }
    Ok(token)
}

fn unused_port() -> Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

fn validate_url(url: &str) -> Result<()> {
    if url.len() > 16_384
        || url.chars().any(char::is_whitespace)
        || url.chars().any(char::is_control)
        || !(url.starts_with("https://") || url == "about:blank")
    {
        return Err(AndroidError::InvalidInput(
            "Dedicated Android browser accepts only bounded HTTPS URLs or about:blank".to_string(),
        ));
    }
    Ok(())
}

fn validate_action(action: &Value) -> Result<()> {
    let object = action.as_object().ok_or_else(|| {
        AndroidError::InvalidInput("Android DOM browser action must be an object".to_string())
    })?;
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !matches!(
        kind,
        "tap" | "click" | "fill" | "type" | "scroll" | "scrollIntoView" | "back"
    ) {
        return Err(AndroidError::InvalidInput(format!(
            "Unsupported Android DOM browser action {kind:?}"
        )));
    }
    let hash = object
        .get("expectedStateHash")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if hash.is_empty() || hash.len() > 256 {
        return Err(AndroidError::InvalidInput(
            "Android DOM browser action requires expectedStateHash".to_string(),
        ));
    }
    if !matches!(kind, "scroll" | "back") {
        let reference = object
            .get("ref")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !reference.starts_with("@d") || reference.len() > 32 {
            return Err(AndroidError::InvalidInput(
                "Android DOM browser action requires a bounded @d reference".to_string(),
            ));
        }
    }
    if let Some(text) = object.get("text").and_then(Value::as_str) {
        if text.len() > 1024 * 1024 {
            return Err(AndroidError::InvalidInput(
                "Android DOM browser action text exceeds 1 MiB".to_string(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedicated_browser_rejects_non_https_navigation() {
        assert!(validate_url("https://example.com").is_ok());
        assert!(validate_url("about:blank").is_ok());
        assert!(validate_url("http://example.com").is_err());
        assert!(validate_url("javascript:alert(1)").is_err());
    }

    #[test]
    fn mutation_requires_state_hash_and_dom_reference() {
        assert!(validate_action(&json!({
            "kind": "tap",
            "ref": "@d2",
            "expectedStateHash": "fnv1a64:1234"
        }))
        .is_ok());
        assert!(validate_action(&json!({"kind": "tap", "ref": "@d2"})).is_err());
        assert!(validate_action(&json!({
            "kind": "tap",
            "ref": "@e2",
            "expectedStateHash": "fnv1a64:1234"
        }))
        .is_err());
    }
}
