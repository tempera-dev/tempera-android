# Authenticated daemon contract

The long-lived JSONL daemon is a local transport, not an ambient authority boundary. It rejects non-loopback binds and also requires an explicit capability token before opening the listener.

## Required authority

Set these variables in the daemon process environment:

```text
TEMPERA_ANDROID_DAEMON_TOKEN=<32-4096 non-whitespace bytes>
TEMPERA_ANDROID_DAEMON_SCOPE=tempera-use
TEMPERA_ANDROID_DAEMON_SESSION_ID=<exact Tempera Use session identifier>
```

`TEMPERA_ANDROID_DAEMON_SCOPE` defaults to `tempera-use`. That scope requires `TEMPERA_ANDROID_DAEMON_SESSION_ID`; an arbitrary request cannot mint a new session authority.

An executor may additionally bind the token to exactly one Android device:

```text
TEMPERA_ANDROID_DAEMON_SERIAL=<exact adb serial>
```

When no serial is bound, the `tempera-use` scope rejects requests that try to select an explicit serial. This prevents a token intended for one local integration from becoming authority over every attached device.

The `admin` scope is deliberately separate and requires both:

```text
TEMPERA_ANDROID_DAEMON_SCOPE=admin
TEMPERA_ANDROID_DAEMON_ALLOW_ADMIN=1
```

Do not use the admin scope for Tempera Use.

## Wire envelope

Each newline-delimited request adds `capabilityToken` to the canonical `CommandRequest` fields:

```json
{
  "capabilityToken": "<secret>",
  "id": "request-1",
  "sessionId": "session-1",
  "serial": "emulator-5554",
  "transport": "bridge",
  "command": {
    "name": "snapshot",
    "arguments": {
      "full": false
    }
  }
}
```

The request frame remains subject to the 256 KiB daemon frame limit. The daemon erases the complete mutable wire buffer after decoding on success and rejection paths; it also erases the provided token bytes after comparison and the retained token when the authority object is dropped.

## Tempera Use scope

The least-authority scope permits only:

- snapshot;
- non-persistent screenshot;
- state-guarded action;
- fused batch;
- state;
- bridge status;
- session close.

It permits only `auto`, `adb`, and `bridge` transports. Appium URLs, Appium capabilities, persistent screenshots, device administration, clipboard access, arbitrary shell execution, and other administrative commands are rejected before command dispatch.

## Rejection and connection behavior

Authentication, session, device, transport, and command-scope failures all use the same generic response:

```json
{
  "id": "unknown",
  "ok": false,
  "result": null,
  "error": "Android daemon request rejected"
}
```

A persistent client may recover from a malformed or unauthorized frame, but the daemon closes the connection after three cumulative rejected frames. Oversized framing returns one bounded failure and closes immediately. Capacity saturation returns a bounded busy response without dispatching the command.

A rejected request is pre-dispatch. It must never be interpreted as an uncertain external effect. Transport loss after an accepted effect remains governed by the command's state guards, action identifiers, replay semantics, and the upstream Tempera Use effect journal.

## Rotation and process boundaries

Generate a high-entropy token for one daemon process and one authority scope. Pass it through process-local secret injection rather than command-line arguments or repository files. Rotate it by replacing the daemon process; do not mutate authority for an already-open listener.

The downstream Tempera Use adapter must add the token only at the final wire boundary. It must not place the token in observations, evidence payloads, receipts, logs, task memory, benchmark artifacts, or durable journals.

## Stack note

This contract has been replayed onto the current `agent/runtime-efficiency` Android-browser/runtime head. Historical PR #6 remains useful validation evidence for the authority delta by itself, but the release SHA must come from the current-runtime composition branch after its own exact-head gates pass.
