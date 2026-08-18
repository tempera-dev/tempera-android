from __future__ import annotations

import copy
import hashlib
import json
import unittest

from android_simulator.tempera_evals import external_adapter_result


DIGEST_A = "sha256:" + "a" * 64
DIGEST_B = "sha256:" + "b" * 64


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


class TemperaEvalExportTests(unittest.TestCase):
    def report(self):
        return {
            "schema_version": "android-agent-eval.v1",
            "created_at": "2026-08-16T00:00:00+00:00",
            "result": {
                "aggregate": {
                    "mean_wall_ms": 100.0,
                    "mean_model_calls": 2.0,
                    "mean_actions": 3.0,
                    "by_population": {
                        "synthetic_fixture": {"cases": 2, "success_rate": 0.5},
                    },
                },
                "cases": [
                    {
                        "case_id": "case.a",
                        "success": True,
                        "agent_reported_done": True,
                        "agent_summary": "done",
                        "error": None,
                        "history": [{"event": "plan"}],
                        "grader": {"checks": {"a": True}},
                    },
                    {
                        "case_id": "case.b",
                        "success": False,
                        "agent_reported_done": True,
                        "agent_summary": "claimed done",
                        "error": "failure",
                        "history": [],
                        "grader": {"checks": {"b": False}},
                    },
                ],
            },
        }

    def test_export_has_exact_v1_fields_and_self_digest(self):
        result = external_adapter_result(
            self.report(),
            run_id="android.local-001",
            plan_sha256=DIGEST_A,
            adapter_descriptor_sha256=DIGEST_B,
        )
        self.assertEqual(result["schema_version"], "tempera.external-adapter-result.v1")
        self.assertEqual(result["population"]["case_count"], 2)
        self.assertEqual(result["population"]["error_count"], 1)
        self.assertEqual(result["metrics"]["success_rate"]["value"], 0.5)
        without_digest = copy.deepcopy(result)
        observed = without_digest.pop("result_sha256")
        expected = "sha256:" + hashlib.sha256(canonical(without_digest)).hexdigest()
        self.assertEqual(observed, expected)

    def test_export_requires_real_governance_digests(self):
        with self.assertRaisesRegex(ValueError, "plan_sha256"):
            external_adapter_result(
                self.report(),
                run_id="android.local-001",
                plan_sha256="not-a-digest",
                adapter_descriptor_sha256=DIGEST_B,
            )


if __name__ == "__main__":
    unittest.main()
