//! Managed Android Virtual Device lifecycle.
//!
//! A managed entry is recorded before it can be reset or deleted. This avoids
//! silently adopting, moving, or destroying a developer's existing AVDs.

use crate::error::{AndroidError, Result};
use crate::model::SnapshotV1;
use crate::session::SessionStore;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedAvdV1 {
    pub name: String,
    pub profile: String,
    pub api: u32,
    pub device: String,
    pub system_image: String,
    pub created_at_ms: u128,
}

#[derive(Debug, Clone)]
pub struct CreateOptions {
    pub name: String,
    pub profile: String,
    pub api: u32,
    pub device: Option<String>,
    pub ram_mb: Option<u32>,
    pub data_gb: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct StartOptions {
    pub name: String,
    pub headless: bool,
    pub cold: bool,
}

#[derive(Debug, Clone)]
pub struct InstallOptions {
    pub profile: String,
    pub api: u32,
}

#[derive(Debug, Clone)]
struct Tools {
    sdk_root: PathBuf,
    sdkmanager: PathBuf,
    avdmanager: PathBuf,
    emulator: PathBuf,
}

pub fn create(store: &SessionStore, options: CreateOptions) -> Result<ManagedAvdV1> {
    validate_name(&options.name)?;
    let profile = normalize_profile(&options.profile)?;
    let tools = tools()?;
    let api = options.api;
    let device = options.device.unwrap_or_else(|| "pixel_8".to_string());
    let system_image = system_image(&profile, api)?;
    let sdk_root = format!("--sdk_root={}", path(&tools.sdk_root));
    run(&tools.sdkmanager, &[&sdk_root, &system_image])?;
    let mut command = Command::new(&tools.avdmanager);
    command
        .args([
            "create",
            "avd",
            "--force",
            "--name",
            &options.name,
            "--package",
            &system_image,
            "--device",
            &device,
        ])
        .env("ANDROID_SDK_ROOT", &tools.sdk_root)
        .env("ANDROID_HOME", &tools.sdk_root)
        .stdin(Stdio::piped());
    let mut child = command.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(b"no\n")?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(AndroidError::Backend(command_error(&output)));
    }
    let managed = ManagedAvdV1 {
        name: options.name,
        profile,
        api,
        device,
        system_image,
        created_at_ms: SnapshotV1::now_ms(),
    };
    save(store, &managed)?;
    if options.ram_mb.is_some() || options.data_gb.is_some() {
        configure_avd(&managed.name, options.ram_mb, options.data_gb)?;
    }
    Ok(managed)
}

/// Install the SDK pieces needed by managed emulators. The Android command-line
/// tools themselves must already be present: fetching an unsigned bootstrap
/// from an arbitrary URL is deliberately outside this command.
pub fn install_sdk(options: InstallOptions) -> Result<serde_json::Value> {
    if !(21..=100).contains(&options.api) {
        return Err(AndroidError::InvalidInput(
            "Android API must be 21..=100".to_string(),
        ));
    }
    let profile = normalize_profile(&options.profile)?;
    let root = sdk_root();
    let sdkmanager = command_line_tool(&root, "sdkmanager").ok_or_else(|| {
        AndroidError::Backend(format!(
            "Required Android SDK command-line tool sdkmanager is missing under {}. Install the official command-line tools first, set ANDROID_SDK_ROOT, then rerun tempera-android install.",
            root.display()
        ))
    })?;
    let image = system_image(&profile, options.api)?;
    let packages = vec![
        "platform-tools".to_string(),
        "emulator".to_string(),
        "cmdline-tools;latest".to_string(),
        format!("platforms;android-{}", options.api),
        image.clone(),
    ];
    let mut command = Command::new(&sdkmanager);
    command
        .arg(format!("--sdk_root={}", path(&root)))
        .args(&packages)
        .env("ANDROID_SDK_ROOT", &root)
        .env("ANDROID_HOME", &root)
        .stdin(Stdio::piped());
    let mut child = command.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(b"y\ny\ny\n")?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(AndroidError::Backend(command_error(&output)));
    }
    Ok(serde_json::json!({
        "sdkRoot": root,
        "sdkmanager": sdkmanager,
        "installedPackages": packages,
        "systemImage": image,
    }))
}

pub fn list(store: &SessionStore) -> Result<Vec<ManagedAvdV1>> {
    let directory = store.root().join("devices");
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut values = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.path().extension().and_then(|value| value.to_str()) == Some("json") {
            values.push(serde_json::from_slice(&fs::read(entry.path())?)?);
        }
    }
    values.sort_by(|left: &ManagedAvdV1, right| left.name.cmp(&right.name));
    Ok(values)
}

pub fn start(store: &SessionStore, options: StartOptions) -> Result<ManagedAvdV1> {
    let managed = load(store, &options.name)?;
    let tools = tools()?;
    let mut command = Command::new(&tools.emulator);
    command.args(["-avd", &managed.name]);
    if options.headless {
        command.args(["-no-window", "-no-audio"]);
    }
    if options.cold {
        command.arg("-no-snapshot");
    }
    command
        .env("ANDROID_SDK_ROOT", &tools.sdk_root)
        .env("ANDROID_HOME", &tools.sdk_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(managed)
}

pub fn stop(serial: &str) -> Result<()> {
    reject_physical(serial, "stop")?;
    crate::adb::AdbBackend::new(serial)?.shell(&["reboot", "-p"])?;
    Ok(())
}

pub fn reset(store: &SessionStore, name: &str, confirmed: bool) -> Result<ManagedAvdV1> {
    if !confirmed {
        return Err(AndroidError::InvalidInput(
            "device reset is destructive; rerun with --yes".to_string(),
        ));
    }
    let managed = load(store, name)?;
    let tools = tools()?;
    run(
        &tools.avdmanager,
        &["delete", "avd", "--name", &managed.name],
    )?;
    create(
        store,
        CreateOptions {
            name: managed.name,
            profile: managed.profile,
            api: managed.api,
            device: Some(managed.device),
            ram_mb: None,
            data_gb: None,
        },
    )
}

pub fn delete(store: &SessionStore, name: &str, confirmed: bool) -> Result<()> {
    if !confirmed {
        return Err(AndroidError::InvalidInput(
            "device delete is destructive; rerun with --yes".to_string(),
        ));
    }
    let managed = load(store, name)?;
    let tools = tools()?;
    run(
        &tools.avdmanager,
        &["delete", "avd", "--name", &managed.name],
    )?;
    fs::remove_file(path_for(store, &managed.name))?;
    Ok(())
}

/// Copy one historical Android Simulator *metadata record* into the Tempera
/// managed-device registry. It intentionally does not invoke the SDK, move an
/// AVD, or modify any Android-owned data. Reset/delete remain separately
/// confirmed operations after this explicit import.
pub fn import_legacy(
    store: &SessionStore,
    name: &str,
    legacy_root: &Path,
    confirmed: bool,
) -> Result<ManagedAvdV1> {
    if !confirmed {
        return Err(AndroidError::InvalidInput(
            "legacy metadata import is explicit; rerun with --yes after confirming the named AVD"
                .to_string(),
        ));
    }
    validate_name(name)?;
    let destination = path_for(store, name);
    if destination.exists() {
        return Err(AndroidError::InvalidInput(format!(
            "{name:?} is already Tempera-managed; refusing to replace its metadata"
        )));
    }
    let source = legacy_root.join("instances").join(format!("{name}.json"));
    if !source.is_file() {
        return Err(AndroidError::InvalidInput(format!(
            "No legacy metadata exists at {}; no Android data was changed",
            source.display()
        )));
    }
    let raw: serde_json::Value = serde_json::from_slice(&fs::read(&source)?)?;
    let profile = raw
        .get("profile")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| AndroidError::InvalidInput("legacy metadata omitted profile".to_string()))?;
    let api = raw
        .get("api")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| {
            AndroidError::InvalidInput("legacy metadata omitted a valid api".to_string())
        })?;
    let device = raw
        .get("device_profile")
        .or_else(|| raw.get("device"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("pixel_8")
        .to_string();
    let managed = ManagedAvdV1 {
        name: name.to_string(),
        profile: normalize_profile(profile)?,
        api,
        device,
        system_image: raw
            .get("image_package")
            .or_else(|| raw.get("system_image"))
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or(system_image(profile, api)?),
        created_at_ms: SnapshotV1::now_ms(),
    };
    save(store, &managed)?;
    Ok(managed)
}

pub fn doctor() -> Result<serde_json::Value> {
    let tools = tools()?;
    Ok(serde_json::json!({
        "sdkRoot": tools.sdk_root,
        "sdkmanager": tools.sdkmanager,
        "avdmanager": tools.avdmanager,
        "emulator": tools.emulator,
        "managedEmulator": true,
    }))
}

fn load(store: &SessionStore, name: &str) -> Result<ManagedAvdV1> {
    validate_name(name)?;
    let metadata = path_for(store, name);
    if !metadata.is_file() {
        return Err(AndroidError::InvalidInput(format!(
            "{name:?} is not a Tempera-managed AVD. Existing AVDs are never adopted automatically; attach them by ADB serial or create/import one explicitly."
        )));
    }
    Ok(serde_json::from_slice(&fs::read(metadata)?)?)
}

fn save(store: &SessionStore, avd: &ManagedAvdV1) -> Result<()> {
    let directory = store.root().join("devices");
    fs::create_dir_all(&directory)?;
    let destination = path_for(store, &avd.name);
    let temporary = destination.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(avd)?)?;
    fs::rename(temporary, destination)?;
    Ok(())
}

fn path_for(store: &SessionStore, name: &str) -> PathBuf {
    store.root().join("devices").join(format!("{name}.json"))
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 80
        || !name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(AndroidError::InvalidInput(
            "AVD names may contain only letters, digits, '.', '-' and '_'".to_string(),
        ));
    }
    Ok(())
}

fn normalize_profile(profile: &str) -> Result<String> {
    match profile {
        "play" | "google" | "aosp" => Ok(profile.to_string()),
        other => Err(AndroidError::InvalidInput(format!(
            "Unknown profile {other:?}; use play, google, or aosp"
        ))),
    }
}

fn system_image(profile: &str, api: u32) -> Result<String> {
    let architecture = if cfg!(target_arch = "aarch64") {
        "arm64-v8a"
    } else {
        "x86_64"
    };
    let tag = match profile {
        "play" => "google_apis_playstore",
        "google" => "google_apis",
        "aosp" => "default",
        _ => unreachable!(),
    };
    Ok(format!("system-images;android-{api};{tag};{architecture}"))
}

fn tools() -> Result<Tools> {
    let root = sdk_root();
    let sdkmanager = command_line_tool(&root, "sdkmanager").unwrap_or_else(|| {
        root.join("cmdline-tools/latest/bin")
            .join(executable("sdkmanager"))
    });
    let avdmanager = command_line_tool(&root, "avdmanager").unwrap_or_else(|| {
        root.join("cmdline-tools/latest/bin")
            .join(executable("avdmanager"))
    });
    let emulator = root.join("emulator").join(executable("emulator"));
    for tool in [&sdkmanager, &avdmanager, &emulator] {
        if !tool.is_file() {
            return Err(AndroidError::Backend(format!(
                "Required Android SDK tool is missing: {}. Set ANDROID_SDK_ROOT or run tempera-android install.",
                tool.display()
            )));
        }
    }
    Ok(Tools {
        sdk_root: root,
        sdkmanager,
        avdmanager,
        emulator,
    })
}

fn sdk_root() -> PathBuf {
    env::var_os("ANDROID_SDK_ROOT")
        .or_else(|| env::var_os("ANDROID_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(default_sdk_root)
}

fn command_line_tool(root: &Path, name: &str) -> Option<PathBuf> {
    let executable = executable(name);
    let latest = root.join("cmdline-tools/latest/bin").join(&executable);
    if latest.is_file() {
        return Some(latest);
    }
    let directory = root.join("cmdline-tools");
    let mut candidates = fs::read_dir(directory)
        .ok()?
        .flatten()
        .map(|entry| entry.path().join("bin").join(&executable))
        .filter(|candidate| candidate.is_file())
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.pop()
}

fn default_sdk_root() -> PathBuf {
    if cfg!(target_os = "macos") {
        home().join("Library/Android/sdk")
    } else if cfg!(windows) {
        env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(home)
            .join("Android/Sdk")
    } else {
        home().join("Android/Sdk")
    }
}

fn home() -> PathBuf {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn configure_avd(name: &str, ram_mb: Option<u32>, data_gb: Option<u32>) -> Result<()> {
    let config = home()
        .join(".android/avd")
        .join(format!("{name}.avd/config.ini"));
    if !config.is_file() {
        return Ok(());
    }
    let mut source = fs::read_to_string(&config)?;
    if let Some(ram) = ram_mb {
        source.push_str(&format!("\nhw.ramSize={ram}\n"));
    }
    if let Some(data) = data_gb {
        source.push_str(&format!("disk.dataPartition.size={data}G\n"));
    }
    fs::write(config, source)?;
    Ok(())
}

fn reject_physical(serial: &str, operation: &str) -> Result<()> {
    if !serial.starts_with("emulator-") {
        return Err(AndroidError::InvalidInput(format!(
            "device {operation} is emulator-only and will not run against physical target {serial:?}"
        )));
    }
    Ok(())
}

fn run(program: &Path, arguments: &[&str]) -> Result<()> {
    let output = Command::new(program).args(arguments).output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(AndroidError::Backend(command_error(&output)))
    }
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap_or_default()
}

fn executable(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.bat")
    } else {
        name.to_string()
    }
}

fn command_error(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stderr.is_empty() {
        stdout
    } else {
        stderr
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_have_explicit_system_image_contracts() {
        assert!(system_image("play", 36)
            .unwrap()
            .contains("google_apis_playstore"));
        assert!(system_image("aosp", 36).unwrap().contains(";default;"));
    }

    #[test]
    fn physical_targets_are_never_stopped() {
        assert!(reject_physical("012345", "reset").is_err());
        assert!(reject_physical("emulator-5554", "reset").is_ok());
    }

    #[test]
    fn legacy_import_is_explicit_and_never_replaces_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let store = SessionStore::new(directory.path().join("tempera")).unwrap();
        let legacy = directory.path().join("legacy");
        std::fs::create_dir_all(legacy.join("instances")).unwrap();
        std::fs::write(
            legacy.join("instances/demo.avd.json"),
            r#"{"name":"demo.avd","profile":"google","api":36,"device_profile":"pixel_8","image_package":"system-images;android-36;google_apis;arm64-v8a"}"#,
        )
        .unwrap();
        assert!(import_legacy(&store, "demo.avd", &legacy, false).is_err());
        let imported = import_legacy(&store, "demo.avd", &legacy, true).unwrap();
        assert_eq!(imported.name, "demo.avd");
        assert!(import_legacy(&store, "demo.avd", &legacy, true).is_err());
        assert!(legacy.join("instances/demo.avd.json").is_file());
    }

    #[test]
    fn system_images_follow_the_host_architecture() {
        let image = system_image("google", 36).unwrap();
        assert!(image.starts_with("system-images;android-36;google_apis;"));
    }
}
