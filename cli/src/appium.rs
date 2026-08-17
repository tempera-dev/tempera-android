//! Optional generic W3C/Appium backend.
//!
//! Appium is deliberately outside the bridge hot path. It supports local
//! UiAutomator2 servers and hosted device labs which expose a W3C endpoint,
//! while direct ADB remains the independent zero-install fallback.

use crate::adb::{self, validate_guard, validate_sensitive};
use crate::error::{AndroidError, Result};
use crate::model::{ActionReceiptV1, ActionV1, SessionV1, SnapshotV1, CONTROL_SCHEMA_V1};
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct AppiumBackend {
    endpoint: String,
    capabilities: Value,
}

impl AppiumBackend {
    pub fn new(endpoint: &str, capabilities: Option<Value>) -> Result<Self> {
        let endpoint = endpoint.trim_end_matches('/').to_string();
        if !(endpoint.starts_with("https://") || endpoint.starts_with("http://")) {
            return Err(AndroidError::InvalidInput(
                "Appium URL must begin with https:// or http://".to_string(),
            ));
        }
        let capabilities = capabilities.unwrap_or_else(|| json!({"platformName": "Android"}));
        if !capabilities.is_object() {
            return Err(AndroidError::InvalidInput(
                "Appium capabilities must be a JSON object".to_string(),
            ));
        }
        reject_secret_capabilities(&capabilities)?;
        Ok(Self {
            endpoint,
            capabilities,
        })
    }

    pub fn status(&self) -> Result<Value> {
        self.get("/status")
    }

    pub fn observe(&self, session: &mut SessionV1) -> Result<SnapshotV1> {
        let id = self.ensure_session(session)?;
        let source = self
            .value(self.get(&format!("/session/{id}/source"))?)?
            .as_str()
            .ok_or_else(|| {
                AndroidError::Backend("Appium page source was not a string".to_string())
            })?
            .to_string();
        let nodes = adb::parse_hierarchy(&source)?;
        let rect = self.value(self.get(&format!("/session/{id}/window/rect"))?)?;
        let width = rect.get("width").and_then(Value::as_u64).unwrap_or(1080) as u32;
        let height = rect.get("height").and_then(Value::as_u64).unwrap_or(1920) as u32;
        let package = self.optional_string(&format!("/session/{id}/appium/device/current_package"));
        let activity =
            self.optional_string(&format!("/session/{id}/appium/device/current_activity"));
        let screen = [width, height];
        let state_hash = SnapshotV1::state_hash_for(&package, &activity, screen, &nodes);
        if session.last_state_hash.as_deref() != Some(state_hash.as_str()) {
            session.last_revision = session.last_revision.saturating_add(1).max(1);
            session.last_state_hash = Some(state_hash.clone());
        }
        session.updated_at_ms = SnapshotV1::now_ms();
        Ok(SnapshotV1 {
            schema_version: CONTROL_SCHEMA_V1.to_string(),
            session_id: session.session_id.clone(),
            serial: session.serial.clone(),
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
        let before = self.observe(session)?;
        validate_guard(action, &before)?;
        validate_sensitive(action, &before)?;
        let id = self.ensure_session(session)?;
        let started_at_ms = SnapshotV1::now_ms();
        match action.kind.as_str() {
            "tap" | "long_press" => {
                let (x, y) = coordinates(action, &before)?;
                self.pointer(&id, x, y, action.kind == "long_press")?;
            }
            "type" | "fill" => {
                if action.selector.is_some() {
                    let (x, y) = coordinates(action, &before)?;
                    self.pointer(&id, x, y, false)?;
                }
                let text = action.text.as_deref().ok_or_else(|| {
                    AndroidError::InvalidInput("type/fill requires text".to_string())
                })?;
                self.post(
                    &format!("/session/{id}/keys"),
                    json!({"text": text, "value": text.chars().map(|character| character.to_string()).collect::<Vec<_>>() }),
                )?;
            }
            "press" => {
                let key = action
                    .key
                    .as_deref()
                    .ok_or_else(|| AndroidError::InvalidInput("press requires key".to_string()))?;
                let code = android_keycode(key).ok_or_else(|| {
                    AndroidError::Unsupported(format!(
                        "Appium press supports BACK, HOME, ENTER, TAB, ESCAPE, or a numeric Android keycode; got {key:?}"
                    ))
                })?;
                self.post(
                    &format!("/session/{id}/appium/device/press_keycode"),
                    json!({"keycode": code}),
                )?;
            }
            "back" => {
                self.post(&format!("/session/{id}/back"), json!({}))?;
            }
            "home" => {
                self.post(
                    &format!("/session/{id}/appium/device/press_keycode"),
                    json!({"keycode": 3}),
                )?;
            }
            "swipe" | "scroll" => {
                let (x, y, end_x, end_y) = scroll_coordinates(
                    action.direction.as_deref().unwrap_or("down"),
                    before.screen,
                )?;
                self.pointer_drag(&id, x, y, end_x, end_y)?;
            }
            "wait" => std::thread::sleep(Duration::from_millis(
                action
                    .metadata
                    .get("milliseconds")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(250)
                    .min(10_000),
            )),
            other => {
                return Err(AndroidError::Unsupported(format!(
                    "Unsupported Appium action {other:?}"
                )))
            }
        }
        let after = self.observe(session)?;
        Ok(ActionReceiptV1 {
            schema_version: CONTROL_SCHEMA_V1.to_string(),
            action_id: action.action_id.clone(),
            kind: action.kind.clone(),
            ok: true,
            transport: "appium-w3c".to_string(),
            started_at_ms,
            completed_at_ms: SnapshotV1::now_ms(),
            before_revision: before.revision,
            after_revision: after.revision,
            before_state_hash: before.state_hash,
            after_state_hash: after.state_hash,
            detail: None,
        })
    }

    pub fn close(&self, session: &mut SessionV1) -> Result<bool> {
        let Some(id) = session.backend_session_id.take() else {
            return Ok(false);
        };
        self.delete(&format!("/session/{id}"))?;
        Ok(true)
    }

    fn ensure_session(&self, session: &mut SessionV1) -> Result<String> {
        if let Some(id) = &session.backend_session_id {
            return Ok(id.clone());
        }
        let response = self.post(
            "/session",
            json!({"capabilities": {"alwaysMatch": self.capabilities}}),
        )?;
        let id = response
            .get("sessionId")
            .and_then(Value::as_str)
            .or_else(|| {
                response
                    .get("value")
                    .and_then(|value| value.get("sessionId"))
                    .and_then(Value::as_str)
            })
            .ok_or_else(|| {
                AndroidError::Backend("Appium did not return a W3C sessionId".to_string())
            })?
            .to_string();
        session.backend_session_id = Some(id.clone());
        Ok(id)
    }

    fn pointer(&self, id: &str, x: u32, y: u32, hold: bool) -> Result<()> {
        let pause = if hold { 700 } else { 0 };
        self.post(&format!("/session/{id}/actions"), json!({"actions":[{"type":"pointer","id":"tempera-finger","parameters":{"pointerType":"touch"},"actions":[{"type":"pointerMove","duration":0,"x":x,"y":y},{"type":"pointerDown","button":0},{"type":"pause","duration":pause},{"type":"pointerUp","button":0}]}]}))?;
        Ok(())
    }

    fn pointer_drag(&self, id: &str, x: u32, y: u32, end_x: u32, end_y: u32) -> Result<()> {
        self.post(&format!("/session/{id}/actions"), json!({"actions":[{"type":"pointer","id":"tempera-finger","parameters":{"pointerType":"touch"},"actions":[{"type":"pointerMove","duration":0,"x":x,"y":y},{"type":"pointerDown","button":0},{"type":"pointerMove","duration":300,"x":end_x,"y":end_y},{"type":"pointerUp","button":0}]}]}))?;
        Ok(())
    }

    fn optional_string(&self, path: &str) -> String {
        self.get(path)
            .ok()
            .and_then(|value| self.value(value).ok())
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default()
    }

    fn get(&self, path: &str) -> Result<Value> {
        self.request("GET", path, None)
    }
    fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.request("POST", path, Some(body))
    }
    fn delete(&self, path: &str) -> Result<Value> {
        self.request("DELETE", path, None)
    }
    fn request(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let url = format!("{}{}", self.endpoint, path);
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(20))
            .build();
        let request = match method {
            "GET" => agent.get(&url),
            "POST" => agent.post(&url),
            "DELETE" => agent.delete(&url),
            _ => unreachable!(),
        };
        let response = match body {
            Some(body) => request.send_json(body),
            None => request.call(),
        }
        .map_err(|error| {
            AndroidError::Backend(format!("Appium {method} {path} failed: {error}"))
        })?;
        response.into_json().map_err(|error| {
            AndroidError::Backend(format!("Appium {method} {path} was not JSON: {error}"))
        })
    }
    fn value(&self, response: Value) -> Result<Value> {
        response
            .get("value")
            .cloned()
            .ok_or_else(|| AndroidError::Backend("Appium response omitted value".to_string()))
    }
}

pub fn status(endpoint: &str) -> Result<Value> {
    AppiumBackend::new(endpoint, None)?.status()
}

fn reject_secret_capabilities(capabilities: &Value) -> Result<()> {
    if contains_credential_key(capabilities) {
        return Err(AndroidError::InvalidInput("Appium capabilities with credential-like keys must be supplied by the provider integration, not tempera-android.json or TEMPERA_ANDROID_APPIUM_CAPABILITIES".to_string()));
    }
    Ok(())
}

fn contains_credential_key(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, value)| {
            let key = key.to_ascii_lowercase();
            key.contains("password")
                || key.contains("secret")
                || key.contains("token")
                || key.contains("apikey")
                || key.contains("accesskey")
                || key.contains("username")
                || contains_credential_key(value)
        }),
        Value::Array(values) => values.iter().any(contains_credential_key),
        _ => false,
    }
}

fn coordinates(action: &ActionV1, snapshot: &SnapshotV1) -> Result<(u32, u32)> {
    if let Some([x, y]) = action.coordinates {
        return Ok((x, y));
    }
    let selector = action.selector.as_deref().ok_or_else(|| {
        AndroidError::InvalidInput("tap/long_press requires selector or coordinates".to_string())
    })?;
    snapshot
        .node(selector)
        .map(|node| node.bounds.center())
        .ok_or_else(|| {
            AndroidError::InvalidInput(format!(
                "No current Android node matches {selector:?}; take a fresh snapshot"
            ))
        })
}

fn scroll_coordinates(direction: &str, [width, height]: [u32; 2]) -> Result<(u32, u32, u32, u32)> {
    match direction {
        "down" => Ok((width / 2, height * 3 / 4, width / 2, height / 4)),
        "up" => Ok((width / 2, height / 4, width / 2, height * 3 / 4)),
        "left" => Ok((width * 3 / 4, height / 2, width / 4, height / 2)),
        "right" => Ok((width / 4, height / 2, width * 3 / 4, height / 2)),
        other => Err(AndroidError::InvalidInput(format!(
            "Unknown scroll direction {other:?}"
        ))),
    }
}

fn android_keycode(key: &str) -> Option<u32> {
    match key.trim().to_ascii_uppercase().as_str() {
        "BACK" | "KEYCODE_BACK" => Some(4),
        "HOME" | "KEYCODE_HOME" => Some(3),
        "ENTER" | "KEYCODE_ENTER" => Some(66),
        "TAB" | "KEYCODE_TAB" => Some(61),
        "ESCAPE" | "ESC" | "KEYCODE_ESCAPE" => Some(111),
        value => value.strip_prefix("KEYCODE_").unwrap_or(value).parse().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_requires_an_http_scheme() {
        assert!(AppiumBackend::new("appium.example.test", None).is_err());
    }

    #[test]
    fn configured_secrets_are_rejected() {
        assert!(AppiumBackend::new(
            "http://localhost:4723",
            Some(json!({"appium:password":"no"}))
        )
        .is_err());
        assert!(AppiumBackend::new(
            "http://localhost:4723",
            Some(json!({"vendor:options":{"accessKey":"no"}}))
        )
        .is_err());
    }

    #[test]
    fn scroll_directions_are_bounded() {
        assert!(scroll_coordinates("northeast", [100, 200]).is_err());
    }
}
