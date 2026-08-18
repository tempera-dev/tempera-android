# Tempera Android Browser

`tempera-android-browser` is the browser-specific execution surface for Chrome-compatible browsers on Android. It is not a second general Android orchestrator and it does not duplicate cross-surface planning from `tempera-use`.

## Architecture

```text
Tempera Browser / tempera-use
            |
     versioned command
            |
  tempera-android-browser
      |             |
      |             +-- temporary read-only CDP target discovery
      |
      +-- native Accessibility bridge (preferred)
      +-- ADB/UIAutomator fallback
            |
      Chrome on Android
```

The native semantic path remains authoritative for actions. CDP target discovery is an optional diagnostic and future DOM-acceleration seam; arbitrary JavaScript is not exposed by the browser CLI.

## Commands

```bash
tempera-android-browser doctor
tempera-android-browser open https://example.com
tempera-android-browser snapshot
tempera-android-browser targets
```

A snapshot returns compact browser nodes plus the underlying Android snapshot, revision, and state hash. Mutating commands require both guards from the latest snapshot:

```bash
tempera-android-browser tap @e7 \
  --expected-revision 12 \
  --expected-state-hash sha256:...
```

The result includes the action receipt and the next semantic snapshot in one process invocation:

```text
observe -> plan -> guarded action -> observe -> receipt
```

This removes a second CLI startup from the normal agent loop. The native bridge itself already supports fused `act_observe`; the Android browser surface is designed to consume that path while retaining the independent ADB fallback.

## Browser packages

Stable Chrome is the default:

```text
com.android.chrome
```

Other Chrome-compatible Android packages can be selected explicitly:

```bash
tempera-android-browser --package org.chromium.chrome snapshot
```

Package identifiers and DevTools socket names are validated before they reach ADB.

## Navigation safety

`open` accepts only bounded HTTP(S) URLs. It rejects embedded credentials, whitespace, control characters, and non-web schemes. Navigation is performed with an Android VIEW intent scoped to the selected browser package.

Browser mutations still pass through Tempera's canonical revision guards, stale-state rejection, sensitive-action approval policy, secret redaction, and action receipts. The Android browser surface does not gain raw shell authority.

## Chrome DevTools targets

`targets` temporarily forwards the Android loopback socket and reads `/json/list`. The forward is removed on success and on every error path.

The default socket is:

```text
chrome_devtools_remote
```

A debuggable WebView may expose a package-specific localabstract socket. WebView debugging must be enabled by the application owner; Tempera does not turn it on or weaken Android's authorization boundary.

## Benchmarks

```bash
tempera-android-browser bench --iterations 100
```

The command reports min, mean, p95, and max semantic-observation time for the exact target and transport. Compare transports on the same device, page, thermal state, and build. Keep raw samples and do not convert one machine's result into a general multiplier claim.

## Product boundary

- `tempera-android-browser`: Android Chrome execution and evidence.
- `tempera-android`: device, app, session, safety, and transport substrate.
- `Tempo` / desktop browser executor: desktop web execution.
- `tempera-use`: cross-surface planning, policy, approvals, handoff, replay, and benchmarks.
- Future Tempera Browser: operator-facing desktop product shell.

The Android browser and desktop browser communicate through the shared versioned orchestration contract, not by importing each other's runtime code.
