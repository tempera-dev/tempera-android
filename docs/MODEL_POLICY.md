# Model policy

Tempera Android treats models as replaceable planners, not execution backends. The Android engine remains authoritative for observation, revision/state-hash checks, approval gates, secret resolution, action execution, receipts, and verification.

## Tiers

The policy layer defines three independent tiers:

- `fast`: default semantic planner for short, obvious UI tasks.
- `reasoning`: stronger semantic planner for consequential, recovery, or long-horizon tasks.
- `vision`: bounded multimodal fallback. Vision does not replace semantic observation and remains opt-in through the runner's existing screenshot escalation path.

The router is deterministic and side-effect free. Selecting a stronger model never grants permission to perform a consequential action.

## Environment

```text
TEMPERA_ANDROID_FAST_MODEL
TEMPERA_ANDROID_FAST_ENDPOINT
TEMPERA_ANDROID_REASONING_MODEL
TEMPERA_ANDROID_REASONING_ENDPOINT
TEMPERA_ANDROID_VISION_MODEL
TEMPERA_ANDROID_VISION_ENDPOINT
TEMPERA_ANDROID_MODEL_LOCAL_ONLY
```

Tier-specific endpoints fall back to `TEMPERA_ANDROID_ENDPOINT`, then to the local OpenAI-compatible default at `http://127.0.0.1:11434/v1/chat/completions`.

`TEMPERA_ANDROID_MODEL_LOCAL_ONLY=true` rejects non-loopback semantic planners. Vision remains separately opt-in because screenshots have a different privacy boundary from semantic state.

API keys are not part of model targets and must never be embedded in endpoint URLs. Existing request authentication remains resolved outside the policy object.

## Routing rules

Explicit model selection always wins, subject to endpoint validation and local-only policy. Otherwise:

1. Consequential workflows prefer `reasoning` when configured.
2. Recovery/error workflows prefer `reasoning` when configured.
3. Long-horizon and multi-step workflows prefer `reasoning` when configured.
4. Ordinary semantic actions use `fast`.
5. If only one semantic planner exists, it is used as the bounded fallback.

These rules select only the planner. All proposed Android actions still flow through the existing typed `ActionV1` conversion, revision binding, sensitive-action approval gate, and canonical command executor.

## Intended runner integration

The next integration step is to resolve a `RouteDecision` at `run` startup when no explicit `--model` is provided, record that decision in run evidence, and preserve the current `--model/--endpoint` behavior as the explicit override path.

Dynamic escalation during a run should be added only with observable criteria (for example repeated invalid planner output or verified recovery state), a bounded escalation budget, and no direct model access to Android transports.
