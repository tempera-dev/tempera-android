# Tempera Android architecture

`tempera-android` uses the same product shape as Tempera's browser engine:

```text
CLI / MCP / daemon JSONL
          │ CommandRequest (versioned)
          ▼
session-bound command executor
          │
          ├── native Accessibility bridge (preferred, optional)
          ├── direct ADB + UIAutomator (independent fallback)
          └── Appium/provider adapter seam (optional integration)
          ▼
SnapshotV1 + ActionReceiptV1 + persisted read-only inspector state
```

There is one canonical public executor. CLI commands, MCP tools, and daemon
requests construct the same `CommandRequest`; no integration owns a second
automation semantics.

## State and concurrency

An observation carries a monotonic revision and deterministic semantic state
hash. Public node references are `@eN` and expire once the revision changes.
The native bridge maps them to private device references only in host memory.
The bridge receives the expected revision before dispatching a batch. It
rejects stale work before invoking any action and then performs a settled
act-observe transition. ADB remains available when the companion is not
installed, but it cannot pretend to have bridge-level atomicity.

Sessions and inspector records live under `TEMPERA_ANDROID_HOME`, defaulting to
`~/.tempera-android`. Session closure removes Tempera state only; it never
stops an attached device.

## Native bridge boundary

The Java companion package is `dev.tempera.android.bridge`. It binds on Android
loopback only. The Rust host creates an `adb forward`, reads a local per-device
token, negotiates protocol v3 and an epoch, and tears down the forward after
the request. The companion has no arbitrary shell API. Password fields are
redacted during observation and blocked from read-modify-write behavior.

## Targets

Managed emulators are explicit records, created by the underlying Android SDK
tools on macOS, Linux, or Windows. A Tempera record is required for reset or
delete; legacy AVDs are never silently adopted. ADB serials cover local
emulators, USB devices, wireless devices, and remote connections. Destructive
emulator operations reject physical serials before sending a command.

Managed emulator capability depends on upstream images and host acceleration.
Where that is unavailable, attached and remote ADB targets remain supported.

## Inspector and integrations

The local dashboard is read-only and polls persisted state so it cannot add
latency or authority to automation. It exposes sessions, latest semantic state,
and receipts. The optional generic Appium W3C adapter translates XML page
source and bounded pointer/key actions into the same public contract; provider
adapters remain behind that seam and must source credentials from a local
resolver, never checked-in configuration.
