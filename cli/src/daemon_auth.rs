use crate::command::{Command, CommandRequest};
use crate::error::{AndroidError, Result};
use serde::Deserialize;
use std::env;

const TOKEN_ENV: &str = "TEMPERA_ANDROID_DAEMON_TOKEN";
const SCOPE_ENV: &str = "TEMPERA_ANDROID_DAEMON_SCOPE";
const SESSION_ENV: &str = "TEMPERA_ANDROID_DAEMON_SESSION_ID";
const ADMIN_CONFIRM_ENV: &str = "TEMPERA_ANDROID_DAEMON_ALLOW_ADMIN";
const MIN_TOKEN_BYTES: usize = 32;
const MAX_TOKEN_BYTES: usize = 4_096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DaemonScope {
    TemperaUse,
    Admin,
}

pub(crate) struct DaemonAuthority {
    token: Box<[u8]>,
    scope: DaemonScope,
    session_id: Option<String>,
}

impl std::fmt::Debug for DaemonAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DaemonAuthority")
            .field("token", &"[REDACTED]")
            .field("scope", &self.scope)
            .field("session_id", &self.session_id)
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
            "admin" if env::var(ADMIN_CONFIRM_ENV).as_deref() == Ok("1") => {
                DaemonScope::Admin
            }
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
        let session_id = env::var(SESSION_ENV).ok();
        if session_id
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 256)
        {
            return Err(AndroidError::InvalidInput(format!(
                "{SESSION_ENV} must contain 1-256 bytes when configured"
            )));
        }
        Ok(Self {
            token,
            scope,
            session_id,
        })
    }

    #[cfg(test)]
    fn for_test(token: &str, scope: DaemonScope, session_id: Option<&str>) -> Result<Self> {
        Ok(Self {
            token: validate_token(token.to_string())?,
            scope,
            session_id: session_id.map(ToString::to_string),
        })
    }

    pub(crate) fn authenticate_frame(&self, frame: &[u8]) -> Result<CommandRequest> {
        let wire: AuthenticatedCommandRequest = serde_json::from_slice(frame).map_err(|error| {
            AndroidError::InvalidInput(format!("Invalid authenticated daemon request: {error}"))
        })?;
        let mut provided = wire.capability_token.into_bytes();
        let authenticated = constant_time_eq(&self.token, &provided);
        provided.fill(0);
        if !authenticated {
            return Err(AndroidError::InvalidInput(
                "Android daemon authentication failed".to_string(),
            ));
        }
        if let Some(expected) = &self.session_id {
            if &wire.request.session_id != expected {
                return Err(AndroidError::InvalidInput(
                    "Android daemon session authority mismatch".to_string(),
                ));
            }
        }
        if !self.scope.permits(&wire.request.command) {
            return Err(AndroidError::Unsupported(
                "command is outside this daemon token's authority".to_string(),
            ));
        }
        Ok(wire.request)
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

fn validate_token(token: String) -> Result<Box<[u8]>> {
    let bytes = token.into_bytes();
    if bytes.len() < MIN_TOKEN_BYTES
        || bytes.len() > MAX_TOKEN_BYTES
        || bytes
            .iter()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
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
    use serde_json::json;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn frame(token: &str, session_id: &str, command: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "capabilityToken": token,
            "id": "request-1",
            "sessionId": session_id,
            "transport": "auto",
            "command": command,
        }))
        .unwrap_or_else(|error| panic!("frame encoding: {error}"))
    }

    #[test]
    fn valid_token_and_allowed_command_are_accepted() -> Result<()> {
        let authority =
            DaemonAuthority::for_test(TOKEN, DaemonScope::TemperaUse, Some("session-1"))?;
        let request = authority.authenticate_frame(&frame(
            TOKEN,
            "session-1",
            json!({"name": "snapshot", "arguments": {"full": false}}),
        ))?;
        assert_eq!(request.session_id, "session-1");
        Ok(())
    }

    #[test]
    fn wrong_token_and_wrong_session_are_rejected() -> Result<()> {
        let authority =
            DaemonAuthority::for_test(TOKEN, DaemonScope::TemperaUse, Some("session-1"))?;
        let wrong_token = authority.authenticate_frame(&frame(
            "fedcba9876543210fedcba9876543210",
            "session-1",
            json!({"name": "state"}),
        ));
        assert!(wrong_token.is_err());
        let wrong_session = authority.authenticate_frame(&frame(
            TOKEN,
            "session-2",
            json!({"name": "state"}),
        ));
        assert!(wrong_session.is_err());
        Ok(())
    }

    #[test]
    fn tempera_use_scope_rejects_admin_and_persistent_screenshot_commands() -> Result<()> {
        let authority = DaemonAuthority::for_test(TOKEN, DaemonScope::TemperaUse, None)?;
        for command in [
            json!({"name": "deviceReset", "arguments": {"name": "device", "confirmed": true}}),
            json!({"name": "clipboardGet"}),
            json!({"name": "screenshot", "arguments": {"path": "/tmp/x.png", "persist": true}}),
        ] {
            assert!(authority
                .authenticate_frame(&frame(TOKEN, "session-1", command))
                .is_err());
        }
        Ok(())
    }

    #[test]
    fn authority_debug_output_never_contains_token() -> Result<()> {
        let authority = DaemonAuthority::for_test(TOKEN, DaemonScope::TemperaUse, None)?;
        let debug = format!("{authority:?}");
        assert!(!debug.contains(TOKEN));
        assert!(debug.contains("REDACTED"));
        Ok(())
    }

    #[test]
    fn constant_time_comparison_handles_length_mismatch() {
        assert!(constant_time_eq(TOKEN.as_bytes(), TOKEN.as_bytes()));
        assert!(!constant_time_eq(TOKEN.as_bytes(), b"short"));
        assert!(!constant_time_eq(TOKEN.as_bytes(), b"0123456789abcdef0123456789abcdee"));
    }
}
