from __future__ import annotations

import copy
import unittest

from verify import verify


def proof(phase: str, run_id: str) -> dict:
    return {
        "ok": True,
        "format": "leash-waveshare-ugv-navigation-proof-v1",
        "phase": phase,
        "run_id": run_id,
        "started_at": f"2026-07-10T10:00:0{len(run_id)}Z",
        "target": {"zone_id": "safe-zone" if phase == "patrol" else None},
        "final_error_m": None if phase == "patrol" else 0.1,
        "terminal": {"status": "completed" if phase == "patrol" else "reached"},
        "safety": {
            "operator_confirmed": True,
            "clear_floor": True,
            "spotter_present": True,
            "estop_reachable": True,
            "final_motor_stop": True,
            "low_speed_cap": 0.22,
        },
        "artifacts": {
            "capture_sha256": "a" * 64,
            "replay_sha256": "b" * 64,
            "replay_validated": True,
        },
        "samples": 2,
        "resource_samples": 2,
    }


class EvidenceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.values = [
            proof("half-meter", "half"),
            proof("map-goal", "goal-1"),
            proof("map-goal", "goal-2"),
            proof("map-goal", "goal-3"),
            proof("patrol", "patrol"),
        ]

    def test_complete_evidence_set_passes(self) -> None:
        result = verify(copy.deepcopy(self.values))
        self.assertTrue(result["ok"])
        self.assertEqual(result["map_goals"], 3)

    def test_missing_goal_fails(self) -> None:
        with self.assertRaisesRegex(ValueError, "three consecutive"):
            verify(copy.deepcopy(self.values[:-2] + self.values[-1:]))

    def test_patrol_must_finish(self) -> None:
        values = copy.deepcopy(self.values)
        values[-1]["terminal"]["status"] = "stopped"
        with self.assertRaisesRegex(ValueError, "terminal waypoint"):
            verify(values)


if __name__ == "__main__":
    unittest.main()
