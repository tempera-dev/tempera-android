from __future__ import annotations

import copy
import hashlib
import json
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .completion import validate_completion_evidence
from .computer_use import Observation
from .program import guard_matches
from .util import atomic_write


SCHEMA_VERSION = "android-agent-skills.v1"
MAX_SKILLS = 128
_SAFE_ACTIONS = {"tap", "back", "home", "scroll", "launch", "wait"}
_SENSITIVE = {
    "send", "post", "publish", "buy", "purchase", "pay", "transfer", "confirm purchase",
    "delete", "remove account", "factory reset", "subscribe", "book", "order", "submit",
}


def default_skill_path() -> Path:
    return Path.home() / ".android-sim" / "agent-skills-v1.json"


def task_digest(task: str) -> str:
    normalized = " ".join(task.casefold().split()).encode("utf-8")
    return hashlib.sha256(normalized).hexdigest()


def _contains_sensitive(value: str) -> bool:
    normalized = value.casefold()
    return any(term in normalized for term in _SENSITIVE)


def _portable_completion(evidence: Any, observation: Observation) -> dict[str, Any] | None:
    if not isinstance(evidence, dict):
        return None
    value: dict[str, Any] = {}
    if isinstance(evidence.get("package"), str) and evidence["package"]:
        value["package"] = evidence["package"]
    if isinstance(evidence.get("activity"), str) and evidence["activity"]:
        value["activity"] = evidence["activity"]

    exact: list[str] = []
    raw_exact = evidence.get("exact", [])
    if isinstance(raw_exact, str):
        raw_exact = [raw_exact]
    if isinstance(raw_exact, list):
        exact.extend(str(item) for item in raw_exact if isinstance(item, str) and item.strip())

    raw_refs = evidence.get("refs", [])
    if isinstance(raw_refs, str):
        raw_refs = [raw_refs]
    if isinstance(raw_refs, list):
        by_ref = {node.ref: node.label for node in observation.nodes if node.label}
        for ref in raw_refs:
            if isinstance(ref, str) and ref in by_ref:
                exact.append(by_ref[ref])

    exact = list(dict.fromkeys(exact))
    if not exact:
        return None
    value["exact"] = exact
    valid, _, _ = validate_completion_evidence(value, observation)
    return value if valid else None


def _program_is_cacheable(program: Any) -> bool:
    if not isinstance(program, list) or not program or len(program) > 12:
        return False
    for raw_step in program:
        if not isinstance(raw_step, dict) or set(raw_step) - {"when", "action"}:
            return False
        action = raw_step.get("action")
        if not isinstance(action, dict):
            return False
        kind = str(action.get("type", ""))
        if kind not in _SAFE_ACTIONS:
            return False
        if action.get("ref"):
            return False
        if any(key in action for key in ("x", "y", "x1", "y1", "x2", "y2", "text", "secret_ref")):
            return False
        selector = action.get("selector")
        if selector is not None:
            if not isinstance(selector, str) or not selector.strip() or _contains_sensitive(selector):
                return False
        guard = raw_step.get("when")
        if guard is not None and not isinstance(guard, dict):
            return False
    return True


def _start_guard(program: list[dict[str, Any]], observation: Observation) -> dict[str, Any]:
    first = program[0]
    guard = copy.deepcopy(first.get("when") or {})
    action = first.get("action") or {}
    kind = action.get("type")
    selector = action.get("selector")
    if kind in {"tap", "scroll"} and isinstance(selector, str) and selector:
        contains = guard.get("contains", [])
        if isinstance(contains, str):
            contains = [contains]
        if not isinstance(contains, list):
            contains = []
        if selector not in contains:
            contains.append(selector)
        guard["contains"] = contains
        if "package" not in guard and observation.package:
            guard["package"] = observation.package
    return guard


@dataclass(frozen=True)
class Skill:
    id: str
    task_sha256: str
    start_guard: dict[str, Any]
    program: list[dict[str, Any]]
    completion_evidence: dict[str, Any]
    learned_at: int
    successes: int = 1
    failures: int = 0

    def as_json(self) -> dict[str, Any]:
        return {
            "id": self.id,
            "task_sha256": self.task_sha256,
            "start_guard": self.start_guard,
            "program": self.program,
            "completion_evidence": self.completion_evidence,
            "learned_at": self.learned_at,
            "successes": self.successes,
            "failures": self.failures,
        }


class SkillStore:
    def __init__(self, path: Path | None = None):
        self.path = (path or default_skill_path()).expanduser()

    def _load(self) -> list[dict[str, Any]]:
        if not self.path.is_file():
            return []
        try:
            value = json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return []
        if not isinstance(value, dict) or value.get("schema_version") != SCHEMA_VERSION:
            return []
        entries = value.get("skills")
        return [item for item in entries if isinstance(item, dict)] if isinstance(entries, list) else []

    def _save(self, entries: list[dict[str, Any]]) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        atomic_write(
            self.path,
            json.dumps({"schema_version": SCHEMA_VERSION, "skills": entries[-MAX_SKILLS:]}, indent=2, sort_keys=True) + "\n",
            mode=0o600,
        )

    def candidates(self, task: str, observation: Observation) -> list[Skill]:
        digest = task_digest(task)
        result: list[Skill] = []
        for raw in self._load():
            if raw.get("task_sha256") != digest:
                continue
            guard = raw.get("start_guard")
            program = raw.get("program")
            evidence = raw.get("completion_evidence")
            if not isinstance(guard, dict) or not _program_is_cacheable(program) or not isinstance(evidence, dict):
                continue
            if not guard_matches(observation, guard):
                continue
            try:
                result.append(Skill(
                    id=str(raw["id"]),
                    task_sha256=digest,
                    start_guard=guard,
                    program=copy.deepcopy(program),
                    completion_evidence=copy.deepcopy(evidence),
                    learned_at=int(raw.get("learned_at", 0)),
                    successes=max(0, int(raw.get("successes", 0))),
                    failures=max(0, int(raw.get("failures", 0))),
                ))
            except (KeyError, TypeError, ValueError):
                continue
        return sorted(result, key=lambda item: (item.successes - item.failures, item.learned_at), reverse=True)

    def learn(
        self,
        *,
        task: str,
        start_observation: Observation,
        program: list[dict[str, Any]],
        final_observation: Observation,
        completion_evidence: Any,
    ) -> Skill | None:
        if not _program_is_cacheable(program):
            return None
        portable = _portable_completion(completion_evidence, final_observation)
        if portable is None:
            return None
        digest = task_digest(task)
        guard = _start_guard(program, start_observation)
        if not guard_matches(start_observation, guard):
            return None
        identity_body = {
            "task_sha256": digest,
            "start_guard": guard,
            "program": program,
            "completion_evidence": portable,
        }
        skill_id = hashlib.sha256(
            json.dumps(identity_body, sort_keys=True, separators=(",", ":")).encode("utf-8")
        ).hexdigest()[:24]
        entries = self._load()
        for item in entries:
            if item.get("id") == skill_id:
                item["successes"] = max(0, int(item.get("successes", 0))) + 1
                item["failures"] = max(0, int(item.get("failures", 0)))
                item["learned_at"] = int(time.time())
                self._save(entries)
                return Skill(
                    skill_id,
                    digest,
                    guard,
                    copy.deepcopy(program),
                    portable,
                    int(item["learned_at"]),
                    int(item["successes"]),
                    int(item["failures"]),
                )
        skill = Skill(
            id=skill_id,
            task_sha256=digest,
            start_guard=guard,
            program=copy.deepcopy(program),
            completion_evidence=portable,
            learned_at=int(time.time()),
        )
        entries.append(skill.as_json())
        self._save(entries)
        return skill

    def record_failure(self, skill_id: str) -> None:
        entries = self._load()
        changed = False
        for item in entries:
            if item.get("id") == skill_id:
                item["failures"] = max(0, int(item.get("failures", 0))) + 1
                changed = True
                # Repeatedly failing skills are removed rather than becoming permanent retry noise.
                if int(item["failures"]) >= max(3, int(item.get("successes", 0)) + 2):
                    item["disabled"] = True
        if changed:
            entries = [item for item in entries if not item.get("disabled")]
            self._save(entries)
