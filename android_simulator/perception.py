from __future__ import annotations

import re
from typing import Any

from .computer_use import Observation, UINode


_TOKEN = re.compile(r"[a-z0-9][a-z0-9_.-]+")
_STOP = {
    "the", "and", "then", "that", "this", "with", "from", "into", "open", "please",
    "android", "app", "screen", "phone", "click", "tap", "go", "to", "a", "an", "of",
}


def task_tokens(task: str) -> tuple[str, ...]:
    values = []
    seen = set()
    for token in _TOKEN.findall(task.casefold()):
        if token in _STOP or len(token) < 2 or token in seen:
            continue
        seen.add(token)
        values.append(token)
    return tuple(values[:32])


def node_relevance(node: UINode, tokens: tuple[str, ...]) -> float:
    haystack = " ".join((node.label, node.text, node.content_desc, node.resource_id, node.class_name)).casefold()
    score = 0.0
    if node.editable:
        score += 9.0
    if node.clickable:
        score += 7.0
    if node.input_focused:
        score += 6.0
    if node.scrollable:
        score += 3.0
    if node.long_clickable:
        score += 1.5
    if node.label:
        score += 2.0
    if node.selected or node.checked:
        score += 1.0
    if not node.enabled:
        score -= 5.0
    for token in tokens:
        if token in haystack:
            score += 11.0
        if node.label and token == node.label.casefold():
            score += 10.0
    return score


def compact_for_task(
    observation: Observation,
    task: str,
    *,
    max_nodes: int = 72,
) -> dict[str, Any]:
    max_nodes = max(16, min(max_nodes, 240))
    tokens = task_tokens(task)
    ranked = sorted(
        observation.nodes,
        key=lambda node: (
            -node_relevance(node, tokens),
            not bool(node.label),
            node.bounds.area,
            node.ref,
        ),
    )
    selected = ranked[:max_nodes]
    value: dict[str, Any] = {
        "serial": observation.serial,
        "package": observation.package,
        "activity": observation.activity,
        "screen": [observation.width, observation.height],
        "state_hash": observation.state_hash,
        "latency_ms": round(observation.latency_ms, 1),
        "perception": "task_ranked",
        "task_tokens": list(tokens),
        "nodes": [node.compact() for node in selected],
        "omitted_nodes": max(0, len(observation.nodes) - len(selected)),
    }
    if observation.revision:
        value["revision"] = observation.revision
    return value


def semantic_diff(before: Observation, after: Observation, *, max_changes: int = 96) -> dict[str, Any]:
    before_nodes = {node.ref: node.compact() for node in before.nodes}
    after_nodes = {node.ref: node.compact() for node in after.nodes}
    added = [value for ref, value in after_nodes.items() if ref not in before_nodes]
    removed = [value for ref, value in before_nodes.items() if ref not in after_nodes]
    changed = [
        {"before": before_nodes[ref], "after": after_nodes[ref]}
        for ref in before_nodes.keys() & after_nodes.keys()
        if before_nodes[ref] != after_nodes[ref]
    ]
    return {
        "from_state": before.state_hash,
        "to_state": after.state_hash,
        "package_changed": before.package != after.package,
        "activity_changed": before.activity != after.activity,
        "added": added[:max_changes],
        "removed": removed[:max_changes],
        "changed": changed[:max_changes],
        "truncated": len(added) + len(removed) + len(changed) > max_changes,
    }
