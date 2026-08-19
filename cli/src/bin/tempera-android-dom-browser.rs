use clap::{Args, Parser, Subcommand};
use serde_json::{json, Map, Value};
use std::io::{self, BufRead, Write};
use tempera_android::android_browser;
use tempera_android::android_dom_browser::DomBrowserClient;

const MAX_JSONL_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Parser)]
#[command(
    name = "tempera-android-dom-browser",
    version,
    about = "Persistent low-latency control plane for the dedicated Tempera Android WebView browser"
)]
struct Cli {
    #[arg(long, global = true)]
    serial: Option<String>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Health,
    Open { url: String },
    Snapshot,
    Tap(RefAction),
    Fill(TextAction),
    Type(TextAction),
    Scroll(DirectionAction),
    Back(StateGuard),
    Wait {
        #[arg(long)]
        previous_state_hash: Option<String>,
        #[arg(long)]
        exact_text: Option<String>,
        #[arg(long, default_value_t = 1_000)]
        timeout_ms: u64,
    },
    Bench {
        #[arg(long, default_value_t = 100)]
        iterations: u32,
    },
    /// Keep one ADB forward and one HTTP connection alive for an agent loop.
    Serve,
}

#[derive(Debug, Args)]
struct StateGuard {
    #[arg(long)]
    expected_state_hash: String,
}

#[derive(Debug, Args)]
struct RefAction {
    reference: String,
    #[command(flatten)]
    guard: StateGuard,
    #[arg(long, default_value_t = 48)]
    settle_ms: u64,
}

#[derive(Debug, Args)]
struct TextAction {
    reference: String,
    text: String,
    #[command(flatten)]
    guard: StateGuard,
    #[arg(long, default_value_t = 48)]
    settle_ms: u64,
}

#[derive(Debug, Args)]
struct DirectionAction {
    #[arg(default_value = "down", value_parser = ["up", "down", "left", "right"])]
    direction: String,
    #[command(flatten)]
    guard: StateGuard,
    #[arg(long, default_value_t = 48)]
    settle_ms: u64,
}

fn main() {
    let cli = Cli::parse();
    let serial = match android_browser::resolve_serial(cli.serial.as_deref()) {
        Ok(serial) => serial,
        Err(error) => exit_error(error.to_string()),
    };
    let mut client = match DomBrowserClient::connect(&serial) {
        Ok(client) => client,
        Err(error) => exit_error(error.to_string()),
    };
    match run(&mut client, cli.command) {
        Ok(value) => print_json(&json!({"ok": true, "result": value})),
        Err(error) => exit_error(error.to_string()),
    }
}

fn run(client: &mut DomBrowserClient, command: Commands) -> tempera_android::Result<Value> {
    match command {
        Commands::Health => client.health(),
        Commands::Open { url } => client.navigate(&url),
        Commands::Snapshot => client.snapshot(),
        Commands::Tap(arguments) => client.act_observe(
            action_value(
                "tap",
                Some(arguments.reference),
                None,
                None,
                arguments.guard.expected_state_hash,
            ),
            arguments.settle_ms,
        ),
        Commands::Fill(arguments) => client.act_observe(
            action_value(
                "fill",
                Some(arguments.reference),
                Some(arguments.text),
                None,
                arguments.guard.expected_state_hash,
            ),
            arguments.settle_ms,
        ),
        Commands::Type(arguments) => client.act_observe(
            action_value(
                "type",
                Some(arguments.reference),
                Some(arguments.text),
                None,
                arguments.guard.expected_state_hash,
            ),
            arguments.settle_ms,
        ),
        Commands::Scroll(arguments) => client.act_observe(
            action_value(
                "scroll",
                None,
                None,
                Some(arguments.direction),
                arguments.guard.expected_state_hash,
            ),
            arguments.settle_ms,
        ),
        Commands::Back(guard) => client.act_observe(
            action_value("back", None, None, None, guard.expected_state_hash),
            48,
        ),
        Commands::Wait {
            previous_state_hash,
            exact_text,
            timeout_ms,
        } => client.wait_for(
            previous_state_hash.as_deref(),
            exact_text.as_deref(),
            timeout_ms,
        ),
        Commands::Bench { iterations } => client.benchmark_snapshots(iterations),
        Commands::Serve => serve(client),
    }
}

fn serve(client: &mut DomBrowserClient) -> tempera_android::Result<Value> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut output = io::stdout().lock();
    let mut line = Vec::new();
    loop {
        line.clear();
        let bytes = input.read_until(b'\n', &mut line)?;
        if bytes == 0 {
            break;
        }
        if line.len() > MAX_JSONL_BYTES {
            write_line(
                &mut output,
                &json!({"ok": false, "error": "request exceeds 2 MiB"}),
            )?;
            continue;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let request: Value = match serde_json::from_slice(&line) {
            Ok(request) => request,
            Err(error) => {
                write_line(
                    &mut output,
                    &json!({"ok": false, "error": format!("invalid JSON: {error}")}),
                )?;
                continue;
            }
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let response = match dispatch(client, &request) {
            Ok(result) => json!({"id": id, "ok": true, "result": result}),
            Err(error) => json!({"id": id, "ok": false, "error": error.to_string()}),
        };
        write_line(&mut output, &response)?;
    }
    Ok(json!({"served": true}))
}

fn dispatch(client: &mut DomBrowserClient, request: &Value) -> tempera_android::Result<Value> {
    let object = request.as_object().ok_or_else(|| {
        tempera_android::AndroidError::InvalidInput("serve request must be an object".to_string())
    })?;
    let operation = object
        .get("op")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match operation {
        "health" => client.health(),
        "snapshot" => client.snapshot(),
        "open" => client.navigate(required_string(object, "url")?),
        "tap" | "fill" | "type" | "scroll" | "back" => {
            let hash = required_string(object, "expectedStateHash")?.to_string();
            let reference = object.get("ref").and_then(Value::as_str).map(str::to_string);
            let text = object.get("text").and_then(Value::as_str).map(str::to_string);
            let direction = object
                .get("direction")
                .and_then(Value::as_str)
                .map(str::to_string);
            let settle_ms = object
                .get("settleMs")
                .and_then(Value::as_u64)
                .unwrap_or(48);
            client.act_observe(
                action_value(operation, reference, text, direction, hash),
                settle_ms,
            )
        }
        "wait" => client.wait_for(
            object.get("previousStateHash").and_then(Value::as_str),
            object.get("exactText").and_then(Value::as_str),
            object
                .get("timeoutMs")
                .and_then(Value::as_u64)
                .unwrap_or(1_000),
        ),
        "bench" => client.benchmark_snapshots(
            object
                .get("iterations")
                .and_then(Value::as_u64)
                .unwrap_or(100)
                .try_into()
                .map_err(|_| {
                    tempera_android::AndroidError::InvalidInput(
                        "iterations exceeds u32".to_string(),
                    )
                })?,
        ),
        other => Err(tempera_android::AndroidError::InvalidInput(format!(
            "unknown Android DOM browser operation {other:?}"
        ))),
    }
}

fn required_string<'a>(object: &'a Map<String, Value>, key: &str) -> tempera_android::Result<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            tempera_android::AndroidError::InvalidInput(format!("{key} must be a non-empty string"))
        })
}

fn action_value(
    kind: &str,
    reference: Option<String>,
    text: Option<String>,
    direction: Option<String>,
    expected_state_hash: String,
) -> Value {
    let mut value = json!({
        "kind": kind,
        "expectedStateHash": expected_state_hash,
    });
    if let Some(reference) = reference {
        value["ref"] = Value::String(reference);
    }
    if let Some(text) = text {
        value["text"] = Value::String(text);
    }
    if let Some(direction) = direction {
        value["direction"] = Value::String(direction);
    }
    value
}

fn write_line(writer: &mut impl Write, value: &Value) -> tempera_android::Result<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn print_json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string())
    );
}

fn exit_error(message: String) -> ! {
    print_json(&json!({"ok": false, "error": message}));
    std::process::exit(2)
}
