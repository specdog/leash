import json
import math
import unittest
from pathlib import Path

from leash_waveshare_slam.contract import (
    DifferentialOdometry,
    build_localization_update,
    covariance_3x3,
    imu_contract,
    laser_scan_contract,
    localization_update_due,
)


REPO_ROOT = Path(__file__).resolve().parents[4]


class BridgeContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        replay_path = REPO_ROOT / "examples/replay/waveshare-ugv-sensors.jsonl"
        cls.sensors = json.loads(replay_path.read_text(encoding="utf-8").splitlines()[0])["data"]

    def test_scrubbed_sensor_fixture_maps_to_ros_scan_and_imu_fields(self):
        scan = laser_scan_contract(self.sensors["range_scan"])
        self.assertIsNotNone(scan)
        self.assertEqual(scan["frame_id"], "base_scan")
        self.assertEqual(len(scan["ranges"]), 36)
        self.assertEqual(sum(math.isinf(value) for value in scan["ranges"]), 2)
        self.assertAlmostEqual(scan["scan_time"], 0.1)

        imu = imu_contract(self.sensors["imu"])
        self.assertIsNotNone(imu)
        self.assertEqual(imu["frame_id"], "base_link")
        self.assertIsNone(imu["orientation"])
        self.assertTrue(all(math.isfinite(value) for value in imu["angular_velocity"]))
        self.assertTrue(all(math.isfinite(value) for value in imu["linear_acceleration"]))

    def test_differential_odometry_handles_straight_and_turn_updates(self):
        straight = DifferentialOdometry(track_width_m=0.2)
        straight.update(0.0, 0.0, 1_000)
        sample = straight.update(0.1, 0.1, 2_000)
        self.assertAlmostEqual(sample["x_m"], 0.1)
        self.assertAlmostEqual(sample["y_m"], 0.0)
        self.assertAlmostEqual(sample["linear_x_mps"], 0.1)

        turn = DifferentialOdometry(track_width_m=0.2)
        turn.update(0.0, 0.0, 1_000)
        sample = turn.update(-0.1, 0.1, 2_000)
        self.assertAlmostEqual(sample["x_m"], 0.0)
        self.assertAlmostEqual(sample["yaw_rad"], 1.0)
        self.assertAlmostEqual(sample["angular_z_radps"], 1.0)

    def test_localization_update_matches_the_generic_leash_contract(self):
        covariance = [0.0] * 36
        covariance[0] = 0.01
        covariance[7] = 0.02
        covariance[35] = 0.03
        map_sample = {
            "ts_ms": 2_000,
            "frame_id": "map",
            "width": 2,
            "height": 2,
            "resolution_m": 0.05,
            "origin": {"x_m": -1.0, "y_m": -1.0, "yaw_rad": 0.0},
            "cells": [-1, 0, 50, 100],
        }
        pose_sample = {
            "ts_ms": 2_100,
            "frame_id": "map",
            "x_m": 0.25,
            "y_m": -0.10,
            "yaw_rad": 0.2,
            "covariance": covariance,
        }
        update = build_localization_update(7, "fixture-map", map_sample, pose_sample)
        self.assertEqual(update["version"], "leash-localization-provider-v1")
        self.assertEqual(update["sequence"], 7)
        self.assertEqual(update["localization"]["health"]["status"], "tracking")
        self.assertEqual(update["occupancy_grid"]["cells"], [-1, 0, 50, 100])
        self.assertEqual(update["costmap"]["costs"], [255, 0, 127, 254])
        self.assertEqual(
            update["localization"]["pose"]["covariance"],
            [0.01, 0.0, 0.0, 0.0, 0.02, 0.0, 0.0, 0.0, 0.03],
        )
        self.assertEqual(
            update["localization"]["map"]["map_revision"],
            build_localization_update(8, "fixture-map", map_sample, pose_sample)["localization"]["map"]["map_revision"],
        )

    def test_covariance_rejects_wrong_shape(self):
        with self.assertRaisesRegex(ValueError, "36"):
            covariance_3x3([0.0] * 9)

    def test_stationary_localization_heartbeats_before_provider_stales(self):
        self.assertTrue(localization_update_due(None, 2_000, None, 10.0))
        self.assertTrue(localization_update_due(2_000, 2_100, 10.0, 10.1))
        self.assertFalse(localization_update_due(2_000, 2_000, 10.0, 10.49))
        self.assertTrue(localization_update_due(2_000, 2_000, 10.0, 10.5))


if __name__ == "__main__":
    unittest.main()
