from __future__ import annotations

from typing import Any

from .computer_use import Observation


def _norm(value: str) -> str:
    return " ".join(value.casefold().split())


def validate_completion_evidence(
    evidence: Any,
    observation: Observation,
) -> tuple[bool, dict[str, bool], str]:
    """Validate planner completion evidence against the exact current observation.

    This is intentionally not a task-specific success grader. It only establishes that a
    planner's termination claim is grounded in the state it actually saw. Deterministic eval
    graders remain authoritative for measured capability.
    """
    if not isinstance(evidence, dict):
        return False, {}, "evidence_object_required"

    allowed = {"package", "activity", "refs", "exact"}
    if set(evidence) - allowed:
        return False, {}, "unknown_evidence_field"

    checks: dict[str, bool] = {}
    semantic_anchors = 0

    if "package" in evidence:
        package = evidence.get("package")
        if not isinstance(package, str) or not package:
            return False, {}, "invalid_package_evidence"
        checks[f"package:{package}"] = observation.package == package

    if "activity" in evidence:
        activity = evidence.get("activity")
        if not isinstance(activity, str) or not activity:
            return False, {}, "invalid_activity_evidence"
        checks[f"activity:{activity}"] = (
            observation.activity == activity or observation.activity.endswith(activity)
        )

    refs = evidence.get("refs", [])
    if isinstance(refs, str):
        refs = [refs]
    if not isinstance(refs, list) or not all(isinstance(item, str) and item for item in refs):
        return False, {}, "invalid_ref_evidence"
    known_refs = {node.ref for node in observation.nodes}
    for ref in refs:
        semantic_anchors += 1
        checks[f"ref:{ref}"] = ref in known_refs

    exact = evidence.get("exact", [])
    if isinstance(exact, str):
        exact = [exact]
    if not isinstance(exact, list) or not all(isinstance(item, str) and item.strip() for item in exact):
        return False, {}, "invalid_exact_evidence"
    visible = {
        _norm(value)
        for node in observation.nodes
        for value in (
            node.label,
            node.text,
            node.content_desc,
            node.resource_id.rsplit("/", 1)[-1] if node.resource_id else "",
        )
        if value
    }
    for value in exact:
        semantic_anchors += 1
        checks[f"exact:{value}"] = _norm(value) in visible

    if semantic_anchors == 0:
        return False, checks, "semantic_anchor_required"
    if not checks or not all(checks.values()):
        return False, checks, "evidence_mismatch"
    return True, checks, "ok"


def completion_contract() -> dict[str, Any]:
    return {
        "required_when_done": True,
        "schema": {
            "package": "optional exact package",
            "activity": "optional exact or suffix activity",
            "refs": ["zero or more refs from the current observation"],
            "exact": ["zero or more exact visible labels/text/id suffixes"],
        },
        "rule": "At least one refs/exact semantic anchor is required and every supplied check must match the current state.",
    }
