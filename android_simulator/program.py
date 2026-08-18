from __future__ import annotations

from typing import Any

from .computer_use import Observation, UINode


_TARGETED_TYPES = {"tap", "long_press", "type", "enter", "scroll"}
_GLOBAL_TYPES = {"back", "home", "recents", "notifications", "launch", "wait"}


def _norm(value: str) -> str:
    return " ".join(value.casefold().split())


def _node_values(node: UINode) -> tuple[str, ...]:
    resource_suffix = node.resource_id.rsplit("/", 1)[-1] if node.resource_id else ""
    return tuple(
        value
        for value in (
            node.label,
            node.text,
            node.content_desc,
            resource_suffix,
            node.resource_id,
        )
        if value
    )


def exact_matches(observation: Observation, selector: str) -> list[UINode]:
    needle = _norm(selector)
    if not needle:
        return []
    result: list[UINode] = []
    for node in observation.nodes:
        if not node.enabled:
            continue
        if any(_norm(value) == needle for value in _node_values(node)):
            result.append(node)
    return result


def unique_exact_node(observation: Observation, selector: str) -> UINode | None:
    matches = exact_matches(observation, selector)
    if len(matches) != 1:
        return None
    return matches[0]


def _visible_exact(observation: Observation, value: str) -> bool:
    return bool(exact_matches(observation, value))


def guard_matches(observation: Observation, guard: Any) -> bool:
    if guard is None:
        return True
    if not isinstance(guard, dict):
        return False

    package = guard.get("package")
    if package is not None and str(package) != observation.package:
        return False
    activity = guard.get("activity")
    if activity is not None:
        wanted = str(activity)
        if observation.activity != wanted and not observation.activity.endswith(wanted):
            return False

    contains = guard.get("contains", [])
    if isinstance(contains, str):
        contains = [contains]
    if not isinstance(contains, list) or not all(isinstance(item, str) for item in contains):
        return False
    if any(not _visible_exact(observation, item) for item in contains):
        return False

    absent = guard.get("absent", [])
    if isinstance(absent, str):
        absent = [absent]
    if not isinstance(absent, list) or not all(isinstance(item, str) for item in absent):
        return False
    if any(_visible_exact(observation, item) for item in absent):
        return False
    return True


def ground_program_action(action: Any, observation: Observation) -> tuple[dict[str, Any] | None, str]:
    """Ground one future-state action without guessing.

    Guarded programs deliberately reject refs and coordinates. Targeted actions require one
    exact semantic match in the *fresh* observation. This trades a small amount of coverage
    for the ability to run several screens from one model plan without speculative clicks.
    """
    if not isinstance(action, dict):
        return None, "action_not_object"
    normalized = dict(action)
    kind = str(normalized.get("type", ""))
    if kind not in _TARGETED_TYPES | _GLOBAL_TYPES:
        return None, f"unsupported_program_action:{kind}"
    if normalized.get("ref"):
        return None, "future_refs_forbidden"
    if any(key in normalized for key in ("x", "y", "x1", "y1", "x2", "y2")):
        return None, "future_coordinates_forbidden"

    if kind in _TARGETED_TYPES:
        selector = normalized.get("selector")
        if not isinstance(selector, str) or not selector.strip():
            return None, "exact_selector_required"
        node = unique_exact_node(observation, selector)
        if node is None:
            matches = len(exact_matches(observation, selector))
            return None, "selector_missing" if matches == 0 else "selector_ambiguous"
        if kind == "type" and not node.editable:
            return None, "selector_not_editable"
        if kind == "scroll" and not node.scrollable:
            return None, "selector_not_scrollable"
        normalized["ref"] = node.ref
        normalized.pop("selector", None)

    return normalized, "ok"


def program_contract(max_steps: int = 6) -> dict[str, Any]:
    return {
        "max_steps": max_steps,
        "step": {
            "when": {
                "package": "optional exact package",
                "activity": "optional exact/suffix activity",
                "contains": ["optional exact visible labels"],
                "absent": ["optional exact labels that must be absent"],
            },
            "action": "one action; future targeted actions use exact selector strings only",
        },
        "rules": [
            "never use refs in program steps",
            "never use coordinates in program steps",
            "target selectors must resolve to exactly one enabled node in each fresh settled state",
            "execution stops and replans on guard failure, ambiguity, stale state, failed action, or no state change",
        ],
    }
