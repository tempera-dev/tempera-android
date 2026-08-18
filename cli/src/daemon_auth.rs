use crate::command::{Command, CommandRequest};
use crate::error::{AndroidError, Result};
use serde::Deserialize;
use std::env;

const TOKEN_ENV: &str = "TEMPERA_ANDROID_DAEMON_TOKEN";
const SCOPE_ENV: &str = "TEMPERA_ANDROID_DAEMON_SCOPE";
const SESSION_ENV: &str = "TEMPERA_ANDROID_DAEMON_SESSION_ID";
const SERIAL_ENV: &str = "TEMPERA_ANDROID_DAEMON_SERIAL";
const ADMIN_CONFIRM_ENV: &str = "TEMPERA_ANDROID_DAEMON_ALLOW_ADMIN";
const MIN_TOKEN_BYTES: usize = 32;
const MAX_TOKEN_BYTES: usize = 4_096;
const MAX_AUTHORITY_TEXT_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DaemonScope {
    TemperaUse,
    Admin,
}

pub(crate) struct DaemonAuthority {
    token: Box<[u8]>,
    scope: DaemonScope,
    session_id: Option<String>,
    serial: Option<String>,
}

impl std::fmt::Debug for DaemonAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DaemonAuthority")
            .field("token", &"[REDACTED]")
            .field("scope", &self.scope)
            .field("session_id", &self.session_id)
            .field("serial", &self.serial)
            .finish()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthenticatedCommandRequest {
    capability_token: String,
    #[serde(flatten)]
    request: CommandRequest,
}

impl DaemonAuthority {
    pub(crate) fn from_environment() -> Result<Self> {
        let token = env::var(TOKEN_ENV).map_err(|_| {
            AndroidError::InvalidInput(format!(
                "{TOKEN_ENV} is required; loopback is not an authentication boundary"
            ))
        })?;
        let token = validate_token(token)?;
        let scope = match env::var(SCOPE_ENV)
            .unwrap_or_else(|_| "tempera-use".to_string())
            .as_str()
        {
            "tempera-use" => DaemonScope::TemperaUse,
            "admin" if env::var(ADMIN_CONFIRM_ENV).as_deref() == Ok("1") => DaemonScope::Admin,
            "admin" => {
                return Err(AndroidError::InvalidInput(format!(
                    "admin daemon scope additionally requires {ADMIN_CONFIRM_ENV}=1"
                )))
            }
            value => {
                return Err(AndroidError::InvalidInput(format!(
                    "unsupported {SCOPE_ENV} value {value:?}"
                )))
            }
        };
        let session_id = optional_bounded_environment_value(SESSION_ENV)?;
        if scope == DaemonScope::TemperaUse && session_id.is_none() {
            return Err(AndroidError::InvalidInput(format!(
                "{SESSION_ENV} is required for the tempera-use daemon scope"
            )));
        }
        let serial = optional_bounded_environment_value(SERIAL_ENV)?;
        Ok(Self {
            token,
            scope,
            session_id,
            serial,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        token: &str,
        scope: DaemonScope,
        session_id: Option<&str>,
        serial: Option<&str>,
    ) -> Result<Self> {
        Ok(Self {
            token: validate_token(token.to_string())?,
            scope,
            session_id: session_id.map(ToString::to_string),
            serial: serial.map(ToString::to_string),
        })
    }

    /// Decode one owned request and erase the complete wire frame on every
    /// success or rejection path. `Vec::clear` alone would retain the token in
    /// reusable capacity until that allocation was overwritten or released.
    pub(crate) fn authenticate_frame(&self, frame: &mut [u8]) -> Result<CommandRequest> {
        let decoded = serde_json::from_slice::<AuthenticatedCommandRequest>(frame).map_err(|_| {
            AndroidError::InvalidInput("invalid authenticated daemon request".to_string())
        });
        frame.fill(0);
        let wire = decoded?;

        let mut provided = wire.capability_token.into_bytes();
        let authenticated = constant_time_eq(&self.token, &provided);
        provided.fill(0);
        if !authenticated {
            return Err(AndroidError::InvalidInput(
                "Android daemon authentication failed".to_string(),
            ));
        }
        self.authorize_request(&wire.request)?;
        Ok(wire.request)
    }

    fn authorize_request(&self, request: &CommandRequest) -> Result<()> {
        if let Some(expected) = &self.session_id {
            if &request.session_id != expected {
                return Err(AndroidError::InvalidInput(
                    "Android daemon session authority mismatch".to_string(),
                ));
            }
        }
        match (&self.serial, request.serial.as_deref()) {
            (Some(expected), Some(actual)) if actual == expected => {}
            (Some(_), _) => {
                return Err(AndroidError::InvalidInput(
                    "Android daemon device authority mismatch".to_string(),
                ))
            }
            (None, Some(_)) if self.scope == DaemonScope::TemperaUse => {
                return Err(AndroidError::InvalidInput(format!(
                    "explicit device selection requires a {SERIAL_ENV} authority binding"
                )))
            }
            (None, _) => {}
        }
        if self.scope == DaemonScope::TemperaUse
            && (!matches!(request.transport.as_str(), "auto" | "adb" | "bridge")
                || request.appium_url.is_some()
                || request.appium_capabilities.is_some())
        {
            return Err(AndroidError::Unsupported(
                "transport is outside this daemon token's authority".to_string(),
            ));
        }
        if !self.scope.permits(&request.command) {
            return Err(AndroidError::Unsupported(
                "command is outside this daemon token's authority".to_string(),
            ));
        }
        Ok(())
    }
}

impl Drop for DaemonAuthority {
    fn drop(&mut self) {
        self.token.fill(0);
    }
}

impl DaemonScope {
    fn permits(self, command: &Command) -> bool {
        match self {
            Self::Admin => true,
            Self::TemperaUse => matches!(
                command,
                Command::Snapshot { .. }
                    | Command::Screenshot { persist: false, .. }
                    | Command::Action { .. }
                    | Command::Batch { .. }
                    | Command::SessionClose
                    | Command::State
                    | Command::BridgeStatus
            ),
        }
    }
}

fn optional_bounded_environment_value(name: &str) -> Result<Option<String>> {
    let value = match env::var(name) {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => return Ok(None),
        Err(error) => {
            return Err(AndroidError::InvalidInput(format!(
                "could not read {name}: {error}"
            )))
        }
    };
    if value.trim().is_empty() || value.len() > MAX_AUTHORITY_TEXT_BYTES {
        return Err(AndroidError::InvalidInput(format!(
            "{name} must contain 1-{MAX_AUTHORITY_TEXT_BYTES} bytes when configured"
        )));
    }
    Ok(Some(value))
}

fn validate_token(token: String) -> Result<Box<[u8]>> {
    let mut bytes = token.into_bytes();
    if bytes.len() < MIN_TOKEN_BYTES
        || bytes.len() > MAX_TOKEN_BYTES
        || bytes
            .iter()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        bytes.fill(0);
        return Err(AndroidError::InvalidInput(format!(
            "{TOKEN_ENV} must contain {MIN_TOKEN_BYTES}-{MAX_TOKEN_BYTES} non-whitespace bytes"
        )));
    }
    Ok(bytes.into_boxed_slice())
}

#[inline(never)]
fn constant_time_eq(expected: &[u8], provided: &[u8]) -> bool {
    let max_len = expected.len().max(provided.len());
    let mut difference = expected.len() ^ provided.len();
    for index in 0..max_len {
        let left = expected.get(index).copied().unwrap_or(0);
        let right = provided.get(index).copied().unwrap_or(0);
        difference |= usize::from(left ^ right);
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn frame(
        token: &str,
        session_id: &str,
        serial: Option<&str>,
        transport: &str,
        command: Value,
    ) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "capabilityToken": token,
            "id": "request-1",
            "sessionId": session_id,
            "serial": serial,
            "transport": transport,
            "command": command,
        }))
        .unwrap_or_else(|error| panic!("frame encoding: {error}"))
    }

    fn authenticate(
        authority: &DaemonAuthority,
        token: &str,
        session_id: &str,
        serial: Option<&str>,
        transport: &str,
        command: Value,
    ) -> Result<CommandRequest> {
        let mut encoded = frame(token, session_id, serial, transport, command);
        let result = authority.authenticate_frame(&mut encoded);
        assert!(encoded.iter().all(|byte| *byte == 0));
        result
    }

    #[test]
    fn valid_bound_token_and_allowed_command_are_accepted() -> Result<()> {
        let authority = DaemonAuthority::for_test(
            TOKEN,
            DaemonScope::TemperaUse,
            Some("session-1"),
            Some("device-1"),
        )?;
        let request = authenticate(
            &authority,
            TOKEN,
            "session-1",
            Some("device-1"),
            "bridge",
            json!({"name": "snapshot", "arguments": {"full": false}}),
        )?;
        assert_eq!(request.session_id, "session-1");
        assert_eq!(request.serial.as_deref(), Some("device-1"));
        Ok(())
    }

    #[test]
    fn wrong_token_session_and_device_are_rejected() -> Result<()> {
        let authority = DaemonAuthority::for_test(
            TOKEN,
            DaemonScope::TemperaUse,
            Some("session-1"),
            Some("device-1"),
        )?;
        assert!(authenticate(
            &authority,
            "fedcba9876543210fedcba9876543210",
            "session-1",
            Some("device-1"),
            "auto",
            json!({"name": "state"}),
        )
        .is_err());
        assert!(authenticate(
            &authority,
            TOKEN,
            "session-2",
            Some("device-1"),
            "auto",
            json!({"name": "state"}),
        )
        .is_err());
        assert!(authenticate(
            &authority,
            TOKEN,
            "session-1",
            Some("device-2"),
            "auto",
            json!({"name": "state"}),
        )
        .is_err());
        Ok(())
    }

    #[test]
    fn unbound_tempera_use_token_cannot_select_an_explicit_device() -> Result<()> {
        let authority =
            DaemonAuthority::for_test(TOKEN, DaemonScope::TemperaUse, Some("session-1"), None)?;
        assert!(authenticate(
            &authority,
            TOKEN,
            "session-1",
            Some("device-1"),
            "auto",
            json!({"name": "state"}),
        )
        .is_err());
        Ok(())
    }

    #[test]
    fn tempera_use_scope_rejects_admin_persistent_and_appium_commands() -> Result<()> {
        let authority =
            DaemonAuthority::for_test(TOKEN, DaemonScope::TemperaUse, Some("session-1"), None)?;
        for command in [
            json!({"name": "deviceReset", "arguments": {"name": "device", "confirmed": true}}),
            json!({"name": "clipboardGet"}),
            json!({"name": "screenshot", "arguments": {"path": "/tmp/x.png", "persist": true}}),
        ] {
            assert!(authenticate(
                &authority,
                TOKEN,
                "session-1",
                None,
                "auto",
                command,
            )
            .is_err());
        }
        let mut appium: Value = serde_json::from_slice(&frame(
            TOKEN,
            "session-1",
            None,
            "appium",
            json!({"name": "state"}),
        ))?;
        appium["appiumUrl"] = json!("http://127.0.0.1:4723");
        let mut encoded = serde_json::to_vec(&appium)?;
        let result = authority.authenticate_frame(&mut encoded);
        assert!(encoded.iter().all(|byte| *byte == 0));
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn malformed_frame_is_erased_before_rejection() -> Result<()> {
        let authority =
            DaemonAuthority::for_test(TOKEN, DaemonScope::TemperaUse, Some("session-1"), None)?;
        let mut malformed = TOKEN.as_bytes().to_vec();
        assert!(authority.authenticate_frame(&mut malformed).is_err());
        assert!(malformed.iter().all(|byte| *byte == 0));
        Ok(())
    }

    #[test]
    fn authority_debug_output_never_contains_token() -> Result<()> {
        let authority =
            DaemonAuthority::for_test(TOKEN, DaemonScope::TemperaUse, Some("session-1"), None)?;
        let debug = format!("{authority:?}");
        assert!(!debug.contains(TOKEN));
        assert!(debug.contains("REDACTED"));
        Ok(())
    }

    #[test]
    fn constant_time_comparison_handles_length_mismatch() {
        assert!(constant_time_eq(TOKEN.as_bytes(), TOKEN.as_bytes()));
        assert!(!constant_time_eq(TOKEN.as_bytes(), b"short"));
        assert!(!constant_time_eq(
            TOKEN.as_bytes(),
            b"0123456789abcdef0123456789abcdee"
        ));
    }
}
