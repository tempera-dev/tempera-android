from __future__ import annotations

import unittest

from android_simulator.gym_env import AndroidGymEnv, SuccessSpec


class GymAdapterTests(unittest.TestCase):
    def env(self) -> AndroidGymEnv:
        env = AndroidGymEnv(None, SuccessSpec())  # type: ignore[arg-type]
        env.steps.append({
            "observation": {"state": "a"},
            "action": {"type": "back"},
            "reward": 0.0,
            "terminated": False,
            "truncated": False,
            "info": {},
            "state_digest": "0" * 64,
        })
        return env

    def test_trajectory_v1_shape(self):
        trajectory = self.env().trajectory_v1(metadata={"policy": "test"})
        self.assertEqual(trajectory["schema_version"], "trajectory-v1")
        self.assertEqual(len(trajectory["environment_digest"]), 64)
        self.assertEqual(len(trajectory["content_hash"]), 64)
        self.assertEqual(trajectory["metadata"]["policy"], "test")

    def test_timing_is_excluded_from_content_identity(self):
        env = self.env()
        first = env.trajectory_v1(metadata={"policy": "p", "timing": {"run_id": "one"}})
        second = env.trajectory_v1(metadata={"policy": "p", "timing": {"run_id": "two"}})
        self.assertEqual(first["content_hash"], second["content_hash"])

    def test_non_timing_metadata_changes_identity(self):
        env = self.env()
        first = env.trajectory_v1(metadata={"policy": "p1"})
        second = env.trajectory_v1(metadata={"policy": "p2"})
        self.assertNotEqual(first["content_hash"], second["content_hash"])


if __name__ == "__main__":
    unittest.main()
