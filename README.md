# Tempera Android

`tempera-android` is a Rust Android computer-use engine for AI agents. It has one canonical CLI, crate, and npm package name; there are no compatibility aliases. The native Accessibility companion is the fast path, direct ADB/UIAutomator is a zero-install independent fallback, and generic Appium is an optional integration boundary.

## Alpha status

Version `0.4.0-alpha.1` is an engineering preview. The Rust unit suite and direct-ADB emulator smoke job are enforced in CI. Real macOS, Windows, and physical-device proof is a release gate, not a claim made by this repository.

## Install and first observation

Build locally with Rust 1.93+:

```bash
cargo install --path cli
tempera-android doctor
tempera-android install --profile google --api 36
tempera-android device list
tempera-android --serial emulator-5554 snapshot --json
tempera-android --serial emulator-5554 find "Continue" --json
tempera-android --serial emulator-5554 network --json
tempera-android --serial emulator-5554 location 37.7749 -122.4194 --json
```

Android SDK tools are discovered through `ANDROID_SDK_ROOT` (or `ANDROID_HOME`) and the host defaults. Managed emulator lifecycle uses `sdkmanager`, `avdmanager`, `emulator`, and `adb` directly; it deliberately does not depend on Google's experimental `android` CLI.

```bash
tempera-android device create --name tempera-google --profile google --api 36
tempera-android device start tempera-google --headless
tempera-android --serial emulator-5554 app install app.apk
tempera-android --serial emulator-5554 snapshot --json
```

## Controls, sessions, and safety

Every machine response carries `tempera.android.control/v1`. `SnapshotV1` contains a monotonic revision, state hash, screen metadata, and compact semantic `@eN` references. References expire on a changed revision. A fused `batch` requires the same `expectedRevision` and `expectedStateHash` for every action; a stale bridge batch executes nothing.

Consequential targets such as send, post, purchase, transfer, or delete require explicit approval metadata. The bridge redacts password values, is loopback only, authenticates each request with a per-device token, rejects stale epochs, and provides at-most-once request handling. Raw shell is intentionally not an agent/MCP surface.

Set `TEMPERA_ANDROID_HOME` to isolate sessions and bridge tokens. The full configuration contract is in [`tempera-android.schema.json`](tempera-android.schema.json). `close` removes only the Tempera session and bridge forwarding; stopping an emulator is always an explicit `device stop` operation.

`tempera-android.json` in the working directory, or the path named by `TEMPERA_ANDROID_CONFIG`, can set a default serial and transport. Environment variables (`TEMPERA_ANDROID_SERIAL`, `TEMPERA_ANDROID_TRANSPORT`, `TEMPERA_ANDROID_APPIUM_URL`, and JSON-valued `TEMPERA_ANDROID_APPIUM_CAPABILITIES`) override file values. Legacy Android Simulator metadata is detected by `doctor`, never moved automatically. To explicitly copy one historical metadata record, use `tempera-android migrate legacy-avd NAME --yes`; it never moves, resets, or deletes the Android-owned AVD data.

When an Appium URL is configured, `doctor` performs a bounded HTTPS/HTTP `/status` probe and reports endpoint health without storing credentials. `--transport appium` creates a generic W3C session, translates XML source into the same revision-bound semantic contract, and supports snapshot/find, tap/long-press, type/fill, bounded gestures, supported Android keys, batch, benchmark, and eval. Use ADB for device and app administration. Credential-like capability keys are rejected in project and environment configuration; a cloud-provider adapter must resolve them outside the engine configuration.

## Native bridge

```bash
tempera-android --serial emulator-5554 bridge setup
tempera-android --serial emulator-5554 bridge status --json
tempera-android --serial emulator-5554 --transport bridge snapshot --json
tempera-android --transport appium --appium-url http://127.0.0.1:4723 snapshot --json
```

`auto` uses the bridge only when its APK, Accessibility service, token, and loopback health check are available; otherwise it uses ADB/UIAutomator. `--transport bridge` never silently falls back. Accessibility permission on a physical device is intentionally manual.

## MCP, daemon, and dashboard

```bash
tempera-android --serial emulator-5554 mcp
tempera-android daemon serve --listen 127.0.0.1:7421
tempera-android dashboard serve --listen 127.0.0.1:7422
tempera-android eval --list --json
tempera-android --serial emulator-5554 bench --iterations 20 --json
```

MCP tools are named `tempera_android_*` and delegate to the same canonical command executor. The read-only dashboard displays persisted sessions, latest semantic snapshots, and action receipts without participating in the control path.

## Target boundaries

- Managed emulator operations create, start, reset, and delete only Tempera-recorded AVDs. Existing AVD data is never silently imported, moved, reset, or deleted.
- Attached USB, wireless, and remote ADB targets use their serial. Emulator reset/delete/stop refuses physical targets.
- APKs may be installed from supplied paths. This project does not scrape Play Store APKs, spoof protected identifiers, or claim attestation equivalence.
- Appium/provider integration is a plugin seam; provider credentials are never stored in `tempera-android.json` or repository configuration.

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md), [`docs/PROTOCOL.md`](docs/PROTOCOL.md), and [`docs/MIGRATION.md`](docs/MIGRATION.md).
