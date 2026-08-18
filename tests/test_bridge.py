from __future__ import annotations

import unittest
from types import SimpleNamespace

from android_simulator.bridge import BridgeController
from android_simulator.computer_use import StaleStateError


class FakeClient:
    def __init__(self):
        self.last_actions = None
        self.last_revision = None
        self.stale = False

    def observe(self):
        return {
            "revision": 7,
            "package": "com.demo",
            "activity": ".Main",
            "screen": [1080, 1920],
            "nodes": [
                {
                    "ref": "babc",
                    "label": "Settings",
                    "text": "Settings",
                    "id": "com.demo:id/settings",
                    "class": "Button",
                    "bounds": [10, 20, 210, 120],
                    "clickable": True,
                },
                {
                    "ref": "bpassword",
                    "label": "password",
                    "id": "com.demo:id/password",
                    "class": "EditText",
                    "bounds": [10, 140, 410, 240],
                    "editable": True,
                    "password": True,
                    "input_focused": True,
                },
            ],
        }

    def act_observe(self, actions, *, expected_revision=0, timeout_ms=900):
        self.last_actions = actions
        self.last_revision = expected_revision
        observation = self.observe()
        observation["revision"] = 8
        if self.stale:
            return {"stale": True, "observation": observation}
        return {
            "stale": False,
            "changed": True,
            "results": [
                {"ok": True, "action": action, "latency_ms": 1.25, "detail": "ok"}
                for action in actions
            ],
            "observation": observation,
        }

    def act(self, actions, *, expected_revision=0):
        return {
            "stale": False,
            "results": [
                {"ok": True, "action": action, "latency_ms": 1.0, "detail": "ok"}
                for action in actions
            ],
        }

    def wait_observe(self, *, after_revision, timeout_ms=2000):
        return {"changed": True, "observation": self.observe()}

    def screenshot(self):
        return b"png"

    def close(self):
        pass


class BridgeControllerTests(unittest.TestCase):
    def controller(self):
        return BridgeController(SimpleNamespace(), "emulator-5554", FakeClient())

    def test_observation_carries_revision_and_password_redaction(self):
        controller = self.controller()
        observation = controller.observe()
        self.assertEqual(observation.revision, 7)
        self.assertEqual(observation.nodes[0].label, "Settings")
        password = observation.nodes[1]
        self.assertTrue(password.password)
        self.assertTrue(password.editable)
        self.assertEqual(password.text, "")
        self.assertNotIn("text", password.compact())

    def test_selector_is_grounded_locally_then_revision_checked(self):
        controller = self.controller()
        observation = controller.observe()
        results, next_observation = controller.act_and_observe(
            [{"type": "tap", "selector": "Settings"}],
            observation,
        )
        self.assertEqual(controller.client.last_revision, 7)
        self.assertEqual(controller.client.last_actions[0]["ref"], "babc")
        self.assertNotIn("selector", controller.client.last_actions[0])
        self.assertEqual(next_observation.revision, 8)
        self.assertEqual(len(results), 1)

    def test_stale_bridge_response_rejects_action(self):
        controller = self.controller()
        observation = controller.observe()
        controller.client.stale = True
        with self.assertRaises(StaleStateError) as ctx:
            controller.act_and_observe([{"type": "tap", "ref": "babc"}], observation)
        self.assertEqual(ctx.exception.observation.revision, 8)


if __name__ == "__main__":
    unittest.main()
