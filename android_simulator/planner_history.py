from __future__ import annotations

from typing import Any


_RELEVANT_EVENTS = {
    "program_action",
    "program_abort",
    "program_truncated",
    "skill_action",
    "skill_abort",
    "skill_rejected",
    "skill_hit",
    "completion_rejected",
    "stale_plan_rejected",
}


def _action(value: Any) -> dict[str, Any] | None:
    if not isinstance(value, dict):
        return None
    result = dict(value)
    if result.get("type") == "type":
        text = result.get("text")
        if isinstance(text, str) and not text.startswith("<redacted:"):
            result["text"] = f"<redacted:{len(text)} chars>"
    # Exact device refs are state-local; retaining old refs only encourages stale reuse.
    result.pop("ref", None)
    return result


def compact_planner_history(history: list[dict[str, Any]], *, max_rows: int = 10) -> list[dict[str, Any]]:
    """Return only information that can affect the next decision.

    Full history remains in run/eval evidence. The planner receives a bounded projection without
    latency bookkeeping, old state hashes, revisions, transition internals, or state-local refs.
    """
    result: list[dict[str, Any]] = []
    for item in reversed(history):
        if len(result) >= max_rows:
            break
        event = item.get("event")
        action = _action(item.get("action"))

        if action is not None:
            row: dict[str, Any] = {
                "event": str(event or "action"),
                "action": action,
                "ok": bool(item.get("ok", True)),
            }
            if item.get("detail"):
                row["detail"] = str(item["detail"])[:240]
            result.append(row)
            continue

        if event not in _RELEVANT_EVENTS:
            continue
        row = {"event": event}
        for key in ("reason", "program_index", "skill_id"):
            if key in item:
                row[key] = item[key]
        checks = item.get("checks")
        if isinstance(checks, dict):
            # Only booleans are decision-relevant; omit state/evidence payload duplication.
            row["checks"] = {str(key): bool(value) for key, value in checks.items()}
        result.append(row)

    result.reverse()
    return result
