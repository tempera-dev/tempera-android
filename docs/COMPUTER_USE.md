# Android computer use

`tempera-android` is the supported Android computer-use runtime. It follows
the same structured, revision-bound architecture as Tempera's browser engine:
the CLI, MCP server, and local daemon all create one `CommandRequest`, which
the session-bound executor sends to the selected Android backend.

The imported Python implementation remains in this repository as a frozen
behavioral reference and fixture source. It is not packaged or supported as a
host runtime, and `android-sim` and `android-agent` are not product aliases.

## Control loop

The normal path is semantic rather than screenshot-first:

1. The native Accessibility bridge observes Android's semantic tree over its
   authenticated loopback protocol when it is installed and healthy.
2. Otherwise the independent ADB/UIAutomator backend creates the same compact
   semantic snapshot.
3. Each public node has a short `@eN` reference, a monotonic revision, and a
   deterministic state hash. References expire when the revision changes.
4. An action carries the revision (and fused batches also carry the state hash)
   that it was planned against. The bridge rejects stale batches before any
   device-side side effect.
5. Vision is opt-in and last-resort: the semantic planner must request it, and
   a configured vision model receives a temporary PNG only after that request.

This keeps image bandwidth, visual grounding, and repeated process startup out
of normal forms, settings, and navigation flows without making the bridge a
correctness dependency.

## Run an agent

Start or attach a target, then inspect its semantic state:

```bash
tempera-android device start my-managed-avd --headless
tempera-android --serial emulator-5554 snapshot --json
tempera-android --serial emulator-5554 find Settings --json
```

The bridge is the preferred performance path. It is optional, and direct ADB
remains a zero-install fallback:

```bash
tempera-android --serial emulator-5554 bridge setup
tempera-android --serial emulator-5554 --transport bridge snapshot --json
```

Point the bounded planner loop at an OpenAI-compatible endpoint only when the
task actually needs a model:

```bash
export TEMPERA_ANDROID_ENDPOINT=http://127.0.0.1:11434/v1/chat/completions
export TEMPERA_ANDROID_MODEL=<your-model>

tempera-android --serial emulator-5554 run \
  "Open Settings, go to Network & internet, and inspect Wi-Fi"
```

Use direct actions for deterministic control. Supply the observed revision and
state hash for actions planned from a particular snapshot; `batch` requires
both on every action and rejects stale work as a whole.

```bash
tempera-android --serial emulator-5554 tap @e3 --expected-revision 12
tempera-android --serial emulator-5554 fill @e7 "example value" --expected-revision 13
tempera-android --serial emulator-5554 press BACK --expected-revision 14
```

The autonomous action surface is deliberately bounded to semantic actions,
gestures, keys, wait, and approved navigation. Raw `adb shell` is human-only,
must be enabled explicitly outside this product surface, and is never an MCP
tool.

## Consequential actions and secrets

Targets that appear to send, post, purchase, transfer, delete, submit
credentials, or perform comparable consequential work require explicit
approval. A user-authorized planner run can use:

```bash
tempera-android --serial emulator-5554 run --approve-sensitive "..."
```

Approval is not a bypass of Android or application security. Credentials and
other secrets are resolved locally after planning; they do not belong in
snapshots, logs, skills, recordings, traces, eval reports, or MCP arguments.

## MCP

Run the provider over stdio:

```bash
tempera-android --serial emulator-5554 mcp
```

MCP tools use the `tempera_android_*` namespace and delegate to the same
executor as CLI requests. Tools are grouped by their names: core
(`snapshot`, `tap`, `batch`), device (`device_create`, `device_reset`), apps
(`app_install`, `app_open`), debug (`logs`, `bridge_status`), network
(`network`, `location`, `clipboard`), state (`session`, `state`, `skills`),
and integrations (`doctor`, `install`, `upgrade`, `migrate_legacy_avd`).

Destructive AVD tools require `confirmed: true`; the backend still rejects
physical targets. Screenshot and record remain deliberately CLI-only because
accepting an arbitrary output path from an MCP client would grant a filesystem
write capability. The MCP server does not expose raw shell.

## Evals and historical Gym contract

The Rust `eval` command retains the deterministic Tempera evaluation contract:

```bash
tempera-android eval --list --json
tempera-android --serial emulator-5554 eval --case settings-wifi --json
```

The retained `android_simulator.gym_env.AndroidGymEnv` reference preserves the
existing `trajectory-v1` shape used by `tempera-dev/tempera-gym`, including
the canonical content hash rule that excludes `metadata.timing`. It exists to
compare port behavior and fixtures; release artifacts ship the Rust runtime
and optional Java companion, not that Python package.

## Performance boundary

The native bridge streams semantic observations and runs revision-safe action
batches over a persistent device connection. ADB/UIAutomator remains the
independent baseline. Use `tempera-android bench` on the same fixture and
target before making performance claims; publish the measured raw reports and
do not claim a fixed speedup without evidence.

This engine controls ordinary Android UI. It does not forge attestation,
bypass Play Integrity, defeat anti-bot systems, spoof protected identifiers,
or conceal emulator use.
