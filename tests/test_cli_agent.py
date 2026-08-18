from __future__ import annotations

import unittest

from android_simulator.agent_cli import build_parser


class AgentCliTests(unittest.TestCase):
    def test_bridge_setup_parser(self):
        args = build_parser().parse_args(["--transport", "bridge", "bridge", "setup"])
        self.assertEqual(args.transport, "bridge")
        self.assertEqual(args.command, "bridge")
        self.assertEqual(args.bridge_command, "setup")

    def test_run_progressive_perception_parser(self):
        args = build_parser().parse_args([
            "--model", "fast-model",
            "--vision-model", "vision-model",
            "run", "Open Settings",
            "--task-context-nodes", "48",
            "--full-context-nodes", "320",
            "--max-program-steps", "4",
            "--skills",
            "--skill-cache", "skills.json",
        ])
        self.assertEqual(args.model, "fast-model")
        self.assertEqual(args.vision_model, "vision-model")
        self.assertEqual(args.task_context_nodes, 48)
        self.assertEqual(args.full_context_nodes, 320)
        self.assertEqual(args.max_program_steps, 4)
        self.assertTrue(args.skills)
        self.assertEqual(str(args.skill_cache), "skills.json")

    def test_eval_parser_supports_case_selection_and_governed_export(self):
        args = build_parser().parse_args([
            "--model", "fast-model",
            "eval",
            "--synthetic-only",
            "--case", "fixture.dialog-multiwindow",
            "--case", "fixture.long-scroll",
            "--output", "report.json",
            "--tempera-output", "tempera.json",
            "--tempera-run-id", "android.local-001",
            "--tempera-plan-sha256", "sha256:" + "a" * 64,
            "--tempera-adapter-sha256", "sha256:" + "b" * 64,
        ])
        self.assertEqual(args.command, "eval")
        self.assertTrue(args.synthetic_only)
        self.assertEqual(
            args.case,
            ["fixture.dialog-multiwindow", "fixture.long-scroll"],
        )
        self.assertEqual(str(args.output), "report.json")
        self.assertEqual(str(args.tempera_output), "tempera.json")
        self.assertEqual(args.tempera_run_id, "android.local-001")


if __name__ == "__main__":
    unittest.main()
