use crate::adb::{self, AdbBackend};
use crate::appium;
use crate::avd::{self, CreateOptions, StartOptions};
use crate::benchmark;
use crate::bridge;
use crate::config;
use crate::error::{AndroidError, Result};
use crate::evals;
use crate::model::{ActionReceiptV1, ActionV1, CONTROL_SCHEMA_V1};
use crate::session::SessionStore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandRequest {
    pub id: String,
    pub session_id: String,
    #[serde(default)]
    pub serial: Option<String>,
    #[serde(default = "default_transport")]
    pub transport: String,
    pub command: Command,
}

fn default_transport() -> String {
    "auto".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "name", content = "arguments", rename_all = "camelCase")]
pub enum Command {
    Doctor,
    DeviceList,
    DeviceConnect {
        endpoint: String,
    },
    DeviceCreate {
        name: String,
        profile: String,
        api: u32,
        device: Option<String>,
        ram_mb: Option<u32>,
        data_gb: Option<u32>,
    },
    DeviceStart {
        name: String,
        headless: bool,
        cold: bool,
    },
    DeviceStop,
    DeviceInfo,
    DeviceReset {
        name: String,
        confirmed: bool,
    },
    DeviceDelete {
        name: String,
        confirmed: bool,
    },
    Snapshot {
        full: bool,
    },
    Screenshot {
        path: PathBuf,
    },
    Find {
        query: String,
    },
    Action {
        action: ActionV1,
    },
    Batch {
        actions: Vec<ActionV1>,
    },
    AppList {
        include_system: bool,
    },
    AppInstall {
        paths: Vec<String>,
    },
    AppManage {
        operation: String,
        package: String,
    },
    AppDeeplink {
        uri: String,
    },
    SessionList,
    SessionClose,
    BridgeStatus,
    BridgeSetup {
        #[serde(default)]
        apk: Option<PathBuf>,
    },
    BridgeEnable,
    BridgeDisable,
    DashboardStatus,
    Bench {
        iterations: u32,
    },
    Eval {
        list: bool,
        case: Option<String>,
        output: Option<PathBuf>,
    },
    Unsupported {
        feature: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandResponse {
    pub schema_version: String,
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl CommandResponse {
    pub fn success(id: String, result: impl Serialize) -> Result<Self> {
        Ok(Self {
            schema_version: CONTROL_SCHEMA_V1.to_string(),
            id,
            ok: true,
            result: Some(serde_json::to_value(result)?),
            error: None,
        })
    }

    pub fn failure(id: String, error: impl ToString) -> Self {
        Self {
            schema_version: CONTROL_SCHEMA_V1.to_string(),
            id,
            ok: false,
            result: None,
            error: Some(error.to_string()),
        }
    }
}

pub fn execute(request: CommandRequest) -> CommandResponse {
    match execute_inner(request.clone()) {
        Ok(response) => response,
        Err(error) => CommandResponse::failure(request.id, error),
    }
}

fn execute_inner(request: CommandRequest) -> Result<CommandResponse> {
    let store = SessionStore::from_environment()?;
    match request.command {
        Command::SessionList => return CommandResponse::success(request.id, store.list()?),
        Command::SessionClose => {
            return CommandResponse::success(
                request.id,
                json!({"closed": store.remove(&request.session_id)?}),
            )
        }
        Command::DeviceList => {
            let backend = AdbBackend::new(request.serial.unwrap_or_else(|| "unused".to_string()))?;
            return CommandResponse::success(request.id, backend.device_list()?);
        }
        Command::DeviceConnect { endpoint } => {
            let backend = AdbBackend::new(request.serial.unwrap_or_else(|| "unused".to_string()))?;
            let output = backend.connect(&endpoint)?;
            return CommandResponse::success(
                request.id,
                json!({"endpoint": endpoint, "output": output.trim()}),
            );
        }
        Command::DeviceCreate {
            name,
            profile,
            api,
            device,
            ram_mb,
            data_gb,
        } => {
            return CommandResponse::success(
                request.id,
                avd::create(
                    &store,
                    CreateOptions {
                        name,
                        profile,
                        api,
                        device,
                        ram_mb,
                        data_gb,
                    },
                )?,
            )
        }
        Command::DeviceStart {
            name,
            headless,
            cold,
        } => {
            return CommandResponse::success(
                request.id,
                avd::start(
                    &store,
                    StartOptions {
                        name,
                        headless,
                        cold,
                    },
                )?,
            )
        }
        Command::DeviceReset { name, confirmed } => {
            return CommandResponse::success(request.id, avd::reset(&store, &name, confirmed)?)
        }
        Command::DeviceDelete { name, confirmed } => {
            avd::delete(&store, &name, confirmed)?;
            return CommandResponse::success(request.id, json!({"deleted": name}));
        }
        Command::Doctor => {
            let backend = AdbBackend::new(request.serial.unwrap_or_else(|| "unused".to_string()))?;
            let devices = backend.device_list()?;
            let managed = avd::doctor();
            let configured_appium = config::load()
                .ok()
                .and_then(|configuration| config::appium_url(&configuration));
            let appium = configured_appium.as_deref().map(|url| match appium::status(url) {
                Ok(status) => json!({"configured": true, "reachable": true, "url": url, "status": status}),
                Err(error) => json!({"configured": true, "reachable": false, "url": url, "detail": error.to_string()}),
            });
            return CommandResponse::success(
                request.id,
                json!({
                    "cli": {"version": env!("CARGO_PKG_VERSION"), "schemaVersion": CONTROL_SCHEMA_V1},
                    "adb": {"available": true, "devices": devices},
                    "managedEmulator": managed.ok(),
                    "configuration": {"schemaVersion": config::CONFIG_SCHEMA_V1, "legacyMetadataDetected": config::legacy_metadata_detected(), "legacyMigration": "explicit migration is required; existing AVD data is untouched"},
                    "transports": {"auto": true, "adb": true, "bridge": true, "appium": appium},
                    "warning": "bridge and Appium are optional integrations; direct ADB/UIAutomator is always independently available"
                }),
            );
        }
        Command::Eval {
            list: true,
            case: None,
            output: None,
        } => return CommandResponse::success(request.id, evals::cases()),
        Command::DashboardStatus => {
            return CommandResponse::success(
                request.id,
                json!({"available": true, "command": "tempera-android dashboard serve"}),
            )
        }
        Command::Unsupported { feature } => {
            return Err(AndroidError::Unsupported(format!(
                "{feature} is not available in this build"
            )))
        }
        _ => {}
    }

    let serial = resolve_serial(request.serial.as_deref())?;
    let backend = AdbBackend::new(serial.clone())?;
    let mut session = store.get_or_create(&request.session_id, &serial, &request.transport)?;
    let bridge_client = match &request.command {
        Command::Snapshot { .. }
        | Command::Find { .. }
        | Command::Action { .. }
        | Command::Batch { .. }
        | Command::Bench { .. }
        | Command::Eval { list: false, .. } => select_bridge(&request.transport, &serial, &store)?,
        _ => None,
    };
    let response = match request.command {
        Command::DeviceStop => {
            avd::stop(&serial)?;
            CommandResponse::success(request.id, json!({"stopped": serial}))?
        }
        Command::DeviceInfo => CommandResponse::success(
            request.id,
            json!({"serial": serial, "bridge": bridge::status(&serial, &store)?}),
        )?,
        Command::Snapshot { full: _ } => {
            let snapshot = if let Some(mut bridge) = bridge_client {
                session.transport = "bridge".to_string();
                bridge.observe(&mut session)?
            } else {
                session.transport = "adb".to_string();
                backend.snapshot(&mut session)?
            };
            store.save(&session)?;
            store.save_snapshot(&session.session_id, &snapshot)?;
            CommandResponse::success(request.id, snapshot)?
        }
        Command::Screenshot { path } => {
            backend.screenshot(&path)?;
            CommandResponse::success(request.id, json!({"path": path}))?
        }
        Command::Find { query } => {
            let snapshot = if let Some(mut bridge) = bridge_client {
                session.transport = "bridge".to_string();
                bridge.observe(&mut session)?
            } else {
                session.transport = "adb".to_string();
                backend.snapshot(&mut session)?
            };
            store.save(&session)?;
            store.save_snapshot(&session.session_id, &snapshot)?;
            let normalized = query.to_lowercase();
            let nodes = snapshot
                .nodes
                .iter()
                .filter(|node| {
                    node.reference == query
                        || node.label.to_lowercase().contains(&normalized)
                        || node
                            .resource_id
                            .as_deref()
                            .is_some_and(|id| id.to_lowercase().contains(&normalized))
                })
                .cloned()
                .collect::<Vec<_>>();
            CommandResponse::success(request.id, json!({"snapshot": snapshot, "nodes": nodes}))?
        }
        Command::Action { action } => {
            let receipt = if let Some(mut bridge) = bridge_client {
                session.transport = "bridge".to_string();
                execute_bridge_actions(&mut bridge, &mut session, &[action])?.remove(0)
            } else {
                session.transport = "adb".to_string();
                backend.execute_action(&mut session, &action)?
            };
            store.save(&session)?;
            store.save_receipts(&session.session_id, std::slice::from_ref(&receipt))?;
            CommandResponse::success(request.id, receipt)?
        }
        Command::Batch { actions } => {
            if actions.is_empty() || actions.len() > 12 {
                return Err(AndroidError::InvalidInput(
                    "batch requires 1-12 actions".to_string(),
                ));
            }
            let receipts: Vec<ActionReceiptV1> = if let Some(mut bridge) = bridge_client {
                session.transport = "bridge".to_string();
                execute_bridge_actions(&mut bridge, &mut session, &actions)?
            } else {
                require_fused_batch_guards(&actions)?;
                session.transport = "adb".to_string();
                let mut receipts = Vec::with_capacity(actions.len());
                for action in &actions {
                    receipts.push(backend.execute_action(&mut session, action)?);
                }
                receipts
            };
            store.save(&session)?;
            store.save_receipts(&session.session_id, &receipts)?;
            CommandResponse::success(
                request.id,
                json!({"receipts": receipts, "session": session}),
            )?
        }
        Command::AppList { include_system } => CommandResponse::success(
            request.id,
            json!({"packages": backend.app_list(include_system)?}),
        )?,
        Command::AppInstall { paths } => {
            backend.app_install(&paths)?;
            CommandResponse::success(request.id, json!({"installed": paths}))?
        }
        Command::AppManage { operation, package } => {
            backend.app_manage(&operation, &package)?;
            CommandResponse::success(
                request.id,
                json!({"operation": operation, "package": package}),
            )?
        }
        Command::AppDeeplink { uri } => {
            backend.app_deeplink(&uri)?;
            CommandResponse::success(request.id, json!({"uri": uri}))?
        }
        Command::BridgeStatus => {
            CommandResponse::success(request.id, bridge::status(&serial, &store)?)?
        }
        Command::BridgeSetup { apk } => {
            CommandResponse::success(request.id, bridge::setup(&serial, &store, apk.as_deref())?)?
        }
        Command::BridgeEnable => {
            bridge::enable(&backend)?;
            CommandResponse::success(request.id, bridge::status(&serial, &store)?)?
        }
        Command::BridgeDisable => {
            bridge::disable(&backend)?;
            CommandResponse::success(request.id, bridge::status(&serial, &store)?)?
        }
        Command::Bench { iterations } => {
            let iterations = iterations.clamp(3, 200);
            let mut observation_ms = Vec::with_capacity(iterations as usize);
            let mut payload_bytes = Vec::with_capacity(iterations as usize);
            let transport = if let Some(mut bridge) = bridge_client {
                session.transport = "bridge".to_string();
                for _ in 0..iterations {
                    let started = std::time::Instant::now();
                    let snapshot = bridge.observe(&mut session)?;
                    observation_ms.push(started.elapsed().as_secs_f64() * 1000.0);
                    payload_bytes.push(serde_json::to_vec(&snapshot)?.len());
                    store.save_snapshot(&session.session_id, &snapshot)?;
                }
                "native-accessibility-bridge"
            } else {
                session.transport = "adb".to_string();
                for _ in 0..iterations {
                    let started = std::time::Instant::now();
                    let snapshot = backend.snapshot(&mut session)?;
                    observation_ms.push(started.elapsed().as_secs_f64() * 1000.0);
                    payload_bytes.push(serde_json::to_vec(&snapshot)?.len());
                    store.save_snapshot(&session.session_id, &snapshot)?;
                }
                "adb-uiautomator"
            };
            store.save(&session)?;
            CommandResponse::success(
                request.id,
                json!({
                    "schemaVersion": "tempera.android.benchmark/v1",
                    "iterations": iterations,
                    "transport": transport,
                    "observation": benchmark::summarize(&observation_ms),
                    "semanticPayloadBytes": {"mean": payload_bytes.iter().sum::<usize>() as f64 / payload_bytes.len() as f64, "min": payload_bytes.iter().min(), "max": payload_bytes.iter().max()},
                    "note": "Observation-only measurement. Compare transports on the same target and publish raw reports before making performance claims.",
                }),
            )?
        }
        Command::Eval { list, case, output } => {
            if list {
                return Err(AndroidError::InvalidInput(
                    "eval --list cannot be combined with a target command".to_string(),
                ));
            }
            let case = case.ok_or_else(|| {
                AndroidError::InvalidInput("eval requires --case CASE_ID or --list".to_string())
            })?;
            let snapshot = if let Some(mut bridge) = bridge_client {
                session.transport = "bridge".to_string();
                bridge.observe(&mut session)?
            } else {
                session.transport = "adb".to_string();
                backend.snapshot(&mut session)?
            };
            store.save(&session)?;
            store.save_snapshot(&session.session_id, &snapshot)?;
            let definition = evals::case(&case).ok_or_else(|| {
                AndroidError::InvalidInput(format!("Unknown eval case {case:?}; use eval --list"))
            })?;
            let report = evals::evaluate(definition, &snapshot);
            if let Some(output) = output {
                std::fs::write(&output, serde_json::to_vec_pretty(&report)?)?;
            }
            CommandResponse::success(request.id, report)?
        }
        Command::Doctor
        | Command::DeviceList
        | Command::DeviceConnect { .. }
        | Command::DeviceCreate { .. }
        | Command::DeviceStart { .. }
        | Command::DeviceReset { .. }
        | Command::DeviceDelete { .. }
        | Command::SessionList
        | Command::SessionClose
        | Command::DashboardStatus
        | Command::Unsupported { .. } => unreachable!(),
    };
    Ok(response)
}

fn select_bridge(
    transport: &str,
    serial: &str,
    store: &SessionStore,
) -> Result<Option<bridge::BridgeClient>> {
    match transport {
        "adb" => Ok(None),
        "bridge" => Ok(Some(bridge::BridgeClient::connect(serial, store)?)),
        "auto" => Ok(bridge::BridgeClient::connect(serial, store).ok()),
        "appium" => Err(AndroidError::Unsupported(
            "Appium endpoint selection requires --appium-url and is not enabled in this alpha build"
                .to_string(),
        )),
        other => Err(AndroidError::InvalidInput(format!(
            "Unknown transport {other:?}; use auto, bridge, adb, or appium"
        ))),
    }
}

fn execute_bridge_actions(
    bridge: &mut bridge::BridgeClient,
    session: &mut crate::model::SessionV1,
    actions: &[ActionV1],
) -> Result<Vec<ActionReceiptV1>> {
    if actions.is_empty() {
        return Err(AndroidError::InvalidInput(
            "batch requires at least one action".to_string(),
        ));
    }
    let before = bridge.observe(session)?;
    if actions.len() > 1 {
        require_fused_batch_guards(actions)?;
    }
    for action in actions {
        adb::validate_guard(action, &before)?;
        adb::validate_sensitive(action, &before)?;
    }
    let payloads = actions
        .iter()
        .map(|action| bridge::action_payload(action, &before))
        .collect::<Result<Vec<_>>>()?;
    let started_at_ms = crate::model::SnapshotV1::now_ms();
    let (after, results) = bridge.act_observe(session, before.revision, payloads)?;
    if results.len() != actions.len() {
        return Err(AndroidError::Backend(
            "Bridge action response did not include one result for each action".to_string(),
        ));
    }
    results
        .iter()
        .zip(actions)
        .map(|(result, action)| {
            if result.get("ok").and_then(Value::as_bool) != Some(true) {
                return Err(AndroidError::Backend(
                    result
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("Native bridge action failed")
                        .to_string(),
                ));
            }
            Ok(ActionReceiptV1 {
                schema_version: CONTROL_SCHEMA_V1.to_string(),
                action_id: action.action_id.clone(),
                kind: action.kind.clone(),
                ok: true,
                transport: "native-accessibility-bridge".to_string(),
                started_at_ms,
                completed_at_ms: crate::model::SnapshotV1::now_ms(),
                before_revision: before.revision,
                after_revision: after.revision,
                before_state_hash: before.state_hash.clone(),
                after_state_hash: after.state_hash.clone(),
                detail: result
                    .get("detail")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect()
}

fn require_fused_batch_guards(actions: &[ActionV1]) -> Result<()> {
    let Some(revision) = actions.first().and_then(|action| action.expected_revision) else {
        return Err(AndroidError::InvalidInput(
            "fused batch requires expectedRevision on every action".to_string(),
        ));
    };
    let Some(hash) = actions
        .first()
        .and_then(|action| action.expected_state_hash.as_deref())
    else {
        return Err(AndroidError::InvalidInput(
            "fused batch requires expectedStateHash on every action".to_string(),
        ));
    };
    if actions.iter().any(|action| {
        action.expected_revision != Some(revision)
            || action.expected_state_hash.as_deref() != Some(hash)
    }) {
        return Err(AndroidError::InvalidInput(
            "every fused batch action must use the same expectedRevision and expectedStateHash"
                .to_string(),
        ));
    }
    Ok(())
}

fn resolve_serial(requested: Option<&str>) -> Result<String> {
    if let Some(serial) = requested {
        return Ok(serial.to_string());
    }
    if let Ok(serial) = std::env::var("TEMPERA_ANDROID_SERIAL") {
        return Ok(serial);
    }
    let backend = AdbBackend::new("unused")?;
    let ready: Vec<_> = backend
        .device_list()?
        .into_iter()
        .filter(|device| device.state == "device")
        .collect();
    match ready.as_slice() {
        [device] => Ok(device.serial.clone()),
        [] => Err(AndroidError::Backend(
            "No ready Android target found; use device start or --serial".to_string(),
        )),
        _ => Err(AndroidError::InvalidInput(
            "Multiple Android targets are ready; pass --serial or TEMPERA_ANDROID_SERIAL"
                .to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn response_has_versioned_contract() {
        let response = CommandResponse::success("x".to_string(), json!({"ok": true})).unwrap();
        assert!(response.ok);
        assert_eq!(response.schema_version, CONTROL_SCHEMA_V1);
    }

    #[test]
    fn fused_batch_requires_matching_full_state_guards() {
        let action = ActionV1 {
            action_id: "a".to_string(),
            kind: "tap".to_string(),
            selector: Some("@e0".to_string()),
            text: None,
            key: None,
            direction: None,
            coordinates: None,
            expected_revision: Some(7),
            expected_state_hash: Some("sha256:state".to_string()),
            metadata: BTreeMap::new(),
        };
        assert!(require_fused_batch_guards(&[action.clone(), action]).is_ok());
        assert!(require_fused_batch_guards(&[ActionV1 {
            expected_state_hash: None,
            ..ActionV1 {
                action_id: "a".to_string(),
                kind: "tap".to_string(),
                selector: None,
                text: None,
                key: None,
                direction: None,
                coordinates: None,
                expected_revision: Some(7),
                expected_state_hash: Some("sha256:state".to_string()),
                metadata: BTreeMap::new(),
            }
        }])
        .is_err());
    }
}
