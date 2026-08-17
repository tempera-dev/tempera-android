# Historical Android computer-use reference

> This document describes the imported Python prototype and is retained only as
> historical design context. It is not a shipped host runtime or command-line
> interface. Use [`tempera-android`](../README.md) and the Rust contracts for
> the supported product surface.

The historical prototype used a structured-first control loop rather than a
screenshot-only loop.

## Why this architecture

A browser-use-style agent often pays for a screenshot, image encoding, a vision-model pass, grounding, and one click for every interaction. Android exposes more structure than a browser screenshot does, so the default loop is:

1. `uiautomator dump --compressed` for a compact semantic hierarchy.
2. Convert nodes to stable short refs (`n0`, `n1`, …), labels, resource IDs, roles, bounds, and actionability.
3. Give the planner only the high-value nodes.
4. Ground locally and execute over ADB.
5. Batch deterministic actions when no intermediate observation is needed.
6. Capture a screenshot only if the planner explicitly reports that structured UI is insufficient.

This keeps image bandwidth and model latency off the common path.

## Run an agent

Start an emulator first:

```bash
android-sim start android-sim-play
```

Point the agent at any OpenAI-compatible chat endpoint (local or hosted):

```bash
export ANDROID_AGENT_ENDPOINT=http://127.0.0.1:11434/v1/chat/completions
export ANDROID_AGENT_MODEL=<your-model>
# export ANDROID_AGENT_API_KEY=...   # only if your endpoint requires it

android-agent run "Open Settings, go to Network & internet, and turn Wi-Fi off"
```

Inspect the exact structured state without a model:

```bash
android-agent observe
```

Execute a deterministic action directly:

```bash
android-agent act '{"type":"tap","selector":"Settings"}'
android-agent act '{"type":"scroll","direction":"down"}'
android-agent act '{"type":"back"}'
```

## Fast action surface

The model-facing action set is deliberately small:

- `tap` by semantic ref/selector or coordinate
- `long_press`
- `type`
- `key`
- `back`
- `home`
- `enter`
- `swipe`
- `scroll`
- `launch`
- `wait`

Arbitrary `adb shell` is **not** exposed to the autonomous planner. Humans still have `android-sim shell` for debugging. This separation keeps the model action surface bounded and auditable.

## Vision fallback

The planner can return:

```json
{"done":false,"need_vision":true,"summary":"canvas has no accessibility nodes","actions":[]}
```

Only then does the runtime capture a PNG and make a multimodal planning call. This is useful for games, maps, custom canvases, images, and badly-instrumented apps while keeping normal form/navigation tasks on the low-latency semantic path.

## Side-effect gate

The runtime pauses before UI targets whose labels indicate consequential actions such as sending, posting, buying, paying, transferring, deleting, subscribing, booking, ordering, or submitting.

For an intentionally authorized task, opt in explicitly:

```bash
android-agent run --approve-sensitive "..."
```

This is a local approval boundary, not an attempt to bypass Android or application security controls.

## Tempera MCP

Run the Android tool provider over stdio:

```bash
android-agent mcp
```

It exposes:

- `android_observe`
- `android_act`
- `android_macro`

The provider is intentionally dependency-free and can be fronted by `tempera-dev/tempera-mcp` for production admission policy, routing, receipts, observability, and transport concerns. Keeping the Android executor separate means `tempera-mcp` owns protocol/governance while this repo owns Android semantics.

`android_macro` is the important latency primitive: a capable parent agent can issue several deterministic key/coordinate actions in one MCP call instead of incurring one network/model round trip per action.

## Tempera Gym

`android_simulator.gym_env.AndroidGymEnv` exposes `reset()` and `step()` and emits the exact top-level `trajectory-v1` shape used by `tempera-dev/tempera-gym`, including its canonical content hash rule (the reserved `metadata.timing` field is excluded from identity).

Example:

```python
from android_simulator.gym_env import AndroidGymEnv, SuccessSpec

env = AndroidGymEnv(
    controller,
    success=SuccessSpec(package="com.android.settings", text_present=("Wi‑Fi",)),
)
obs = env.reset(home=True)
obs, reward, terminated, truncated, info = env.step({"type":"tap", "selector":"Settings"})
trajectory = env.trajectory_v1(metadata={"policy": "my-agent"})
```

This lets Tempera Gym benchmark policies, collect trajectories, compare latency/reward, and later train grounding/planning policies without inventing a separate Android environment implementation.

## Performance strategy

The current optimization order is intentional:

1. structured hierarchy before pixels
2. local selector resolution before model grounding
3. compact node ranking before raw XML
4. deterministic macro batching before repeated planner calls
5. state hashing to detect stalls
6. vision only on demand
7. model endpoint is swappable, so a local low-latency planner can be used for routine navigation while a larger multimodal model handles rare ambiguous states

The next performance tier should be an on-device accessibility helper APK that streams hierarchy diffs and executes selector actions over a persistent socket. That removes repeated `uiautomator dump` process startup and several ADB round trips per observation. The current architecture deliberately keeps that as an optimization layer, not a requirement for correctness.

## Boundaries

This runtime controls normal Android UI and applications. It does not forge device attestation, bypass Play Integrity, defeat anti-bot systems, spoof protected hardware identifiers, or attempt to hide that the device is an emulator.
