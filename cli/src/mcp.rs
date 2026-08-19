//! Stdio MCP server whose tools delegate to the canonical command executor.

use crate::command::{execute, Command, CommandRequest};
use crate::model::{next_action_id, ActionV1};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

const PROTOCOL_VERSION: &str = "2025-11-25";

pub fn serve(
    serial: Option<String>,
    session_id: String,
    transport: String,
    appium_url: Option<String>,
    appium_capabilities: Option<Value>,
) -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) => handle(
                request,
                serial.clone(),
                session_id.clone(),
                transport.clone(),
                appium_url.clone(),
                appium_capabilities.clone(),
            ),
            Err(parse_error) => Some(error(
                Value::Null,
                -32700,
                format!("Parse error: {parse_error}"),
            )),
        };
        if let Some(response) = response {
            serde_json::to_writer(&mut stdout, &response)?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

fn handle(
    request: Value,
    serial: Option<String>,
    session_id: String,
    transport: String,
    appium_url: Option<String>,
    appium_capabilities: Option<Value>,
) -> Option<Value> {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
    match method {
        "notifications/initialized" => None,
        "initialize" => Some(ok(
            id,
            json!({
                "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL_VERSION),
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "tempera-android", "version": env!("CARGO_PKG_VERSION")}
            }),
        )),
        "ping" => Some(ok(id, json!({}))),
        "tools/list" => Some(ok(id, json!({"tools": tools()}))),
        "tools/call" => Some(call_tool(
            id,
            params,
            serial,
            session_id,
            transport,
            appium_url,
            appium_capabilities,
        )),
        _ => Some(error(id, -32601, format!("Method not found: {method}"))),
    }
}

fn tools() -> Vec<Value> {
    vec![
        // Core control profile. Every mutating tool accepts the same revision
        // guards as the CLI ActionV1 contract.
        tool("tempera_android_snapshot", "Capture the current semantic Android UI snapshot with revision-bound @e references.", json!({"type":"object","properties":{"full":{"type":"boolean"}}})),
        tool("tempera_android_stream", "Capture a bounded, read-only sequence of semantic snapshots through the canonical session path.", json!({"type":"object","properties":{"observations":{"type":"integer","minimum":1,"maximum":300},"intervalMs":{"type":"integer","minimum":0,"maximum":10000}}})),
        tool("tempera_android_find", "Find current semantic nodes by @e reference, visible label, or Android resource id.", json!({"type":"object","required":["query"],"properties":{"query":{"type":"string"}}})),
        tool("tempera_android_tap", "Tap one current @e reference, label, resource id, or coordinate pair.", action_schema(json!({}))),
        tool("tempera_android_long_press", "Long-press one current @e reference, label, resource id, or coordinate pair.", action_schema(json!({}))),
        tool("tempera_android_fill", "Replace text in the selected editable node. Resolve secret references locally; never place secret values in MCP arguments.", text_action_schema()),
        tool("tempera_android_type", "Type into the current Android focus or selected editable node. Resolve secret references locally; never place secret values in MCP arguments.", text_action_schema()),
        tool("tempera_android_press", "Press an Android key such as ENTER, BACK, HOME, or TAB.", json!({"type":"object","required":["key"],"properties":{"key":{"type":"string"},"expectedRevision":{"type":"integer"},"expectedStateHash":{"type":"string"}}})),
        tool("tempera_android_swipe", "Swipe in a direction on the current Android screen.", direction_action_schema()),
        tool("tempera_android_scroll", "Scroll in a direction on the current Android screen.", direction_action_schema()),
        tool("tempera_android_wait", "Wait for a bounded duration without sending Android input.", json!({"type":"object","properties":{"milliseconds":{"type":"integer","minimum":0,"maximum":60000}}})),
        tool("tempera_android_batch", "Execute a bounded, revision-guarded batch; processing stops on the first error.", json!({"type":"object","required":["actions"],"properties":{"actions":{"type":"array","maxItems":12}}})),

        // Device profile. Destructive AVD operations are explicitly confirmed
        // and the canonical executor rejects physical serials.
        tool("tempera_android_device_list", "List attached Android emulators and physical devices.", json!({"type":"object","properties":{}})),
        tool("tempera_android_device_connect", "Connect ADB to a user-authorized remote endpoint.", json!({"type":"object","required":["endpoint"],"properties":{"endpoint":{"type":"string"}}})),
        tool("tempera_android_device_create", "Create a managed Android emulator record and AVD.", json!({"type":"object","required":["name"],"properties":{"name":{"type":"string"},"profile":{"type":"string"},"api":{"type":"integer","minimum":1,"maximum":2147483647},"device":{"type":"string"},"ramMb":{"type":"integer","minimum":512,"maximum":65536},"dataGb":{"type":"integer","minimum":1,"maximum":1024}}})),
        tool("tempera_android_device_start", "Start a managed Android emulator.", json!({"type":"object","required":["name"],"properties":{"name":{"type":"string"},"headless":{"type":"boolean"},"cold":{"type":"boolean"}}})),
        tool("tempera_android_device_stop", "Stop the selected managed emulator; physical targets are rejected.", json!({"type":"object","properties":{}})),
        tool("tempera_android_device_info", "Inspect the selected Android target and bridge status.", json!({"type":"object","properties":{}})),
        tool("tempera_android_device_reset", "Reset one managed emulator. Requires confirmed: true and always rejects physical targets.", confirmed_name_schema()),
        tool("tempera_android_device_delete", "Delete one managed emulator. Requires confirmed: true and always rejects physical targets.", confirmed_name_schema()),

        // Apps profile.
        tool("tempera_android_app_list", "List installed Android application packages.", json!({"type":"object","properties":{"includeSystem":{"type":"boolean"}}})),
        tool("tempera_android_app_install", "Install explicitly supplied APK paths on the selected target.", json!({"type":"object","required":["paths"],"properties":{"paths":{"type":"array","minItems":1,"items":{"type":"string"}}}})),
        tool("tempera_android_app_open", "Open an installed Android package.", package_schema()),
        tool("tempera_android_app_stop", "Stop an installed Android package.", package_schema()),
        tool("tempera_android_app_clear", "Clear an installed Android package's app data.", package_schema()),
        tool("tempera_android_app_uninstall", "Uninstall an Android package.", package_schema()),
        tool("tempera_android_app_deeplink", "Open an Android deeplink URI.", json!({"type":"object","required":["uri"],"properties":{"uri":{"type":"string"}}})),
        tool("tempera_android_app_permissions", "Inspect Android permissions for an installed package.", package_schema()),

        // Debug profile. Screenshot/record remain CLI-only because accepting a
        // filesystem path from an MCP client would create an unbounded write.
        tool("tempera_android_logs", "Read a bounded logcat tail for the current target.", json!({"type":"object","properties":{"lines":{"type":"integer","minimum":1,"maximum":2000}}})),
        tool("tempera_android_bridge_status", "Inspect native Accessibility bridge availability and protocol health.", json!({"type":"object","properties":{}})),
        tool("tempera_android_bridge_setup", "Install and provision the native bridge; optional apk is a host-authorized local path.", json!({"type":"object","properties":{"apk":{"type":"string"}}})),
        tool("tempera_android_bridge_enable", "Enable the bridge Accessibility service when Android permits it.", json!({"type":"object","properties":{}})),
        tool("tempera_android_bridge_disable", "Disable the bridge Accessibility service.", json!({"type":"object","properties":{}})),
        tool("tempera_android_dashboard", "Report local inspector dashboard availability. Serving remains a local CLI process.", json!({"type":"object","properties":{}})),

        // Network profile.
        tool("tempera_android_network", "Read the current Android connectivity diagnostic state.", json!({"type":"object","properties":{}})),
        tool("tempera_android_location", "Set the selected managed emulator's location; physical targets are rejected by the backend.", json!({"type":"object","required":["latitude","longitude"],"properties":{"latitude":{"type":"number","minimum":-90,"maximum":90},"longitude":{"type":"number","minimum":-180,"maximum":180}}})),
        tool("tempera_android_clipboard", "Read or set the target clipboard. Values are never persisted in snapshots or receipts.", json!({"type":"object","properties":{"text":{"type":"string"}}})),

        // State and integration profiles.
        tool("tempera_android_session", "Inspect the current Tempera Android session.", json!({"type":"object","properties":{}})),
        tool("tempera_android_close", "Close the Tempera session and bridge forwarding without stopping the target device.", json!({"type":"object","properties":{}})),
        tool("tempera_android_state", "Read the persisted latest semantic snapshot and recent action receipts without observing or mutating the target.", json!({"type":"object","properties":{}})),
        tool("tempera_android_skills", "List locally stored, verified Android navigation skills.", json!({"type":"object","properties":{}})),
        tool("tempera_android_doctor", "Inspect SDK, ADB, target, bridge, legacy metadata, and optional Appium readiness.", json!({"type":"object","properties":{}})),
        tool("tempera_android_install", "Install an Android SDK profile using the local SDK tooling.", json!({"type":"object","properties":{"profile":{"type":"string"},"api":{"type":"integer","minimum":1,"maximum":2147483647}}})),
        tool("tempera_android_upgrade", "Upgrade Android SDK tooling using the local SDK tooling.", json!({"type":"object","properties":{}})),
        tool("tempera_android_migrate_legacy_avd", "Explicitly copy one legacy metadata record without touching Android-owned AVD data. Requires confirmed: true.", confirmed_name_schema()),
        tool("tempera_android_run", "Run a bounded semantic planner loop through the canonical executor. A multimodal model is used only after a semantic planner explicitly requests vision; credentials remain process-local.", json!({"type":"object","required":["task"],"properties":{"task":{"type":"string"},"model":{"type":"string"},"endpoint":{"type":"string"},"visionModel":{"type":"string"},"visionEndpoint":{"type":"string"},"maxSteps":{"type":"integer","minimum":1,"maximum":40},"skills":{"type":"boolean"},"approval":{"type":"string","enum":["granted"]}}})),
        tool("tempera_android_eval", "List deterministic evaluation contracts or grade the current observed state against one contract.", json!({"type":"object","properties":{"list":{"type":"boolean"},"case":{"type":"string"}}})),
        tool("tempera_android_bench", "Measure semantic observation latency without mutating the Android target.", json!({"type":"object","properties":{"iterations":{"type":"integer","minimum":3,"maximum":200}}})),
    ]
}

fn action_schema(mut properties: Value) -> Value {
    let object = properties
        .as_object_mut()
        .expect("action schema starts as an object");
    object.insert(
        "properties".to_string(),
        json!({
            "selector": {"type": "string"},
            "coordinates": {"type": "array", "minItems": 2, "maxItems": 2, "items": {"type": "integer", "minimum": 0, "maximum": 4294967295_u64}},
            "expectedRevision": {"type": "integer", "minimum": 0},
            "expectedStateHash": {"type": "string"},
            "approval": {"type": "string", "enum": ["granted"]}
        }),
    );
    properties
}

fn text_action_schema() -> Value {
    json!({"type":"object","required":["text"],"properties":{"selector":{"type":"string"},"text":{"type":"string"},"expectedRevision":{"type":"integer","minimum":0},"expectedStateHash":{"type":"string"}}})
}

fn direction_action_schema() -> Value {
    json!({"type":"object","properties":{"direction":{"type":"string","enum":["up","down","left","right"]},"expectedRevision":{"type":"integer","minimum":0},"expectedStateHash":{"type":"string"}}})
}

fn package_schema() -> Value {
    json!({"type":"object","required":["package"],"properties":{"package":{"type":"string"}}})
}

fn confirmed_name_schema() -> Value {
    json!({"type":"object","required":["name","confirmed"],"properties":{"name":{"type":"string"},"source":{"type":"string"},"confirmed":{"type":"boolean","const":true}}})
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({"name": name, "description": description, "inputSchema": input_schema})
}

fn call_tool(
    id: Value,
    params: Value,
    serial: Option<String>,
    session_id: String,
    transport: String,
    appium_url: Option<String>,
    appium_capabilities: Option<Value>,
) -> Value {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let request = match command_for_tool(
        name,
        &arguments,
        serial,
        session_id,
        transport,
        appium_url,
        appium_capabilities,
    ) {
        Ok(request) => request,
        Err(message) => return error(id, -32602, message),
    };
    let response = execute(request);
    if response.ok {
        ok(
            id,
            json!({"content": [{"type": "text", "text": serde_json::to_string(&response.result).unwrap_or_else(|_| "{}".to_string())}], "isError": false}),
        )
    } else {
        ok(
            id,
            json!({"content": [{"type": "text", "text": response.error.unwrap_or_else(|| "Unknown Android error".to_string())}], "isError": true}),
        )
    }
}

fn command_for_tool(
    name: &str,
    arguments: &Value,
    serial: Option<String>,
    session_id: String,
    transport: String,
    appium_url: Option<String>,
    appium_capabilities: Option<Value>,
) -> std::result::Result<CommandRequest, String> {
    let id = next_action_id(&format!(
        "mcp-{}",
        name.trim_start_matches("tempera_android_")
    ));
    let action = |kind: &str| -> std::result::Result<ActionV1, String> {
        let selector = arguments
            .get("selector")
            .and_then(Value::as_str)
            .map(str::to_string);
        let text = arguments
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string);
        let key = arguments
            .get("key")
            .and_then(Value::as_str)
            .map(str::to_string);
        let direction = arguments
            .get("direction")
            .and_then(Value::as_str)
            .map(str::to_string);
        let expected_revision = arguments.get("expectedRevision").and_then(Value::as_u64);
        let expected_state_hash = arguments
            .get("expectedStateHash")
            .and_then(Value::as_str)
            .map(str::to_string);
        let coordinates = match arguments.get("coordinates") {
            None => None,
            Some(Value::Array(values)) if values.len() == 2 => {
                let coordinate = |index: usize| {
                    values[index]
                        .as_u64()
                        .and_then(|value| u32::try_from(value).ok())
                        .ok_or_else(|| {
                            "coordinates must contain two unsigned 32-bit integers".to_string()
                        })
                };
                Some([coordinate(0)?, coordinate(1)?])
            }
            Some(_) => {
                return Err(
                    "coordinates must contain exactly two unsigned 32-bit integers".to_string(),
                )
            }
        };
        let mut metadata = BTreeMap::new();
        if arguments.get("approval").and_then(Value::as_str) == Some("granted") {
            metadata.insert("approval".to_string(), "granted".to_string());
        }
        if kind == "wait" {
            let milliseconds = arguments
                .get("milliseconds")
                .and_then(Value::as_u64)
                .unwrap_or(250);
            if milliseconds > 60_000 {
                return Err("wait milliseconds must be 0..=60000".to_string());
            }
            metadata.insert("milliseconds".to_string(), milliseconds.to_string());
        }
        Ok(ActionV1 {
            action_id: id.clone(),
            kind: kind.to_string(),
            selector,
            text,
            key,
            direction,
            coordinates,
            expected_revision,
            expected_state_hash,
            metadata,
        })
    };
    let command = match name {
        "tempera_android_snapshot" => Command::Snapshot {
            full: arguments
                .get("full")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        "tempera_android_stream" => {
            let observations = arguments
                .get("observations")
                .and_then(Value::as_u64)
                .unwrap_or(10);
            let interval_ms = arguments
                .get("intervalMs")
                .and_then(Value::as_u64)
                .unwrap_or(500);
            if !(1..=300).contains(&observations) || interval_ms > 10_000 {
                return Err(
                    "stream observations must be 1..=300 and intervalMs 0..=10000".to_string(),
                );
            }
            Command::Stream {
                observations: observations as u32,
                interval_ms,
            }
        }
        "tempera_android_find" => Command::Find {
            query: arguments
                .get("query")
                .and_then(Value::as_str)
                .ok_or_else(|| "query is required".to_string())?
                .to_string(),
        },
        "tempera_android_tap" => Command::Action {
            action: action("tap")?,
        },
        "tempera_android_long_press" => Command::Action {
            action: action("long_press")?,
        },
        "tempera_android_fill" => Command::Action {
            action: action("fill")?,
        },
        "tempera_android_type" => Command::Action {
            action: action("type")?,
        },
        "tempera_android_press" => Command::Action {
            action: action("press")?,
        },
        "tempera_android_swipe" => Command::Action {
            action: action("swipe")?,
        },
        "tempera_android_scroll" => Command::Action {
            action: action("scroll")?,
        },
        "tempera_android_wait" => Command::Action {
            action: action("wait")?,
        },
        "tempera_android_batch" => Command::Batch {
            actions: serde_json::from_value(
                arguments
                    .get("actions")
                    .cloned()
                    .ok_or_else(|| "actions is required".to_string())?,
            )
            .map_err(|error| error.to_string())?,
        },
        "tempera_android_device_list" => Command::DeviceList,
        "tempera_android_device_connect" => Command::DeviceConnect {
            endpoint: required_string(arguments, "endpoint")?,
        },
        "tempera_android_device_create" => Command::DeviceCreate {
            name: required_string(arguments, "name")?,
            profile: optional_string(arguments, "profile").unwrap_or_else(|| "google".to_string()),
            api: optional_u32(arguments, "api")?.unwrap_or(36),
            device: optional_string(arguments, "device"),
            ram_mb: optional_u32(arguments, "ramMb")?,
            data_gb: optional_u32(arguments, "dataGb")?,
        },
        "tempera_android_device_start" => Command::DeviceStart {
            name: required_string(arguments, "name")?,
            headless: arguments
                .get("headless")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            cold: arguments
                .get("cold")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        "tempera_android_device_stop" => Command::DeviceStop,
        "tempera_android_device_info" => Command::DeviceInfo,
        "tempera_android_device_reset" => Command::DeviceReset {
            name: required_confirmed_name(arguments)?,
            confirmed: true,
        },
        "tempera_android_device_delete" => Command::DeviceDelete {
            name: required_confirmed_name(arguments)?,
            confirmed: true,
        },
        "tempera_android_app_list" => Command::AppList {
            include_system: arguments
                .get("includeSystem")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        "tempera_android_app_install" => Command::AppInstall {
            paths: required_string_array(arguments, "paths")?,
        },
        "tempera_android_app_open" => app_manage(arguments, "open")?,
        "tempera_android_app_stop" => app_manage(arguments, "stop")?,
        "tempera_android_app_clear" => app_manage(arguments, "clear")?,
        "tempera_android_app_uninstall" => app_manage(arguments, "uninstall")?,
        "tempera_android_app_deeplink" => Command::AppDeeplink {
            uri: required_string(arguments, "uri")?,
        },
        "tempera_android_app_permissions" => Command::AppPermissions {
            package: required_string(arguments, "package")?,
        },
        "tempera_android_session" => Command::SessionList,
        "tempera_android_close" => Command::SessionClose,
        "tempera_android_logs" => Command::Logs {
            lines: arguments
                .get("lines")
                .and_then(Value::as_u64)
                .unwrap_or(200) as u32,
        },
        "tempera_android_network" => Command::NetworkStatus,
        "tempera_android_location" => Command::LocationSet {
            latitude: required_f64(arguments, "latitude", -90.0, 90.0)?,
            longitude: required_f64(arguments, "longitude", -180.0, 180.0)?,
        },
        "tempera_android_clipboard" => match arguments.get("text").and_then(Value::as_str) {
            Some(text) => Command::ClipboardSet {
                text: text.to_string(),
            },
            None => Command::ClipboardGet,
        },
        "tempera_android_state" => Command::State,
        "tempera_android_skills" => Command::SkillsList,
        "tempera_android_bridge_status" => Command::BridgeStatus,
        "tempera_android_bridge_setup" => Command::BridgeSetup {
            apk: optional_string(arguments, "apk").map(PathBuf::from),
        },
        "tempera_android_bridge_enable" => Command::BridgeEnable,
        "tempera_android_bridge_disable" => Command::BridgeDisable,
        "tempera_android_dashboard" => Command::DashboardStatus,
        "tempera_android_doctor" => Command::Doctor,
        "tempera_android_install" => Command::InstallSdk {
            profile: optional_string(arguments, "profile").unwrap_or_else(|| "google".to_string()),
            api: optional_u32(arguments, "api")?.unwrap_or(36),
        },
        "tempera_android_upgrade" => Command::UpgradeSdk,
        "tempera_android_migrate_legacy_avd" => Command::MigrateLegacyAvd {
            name: required_confirmed_name(arguments)?,
            source: optional_string(arguments, "source").map(PathBuf::from),
            confirmed: true,
        },
        "tempera_android_run" => {
            let max_steps = arguments
                .get("maxSteps")
                .and_then(Value::as_u64)
                .unwrap_or(20);
            if max_steps > 40 {
                return Err("maxSteps must be 1..=40".to_string());
            }
            Command::Run {
                task: arguments
                    .get("task")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "task is required".to_string())?
                    .to_string(),
                model: arguments
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                endpoint: arguments
                    .get("endpoint")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                vision_model: arguments
                    .get("visionModel")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                vision_endpoint: arguments
                    .get("visionEndpoint")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                max_steps: max_steps as u32,
                approve_sensitive: arguments.get("approval").and_then(Value::as_str)
                    == Some("granted"),
                use_skills: arguments
                    .get("skills")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            }
        }
        "tempera_android_eval" => Command::Eval {
            list: arguments
                .get("list")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            case: arguments
                .get("case")
                .and_then(Value::as_str)
                .map(str::to_string),
            output: None,
        },
        "tempera_android_bench" => Command::Bench {
            iterations: arguments
                .get("iterations")
                .and_then(Value::as_u64)
                .unwrap_or(20) as u32,
        },
        _ => return Err(format!("Unknown tool: {name}")),
    };
    Ok(CommandRequest {
        id,
        session_id,
        serial,
        transport,
        appium_url,
        appium_capabilities,
        command,
    })
}

fn required_string(arguments: &Value, key: &str) -> std::result::Result<String, String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

fn optional_string(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn optional_u32(arguments: &Value, key: &str) -> std::result::Result<Option<u32>, String> {
    arguments
        .get(key)
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| format!("{key} must be an unsigned 32-bit integer"))
        })
        .transpose()
}

fn required_string_array(arguments: &Value, key: &str) -> std::result::Result<Vec<String>, String> {
    let values = arguments
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{key} is required"))?;
    if values.is_empty() {
        return Err(format!("{key} must not be empty"));
    }
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| format!("{key} must contain non-empty strings"))
        })
        .collect()
}

fn required_confirmed_name(arguments: &Value) -> std::result::Result<String, String> {
    if arguments.get("confirmed").and_then(Value::as_bool) != Some(true) {
        return Err("confirmed must be true".to_string());
    }
    required_string(arguments, "name")
}

fn required_f64(
    arguments: &Value,
    key: &str,
    minimum: f64,
    maximum: f64,
) -> std::result::Result<f64, String> {
    let value = arguments
        .get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("{key} is required"))?;
    if !value.is_finite() || !(minimum..=maximum).contains(&value) {
        return Err(format!("{key} must be in {minimum}..={maximum}"));
    }
    Ok(value)
}

fn app_manage(arguments: &Value, operation: &str) -> std::result::Result<Command, String> {
    Ok(Command::AppManage {
        operation: operation.to_string(),
        package: required_string(arguments, "package")?,
    })
}

fn ok(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}
fn error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_exposes_revision_guarded_core_tools() {
        let listed_tools = tools();
        let names: Vec<_> = listed_tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .collect();
        assert!(names.contains(&"tempera_android_snapshot"));
        assert!(names.contains(&"tempera_android_stream"));
        assert!(names.contains(&"tempera_android_batch"));
        assert!(names.contains(&"tempera_android_find"));
        assert!(names.contains(&"tempera_android_long_press"));
        assert!(names.contains(&"tempera_android_fill"));
        assert!(names.contains(&"tempera_android_swipe"));
        assert!(names.contains(&"tempera_android_scroll"));
        assert!(names.contains(&"tempera_android_wait"));
        assert!(names.contains(&"tempera_android_device_create"));
        assert!(names.contains(&"tempera_android_device_delete"));
        assert!(names.contains(&"tempera_android_app_install"));
        assert!(names.contains(&"tempera_android_app_permissions"));
        assert!(names.contains(&"tempera_android_bridge_status"));
        assert!(names.contains(&"tempera_android_location"));
        assert!(names.contains(&"tempera_android_close"));
        assert!(names.contains(&"tempera_android_doctor"));
        assert!(names.contains(&"tempera_android_migrate_legacy_avd"));
        assert!(names.contains(&"tempera_android_eval"));
        assert!(names.contains(&"tempera_android_bench"));
        assert!(names.contains(&"tempera_android_logs"));
        assert!(names.contains(&"tempera_android_state"));
        assert!(names.contains(&"tempera_android_run"));
        let run = listed_tools
            .iter()
            .find(|tool| tool.get("name").and_then(Value::as_str) == Some("tempera_android_run"))
            .expect("run tool is listed");
        assert!(run.pointer("/inputSchema/properties/visionModel").is_some());
        assert!(run
            .pointer("/inputSchema/properties/visionEndpoint")
            .is_some());
    }

    #[test]
    fn stream_mcp_input_is_bounded_before_integer_conversion() {
        let result = command_for_tool(
            "tempera_android_stream",
            &json!({"observations": 4_294_967_297_u64}),
            None,
            "session".to_string(),
            "adb".to_string(),
            None,
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn mcp_preserves_action_kind_and_coordinates() {
        let request = command_for_tool(
            "tempera_android_swipe",
            &json!({"direction": "left", "coordinates": [10, 20]}),
            None,
            "session".to_string(),
            "adb".to_string(),
            None,
            None,
        )
        .expect("valid MCP swipe");
        let Command::Action { action } = request.command else {
            panic!("expected Action command");
        };
        assert_eq!(action.kind, "swipe");
        assert_eq!(action.coordinates, Some([10, 20]));
    }

    #[test]
    fn mcp_requires_explicit_confirmation_for_destructive_avd_tools() {
        let rejected = command_for_tool(
            "tempera_android_device_delete",
            &json!({"name": "tempera-proof-test"}),
            None,
            "session".to_string(),
            "adb".to_string(),
            None,
            None,
        );
        assert!(rejected.is_err());

        let accepted = command_for_tool(
            "tempera_android_device_delete",
            &json!({"name": "tempera-proof-test", "confirmed": true}),
            None,
            "session".to_string(),
            "adb".to_string(),
            None,
            None,
        )
        .expect("confirmed deletion request");
        assert!(matches!(
            accepted.command,
            Command::DeviceDelete {
                name,
                confirmed: true
            } if name == "tempera-proof-test"
        ));
    }
}
