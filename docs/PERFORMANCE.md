# Performance contract

The project does not use “10x faster” as an unmeasured claim. Performance is decomposed and measured at the boundaries that dominate an Android agent loop.

## Required measurements

On the target Apple Silicon Mac, run:

```bash
tempera-android --serial emulator-5554 bridge setup
tempera-android --serial emulator-5554 bench --iterations 30 --json
```

The benchmark reports:

- semantic observation p50 / p95
- semantic payload bytes
- sequential action control overhead
- one-transaction batch overhead
- fused action + next-observation latency
- bridge-vs-ADB observation speedup
- bridge-vs-ADB fused-cycle speedup

## End-to-end task metrics

Control-plane speed is not enough. A production task suite should also record:

- task success rate
- verified useful state transitions per second
- planner calls per task
- vision escalations per task
- stale-plan rejections per task
- semantic context bytes/tokens per planner call
- action attempts per successful state transition
- p50/p95 end-to-end task latency

The optimization objective is to minimize latency and model work **subject to equal or better task success and safety**. A faster agent that clicks stale coordinates or needs more retries is not an improvement.

## Target architecture

The bridge path is designed so a normal semantic step requires one model call and one persistent host/device transaction:

`ranked semantic state -> planner -> revision-bound action batch -> event wait -> next semantic state`

Full semantic context and screenshots are explicit escalation tiers rather than default costs.
