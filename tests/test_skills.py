from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from android_simulator.computer_use import Observation, Rect, UINode
from android_simulator.skills import SkillStore, task_digest


def node(ref: str, label: str) -> UINode:
    return UINode(
        ref=ref,
        text=label,
        content_desc="",
        resource_id=f"com.demo:id/{ref}",
        class_name="android.widget.Button",
        package="com.demo",
        bounds=Rect(0, 0, 200, 80),
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


class SkillTests(unittest.TestCase):
    def test_navigation_skill_never_persists_task_text(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "skills.json"
            store = SkillStore(path)
            task = "Open private customer workspace 12345"
            start = obs(1, "Settings")
            final = obs(2, "Done")
            program = [{"action": {"type": "tap", "selector": "Settings"}}]
            learned = store.learn(
                task=task,
                start_observation=start,
                program=program,
                final_observation=final,
                completion_evidence={"refs": ["r2"]},
            )
            self.assertIsNotNone(learned)
            raw = path.read_text(encoding="utf-8")
            self.assertNotIn(task, raw)
            self.assertIn(task_digest(task), raw)
            self.assertNotIn('"refs"', raw)
            self.assertIn('"Done"', raw)

    def test_typed_program_is_never_cacheable(self):
        with tempfile.TemporaryDirectory() as directory:
            store = SkillStore(Path(directory) / "skills.json")
            learned = store.learn(
                task="Fill form",
                start_observation=obs(1, "Name"),
                program=[{"action": {"type": "type", "selector": "Name", "text": "secret"}}],
                final_observation=obs(2, "Done"),
                completion_evidence={"exact": ["Done"]},
            )
            self.assertIsNone(learned)

    def test_sensitive_navigation_selector_is_not_cached(self):
        with tempfile.TemporaryDirectory() as directory:
            store = SkillStore(Path(directory) / "skills.json")
            learned = store.learn(
                task="Purchase thing",
                start_observation=obs(1, "Buy now"),
                program=[{"action": {"type": "tap", "selector": "Buy now"}}],
                final_observation=obs(2, "Done"),
                completion_evidence={"exact": ["Done"]},
            )
            self.assertIsNone(learned)

    def test_candidate_requires_fresh_start_guard(self):
        with tempfile.TemporaryDirectory() as directory:
            store = SkillStore(Path(directory) / "skills.json")
            task = "Open Settings"
            program = [{"action": {"type": "tap", "selector": "Settings"}}]
            learned = store.learn(
                task=task,
                start_observation=obs(1, "Settings"),
                program=program,
                final_observation=obs(2, "Done"),
                completion_evidence={"exact": ["Done"]},
            )
            self.assertIsNotNone(learned)
            self.assertEqual(len(store.candidates(task, obs(3, "Settings"))), 1)
            self.assertEqual(store.candidates(task, obs(4, "Different screen")), [])


if __name__ == "__main__":
    unittest.main()
