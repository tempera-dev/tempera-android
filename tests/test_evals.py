from __future__ import annotations

import unittest

from android_simulator.computer_use import Observation, Rect, UINode
from android_simulator.evals import EvalVerifier, SYNTHETIC_CASES, SETTINGS_CASES, builtin_cases


def observation(*labels: str, package: str = "dev.jadenfix.androidbridge") -> Observation:
    nodes = tuple(
        UINode(
            ref=f"n{index}",
            text=label,
            content_desc="",
            resource_id=f"{package}:id/n{index}",
            class_name="android.widget.TextView",
            package=package,
            bounds=Rect(0, index * 10, 100, index * 10 + 10),
            clickable=False,
            enabled=True,
            focusable=False,
            scrollable=False,
            selected=False,
            checked=False,
        )
        for index, label in enumerate(labels)
    )
    return Observation(
        "emulator-5554",
        package,
        ".EvalFixtureActivity",
        1080,
        1920,
        nodes,
        1.0,
        1.0,
        10,
    )


class EvalTests(unittest.TestCase):
    def test_exact_end_state_grader_does_not_accept_substrings(self):
        verifier = EvalVerifier(exact_present=("Internet",))
        success, checks = verifier.evaluate(observation("Network & internet"))
        self.assertFalse(success)
        self.assertFalse(checks["present:Internet"])

    def test_exact_end_state_grader_checks_package_and_absence(self):
        verifier = EvalVerifier(
            package="dev.jadenfix.androidbridge",
            exact_present=("Dialog accepted",),
            exact_absent=("Permission simulation",),
        )
        success, checks = verifier.evaluate(observation("Dialog accepted", "Fixture state: complete"))
        self.assertTrue(success, checks)

    def test_builtin_populations_are_separate(self):
        all_cases = builtin_cases(include_settings=True)
        synthetic_only = builtin_cases(include_settings=False)
        self.assertEqual(len(synthetic_only), len(SYNTHETIC_CASES))
        self.assertEqual(len(all_cases), len(SYNTHETIC_CASES) + len(SETTINGS_CASES))
        self.assertEqual({case.population for case in synthetic_only}, {"synthetic_fixture"})
        self.assertEqual({case.population for case in SETTINGS_CASES}, {"android_settings"})

    def test_case_ids_are_unique(self):
        ids = [case.id for case in builtin_cases(include_settings=True)]
        self.assertEqual(len(ids), len(set(ids)))


if __name__ == "__main__":
    unittest.main()
