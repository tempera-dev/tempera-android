//! Stdio MCP server whose tools delegate to the canonical command executor.

use crate::command::{execute, Command, CommandRequest};
use crate::model::ActionV1;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};

const PROTOCOL_VERSION: &str = "2025-11-25";

pub fn serve(serial: Option<String>, session_id: String, transport: String) -> io::Result<()> {
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
        "tools/call" => Some(call_tool(id, params, serial, session_id, transport)),
        _ => Some(error(id, -32601, format!("Method not found: {method}"))),
    }
}

fn tools() -> Vec<Value> {
    vec![
        tool("tempera_android_snapshot", "Capture the current semantic Android UI snapshot with revision-bound @e references.", json!({"type":"object","properties":{"full":{"type":"boolean"}}})),
        tool("tempera_android_find", "Find current semantic nodes by @e reference, visible label, or Android resource id.", json!({"type":"object","required":["query"],"properties":{"query":{"type":"string"}}})),
        tool("tempera_android_tap", "Tap one current @e reference, label, resource id, or coordinate pair.", json!({"type":"object","required":["selector"],"properties":{"selector":{"type":"string"},"expectedRevision":{"type":"integer"},"expectedStateHash":{"type":"string"},"approval":{"type":"string","enum":["granted"]}}})),
        tool("tempera_android_type", "Type into the current Android focus or selected editable node. Secret values must be resolved outside MCP.", json!({"type":"object","required":["text"],"properties":{"selector":{"type":"string"},"text":{"type":"string"},"expectedRevision":{"type":"integer"}}})),
        tool("tempera_android_press", "Press an Android key such as ENTER, BACK, HOME, or TAB.", json!({"type":"object","required":["key"],"properties":{"key":{"type":"string"},"expectedRevision":{"type":"integer"}}})),
        tool("tempera_android_swipe", "Swipe or scroll a current Android screen.", json!({"type":"object","properties":{"direction":{"type":"string","enum":["up","down","left","right"]},"expectedRevision":{"type":"integer"}}})),
        tool("tempera_android_batch", "Execute a bounded, revision-guarded batch; processing stops on the first error.", json!({"type":"object","required":["actions"],"properties":{"actions":{"type":"array","maxItems":12}}})),
        tool("tempera_android_apps", "List installed Android application packages.", json!({"type":"object","properties":{"includeSystem":{"type":"boolean"}}})),
        tool("tempera_android_devices", "List attached Android emulators and physical devices.", json!({"type":"object","properties":{}})),
        tool("tempera_android_session", "Inspect or close a Tempera Android session.", json!({"type":"object","properties":{"close":{"type":"boolean"}}})),
        tool("tempera_android_eval", "List deterministic evaluation contracts or grade the current observed state against one contract.", json!({"type":"object","properties":{"list":{"type":"boolean"},"case":{"type":"string"}}})),
        tool("tempera_android_bench", "Measure semantic observation latency without mutating the Android target.", json!({"type":"object","properties":{"iterations":{"type":"integer","minimum":3,"maximum":200}}})),
    ]
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
) -> Value {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let request = match command_for_tool(name, &arguments, serial, session_id, transport) {
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
) -> std::result::Result<CommandRequest, String> {
    let id = format!("mcp-{}", name.trim_start_matches("tempera_android_"));
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
        let mut metadata = BTreeMap::new();
        if arguments.get("approval").and_then(Value::as_str) == Some("granted") {
            metadata.insert("approval".to_string(), "granted".to_string());
        }
        Ok(ActionV1 {
            action_id: id.clone(),
            kind: kind.to_string(),
            selector,
            text,
            key,
            direction,
            coordinates: None,
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
        "tempera_android_type" => Command::Action {
            action: action("type")?,
        },
        "tempera_android_press" => Command::Action {
            action: action("press")?,
        },
        "tempera_android_swipe" => Command::Action {
            action: action("scroll")?,
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
        "tempera_android_apps" => Command::AppList {
            include_system: arguments
                .get("includeSystem")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        "tempera_android_devices" => Command::DeviceList,
        "tempera_android_session"
            if arguments
                .get("close")
                .and_then(Value::as_bool)
                .unwrap_or(false) =>
        {
            Command::SessionClose
        }
        "tempera_android_session" => Command::SessionList,
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
        command,
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
        assert!(names.contains(&"tempera_android_batch"));
        assert!(names.contains(&"tempera_android_find"));
        assert!(names.contains(&"tempera_android_eval"));
        assert!(names.contains(&"tempera_android_bench"));
    }
}
