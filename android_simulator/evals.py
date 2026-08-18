from __future__ import annotations

import hashlib
import json
import statistics
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Any, Iterable

from . import adb as adb_module
from .agent import AgentConfig, ComputerUseAgent, PlannerClient
from .bridge import PACKAGE
from .computer_use import DeviceController, Observation


FIXTURE_COMPONENT = f"{PACKAGE}/.EvalFixtureActivity"


def _canonical(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def _sha(value: Any) -> str:
    return "sha256:" + hashlib.sha256(_canonical(value)).hexdigest()


@dataclass(frozen=True)
class EvalVerifier:
    package: str | None = None
    exact_present: tuple[str, ...] = ()
    exact_absent: tuple[str, ...] = ()

    def evaluate(self, observation: Observation) -> tuple[bool, dict[str, bool]]:
        labels = {" ".join(node.label.casefold().split()) for node in observation.nodes if node.label}
        checks: dict[str, bool] = {}
        if self.package:
            checks["package"] = observation.package == self.package
        for value in self.exact_present:
            checks[f"present:{value}"] = " ".join(value.casefold().split()) in labels
        for value in self.exact_absent:
            checks[f"absent:{value}"] = " ".join(value.casefold().split()) not in labels
        return (bool(checks) and all(checks.values())), checks


@dataclass(frozen=True)
class EvalCase:
    id: str
    population: str
    task: str
    setup: str
    verifier: EvalVerifier
    max_steps: int = 20


SYNTHETIC_CASES: tuple[EvalCase, ...] = (
    EvalCase(
        id="fixture.wifi-multiscreen",
        population="synthetic_fixture",
        task="In Android Agent Eval, open Wi-Fi lab, open Advanced, then finish the Wi-Fi lab.",
        setup="fixture",
        verifier=EvalVerifier(
            package=PACKAGE,
            exact_present=("Wi-Fi lab complete", "Fixture state: complete"),
        ),
    ),
    EvalCase(
        id="fixture.profile-text-entry",
        population="synthetic_fixture",
        task="In Android Agent Eval, open Profile lab. Enter Ada in Display name, enter hello in Short note, then save the profile.",
        setup="fixture",
        verifier=EvalVerifier(
            package=PACKAGE,
            exact_present=("Profile saved", "Lengths: name=3, note=5"),
        ),
    ),
    EvalCase(
        id="fixture.dialog-multiwindow",
        population="synthetic_fixture",
        task="In Android Agent Eval, open Dialog lab, open the permission dialog, then choose Allow once.",
        setup="fixture",
        verifier=EvalVerifier(
            package=PACKAGE,
            exact_present=("Dialog accepted", "Fixture state: complete"),
        ),
    ),
    EvalCase(
        id="fixture.long-scroll",
        population="synthetic_fixture",
        task="In Android Agent Eval, open Long list, find Target 40, tap it, and finish the task.",
        setup="fixture",
        verifier=EvalVerifier(
            package=PACKAGE,
            exact_present=("Long list complete", "Fixture state: complete"),
        ),
        max_steps=28,
    ),
)


SETTINGS_CASES: tuple[EvalCase, ...] = (
    EvalCase(
        id="android.open-settings",
        population="android_settings",
        task="Open Android Settings.",
        setup="home",
        verifier=EvalVerifier(package="com.android.settings"),
        max_steps=12,
    ),
    EvalCase(
        id="android.network-internet",
        population="android_settings",
        task="Open Network & internet in Android Settings.",
        setup="settings",
        verifier=EvalVerifier(
            package="com.android.settings",
            exact_present=("Internet",),
        ),
        max_steps=16,
    ),
    EvalCase(
        id="android.about-phone",
        population="android_settings",
        task="Open About phone in Android Settings.",
        setup="settings",
        verifier=EvalVerifier(
            package="com.android.settings",
            exact_present=("Android version",),
        ),
        max_steps=20,
    ),
)


def _shell(controller: DeviceController, args: list[str]) -> None:
    adb_module.shell(controller.toolchain, controller.serial, args, check=True, quiet=True)


def _setup_case(controller: DeviceController, case: EvalCase) -> None:
    if case.setup == "fixture":
        _shell(controller, ["am", "force-stop", PACKAGE])
        _shell(controller, ["am", "start", "-W", "-n", FIXTURE_COMPONENT])
    elif case.setup == "settings":
        _shell(controller, ["am", "force-stop", "com.android.settings"])
        _shell(controller, ["am", "start", "-W", "-a", "android.settings.SETTINGS"])
    elif case.setup == "home":
        controller.act({"type": "home"})
    else:
        raise ValueError(f"unknown eval setup: {case.setup}")
    time.sleep(0.12)
    controller.observe()


def _count(history: list[dict[str, Any]], event: str) -> int:
    return sum(item.get("event") == event for item in history)


def _action_rows(history: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [item for item in history if isinstance(item.get("action"), dict)]


def _case_metrics(history: list[dict[str, Any]], wall_ms: float) -> dict[str, Any]:
    plans = [item for item in history if item.get("event") == "plan"]
    actions = _action_rows(history)
    planner_latencies = [
        float(item["planner_latency_ms"])
        for item in plans
        if isinstance(item.get("planner_latency_ms"), (int, float))
    ]
    return {
        "wall_ms": round(wall_ms, 3),
        "model_calls": len(plans),
        "planner_ms": round(sum(planner_latencies), 3),
        "actions": len(actions),
        "program_actions": _count(history, "program_action"),
        "program_aborts": _count(history, "program_abort"),
        "skill_attempts": _count(history, "skill_attempt"),
        "skill_actions": _count(history, "skill_action"),
        "skill_hits": _count(history, "skill_hit"),
        "skill_rejections": _count(history, "skill_rejected"),
        "skills_learned": _count(history, "skill_learned"),
        "completion_rejects": _count(history, "completion_rejected"),
        "stale_rejects": _count(history, "stale_plan_rejected"),
        "vision_calls": sum(item.get("perception") == "vision" for item in plans),
    }


def _mean(rows: list[dict[str, Any]], key: str) -> float:
    values = [float(row["metrics"][key]) for row in rows if isinstance(row.get("metrics", {}).get(key), (int, float))]
    return round(statistics.fmean(values), 3) if values else 0.0


def _aggregate(rows: list[dict[str, Any]]) -> dict[str, Any]:
    populations = sorted({str(row["population"]) for row in rows})
    by_population: dict[str, Any] = {}
    for population in populations:
        subset = [row for row in rows if row["population"] == population]
        successes = sum(bool(row["success"]) for row in subset)
        by_population[population] = {
            "cases": len(subset),
            "successes": successes,
            "success_rate": round(successes / len(subset), 4) if subset else 0.0,
            "mean_wall_ms": _mean(subset, "wall_ms"),
            "mean_model_calls": _mean(subset, "model_calls"),
            "mean_actions": _mean(subset, "actions"),
            "mean_program_actions": _mean(subset, "program_actions"),
            "mean_skill_actions": _mean(subset, "skill_actions"),
            "skill_hits": int(sum(row["metrics"]["skill_hits"] for row in subset)),
            "vision_calls": int(sum(row["metrics"]["vision_calls"] for row in subset)),
            "stale_rejects": int(sum(row["metrics"]["stale_rejects"] for row in subset)),
            "completion_rejects": int(sum(row["metrics"]["completion_rejects"] for row in subset)),
        }
    successes = sum(bool(row["success"]) for row in rows)
    return {
        "cases": len(rows),
        "successes": successes,
        "success_rate": round(successes / len(rows), 4) if rows else 0.0,
        "mean_wall_ms": _mean(rows, "wall_ms"),
        "mean_model_calls": _mean(rows, "model_calls"),
        "mean_actions": _mean(rows, "actions"),
        "skill_hits": int(sum(row["metrics"]["skill_hits"] for row in rows)),
        "vision_calls": int(sum(row["metrics"]["vision_calls"] for row in rows)),
        "by_population": by_population,
    }


def builtin_cases(*, include_settings: bool = True) -> tuple[EvalCase, ...]:
    return SYNTHETIC_CASES + (SETTINGS_CASES if include_settings else ())


def run_eval_suite(
    controller: DeviceController,
    planner: PlannerClient,
    base_config: AgentConfig,
    *,
    cases: Iterable[EvalCase] | None = None,
    include_settings: bool = True,
) -> dict[str, Any]:
    selected = tuple(cases or builtin_cases(include_settings=include_settings))
    results: list[dict[str, Any]] = []
    started_at = datetime.now(timezone.utc).isoformat()

    for case in selected:
        _setup_case(controller, case)
        config = AgentConfig(**{
            **base_config.__dict__,
            "max_steps": min(base_config.max_steps, case.max_steps),
        })
        agent = ComputerUseAgent(controller, planner, config)
        started = time.perf_counter()
        run_error = ""
        agent_done = False
        summary = ""
        history: list[dict[str, Any]] = []
        try:
            run = agent.run(case.task)
            agent_done = run.done
            summary = run.summary
            history = run.history
        except Exception as exc:
            run_error = f"{type(exc).__name__}: {exc}"
        wall_ms = (time.perf_counter() - started) * 1000

        final_observation = controller.observe()
        success, checks = case.verifier.evaluate(final_observation)
        results.append({
            "case_id": case.id,
            "population": case.population,
            "task": case.task,
            "success": success,
            "grader": {
                "type": "deterministic_android_end_state",
                "checks": checks,
                "final_state_hash": final_observation.state_hash,
                "final_revision": final_observation.revision,
                "final_package": final_observation.package,
            },
            "agent_reported_done": agent_done,
            "agent_summary": summary,
            "error": run_error or None,
            "metrics": _case_metrics(history, wall_ms),
            "history": history,
        })

    finished_at = datetime.now(timezone.utc).isoformat()
    populations = sorted({case.population for case in selected})
    case_ids = [case.id for case in selected]
    report: dict[str, Any] = {
        "schema_version": "android-agent-eval.v1",
        "suite": {
            "id": "android.computer-use.local-v1",
            "case_count": len(results),
            "case_ids_sha256": _sha(case_ids),
            "populations": populations,
        },
        "started_at": started_at,
        "finished_at": finished_at,
        "transport": controller.transport_name,
        "model": base_config.model,
        "vision_model": base_config.vision_model or base_config.model,
        "agent_config": {
            "max_steps": base_config.max_steps,
            "max_actions_per_step": base_config.max_actions_per_step,
            "max_program_steps": base_config.max_program_steps,
            "task_context_nodes": base_config.task_context_nodes,
            "full_context_nodes": base_config.full_context_nodes,
            "settle_timeout_ms": base_config.settle_timeout_ms,
            "use_vision": base_config.use_vision,
            "require_completion_evidence": base_config.require_completion_evidence,
            "use_skill_cache": base_config.use_skill_cache,
        },
        "result": {
            "aggregate": _aggregate(results),
            "cases": results,
        },
        "claim_scope": "local regression and engineering evidence only; not an official capability claim",
        "limitations": [
            "Synthetic fixture cases prove control-plane behavior, not arbitrary third-party app capability.",
            "Android Settings labels may vary by Android release, OEM image, and locale; run the suite on the target AVD image.",
            "The model's done flag is recorded but never used as the success grader.",
            "Skill-cache results must be reported separately from cold planner results when making latency comparisons.",
            "No 10x performance claim is valid without equal-or-better deterministic success on the target M2 task population.",
        ],
    }
    report["report_sha256"] = _sha(report)
    return report


def dumps_eval_report(report: dict[str, Any]) -> str:
    return json.dumps(report, indent=2, sort_keys=True, ensure_ascii=False)
