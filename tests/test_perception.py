from __future__ import annotations

import unittest

from android_simulator.computer_use import Observation, Rect, UINode
from android_simulator.perception import compact_for_task, semantic_diff, task_tokens


def node(ref: str, label: str, *, clickable: bool = False, editable: bool = False, top: int = 0) -> UINode:
    return UINode(
        ref=ref,
        text=label,
        content_desc="",
        resource_id=f"com.demo:id/{ref}",
        class_name="android.widget.EditText" if editable else "android.widget.Button",
        package="com.demo",
        bounds=Rect(0, top, 400, top + 80),
        clickable=clickable,
        enabled=True,
        focusable=editable,
        scrollable=False,
        selected=False,
        checked=False,
        editable=editable,
    )


def observation(nodes: tuple[UINode, ...], *, revision: int = 1) -> Observation:
    return Observation(
        serial="emulator-5554",
        package="com.demo",
        activity=".Main",
        width=1080,
        height=1920,
        nodes=nodes,
        captured_at=1.0,
        latency_ms=1.0,
        revision=revision,
    )


class PerceptionTests(unittest.TestCase):
    def test_task_tokens_remove_generic_control_words(self):
        self.assertEqual(task_tokens("Open the app and search for Daft Punk"), ("search", "for", "daft", "punk"))

    def test_task_ranked_context_keeps_relevant_target(self):
        filler = tuple(node(f"n{i}", f"Generic item {i}", clickable=True, top=i * 10) for i in range(40))
        target = node("target", "Daft Punk", clickable=False, top=900)
        search = node("search", "Search artists", editable=True, top=1000)
        obs = observation(filler + (target, search))
        compact = compact_for_task(obs, "Search for Daft Punk", max_nodes=16)
        refs = {item["ref"] for item in compact["nodes"]}
        self.assertIn("target", refs)
        self.assertIn("search", refs)
        self.assertGreater(compact["omitted_nodes"], 0)
        self.assertEqual(compact["perception"], "task_ranked")

    def test_semantic_diff_detects_added_removed_and_changed(self):
        before = observation((node("a", "One", clickable=True), node("b", "Two")), revision=4)
        after = observation((node("a", "One changed", clickable=True), node("c", "Three")), revision=5)
        diff = semantic_diff(before, after)
        self.assertEqual(diff["from_state"], before.state_hash)
        self.assertEqual(diff["to_state"], after.state_hash)
        self.assertEqual({item["ref"] for item in diff["added"]}, {"c"})
        self.assertEqual({item["ref"] for item in diff["removed"]}, {"b"})
        self.assertEqual(len(diff["changed"]), 1)


if __name__ == "__main__":
    unittest.main()
