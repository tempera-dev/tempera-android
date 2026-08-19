# Tempera Android Browser

A dedicated, hardened Android browser for agent execution. It is not a generic alias for screen automation.

## Control paths

```text
Tempera browser planner
        │
        ├─ primary: loopback WebView DOM control
        │      compact DOM snapshot
        │      deterministic @dN references
        │      expected document-state hash
        │      fused action + observation
        │
        └─ fallback/verifier: Tempera Android
               native Accessibility bridge
               Android/browser chrome
               screenshots only by escalation
```

The direct DOM path avoids UIAutomator XML generation and coordinate guessing. It does **not** grant the page authority over Android: no `addJavascriptInterface` is registered and no website can call the native control server.

DOM evidence is marked `trustedForConsequentialActions: false`. Purchasing, sending, posting, deleting, credential submission, and comparable actions must still pass the Tempera Android approval policy and should be independently verified through Accessibility or visual evidence.

## Security boundaries

- Control binds only to `127.0.0.1:7433` inside the device.
- Every request requires a 256-bit token stored in the app-private files directory.
- The host reads the token through `adb shell run-as`; it is not persisted in traces or repository configuration.
- WebView remote debugging is disabled.
- File and content URL access are disabled.
- Mixed content and cleartext browsing are disabled.
- Only HTTPS and `about:blank` navigation are accepted.
- Third-party cookies are disabled by default.
- Requests and headers are strictly bounded.

## Build

```bash
./scripts/build-android-browser.sh
```

The output is:

```text
android-browser/app/build/outputs/apk/debug/app-debug.apk
```

Install and launch:

```bash
adb install -r android-browser/app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n \
  dev.tempera.android.browser/dev.tempera.android.browser.TemperaBrowserActivity
```

## Host CLI

The Rust package automatically exposes a second binary from `cli/src/bin`:

```bash
cargo run -p tempera-android --bin tempera-android-browser -- health
cargo run -p tempera-android --bin tempera-android-browser -- open https://example.com
cargo run -p tempera-android --bin tempera-android-browser -- snapshot
```

Actions must carry the exact state hash returned by the snapshot:

```bash
cargo run -p tempera-android --bin tempera-android-browser -- \
  tap @d3 --expected-state-hash fnv1a64:0123456789abcdef
```

The host CLI creates an ADB loopback forwarding rule and resolves the control token just in time. It never accepts the token as a command-line flag.

## API

- `GET /v1/health`
- `GET /v1/snapshot`
- `POST /v1/navigate`
- `POST /v1/action`
- `POST /v1/act-observe`
- `POST /v1/wait`

Normal agents should prefer `act-observe` to remove one host/device round trip and bind the result directly to the executed action.

## Performance evidence required

No global speed multiple is claimed. Release evidence must compare, on the same device and page fixtures:

- WebView DOM snapshot versus Accessibility snapshot and ADB/UIAutomator;
- sequential action + snapshot versus fused act-observe;
- warm and cold page state;
- payload bytes and node count;
- p50, p95, p99 latency and verifier success;
- stale action side effects, which must remain zero.
