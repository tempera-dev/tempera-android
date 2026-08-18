from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass, field
from typing import Any

from .computer_use import DeviceController, Observation


def _sha256_json(value: Any) -> str:
    body = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    return hashlib.sha256(body).hexdigest()


@dataclass(frozen=True)
class SuccessSpec:
    package: str | None = None
    text_present: tuple[str, ...] = ()
    text_absent: tuple[str, ...] = ()

    def evaluate(self, observation: Observation) -> tuple[bool, dict[str, bool]]:
        labels = "\n".join(node.label for node in observation.nodes).casefold()
        checks: dict[str, bool] = {}
        if self.package:
            checks["package"] = observation.package == self.package
        for text in self.text_present:
            checks[f"present:{text}"] = text.casefold() in labels
        for text in self.text_absent:
            checks[f"absent:{text}"] = text.casefold() not in labels
        return (all(checks.values()) if checks else False), checks


@dataclass
class AndroidGymEnv:
    controller: DeviceController
    success: SuccessSpec
    environment_id: str = "android-computer-use"
    environment_version: str = "0.1.0"
    environment_digest: str = field(default_factory=lambda: hashlib.sha256(b"android-computer-use/v0.1.0").hexdigest())
    seed: int = 0
    steps: list[dict[str, Any]] = field(default_factory=list)

    def reset(self, *, home: bool = True, launch_package: str | None = None) -> dict[str, Any]:
        self.steps.clear()
        if home:
            self.controller.act({"type": "home"})
        if launch_package:
            self.controller.act({"type": "launch", "package": launch_package})
        return self.controller.observe().compact()

    def step(self, action: dict[str, Any]) -> tuple[dict[str, Any], float, bool, bool, dict[str, Any]]:
        before = self.controller.observe()
        result = self.controller.act(action, before)
        after = self.controller.observe()
        success, checks = self.success.evaluate(after)
        reward = 1.0 if success else -0.001
        info = {
            "checks": checks,
            "action_latency_ms": round(result.latency_ms, 3),
            "observation_latency_ms": round(after.latency_ms, 3),
        }
        self.steps.append({
            "observation": before.compact(),
            "action": action,
            "reward": reward,
            "terminated": success,
            "truncated": False,
            "info": info,
            "state_digest": hashlib.sha256(before.state_hash.encode()).hexdigest(),
        })
        return after.compact(), reward, success, False, info

    def trajectory_v1(self, *, metadata: dict[str, Any] | None = None) -> dict[str, Any]:
        metadata = dict(metadata or {})
        hashed_metadata = {k: v for k, v in metadata.items() if k != "timing"}
        hash_body = {
            "environment_id": self.environment_id,
            "environment_version": self.environment_version,
            "environment_digest": self.environment_digest,
            "seed": self.seed,
            "steps": self.steps,
            "metadata": hashed_metadata,
        }
        return {
            "schema_version": "trajectory-v1",
            "environment_id": self.environment_id,
            "environment_version": self.environment_version,
            "environment_digest": self.environment_digest,
            "seed": self.seed,
            "steps": self.steps,
            "metadata": metadata,
            "content_hash": _sha256_json(hash_body),
        }
