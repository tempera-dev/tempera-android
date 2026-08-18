//! Deterministic model-tier policy for Android planning.
//!
//! This module deliberately has no device authority. It selects a planner
//! endpoint/model tuple before the existing runner performs typed,
//! revision-bound execution through the canonical command path.

use crate::error::{AndroidError, Result};
use serde::{Deserialize, Serialize};
use std::env;

const DEFAULT_LOCAL_ENDPOINT: &str = "http://127.0.0.1:11434/v1/chat/completions";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelTier {
    Fast,
    Reasoning,
    Vision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTarget {
    pub model: String,
    pub endpoint: String,
    pub tier: ModelTier,
    pub local: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPolicy {
    pub fast: Option<ModelTarget>,
    pub reasoning: Option<ModelTarget>,
    pub vision: Option<ModelTarget>,
    /// Never route semantic planning to a non-loopback endpoint. Vision is
    /// separately opt-in and remains subject to the runner's screenshot gate.
    pub local_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteReason {
    ExplicitOverride,
    ConsequentialTask,
    LongHorizonTask,
    RecoveryTask,
    DefaultFast,
    OnlyAvailablePlanner,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteDecision {
    pub target: ModelTarget,
    pub reason: RouteReason,
}

impl ModelPolicy {
    pub fn from_environment() -> Result<Self> {
        let local_only = env_bool("TEMPERA_ANDROID_MODEL_LOCAL_ONLY")?;
        let default_endpoint = env_nonempty("TEMPERA_ANDROID_ENDPOINT")
            .unwrap_or_else(|| DEFAULT_LOCAL_ENDPOINT.to_string());

        let fast = target_from_env(
            ModelTier::Fast,
            "TEMPERA_ANDROID_FAST_MODEL",
            "TEMPERA_ANDROID_FAST_ENDPOINT",
            &default_endpoint,
        )?;
        let reasoning = target_from_env(
            ModelTier::Reasoning,
            "TEMPERA_ANDROID_REASONING_MODEL",
            "TEMPERA_ANDROID_REASONING_ENDPOINT",
            &default_endpoint,
        )?;
        let vision = target_from_env(
            ModelTier::Vision,
            "TEMPERA_ANDROID_VISION_MODEL",
            "TEMPERA_ANDROID_VISION_ENDPOINT",
            &default_endpoint,
        )?;

        let policy = Self {
            fast,
            reasoning,
            vision,
            local_only,
        };
        policy.validate()?;
        Ok(policy)
    }

    pub fn validate(&self) -> Result<()> {
        for target in self.fast.iter().chain(self.reasoning.iter()) {
            validate_target(target)?;
            if self.local_only && !target.local {
                return Err(AndroidError::InvalidInput(format!(
                    "model policy is local-only but {} targets non-loopback endpoint {}",
                    target.model, target.endpoint
                )));
            }
        }
        if let Some(target) = &self.vision {
            validate_target(target)?;
        }
        Ok(())
    }

    /// Choose a semantic planner for a task. The router is intentionally
    /// deterministic and side-effect free; the selected model still cannot
    /// execute an Android action directly.
    pub fn route(&self, task: &str, explicit: Option<ModelTarget>) -> Result<RouteDecision> {
        self.validate()?;
        if let Some(target) = explicit {
            validate_target(&target)?;
            if self.local_only && !target.local {
                return Err(AndroidError::InvalidInput(
                    "explicit planner violates TEMPERA_ANDROID_MODEL_LOCAL_ONLY".to_string(),
                ));
            }
            return Ok(RouteDecision {
                target,
                reason: RouteReason::ExplicitOverride,
            });
        }

        let normalized = normalize(task);
        if normalized.is_empty() {
            return Err(AndroidError::InvalidInput(
                "cannot route an empty Android task".to_string(),
            ));
        }

        let reasoning_reason = classify_reasoning(&normalized);
        if let Some(reason) = reasoning_reason {
            if let Some(target) = self.reasoning.clone() {
                return Ok(RouteDecision { target, reason });
            }
        }

        if let Some(target) = self.fast.clone() {
            return Ok(RouteDecision {
                target,
                reason: RouteReason::DefaultFast,
            });
        }
        if let Some(target) = self.reasoning.clone() {
            return Ok(RouteDecision {
                target,
                reason: RouteReason::OnlyAvailablePlanner,
            });
        }

        Err(AndroidError::InvalidInput(
            "model policy has no semantic planner; configure TEMPERA_ANDROID_FAST_MODEL or TEMPERA_ANDROID_REASONING_MODEL, or pass an explicit model"
                .to_string(),
        ))
    }
}

pub fn explicit_target(model: String, endpoint: Option<String>) -> Result<ModelTarget> {
    let endpoint = endpoint
        .or_else(|| env_nonempty("TEMPERA_ANDROID_ENDPOINT"))
        .unwrap_or_else(|| DEFAULT_LOCAL_ENDPOINT.to_string());
    let target = ModelTarget {
        model,
        local: is_loopback_endpoint(&endpoint),
        endpoint,
        tier: ModelTier::Reasoning,
    };
    validate_target(&target)?;
    Ok(target)
}

fn target_from_env(
    tier: ModelTier,
    model_key: &str,
    endpoint_key: &str,
    default_endpoint: &str,
) -> Result<Option<ModelTarget>> {
    let Some(model) = env_nonempty(model_key) else {
        return Ok(None);
    };
    let endpoint = env_nonempty(endpoint_key).unwrap_or_else(|| default_endpoint.to_string());
    let target = ModelTarget {
        model,
        local: is_loopback_endpoint(&endpoint),
        endpoint,
        tier,
    };
    validate_target(&target)?;
    Ok(Some(target))
}

fn validate_target(target: &ModelTarget) -> Result<()> {
    if target.model.trim().is_empty() {
        return Err(AndroidError::InvalidInput(
            "model target requires a non-empty model".to_string(),
        ));
    }
    let endpoint = target.endpoint.trim();
    if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
        return Err(AndroidError::InvalidInput(format!(
            "model endpoint must use http:// or https://: {endpoint}"
        )));
    }
    if endpoint.contains('@') {
        return Err(AndroidError::InvalidInput(
            "model endpoint must not embed credentials".to_string(),
        ));
    }
    Ok(())
}

fn classify_reasoning(task: &str) -> Option<RouteReason> {
    // Consequential tasks are routed to the stronger planner when available,
    // but the existing approval gate remains authoritative at execution time.
    const CONSEQUENTIAL: &[&str] = &[
        "buy",
        "purchase",
        "pay",
        "transfer",
        "send",
        "post",
        "submit",
        "book",
        "order",
        "delete",
        "remove account",
        "unsubscribe",
        "permission",
        "credential",
    ];
    if contains_any(task, CONSEQUENTIAL) {
        return Some(RouteReason::ConsequentialTask);
    }

    const RECOVERY: &[&str] = &[
        "recover",
        "retry",
        "fix",
        "failed",
        "error",
        "unexpected",
        "stuck",
        "wrong screen",
        "go back and",
        "if that doesn't work",
        "if that does not work",
    ];
    if contains_any(task, RECOVERY) {
        return Some(RouteReason::RecoveryTask);
    }

    const LONG_HORIZON: &[&str] = &[
        "then",
        "after that",
        "and then",
        "across apps",
        "another app",
        "compare",
        "find and",
        "open and",
        "sign in and",
        "navigate to",
        "set up",
        "configure",
    ];
    if contains_any(task, LONG_HORIZON) || task.split_whitespace().count() >= 18 {
        return Some(RouteReason::LongHorizonTask);
    }

    None
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn normalize(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn env_nonempty(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_bool(key: &str) -> Result<bool> {
    let Some(value) = env_nonempty(key) else {
        return Ok(false);
    };
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(AndroidError::InvalidInput(format!(
            "{key} must be one of true/false, 1/0, yes/no, on/off"
        ))),
    }
}

fn is_loopback_endpoint(endpoint: &str) -> bool {
    let lower = endpoint.to_ascii_lowercase();
    lower.starts_with("http://127.0.0.1:")
        || lower.starts_with("http://localhost:")
        || lower.starts_with("https://127.0.0.1:")
        || lower.starts_with("https://localhost:")
        || lower == "http://127.0.0.1"
        || lower == "http://localhost"
        || lower == "https://127.0.0.1"
        || lower == "https://localhost"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(tier: ModelTier, model: &str) -> ModelTarget {
        ModelTarget {
            model: model.to_string(),
            endpoint: DEFAULT_LOCAL_ENDPOINT.to_string(),
            tier,
            local: true,
        }
    }

    #[test]
    fn obvious_single_step_uses_fast_model() {
        let policy = ModelPolicy {
            fast: Some(target(ModelTier::Fast, "fast")),
            reasoning: Some(target(ModelTier::Reasoning, "reasoning")),
            vision: None,
            local_only: false,
        };
        let decision = policy.route("Open Settings", None).unwrap();
        assert_eq!(decision.target.model, "fast");
        assert_eq!(decision.reason, RouteReason::DefaultFast);
    }

    #[test]
    fn consequential_task_prefers_reasoning_without_granting_approval() {
        let policy = ModelPolicy {
            fast: Some(target(ModelTier::Fast, "fast")),
            reasoning: Some(target(ModelTier::Reasoning, "reasoning")),
            vision: None,
            local_only: false,
        };
        let decision = policy
            .route("Open the store and purchase the item", None)
            .unwrap();
        assert_eq!(decision.target.model, "reasoning");
        assert_eq!(decision.reason, RouteReason::ConsequentialTask);
    }

    #[test]
    fn explicit_override_wins() {
        let policy = ModelPolicy {
            fast: Some(target(ModelTier::Fast, "fast")),
            reasoning: Some(target(ModelTier::Reasoning, "reasoning")),
            vision: None,
            local_only: false,
        };
        let explicit = target(ModelTier::Reasoning, "manual");
        let decision = policy.route("Open Settings", Some(explicit)).unwrap();
        assert_eq!(decision.target.model, "manual");
        assert_eq!(decision.reason, RouteReason::ExplicitOverride);
    }

    #[test]
    fn local_only_rejects_remote_semantic_planner() {
        let policy = ModelPolicy {
            fast: Some(ModelTarget {
                model: "remote".to_string(),
                endpoint: "https://example.com/v1/chat/completions".to_string(),
                tier: ModelTier::Fast,
                local: false,
            }),
            reasoning: None,
            vision: None,
            local_only: true,
        };
        assert!(policy.route("Open Settings", None).is_err());
    }

    #[test]
    fn credentials_in_endpoint_are_rejected() {
        let candidate = ModelTarget {
            model: "bad".to_string(),
            endpoint: "https://token@example.com/v1/chat/completions".to_string(),
            tier: ModelTier::Fast,
            local: false,
        };
        assert!(validate_target(&candidate).is_err());
    }
}
