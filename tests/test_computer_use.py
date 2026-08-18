from __future__ import annotations

import unittest

from android_simulator.agent import _safe_batch, _sensitive
from android_simulator.computer_use import DeviceController, Observation


XML = '''<hierarchy rotation="0">
  <node index="0" text="" resource-id="" class="android.widget.FrameLayout" package="com.demo" content-desc="" clickable="false" enabled="true" focusable="false" scrollable="false" selected="false" checked="false" bounds="[0,0][1080,1920]">
    <node index="1" text="Settings" resource-id="com.demo:id/settings" class="android.widget.Button" package="com.demo" content-desc="Open settings" clickable="true" enabled="true" focusable="true" scrollable="false" selected="false" checked="false" bounds="[100,200][500,320]" />
    <node index="2" text="Send" resource-id="com.demo:id/send" class="android.widget.Button" package="com.demo" content-desc="" clickable="true" enabled="true" focusable="true" scrollable="false" selected="false" checked="false" bounds="[100,400][500,520]" />
  </node>
</hierarchy>'''


class ComputerUseTests(unittest.TestCase):
    def observation(self) -> Observation:
        nodes = DeviceController._parse_nodes(XML)
        return Observation("emulator-5554", "com.demo", ".Main", 1080, 1920, nodes, 1.0, 3.5)

    def test_parse_and_compact(self):
        obs = self.observation()
        self.assertEqual(len(obs.nodes), 3)
        self.assertEqual(obs.nodes[1].label, "Settings")
        compact = obs.compact()
        self.assertEqual(compact["package"], "com.demo")
        self.assertEqual(len(compact["state_hash"]), 24)
        self.assertTrue(any(node.get("clickable") for node in compact["nodes"]))

    def test_state_hash_stable_across_timing(self):
        first = self.observation()
        second = Observation(first.serial, first.package, first.activity, first.width, first.height, first.nodes, 99.0, 999.0)
        self.assertEqual(first.state_hash, second.state_hash)

    def test_sensitive_confirmation_label(self):
        obs = self.observation()
        self.assertEqual(_sensitive({"type": "tap", "ref": "n2"}, obs), "Send")
        self.assertIsNone(_sensitive({"type": "tap", "ref": "n1"}, obs))

    def test_input_text_encoding(self):
        self.assertEqual(DeviceController._input_text("hello world"), "hello%sworld")
        self.assertEqual(DeviceController._input_text("10%"), "10%25")

    def test_batch_stops_before_second_selector(self):
        actions = [
            {"type": "tap", "ref": "n1"},
            {"type": "type", "text": "hello"},
            {"type": "enter"},
            {"type": "tap", "ref": "n2"},
            {"type": "back"},
        ]
        self.assertEqual(_safe_batch(actions, 8), actions[:3])

    def test_compiler_grounds_selector_locally(self):
        controller = DeviceController(None, "emulator-test")  # type: ignore[arg-type]
        command, detail = controller._compile_action({"type": "tap", "ref": "n1"}, self.observation())
        self.assertEqual(command, "input tap 300 260")
        self.assertEqual(detail, "Settings")

    def test_compiler_quotes_typed_text(self):
        controller = DeviceController(None, "emulator-test")  # type: ignore[arg-type]
        command, detail = controller._compile_action({"type": "type", "text": "hello world"}, self.observation())
        self.assertIn("input text", command)
        self.assertIn("hello%sworld", command)
        self.assertEqual(detail, "11 chars")


if __name__ == "__main__":
    unittest.main()
