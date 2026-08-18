from __future__ import annotations

import unittest
from pathlib import Path
from tempfile import NamedTemporaryFile

from android_simulator.agent import AgentConfig, ComputerUseAgent
from android_simulator.computer_use import ActionResult, Observation, Rect, StaleStateError, UINode


def node(ref: str, label: str, *, editable: bool = False) -> UINode:
    return UINode(
        ref=ref,
        text=label,
        content_desc="",
        resource_id=f"com.demo:id/{ref}",
        class_name="android.widget.EditText" if editable else "android.widget.Button",
        package="com.demo",
        bounds=Rect(0, 0, 200, 80),
        clickable=not editable,
        enabled=True,
        focusable=editable,
        scrollable=False,
        selected=False,
        checked=False,
        editable=editable,
        input_focused=editable,
    )


def obs(revision: int, label: str = "Settings", *, nodes: tuple[UINode, ...] | None = None) -> Observation:
    values = nodes if nodes is not None else (node("b1", label),)
    return Observation("emulator-5554", "com.demo", ".Main", 1080, 1920, values, 1.0, 1.0, revision)


def done(label: str, summary: str = "done") -> dict:
    return {"done": True, "summary": summary, "evidence": {"exact": [label]}, "actions": []}


class FakePlanner:
    def __init__(self, plans):
        self.plans = list(plans)
        self.calls = []

    def plan(self, task, observation, history, *, context_mode="ranked", screenshot=None, model=None):
        self.calls.append((context_mode, screenshot is not None, model))
        value = dict(self.plans.pop(0))
        value.setdefault("_perception", "vision" if screenshot else context_mode)
        value.setdefault("_planner_model", model or "fake")
        value.setdefault("_planner_latency_ms", 1.0)
        return value


class FakeController:
    transport_name = "fake"

    def __init__(self, observations, *, stale_once=False):
        self.observations = list(observations)
        self.current = self.observations.pop(0)
        self.stale_once = stale_once
        self.actions = []
        self.last_transition = {}

    def observe(self):
        return self.current

    def screenshot(self):
        handle = NamedTemporaryFile(suffix=".png", delete=False)
        handle.write(b"png")
        handle.close()
        return Path(handle.name)

    def act_and_observe(self, actions, observation, *, timeout_ms=900):
        if self.stale_once:
            self.stale_once = False
            self.current = self.observations.pop(0)
            raise StaleStateError(self.current)
        self.actions.extend(actions)
        before = self.current
        if self.observations:
            self.current = self.observations.pop(0)
        self.last_transition = {
            "changed": self.current.state_hash != before.state_hash,
            "settled": True,
            "events": max(0, self.current.revision - before.revision),
        }
        return [ActionResult(action, True, 1.0, "ok") for action in actions], self.current


class AgentTests(unittest.TestCase):
    def test_ranked_then_full_context_escalation(self):
        controller = FakeController([obs(1), obs(2)])
        planner = FakePlanner([
            {"done": False, "need_context": True, "need_vision": False, "actions": []},
            {"done": False, "need_context": False, "need_vision": False, "actions": [{"type": "tap", "ref": "b1"}]},
            done("Settings"),
        ])
        config = AgentConfig(model="fake", use_vision=False, max_steps=3)
        result = ComputerUseAgent(controller, planner, config).run("Open Settings")
        self.assertTrue(result.done)
        self.assertEqual(planner.calls[0][0], "ranked")
        self.assertEqual(planner.calls[1][0], "full")
        self.assertEqual(len(controller.actions), 1)
        self.assertTrue(any(item.get("event") == "completion_accepted" for item in result.history))

    def test_vision_is_last_perception_tier(self):
        controller = FakeController([obs(1), obs(2)])
        planner = FakePlanner([
            {"done": False, "need_context": False, "need_vision": True, "actions": []},
            {"done": False, "need_context": False, "need_vision": False, "actions": [{"type": "tap", "ref": "b1"}]},
            done("Settings"),
        ])
        config = AgentConfig(model="fast", vision_model="vision", max_steps=3)
        result = ComputerUseAgent(controller, planner, config).run("Open Settings")
        self.assertTrue(result.done)
        self.assertFalse(planner.calls[0][1])
        self.assertTrue(planner.calls[1][1])
        self.assertEqual(planner.calls[1][2], "vision")

    def test_stale_revision_replans_without_executing_old_action(self):
        controller = FakeController([obs(1), obs(2), obs(3)], stale_once=True)
        planner = FakePlanner([
            {"done": False, "actions": [{"type": "tap", "ref": "b1"}]},
            {"done": False, "actions": [{"type": "tap", "ref": "b1"}]},
            done("Settings"),
        ])
        config = AgentConfig(model="fake", max_steps=4)
        result = ComputerUseAgent(controller, planner, config).run("Open Settings")
        self.assertTrue(result.done)
        self.assertEqual(len(controller.actions), 1)
        self.assertTrue(any(item.get("event") == "stale_plan_rejected" for item in result.history))

    def test_guarded_program_crosses_states_without_replanning_each_screen(self):
        controller = FakeController([
            obs(1, "Settings"),
            obs(2, "Network & internet"),
            obs(3, "Internet"),
        ])
        planner = FakePlanner([
            {
                "done": False,
                "program": [
                    {"action": {"type": "tap", "selector": "Settings"}},
                    {
                        "when": {"contains": ["Network & internet"]},
                        "action": {"type": "tap", "selector": "Network & internet"},
                    },
                ],
                "actions": [],
            },
            done("Internet", "reached Internet"),
        ])
        result = ComputerUseAgent(controller, planner, AgentConfig(model="fake", max_steps=3)).run("Open Internet settings")
        self.assertTrue(result.done)
        self.assertEqual(len(controller.actions), 2)
        self.assertEqual(len(planner.calls), 2)
        self.assertEqual(sum(item.get("event") == "program_action" for item in result.history), 2)

    def test_guarded_program_fails_closed_on_ambiguous_selector(self):
        ambiguous = obs(1, nodes=(node("a", "Continue"), node("b", "Continue")))
        controller = FakeController([ambiguous])
        planner = FakePlanner([
            {"done": False, "program": [{"action": {"type": "tap", "selector": "Continue"}}], "actions": []},
            {"done": True, "summary": "stopped safely", "evidence": {"refs": ["a"]}, "actions": []},
        ])
        result = ComputerUseAgent(controller, planner, AgentConfig(model="fake", max_steps=3)).run("Continue")
        self.assertTrue(result.done)
        self.assertEqual(controller.actions, [])
        aborts = [item for item in result.history if item.get("event") == "program_abort"]
        self.assertEqual(aborts[0]["reason"], "selector_ambiguous")

    def test_typed_action_history_is_redacted(self):
        controller = FakeController([obs(1, nodes=(node("field", "Email", editable=True),)), obs(2, "Done")])
        planner = FakePlanner([
            {"done": False, "actions": [{"type": "type", "ref": "field", "text": "secret@example.com"}]},
            done("Done"),
        ])
        result = ComputerUseAgent(controller, planner, AgentConfig(model="fake", max_steps=3)).run("Enter email")
        action_rows = [item for item in result.history if isinstance(item.get("action"), dict)]
        self.assertEqual(action_rows[0]["action"]["text"], "<redacted:18 chars>")
        self.assertNotIn("secret@example.com", str(result.history))

    def test_ungrounded_done_is_rejected_and_replanned_with_full_context(self):
        controller = FakeController([obs(1, "Settings")])
        planner = FakePlanner([
            {"done": True, "summary": "claimed", "actions": []},
            done("Settings", "grounded"),
        ])
        result = ComputerUseAgent(controller, planner, AgentConfig(model="fake", max_steps=3)).run("Open Settings")
        self.assertTrue(result.done)
        self.assertEqual(result.summary, "grounded")
        self.assertEqual(planner.calls[0][0], "ranked")
        self.assertEqual(planner.calls[1][0], "full")
        self.assertEqual(sum(item.get("event") == "completion_rejected" for item in result.history), 1)
        self.assertEqual(sum(item.get("event") == "completion_accepted" for item in result.history), 1)


if __name__ == "__main__":
    unittest.main()
