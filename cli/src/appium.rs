//! Generic Appium endpoint probing.
//!
//! Appium remains outside the hot path. This module deliberately starts with
//! endpoint discovery/health so a provider can be configured without placing
//! credentials in repository configuration.

use crate::error::{AndroidError, Result};
use serde_json::Value;
use std::time::Duration;

pub fn status(endpoint: &str) -> Result<Value> {
    let endpoint = endpoint.trim_end_matches('/');
    if !(endpoint.starts_with("https://") || endpoint.starts_with("http://")) {
        return Err(AndroidError::InvalidInput(
            "Appium URL must begin with https:// or http://".to_string(),
        ));
    }
    let response = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(5))
        .build()
        .get(&format!("{endpoint}/status"))
        .call()
        .map_err(|error| AndroidError::Backend(format!("Appium status failed: {error}")))?;
    let value: Value = response
        .into_json()
        .map_err(|error| AndroidError::Backend(format!("Appium status was not JSON: {error}")))?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_requires_an_http_scheme() {
        assert!(status("appium.example.test").is_err());
    }
}
