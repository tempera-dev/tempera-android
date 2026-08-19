# Tempera Android Browser

Tempera Android now has two intentionally separate browser execution surfaces:

- `tempera-android-dom-browser`: the dedicated, instrumented `dev.tempera.android.browser` WebView runtime. This is the lowest-latency semantic path for agent-owned browsing.
- `tempera-android-browser`: compatibility control for Chrome-compatible Android browsers through Tempera Android Accessibility/ADB, with bounded CDP target discovery for diagnostics.

Neither surface owns cross-device planning. `tempera-use` / Tempera Browser remains responsible for cross-surface planning, approvals, handoff, replay, and evaluation.

## Fast-path architecture

```text
Tempera Browser / tempera-use
            |
     versioned command
            |
  tempera-android-dom-browser serve
            |
    one long-lived host process
            |
       one ADB forward
            |
   one authenticated HTTP/1.1
        keep-alive channel
            |
 dev.tempera.android.browser
            |
  resident DOM semantic runtime
      |              |
 mutation cache    stable @d refs
      |              |
 snapshot delta   fused act-observe
```

The dedicated browser app binds its control server only to Android loopback. The host reads an app-private bearer token with `run-as`, forwards one loopback port through ADB, and authenticates every request. The server bounds header lines, header count, body size, idle duration, client threads, and requests per connection.

The host keeps the forward and TCP channel alive. Read-only requests may reconnect once after transport failure; mutating requests are never automatically replayed after delivery becomes ambiguous.

## Dedicated browser commands

```bash
tempera-android-dom-browser health
tempera-android-dom-browser open https://example.com
tempera-android-dom-browser snapshot
tempera-android-dom-browser snapshot-delta --previous-state-hash fnv1a64:...
```

For a resident agent loop, use:

```bash
tempera-android-dom-browser serve
```

and send one JSON object per line. The resident process avoids repeated CLI startup, ADB-forward creation, bearer-token lookup, and TCP setup.

A mutating DOM action requires the latest document state hash and a stable `@dN` reference. The normal hot path is fused action plus observation:

```text
snapshot/delta -> plan -> guarded action -> resulting snapshot
```

The DOM runtime is installed once per document, then hot calls invoke small resident functions. A MutationObserver plus input/change/scroll/resize invalidation keeps a cached semantic tree. Stable references are attached per element rather than re-numbered from scratch on every clean snapshot. When the previous state hash still matches, the delta endpoint omits the node array entirely.

DOM evidence is explicitly not trusted by itself for consequential actions. Password and payment/autocomplete values are suppressed from semantic snapshots. Cross-surface verification can still use Tempera Android Accessibility or visual evidence when required.

## Chrome compatibility surface

```bash
tempera-android-browser doctor
tempera-android-browser open https://example.com
tempera-android-browser snapshot
tempera-android-browser targets
```

Stable Chrome defaults to `com.android.chrome`; another Chrome-compatible package can be selected explicitly. Accessibility actions retain revision/state-hash guards and the native bridge's fused `act_observe` path, with ADB/UIAutomator as an independent fallback.

`targets` temporarily forwards an Android localabstract DevTools socket and reads target metadata only. The forward is removed on success and error. The compatibility CLI does not grant arbitrary JavaScript authority.

## Navigation safety

The dedicated browser accepts bounded HTTPS URLs plus `about:blank`. Its WebView disables file/content access, rejects mixed content, requires user gestures for media, disables third-party cookies, and keeps WebView debugging disabled.

The Chrome compatibility surface validates package identifiers, socket names, and browser URLs before they reach ADB.

## Benchmarks

The dedicated host records distributions rather than one sample:

```bash
tempera-android-dom-browser bench --iterations 100
```

Record p50, p95, p99, max, mean, payload bytes, target/device, page, thermal state, build SHA, and verifier success. Compare full snapshots, unchanged deltas, fused action-observe, Accessibility bridge, and ADB fallback on the same target. Do not publish a speed multiplier until equivalent-success benchmark artifacts exist.

## Product boundary

- `tempera-android-dom-browser`: fastest dedicated Android web execution surface.
- `tempera-android-browser`: Chrome-compatible browser control and fallback.
- `tempera-android`: device/app/session/safety/transport substrate.
- desktop browser executor: desktop web execution.
- `tempera-use` / Tempera Browser: cross-surface planner, policy, approvals, evidence, and benchmarks.

The desktop and Android browser engines integrate through versioned contracts rather than importing each other's runtime implementation.
