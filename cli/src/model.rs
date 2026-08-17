use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub const CONTROL_SCHEMA_V1: &str = "tempera.android.control/v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RectV1 {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

impl RectV1 {
    pub fn center(&self) -> (u32, u32) {
        ((self.left + self.right) / 2, (self.top + self.bottom) / 2)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NodeV1 {
    pub reference: String,
    /// Backend-private stable identifier. Public clients use the reference field only.
    #[serde(default, skip_serializing)]
    pub backend_reference: Option<String>,
    pub role: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<String>,
    pub bounds: RectV1,
    pub enabled: bool,
    pub clickable: bool,
    pub editable: bool,
    pub scrollable: bool,
    pub password: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotV1 {
    pub schema_version: String,
    pub session_id: String,
    pub serial: String,
    pub target_kind: String,
    pub package: String,
    pub activity: String,
    pub screen: [u32; 2],
    pub revision: u64,
    pub state_hash: String,
    pub captured_at_ms: u128,
    #[serde(default)]
    pub nodes: Vec<NodeV1>,
}

impl SnapshotV1 {
    pub fn state_hash_for(
        package: &str,
        activity: &str,
        screen: [u32; 2],
        nodes: &[NodeV1],
    ) -> String {
        let canonical = serde_json::json!({
            "package": package,
            "activity": activity,
            "screen": screen,
            "nodes": nodes,
        });
        let payload = serde_json::to_vec(&canonical).expect("model values are serializable");
        let digest = Sha256::digest(payload);
        format!("sha256:{}", hex::encode(digest))
    }

    pub fn now_ms() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    }

    pub fn node(&self, selector: &str) -> Option<&NodeV1> {
        self.nodes.iter().find(|node| {
            node.reference == selector
                || (!node.label.is_empty() && node.label.eq_ignore_ascii_case(selector))
                || node
                    .resource_id
                    .as_deref()
                    .is_some_and(|id| id == selector || id.rsplit('/').next() == Some(selector))
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActionV1 {
    pub action_id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinates: Option<[u32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_state_hash: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActionReceiptV1 {
    pub schema_version: String,
    pub action_id: String,
    pub kind: String,
    pub ok: bool,
    pub transport: String,
    pub started_at_ms: u128,
    pub completed_at_ms: u128,
    pub before_revision: u64,
    pub after_revision: u64,
    pub before_state_hash: String,
    pub after_state_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionV1 {
    pub schema_version: String,
    pub session_id: String,
    pub serial: String,
    pub target_kind: String,
    pub transport: String,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
    pub last_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_state_hash: Option<String>,
    /// Private W3C/Appium session identifier. SessionStore writes it to an
    /// internal sidecar, never to the public SessionV1 response.
    #[serde(default, skip_serializing)]
    pub backend_session_id: Option<String>,
}
