from __future__ import annotations

import hashlib
import json
import re
from typing import Any


_DIGEST = re.compile(r"^sha256:[a-f0-9]{64}$")
_SAFE_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$")


def _canonical(value: Any) -> bytes:
    # Matches tempera-evals models.canonical_json at the pinned contract revision.
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def _sha(value: Any) -> str:
    return "sha256:" + hashlib.sha256(_canonical(value)).hexdigest()


def _digest(value: str, name: str) -> str:
    if not isinstance(value, str) or _DIGEST.fullmatch(value) is None:
        raise ValueError(f"{name} must be sha256:<64 lowercase hex chars>")
    return value


def _safe_id(value: str, name: str) -> str:
    if not isinstance(value, str) or _SAFE_ID.fullmatch(value) is None:
        raise ValueError(f"{name} must be a Tempera-safe identifier")
    return value


def _metric(value: float, count: int, errors: int) -> dict[str, Any]:
    return {
        "value": float(value),
        "sample_count": int(count),
        "missing_count": 0,
        "error_count": int(errors),
    }


def external_adapter_result(
    local_report: dict[str, Any],
    *,
    run_id: str,
    plan_sha256: str,
    adapter_descriptor_sha256: str,
) -> dict[str, Any]:
    """Normalize a completed local report to the pinned Tempera Evals result schema.

    This deliberately requires externally produced plan/descriptor digests. It does not create,
    sign, attest, or mark an eval run official-eligible; those governance steps remain owned by
    tempera-evals and its execution environment.
    """
    _safe_id(run_id, "run_id")
    _digest(plan_sha256, "plan_sha256")
    _digest(adapter_descriptor_sha256, "adapter_descriptor_sha256")

    cases = local_report.get("result", {}).get("cases", [])
    if not isinstance(cases, list) or not cases:
        raise ValueError("local Android eval report has no cases")
    case_ids = [str(case.get("case_id", "")) for case in cases]
    if any(not case_id for case_id in case_ids) or len(case_ids) != len(set(case_ids)):
        raise ValueError("local Android eval case IDs must be non-empty and unique")

    errors = sum(bool(case.get("error")) for case in cases)
    successes = sum(bool(case.get("success")) for case in cases)
    count = len(cases)
    aggregate = local_report.get("result", {}).get("aggregate", {})

    subject_output = [
        {
            "case_id": case.get("case_id"),
            "agent_reported_done": case.get("agent_reported_done"),
            "agent_summary": case.get("agent_summary"),
            "error": case.get("error"),
            "history": case.get("history", []),
        }
        for case in cases
    ]
    scorer_output = [
        {
            "case_id": case.get("case_id"),
            "success": bool(case.get("success")),
            "grader": case.get("grader", {}),
        }
        for case in cases
    ]

    metrics: dict[str, Any] = {
        "success_rate": _metric(successes / count, count, errors),
        "mean_wall_ms": _metric(float(aggregate.get("mean_wall_ms", 0.0)), count, errors),
        "mean_model_calls": _metric(float(aggregate.get("mean_model_calls", 0.0)), count, errors),
        "mean_actions": _metric(float(aggregate.get("mean_actions", 0.0)), count, errors),
    }
    populations = aggregate.get("by_population", {})
    if isinstance(populations, dict):
        for name, row in sorted(populations.items()):
            if not isinstance(row, dict):
                continue
            safe_name = re.sub(r"[^A-Za-z0-9._:-]+", "_", str(name))[:64]
            metrics[f"success_rate.{safe_name}"] = _metric(
                float(row.get("success_rate", 0.0)),
                int(row.get("cases", 0)),
                0,
            )

    result: dict[str, Any] = {
        "schema_version": "tempera.external-adapter-result.v1",
        "run_id": run_id,
        "plan_sha256": plan_sha256,
        "adapter_descriptor_sha256": adapter_descriptor_sha256,
        "started_at": str(local_report.get("started_at") or local_report.get("created_at")),
        "finished_at": str(local_report.get("finished_at") or local_report.get("created_at")),
        "population": {
            "case_ids_sha256": _sha(case_ids),
            "case_count": count,
            "completed_count": count,
            "missing_count": 0,
            "error_count": errors,
        },
        "metrics": metrics,
        "artifacts": [],
        "subject_output_sha256": _sha(subject_output),
        "scorer_output_sha256": _sha(scorer_output),
    }
    result["result_sha256"] = _sha(result)
    return result
