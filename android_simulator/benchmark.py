from __future__ import annotations

import json
import statistics
import time
from typing import Any

from .computer_use import DeviceController


def _stats(values: list[float]) -> dict[str, float]:
    ordered = sorted(values)
    if not ordered:
        return {"mean_ms": 0.0, "p50_ms": 0.0, "p95_ms": 0.0, "min_ms": 0.0, "max_ms": 0.0}
    p50 = ordered[(len(ordered) - 1) // 2]
    p95 = ordered[min(len(ordered) - 1, int(round((len(ordered) - 1) * 0.95)))]
    return {
        "mean_ms": round(statistics.fmean(ordered), 3),
        "p50_ms": round(p50, 3),
        "p95_ms": round(p95, 3),
        "min_ms": round(ordered[0], 3),
        "max_ms": round(ordered[-1], 3),
    }


def _measure(
    controller: DeviceController,
    *,
    iterations: int,
    batch_size: int,
    warmup: int,
) -> dict[str, Any]:
    for _ in range(warmup):
        observation = controller.observe()
        controller.act_and_observe([{"type": "wait", "seconds": 0}], observation, timeout_ms=0)

    observations: list[float] = []
    payload_sizes: list[float] = []
    singles: list[float] = []
    batches: list[float] = []
    fused: list[float] = []

    for _ in range(iterations):
        started = time.perf_counter()
        observation = controller.observe()
        observations.append((time.perf_counter() - started) * 1000)
        payload_sizes.append(float(len(json.dumps(observation.compact(), separators=(",", ":")).encode())))

    # Harmless zero-second waits isolate control-plane transaction overhead without changing device state.
    for _ in range(iterations):
        started = time.perf_counter()
        for _ in range(batch_size):
            controller.act({"type": "wait", "seconds": 0})
        singles.append((time.perf_counter() - started) * 1000)

        started = time.perf_counter()
        controller.macro([{"type": "wait", "seconds": 0} for _ in range(batch_size)])
        batches.append((time.perf_counter() - started) * 1000)

        observation = controller.observe()
        started = time.perf_counter()
        controller.act_and_observe(
            [{"type": "wait", "seconds": 0} for _ in range(batch_size)],
            observation,
            timeout_ms=0,
        )
        fused.append((time.perf_counter() - started) * 1000)

    single_mean = statistics.fmean(singles)
    batch_mean = statistics.fmean(batches)
    speedup = single_mean / batch_mean if batch_mean > 0 else 0.0
    return {
        "transport": controller.transport_name,
        "observation": _stats(observations),
        "semantic_payload_bytes": _stats(payload_sizes),
        "sequential_actions": _stats(singles),
        "single_transaction_batch": _stats(batches),
        "fused_act_observe": _stats(fused),
        "effective_batch_speedup_x": round(speedup, 3),
    }


def run_benchmark(
    controller: DeviceController,
    *,
    iterations: int = 20,
    batch_size: int = 8,
    warmup: int = 2,
) -> dict[str, Any]:
    iterations = max(3, min(iterations, 200))
    batch_size = max(2, min(batch_size, 12))
    warmup = max(0, min(warmup, 10))

    primary = _measure(
        controller,
        iterations=iterations,
        batch_size=batch_size,
        warmup=warmup,
    )
    result: dict[str, Any] = {
        "iterations": iterations,
        "batch_size": batch_size,
        "primary": primary,
        "note": "zero-second waits isolate host/device control-plane overhead; run real task evals separately",
    }

    if controller.transport_name != "adb-uiautomator":
        baseline_iterations = min(iterations, 10)
        baseline = DeviceController(controller.toolchain, controller.serial)
        adb_metrics = _measure(
            baseline,
            iterations=baseline_iterations,
            batch_size=batch_size,
            warmup=min(warmup, 1),
        )
        result["adb_baseline"] = adb_metrics
        bridge_observe = primary["observation"]["mean_ms"]
        adb_observe = adb_metrics["observation"]["mean_ms"]
        bridge_fused = primary["fused_act_observe"]["mean_ms"]
        adb_fused = adb_metrics["fused_act_observe"]["mean_ms"]
        result["speedup_vs_adb"] = {
            "observation_x": round(adb_observe / bridge_observe, 3) if bridge_observe else 0.0,
            "fused_act_observe_x": round(adb_fused / bridge_fused, 3) if bridge_fused else 0.0,
        }
    return result
