use clap::{Args, Parser, Subcommand};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tempera_android::command::{execute, Command, CommandRequest};
use tempera_android::daemon;
use tempera_android::mcp;
use tempera_android::model::ActionV1;

#[derive(Debug, Parser)]
#[command(
    name = "tempera-android",
    version,
    about = "High-performance Android automation engine for AI agents"
)]
struct Cli {
    /// Persistent Tempera Android session ID.
    #[arg(long, default_value = "default", global = true)]
    session: String,
    /// ADB serial for an emulator, USB device, or wireless device.
    #[arg(long, global = true)]
    serial: Option<String>,
    /// auto prefers the native bridge when configured; adb remains the independent fallback.
    #[arg(long, default_value = "auto", value_parser = ["auto", "bridge", "adb", "appium"], global = true)]
    transport: String,
    /// Emit the versioned JSON response contract.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Doctor,
    Device {
        #[command(subcommand)]
        command: DeviceCommand,
    },
    App {
        #[command(subcommand)]
        command: AppCommand,
    },
    Snapshot {
        #[arg(long)]
        full: bool,
    },
    Screenshot {
        path: PathBuf,
    },
    Find {
        query: String,
    },
    Tap(ActionArgs),
    #[command(name = "long-press")]
    LongPress(ActionArgs),
    Fill(TextActionArgs),
    Type(TextActionArgs),
    Press(KeyActionArgs),
    Swipe(DirectionActionArgs),
    Scroll(DirectionActionArgs),
    Wait {
        #[arg(default_value_t = 250)]
        milliseconds: u64,
    },
    Batch {
        actions: PathBuf,
    },
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    Bridge {
        #[command(subcommand)]
        command: BridgeCommand,
    },
    Mcp,
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    Dashboard {
        #[command(subcommand)]
        command: DashboardCommand,
    },
    Install,
    Upgrade,
    Run {
        task: String,
    },
    Bench,
    Eval {
        #[arg(long)]
        list: bool,
        #[arg(long = "case")]
        case_id: Option<String>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    Skills,
    Close,
}

#[derive(Debug, Subcommand)]
enum DeviceCommand {
    List,
    Connect {
        endpoint: String,
    },
    Create {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "google")]
        profile: String,
        #[arg(long, default_value_t = 36)]
        api: u32,
        #[arg(long)]
        device: Option<String>,
        #[arg(long)]
        ram_mb: Option<u32>,
        #[arg(long)]
        data_gb: Option<u32>,
    },
    Start {
        name: String,
        #[arg(long)]
        headless: bool,
        #[arg(long)]
        cold: bool,
    },
    Stop,
    Info,
    Reset {
        name: String,
        #[arg(long)]
        yes: bool,
    },
    Delete {
        name: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
enum AppCommand {
    List {
        #[arg(long)]
        all: bool,
    },
    Install {
        paths: Vec<String>,
    },
    Open {
        package: String,
    },
    Stop {
        package: String,
    },
    Clear {
        package: String,
    },
    Uninstall {
        package: String,
    },
    Deeplink {
        uri: String,
    },
    Permissions,
}

#[derive(Debug, Subcommand)]
enum SessionCommand {
    List,
    Close,
}

#[derive(Debug, Subcommand)]
enum BridgeCommand {
    Status,
    Setup {
        /// Use a prebuilt bridge APK instead of building the checked-in companion.
        #[arg(long)]
        apk: Option<PathBuf>,
    },
    Enable,
    Disable,
}

#[derive(Debug, Subcommand)]
enum DaemonCommand {
    Serve {
        #[arg(long, default_value = "127.0.0.1:7421")]
        listen: String,
    },
}

#[derive(Debug, Subcommand)]
enum DashboardCommand {
    Status,
    Serve {
        #[arg(long, default_value = "127.0.0.1:7422")]
        listen: String,
    },
}

#[derive(Debug, Args)]
struct ActionArgs {
    selector: Option<String>,
    #[arg(long, value_parser = parse_coordinates)]
    coordinates: Option<[u32; 2]>,
    #[arg(long)]
    expected_revision: Option<u64>,
    #[arg(long)]
    expected_state_hash: Option<String>,
    #[arg(long)]
    approve_sensitive: bool,
}

#[derive(Debug, Args)]
struct TextActionArgs {
    selector: Option<String>,
    text: String,
    #[arg(long)]
    expected_revision: Option<u64>,
}

#[derive(Debug, Args)]
struct KeyActionArgs {
    key: String,
    #[arg(long)]
    expected_revision: Option<u64>,
}
#[derive(Debug, Args)]
struct DirectionActionArgs {
    #[arg(default_value = "down")]
    direction: String,
    #[arg(long)]
    expected_revision: Option<u64>,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Mcp => {
            if let Err(error) = mcp::serve(cli.serial, cli.session, cli.transport) {
                eprintln!("error: {error}");
                std::process::exit(2);
            }
            return;
        }
        Commands::Daemon {
            command: DaemonCommand::Serve { listen },
        } => {
            if let Err(error) = daemon::serve(&listen) {
                eprintln!("error: {error}");
                std::process::exit(2);
            }
            return;
        }
        Commands::Dashboard {
            command: DashboardCommand::Serve { listen },
        } => {
            if let Err(error) = tempera_android::dashboard::serve(&listen) {
                eprintln!("error: {error}");
                std::process::exit(2);
            }
            return;
        }
        _ => {}
    }

    let request = CommandRequest {
        id: "cli".to_string(),
        session_id: cli.session,
        serial: cli.serial,
        transport: cli.transport,
        command: command_from_cli(cli.command),
    };
    let response = execute(request);
    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&response).expect("response is serializable")
        );
    } else if response.ok {
        print_human(&response.result.unwrap_or(serde_json::Value::Null));
    } else {
        eprintln!(
            "error: {}",
            response
                .error
                .unwrap_or_else(|| "Unknown error".to_string())
        );
        std::process::exit(2);
    }
}

fn command_from_cli(command: Commands) -> Command {
    match command {
        Commands::Doctor => Command::Doctor,
        Commands::Device {
            command: DeviceCommand::List,
        } => Command::DeviceList,
        Commands::Device {
            command: DeviceCommand::Connect { endpoint },
        } => Command::DeviceConnect { endpoint },
        Commands::Device {
            command:
                DeviceCommand::Create {
                    name,
                    profile,
                    api,
                    device,
                    ram_mb,
                    data_gb,
                },
        } => Command::DeviceCreate {
            name,
            profile,
            api,
            device,
            ram_mb,
            data_gb,
        },
        Commands::Device {
            command:
                DeviceCommand::Start {
                    name,
                    headless,
                    cold,
                },
        } => Command::DeviceStart {
            name,
            headless,
            cold,
        },
        Commands::Device {
            command: DeviceCommand::Stop,
        } => Command::DeviceStop,
        Commands::Device {
            command: DeviceCommand::Info,
        } => Command::DeviceInfo,
        Commands::Device {
            command: DeviceCommand::Reset { name, yes },
        } => Command::DeviceReset {
            name,
            confirmed: yes,
        },
        Commands::Device {
            command: DeviceCommand::Delete { name, yes },
        } => Command::DeviceDelete {
            name,
            confirmed: yes,
        },
        Commands::App {
            command: AppCommand::List { all },
        } => Command::AppList {
            include_system: all,
        },
        Commands::App {
            command: AppCommand::Install { paths },
        } => Command::AppInstall { paths },
        Commands::App {
            command: AppCommand::Open { package },
        } => Command::AppManage {
            operation: "open".to_string(),
            package,
        },
        Commands::App {
            command: AppCommand::Stop { package },
        } => Command::AppManage {
            operation: "stop".to_string(),
            package,
        },
        Commands::App {
            command: AppCommand::Clear { package },
        } => Command::AppManage {
            operation: "clear".to_string(),
            package,
        },
        Commands::App {
            command: AppCommand::Uninstall { package },
        } => Command::AppManage {
            operation: "uninstall".to_string(),
            package,
        },
        Commands::App {
            command: AppCommand::Deeplink { uri },
        } => Command::AppDeeplink { uri },
        Commands::App {
            command: AppCommand::Permissions,
        } => Command::Unsupported {
            feature: "app permissions is pending the managed-permissions port".to_string(),
        },
        Commands::Snapshot { full } => Command::Snapshot { full },
        Commands::Screenshot { path } => Command::Screenshot { path },
        Commands::Find { query } => Command::Find { query },
        Commands::Tap(arguments) => Command::Action {
            action: action(
                "tap",
                arguments.selector,
                None,
                None,
                None,
                arguments.coordinates,
                arguments.expected_revision,
                arguments.expected_state_hash,
                arguments.approve_sensitive,
            ),
        },
        Commands::LongPress(arguments) => Command::Action {
            action: action(
                "long_press",
                arguments.selector,
                None,
                None,
                None,
                arguments.coordinates,
                arguments.expected_revision,
                arguments.expected_state_hash,
                arguments.approve_sensitive,
            ),
        },
        Commands::Fill(arguments) => Command::Action {
            action: action(
                "fill",
                arguments.selector,
                Some(arguments.text),
                None,
                None,
                None,
                arguments.expected_revision,
                None,
                false,
            ),
        },
        Commands::Type(arguments) => Command::Action {
            action: action(
                "type",
                arguments.selector,
                Some(arguments.text),
                None,
                None,
                None,
                arguments.expected_revision,
                None,
                false,
            ),
        },
        Commands::Press(arguments) => Command::Action {
            action: action(
                "press",
                None,
                None,
                Some(arguments.key),
                None,
                None,
                arguments.expected_revision,
                None,
                false,
            ),
        },
        Commands::Swipe(arguments) => Command::Action {
            action: action(
                "swipe",
                None,
                None,
                None,
                Some(arguments.direction),
                None,
                arguments.expected_revision,
                None,
                false,
            ),
        },
        Commands::Scroll(arguments) => Command::Action {
            action: action(
                "scroll",
                None,
                None,
                None,
                Some(arguments.direction),
                None,
                arguments.expected_revision,
                None,
                false,
            ),
        },
        Commands::Wait { milliseconds } => {
            let mut result = action("wait", None, None, None, None, None, None, None, false);
            result
                .metadata
                .insert("milliseconds".to_string(), milliseconds.to_string());
            Command::Action { action: result }
        }
        Commands::Batch { actions } => match std::fs::read(&actions)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        {
            Some(actions) => Command::Batch { actions },
            None => Command::Unsupported {
                feature: format!(
                    "batch file {} must be a JSON ActionV1 array",
                    actions.display()
                ),
            },
        },
        Commands::Session {
            command: SessionCommand::List,
        } => Command::SessionList,
        Commands::Session {
            command: SessionCommand::Close,
        }
        | Commands::Close => Command::SessionClose,
        Commands::Bridge {
            command: BridgeCommand::Status,
        } => Command::BridgeStatus,
        Commands::Bridge {
            command: BridgeCommand::Setup { apk },
        } => Command::BridgeSetup { apk },
        Commands::Bridge {
            command: BridgeCommand::Enable,
        } => Command::BridgeEnable,
        Commands::Bridge {
            command: BridgeCommand::Disable,
        } => Command::BridgeDisable,
        Commands::Dashboard {
            command: DashboardCommand::Status,
        } => Command::DashboardStatus,
        Commands::Install => Command::Unsupported {
            feature: "install is pending the cross-platform SDK bootstrap port".to_string(),
        },
        Commands::Upgrade => Command::Unsupported {
            feature: "upgrade is managed by npm, Cargo, Homebrew, or GitHub Releases".to_string(),
        },
        Commands::Run { task: _ } => Command::Unsupported {
            feature: "run is pending the model-planner port".to_string(),
        },
        Commands::Bench => Command::Unsupported {
            feature: "bench is pending the benchmark port".to_string(),
        },
        Commands::Eval {
            list,
            case_id,
            output,
        } => Command::Eval {
            list,
            case: case_id,
            output,
        },
        Commands::Skills => Command::Unsupported {
            feature: "skills is pending the privacy-bounded cache port".to_string(),
        },
        Commands::Mcp
        | Commands::Daemon { .. }
        | Commands::Dashboard {
            command: DashboardCommand::Serve { .. },
        } => unreachable!(),
    }
}

#[allow(clippy::too_many_arguments)]
fn action(
    kind: &str,
    selector: Option<String>,
    text: Option<String>,
    key: Option<String>,
    direction: Option<String>,
    coordinates: Option<[u32; 2]>,
    expected_revision: Option<u64>,
    expected_state_hash: Option<String>,
    approve_sensitive: bool,
) -> ActionV1 {
    let mut metadata = BTreeMap::new();
    if approve_sensitive {
        metadata.insert("approval".to_string(), "granted".to_string());
    }
    ActionV1 {
        action_id: format!("cli-{kind}"),
        kind: kind.to_string(),
        selector,
        text,
        key,
        direction,
        coordinates,
        expected_revision,
        expected_state_hash,
        metadata,
    }
}

fn parse_coordinates(value: &str) -> Result<[u32; 2], String> {
    let (x, y) = value
        .split_once(',')
        .ok_or_else(|| "coordinates must be X,Y".to_string())?;
    Ok([
        x.parse().map_err(|_| "invalid X coordinate".to_string())?,
        y.parse().map_err(|_| "invalid Y coordinate".to_string())?,
    ])
}

fn print_human(value: &serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).expect("result is serializable")
    );
}
