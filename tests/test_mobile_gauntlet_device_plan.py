from __future__ import annotations

import copy
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "mobile-gauntlet-device.py"
FIXTURE = ROOT / "tests" / "fixtures" / "mobile-gauntlet-plan.json"

spec = importlib.util.spec_from_file_location("mobile_gauntlet_device", SCRIPT)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class MobileGauntletPlanTests(unittest.TestCase):
    def plan(self):
        return json.loads(FIXTURE.read_text(encoding="utf-8"))

    def test_example_plan_validates(self):
        plan = module.validate_plan(self.plan())
        self.assertEqual(plan["schemaVersion"], module.PLAN_SCHEMA)
        self.assertEqual(plan["maxSteps"], 40)
        self.assertEqual(plan["networkPolicy"], "deny")

    def test_plan_rejects_open_network(self):
        plan = self.plan()
        plan["networkPolicy"] = "open"
        with self.assertRaises(module.PlanError):
            module.validate_plan(plan)

    def test_plan_rejects_physical_device(self):
        plan = self.plan()
        plan["serial"] = "R58M123456A"
        plan["targetKind"] = "physical"
        with self.assertRaises(module.PlanError):
            module.validate_plan(plan)

    def test_plan_rejects_runtime_step_overflow(self):
        plan = self.plan()
        plan["maxSteps"] = 41
        with self.assertRaises(module.PlanError):
            module.validate_plan(plan)

    def test_plan_rejects_undeclared_clear_target(self):
        plan = self.plan()
        plan["setup"]["clearPackages"].append("com.android.settings")
        with self.assertRaises(module.PlanError):
            module.validate_plan(plan)

    def test_plan_rejects_external_launch_uri(self):
        plan = self.plan()
        plan["setup"].pop("launchPackage")
        plan["setup"]["launchUri"] = "https://example.com/benchmark-answer"
        with self.assertRaises(module.PlanError):
            module.validate_plan(plan)

    def test_plan_rejects_path_traversal(self):
        plan = self.plan()
        plan["fixtures"][0]["apkPath"] = "../../secret.apk"
        with self.assertRaises(module.PlanError):
            module.validate_plan(plan)

    def test_plan_rejects_unknown_fields(self):
        plan = self.plan()
        plan["grader"] = {"expected": "do not embed hidden truth"}
        with self.assertRaises(module.PlanError):
            module.validate_plan(plan)

    def test_task_is_hashed_not_echoed_by_validation_receipt(self):
        plan = module.validate_plan(self.plan())
        receipt = {
            "planSha256": module.digest_bytes(module.canonical(plan)),
            "taskSha256": module.digest_bytes(plan["task"].encode()),
        }
        encoded = json.dumps(receipt)
        self.assertNotIn(plan["task"], encoded)
        self.assertEqual(len(receipt["taskSha256"]), 64)

    def test_fixture_package_must_be_unique(self):
        plan = self.plan()
        duplicate = copy.deepcopy(plan["fixtures"][0])
        duplicate["apkPath"] = "fixtures/apps/duplicate.apk"
        plan["fixtures"].append(duplicate)
        with self.assertRaises(module.PlanError):
            module.validate_plan(plan)


if __name__ == "__main__":
    unittest.main()
