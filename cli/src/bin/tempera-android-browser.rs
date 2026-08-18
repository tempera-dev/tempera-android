use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tempera_android::android_browser::{self, BrowserRequest};
use tempera_android::android_browser_fast;
use tempera_android::model::{next_action_id, ActionV1};

#[derive(Debug, Parser)]
#[command(
    name = "tempera-android-browser",
    version,
    about = "Revision-safe, low-latency browser engine for Chrome on Android"
)]
struct Cli {
    /// ADB serial for an emulator, USB device, or wireless device.
    #[arg(long, global = true)]
    serial: Option<String>,
    /// Isolated Tempera Android session used for browser revisions and receipts.
    #[arg(long, default_value = "browser", global = true)]
    session: String,
    /// auto prefers the native Accessibility bridge and retains ADB fallback.
    #[arg(
        long,
        default_value = "auto",
        value_parser = ["auto", "bridge", "adb"],
        global = true
    )]
    transport: String,
    /// Android browser package. Defaults to stable Chrome.
    #[arg(long, default_value = "com.android.chrome", global = true)]
    package: String,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Verify the Android browser, semantic transport, and optional CDP target socket.
    Doctor {
        #[arg(long, default_value = "chrome_devtools_remote")]
        cdp_socket: String,
    },
    /// Open a validated HTTP(S) URL and return the first semantic browser state.
    Open {
        url: String,
        #[arg(long, default_value_t = 15_000)]
        timeout_ms: u64,
    },
    /// Return a compact browser-specific semantic snapshot.
    Snapshot,
    /// Read Chrome/WebView DevTools targets through a temporary ADB forward.
    Targets {
        #[arg(long, default_value = "chrome_devtools_remote")]
        cdp_socket: String,
    },
    /// Tap a semantic browser reference and return the resulting snapshot.
    Tap(SelectorAction),
    /// Long-press a semantic browser reference and return the resulting snapshot.
    #[command(name = "long-press")]
    LongPress(SelectorAction),
    /// Clear/fill a semantic browser field and return the resulting snapshot.
    Fill(TextAction),
    /// Type into a semantic browser field and return the resulting snapshot.
    Type(TextAction),
    /// Press an Android key and return the resulting snapshot.
    Press(KeyAction),
    /// Navigate through Android's browser back stack.
    Back(Guard),
    /// Scroll the browser viewport in one bounded direction.
    Scroll(DirectionAction),
    /// Reopen the current URL hint and return a fresh snapshot.
    Reload {
        #[arg(long, default_value_t = 15_000)]
        timeout_ms: u64,
    },
    /// Measure browser semantic-observation latency without claiming universal speed.
    Bench {
        #[arg(long, default_value_t = 20)]
        iterations: u32,
    },
}

#[derive(Debug, Args)]
struct Guard {
    #[arg(long)]
    expected_revision: u64,
    #[arg(long)]
    expected_state_hash: String,
}

#[derive(Debug, Args)]
struct SelectorAction {
    selector: String,
    #[command(flatten)]
    guard: Guard,
    /// Grant only an approval the user explicitly gave for this exact action.
    #[arg(long)]
    approve_sensitive: bool,
}

#[derive(Debug, Args)]
struct TextAction {
    selector: String,
    text: String,
    #[command(flatten)]
    guard: Guard,
}

#[derive(Debug, Args)]
struct KeyAction {
    key: String,
    #[command(flatten)]
    guard: Guard,
}

#[derive(Debug, Args)]
struct DirectionAction {
    #[arg(default_value = "down", value_parser = ["up", "down", "left", "right"])]
    direction: String,
    #[command(flatten)]
    guard: Guard,
}

fn main() {
    let cli = Cli::parse();
    let request = BrowserRequest {
        session_id: cli.session,
        serial: cli.serial,
        transport: cli.transport,
        package: cli.package,
    };
    match run(request, cli.command) {
        Ok(value) => println!(
            "{}",
            serde_json::to_string_pretty(&json!({"ok": true, "result": value}))
                .expect("browser result is serializable")
        ),
        Err(error) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"ok": false, "error": error.to_string()}))
                    .expect("browser error is serializable")
            );
            std::process::exit(2);
        }
    }
}

fn run(request: BrowserRequest, command: Commands) -> tempera_android::Result<Value> {
    match command {
        Commands::Doctor { cdp_socket } => android_browser::doctor(&request, Some(&cdp_socket)),
        Commands::Open { url, timeout_ms } => android_browser::open(&request, &url, timeout_ms),
        Commands::Snapshot => serde_json::to_value(android_browser::snapshot(&request)?)
            .map_err(tempera_android::AndroidError::from),
        Commands::Targets { cdp_socket } => {
            let serial = android_browser::resolve_serial(request.serial.as_deref())?;
            android_browser::targets(&serial, &cdp_socket)
        }
        Commands::Tap(arguments) => android_browser_fast::step(
            &request,
            action(
                "tap",
                Some(arguments.selector),
                None,
                None,
                None,
                arguments.guard,
                arguments.approve_sensitive,
            ),
        ),
        Commands::LongPress(arguments) => android_browser_fast::step(
            &request,
            action(
                "long_press",
                Some(arguments.selector),
                None,
                None,
                None,
                arguments.guard,
                arguments.approve_sensitive,
            ),
        ),
        Commands::Fill(arguments) => android_browser_fast::step(
            &request,
            action(
                "fill",
                Some(arguments.selector),
                Some(arguments.text),
                None,
                None,
                arguments.guard,
                false,
            ),
        ),
        Commands::Type(arguments) => android_browser_fast::step(
            &request,
            action(
                "type",
                Some(arguments.selector),
                Some(arguments.text),
                None,
                None,
                arguments.guard,
                false,
            ),
        ),
        Commands::Press(arguments) => android_browser_fast::step(
            &request,
            action(
                "press",
                None,
                None,
                Some(arguments.key),
                None,
                arguments.guard,
                false,
            ),
        ),
        Commands::Back(guard) => android_browser_fast::step(
            &request,
            action("back", None, None, None, None, guard, false),
        ),
        Commands::Scroll(arguments) => android_browser_fast::step(
            &request,
            action(
                "scroll",
                None,
                None,
                None,
                Some(arguments.direction),
                arguments.guard,
                false,
            ),
        ),
        Commands::Reload { timeout_ms } => {
            let current = android_browser::snapshot(&request)?;
            let url = current.url_hint.ok_or_else(|| {
                tempera_android::AndroidError::Unsupported(
                    "reload requires a current semantic URL hint; use open with an explicit URL when Chrome does not expose its URL bar"
                        .to_string(),
                )
            })?;
            android_browser::open(&request, &url, timeout_ms)
        }
        Commands::Bench { iterations } => android_browser::bench(&request, iterations),
    }
}

#[allow(clippy::too_many_arguments)]
fn action(
    kind: &str,
    selector: Option<String>,
    text: Option<String>,
    key: Option<String>,
    direction: Option<String>,
    guard: Guard,
    approve_sensitive: bool,
) -> ActionV1 {
    let mut metadata = BTreeMap::new();
    metadata.insert("surface".to_string(), "android-browser".to_string());
    if approve_sensitive {
        metadata.insert("approval".to_string(), "granted".to_string());
    }
    ActionV1 {
        action_id: next_action_id(&format!("android-browser-{kind}")),
        kind: kind.to_string(),
        selector,
        text,
        key,
        direction,
        coordinates: None,
        expected_revision: Some(guard.expected_revision),
        expected_state_hash: Some(guard.expected_state_hash),
        metadata,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_action_always_carries_both_state_guards() {
        let action = action(
            "tap",
            Some("@e7".to_string()),
            None,
            None,
            None,
            Guard {
                expected_revision: 8,
                expected_state_hash: "sha256:test".to_string(),
            },
            false,
        );
        assert_eq!(action.expected_revision, Some(8));
        assert_eq!(action.expected_state_hash.as_deref(), Some("sha256:test"));
        assert_eq!(
            action.metadata.get("surface").map(String::as_str),
            Some("android-browser")
        );
    }

    #[test]
    fn sensitive_approval_is_explicit_metadata() {
        let action = action(
            "tap",
            Some("@e9".to_string()),
            None,
            None,
            None,
            Guard {
                expected_revision: 3,
                expected_state_hash: "sha256:state".to_string(),
            },
            true,
        );
        assert_eq!(
            action.metadata.get("approval").map(String::as_str),
            Some("granted")
        );
    }
}
