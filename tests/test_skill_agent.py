from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from android_simulator.agent import AgentConfig, ComputerUseAgent
from android_simulator.computer_use import ActionResult, Observation, Rect, UINode
from android_simulator.skills import SkillStore


def node(ref: str, label: str) -> UINode:
    return UINode(
        ref=ref,
        text=label,
        content_desc="",
        resource_id=f"com.demo:id/{ref}",
        class_name="android.widget.Button",
        package="com.demo",
        bounds=Rect(0, 0, 200, 100),
        clickable=True,
        enabled=True,
        focusable=False,
        scrollable=False,
        selected=False,
        checked=False,
    )


def obs(revision: int, label: str) -> Observation:
    return Observation(
        "emulator-5554",
        "com.demo",
        ".Main",
        1080,
        1920,
        (node(f"r{revision}", label),),
        1.0,
        1.0,
        revision,
    )


class ExplodingPlanner:
    def plan(self, *args, **kwargs):
        raise AssertionError("planner must not be called on a verified skill hit")


class ReplayController:
    transport_name = "fake"

    def __init__(self, start: Observation, final: Observation):
        self.current = start
        self.final = final
        self.last_transition = {}
        self.actions = []

    def observe(self):
        return self.current

    def act_and_observe(self, actions, observation, *, timeout_ms=900):
        self.actions.extend(actions)
        self.current = self.final
        self.last_transition = {
            "changed": True,
            "settled": True,
            "events": self.final.revision - observation.revision,
        }
        return [ActionResult(action, True, 1.0, "ok") for action in actions], self.current


class CachedSkillAgentTests(unittest.TestCase):
    def test_verified_skill_hit_uses_zero_planner_steps(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "skills.json"
            start = obs(1, "Settings")
            final = obs(2, "Done")
            task = "Open Settings"
            program = [{"action": {"type": "tap", "selector": "Settings"}}]
            learned = SkillStore(path).learn(
                task=task,
                start_observation=start,
                program=program,
                final_observation=final,
                completion_evidence={"exact": ["Done"]},
            )
            self.assertIsNotNone(learned)

            controller = ReplayController(start, final)
            config = AgentConfig(
                model="unused",
                use_skill_cache=True,
                skill_cache_path=str(path),
            )
            result = ComputerUseAgent(controller, ExplodingPlanner(), config).run(task)
            self.assertTrue(result.done)
            self.assertEqual(result.steps, 0)
            self.assertEqual(result.actions, 1)
            self.assertEqual(len(controller.actions), 1)
            self.assertTrue(any(item.get("event") == "skill_hit" for item in result.history))
            self.assertFalse(any(item.get("event") == "plan" for item in result.history))


if __name__ == "__main__":
    unittest.main()
