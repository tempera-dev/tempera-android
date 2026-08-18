//! User configuration and explicit legacy-state detection.

use crate::error::{AndroidError, Result};
use serde::Deserialize;
use serde_json::Value;
use std::env;
use std::path::PathBuf;

pub const CONFIG_SCHEMA_V1: &str = "tempera.android.config/v1";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigV1 {
    #[serde(default)]
    pub schema_version: Option<String>,
    #[serde(default)]
    pub default_serial: Option<String>,
    #[serde(default)]
    pub transport: Option<String>,
    #[serde(default)]
    pub appium: Option<AppiumConfigV1>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppiumConfigV1 {
    pub url: Option<String>,
    /// Non-secret W3C alwaysMatch capabilities. Credentials belong in a
    /// provider integration or process-local environment, never this file.
    #[serde(default)]
    pub capabilities: Option<Value>,
}

pub fn load() -> Result<ConfigV1> {
    let path = config_path();
    let Some(path) = path else {
        return Ok(ConfigV1::default());
    };
    let bytes = std::fs::read(&path)?;
    let config: ConfigV1 = serde_json::from_slice(&bytes)?;
    if let Some(version) = &config.schema_version {
        if version != CONFIG_SCHEMA_V1 {
            return Err(AndroidError::InvalidInput(format!(
                "{} declares unsupported schemaVersion {version:?}; expected {CONFIG_SCHEMA_V1}",
                path.display()
            )));
        }
    }
    Ok(config)
}

pub fn serial(config: &ConfigV1) -> Option<String> {
    env::var("TEMPERA_ANDROID_SERIAL")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| config.default_serial.clone())
}

pub fn transport(config: &ConfigV1) -> Option<String> {
    env::var("TEMPERA_ANDROID_TRANSPORT")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| config.transport.clone())
}

pub fn appium_url(config: &ConfigV1) -> Option<String> {
    env::var("TEMPERA_ANDROID_APPIUM_URL")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| config.appium.as_ref().and_then(|appium| appium.url.clone()))
}

pub fn appium_capabilities(config: &ConfigV1) -> Result<Option<Value>> {
    env::var("TEMPERA_ANDROID_APPIUM_CAPABILITIES")
        .ok()
        .filter(|value| !value.is_empty())
        .map(|value| {
            serde_json::from_str(&value).map_err(|error| {
                AndroidError::InvalidInput(format!(
                    "TEMPERA_ANDROID_APPIUM_CAPABILITIES must be valid JSON: {error}"
                ))
            })
        })
        .transpose()
        .map(|configured| {
            configured.or_else(|| {
                config
                    .appium
                    .as_ref()
                    .and_then(|appium| appium.capabilities.clone())
            })
        })
}

pub fn legacy_metadata_detected() -> bool {
    legacy_root().exists()
}

/// Historical Android Simulator state. The caller must opt in before any
/// metadata is read or copied; this helper never changes the source tree.
pub fn legacy_root() -> PathBuf {
    env::var_os("ANDROID_SIM_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".android-simulator"))
}

pub fn config_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("TEMPERA_ANDROID_CONFIG") {
        return Some(PathBuf::from(path));
    }
    let local = PathBuf::from("tempera-android.json");
    local.is_file().then_some(local)
}

fn home() -> PathBuf {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_schema_is_versioned() {
        let config: ConfigV1 = serde_json::from_str(
            r#"{"schemaVersion":"tempera.android.config/v1","defaultSerial":"emulator-5554"}"#,
        )
        .unwrap();
        assert_eq!(config.schema_version.as_deref(), Some(CONFIG_SCHEMA_V1));
    }
}
