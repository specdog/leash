from __future__ import annotations

import copy
import json
import math
import pathlib
import tempfile
import unittest

from analyze import analyze_one
from profile import canonical_bytes, env_lines, validate
from replay import replay_events


def candidate_profile() -> dict:
    return {
        "schema": "leash-waveshare-ugv-calibration-v1",
        "profile": "pinkie-v1",
        "status": "candidate",
        "measurement": {
            "procedure_revision": "issue-166-v1",
            "measured_at": "2026-07-10T00:00:00Z",
            "evidence_sha256": [],
        },
        "wheels": {"track_width_m": 0.2, "distance_scale": 1.0},
        "lidar": {
            "transform": {"x_m": 0.0, "y_m": 0.0, "z_m": 0.1, "yaw_rad": 0.0},
            "yaw_offset_deg": 180.0,
            "clockwise": False,
            "body_masks_deg": [],
        },
        "camera": {
            "transform": {"x_m": 0.0, "y_m": 0.0, "z_m": 0.15, "yaw_rad": 0.0}
        },
        "imu": {
            "accel_lsb_per_g": 8192.0,
            "gyro_dps_per_lsb": 0.0164,
            "axis_map": "x,y,z",
            "accel_bias_mps2": [0.0, 0.0, 0.0],
            "gyro_bias_radps": [0.0, 0.0, 0.0],
        },
    }


def verified_zero(command_sequence: int, adapter_sample_sequence: int) -> dict:
    return {
        "command_sequence": command_sequence,
        "write_completed_at_ms": 1_700_000_000_000 + command_sequence,
        "acknowledged": True,
        "adapter_sample_sequence": adapter_sample_sequence,
        "confirmation_received_at_ms": 1_700_000_000_100 + command_sequence,
        "source": "waveshare-ugv",
        "statement": "zero command confirmed",
    }


def sample(
    elapsed_ms: int,
    x_m: float = 0.0,
    yaw_rad: float = 0.0,
    odometry_left: float | None = None,
    odometry_right: float | None = None,
) -> dict:
    ts_ms = 1_700_000_000_000 + elapsed_ms
    pose = {
        "pose": {"ts_ms": ts_ms, "frame_id": "map", "x_m": x_m, "y_m": 0.0, "yaw_rad": yaw_rad},
        "covariance": [0.01, 0.0, 0.0, 0.0, 0.01, 0.0, 0.0, 0.0, 0.01],
    }
    return {
        "kind": "sample",
        "elapsed_ms": elapsed_ms,
        "health": {"ok": True},
        "telemetry": {
            "ts_ms": ts_ms,
            "left_cmd": 0.0,
            "right_cmd": 0.0,
            "odometry_left": x_m if odometry_left is None else odometry_left,
            "odometry_right": x_m if odometry_right is None else odometry_right,
            "sensors": {
                "range_scan": {
                    "status": "available",
                    "last_ms": ts_ms,
                    "sample": {
                        "angle_min_rad": -math.pi,
                        "angle_increment_rad": math.pi / 2.0,
                        "ranges_m": [1.0, 1.0, 1.0, 1.0],
                    },
                },
                "imu": {
                    "status": "available",
                    "last_ms": ts_ms,
                    "sample": {
                        "linear_acceleration_mps2": {"x": 0.0, "y": 0.0, "z": 9.80665},
                        "angular_velocity_radps": {"x": 0.0, "y": 0.0, "z": 0.0},
                    },
                },
            },
            "localization": {"pose": pose},
        },
        "provider": {"state": "tracking"},
        "container": {
            "running": True,
            "oom_killed": False,
            "restart_count": 0,
            "cpu_pct": 1.0,
            "memory_usage": "100MiB / 1.5GiB",
        },
    }


class CalibrationTests(unittest.TestCase):
    def capture_result(
        self,
        profile: dict,
        phase: str,
        samples: list[dict],
        *,
        expected_distance_m: float | None = None,
        expected_turn_deg: float = 360.0,
        expected_side_m: float | None = None,
        legacy_zero: bool = False,
    ) -> dict:
        digest = __import__("hashlib").sha256(canonical_bytes(profile)).hexdigest()
        repository = pathlib.Path(__file__).resolve().parents[4]
        source_event = next(
            json.loads(line)
            for line in (repository / "examples/replay/sim-mapping.jsonl").read_text().splitlines()
            if json.loads(line)["kind"] == "telemetry"
        )
        replay = replay_events([source_event["data"]])
        replay_text = "\n".join(json.dumps(event) for event in replay) + "\n"
        replay_digest = __import__("hashlib").sha256(replay_text.encode()).hexdigest()
        events = [
            {
                "kind": "capture-start",
                "format": "leash-waveshare-ugv-calibration-capture-v1",
                "phase": phase,
                "profile": "pinkie-v1",
                "calibration_sha256": digest,
                "duration_secs": 60,
                "run_index": 1,
                "expected_distance_m": expected_distance_m,
                "expected_turn_deg": expected_turn_deg,
                "expected_side_m": expected_side_m,
                **(
                    {"initial_motor_stop": True}
                    if legacy_zero
                    else {"verified_zero": verified_zero(1, 10)}
                ),
            },
            *samples,
            {
                "kind": "capture-end",
                "ok": True,
                **(
                    {"final_motor_stop": True}
                    if legacy_zero
                    else {"verified_zero": verified_zero(2, 11)}
                ),
                "replay": {
                    "file": "capture-replay.jsonl",
                    "sha256": replay_digest,
                    "format": "leash-replay-v1",
                },
            },
        ]
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / "capture.jsonl"
            path.write_text("\n".join(json.dumps(event) for event in events) + "\n")
            path.with_name("capture-replay.jsonl").write_text(replay_text)
            return analyze_one(path, profile)

    def test_candidate_values_validate_and_emit_only_calibration(self):
        profile = validate(candidate_profile(), require_values=True)
        lines = env_lines(profile, "leash") + env_lines(profile, "ros")
        rendered = "\n".join(lines)
        self.assertIn("LEASH_ROS_WHEEL_TRACK_M=0.2", rendered)
        self.assertIn("LEASH_UGV_IMU_GYRO_BIAS_RADPS=0.0,0.0,0.0", rendered)
        for forbidden in ("TOKEN", "DEVICE", "SERIAL", "ADDRESS", "MAP_STATE"):
            self.assertNotIn(forbidden, rendered)

    def test_calibration_digest_survives_evidence_promotion(self):
        candidate = candidate_profile()
        accepted = copy.deepcopy(candidate)
        accepted["status"] = "accepted"
        accepted["measurement"]["evidence_sha256"] = ["a" * 64]
        validate(accepted, require_accepted=True)
        self.assertEqual(canonical_bytes(candidate), canonical_bytes(accepted))

    def test_capture_tool_records_typed_verified_zero(self):
        repository = pathlib.Path(__file__).resolve().parents[4]
        script = (repository / "implementations/waveshare-ugv/calibration/capture.sh").read_text()

        self.assertIn('verified_stop calibration-entry', script)
        self.assertIn('verified_stop calibration-exit', script)
        self.assertIn('verified_zero:$verified_zero', script)
        self.assertNotIn('initial_motor_stop:true', script)
        self.assertNotIn('final_motor_stop:true', script)

    def test_map_reload_proof_records_typed_verified_zero(self):
        repository = pathlib.Path(__file__).resolve().parents[4]
        script = (
            repository / "implementations/waveshare-ugv/calibration/map-reload-proof.sh"
        ).read_text()

        self.assertIn('verified_stop map-reload-entry', script)
        self.assertIn('verified_stop map-reload-exit', script)
        self.assertNotIn('final_motor_stop:true', script)

    def test_analysis_rejects_legacy_boolean_only_stop_evidence(self):
        with self.assertRaisesRegex(ValueError, "verified zero"):
            self.capture_result(
                candidate_profile(),
                "stationary",
                [sample(0), sample(60_000)],
                legacy_zero=True,
            )

    def test_stationary_capture_accepts_bounded_drift(self):
        profile = candidate_profile()
        result = self.capture_result(
            profile,
            "stationary",
            [sample(0), sample(60_000, x_m=0.04, yaw_rad=math.radians(1.5))],
        )
        self.assertTrue(result["accepted"])
        self.assertLessEqual(result["phase_metrics"]["drift_m"], 0.05)

    def test_stationary_capture_rejects_excess_drift(self):
        profile = candidate_profile()
        result = self.capture_result(profile, "stationary", [sample(0), sample(60_000, x_m=0.06)])
        self.assertFalse(result["accepted"])

    def test_straight_capture_checks_wheel_and_localized_distance(self):
        profile = candidate_profile()
        result = self.capture_result(
            profile,
            "straight",
            [sample(0), sample(10_000, x_m=1.0)],
            expected_distance_m=1.0,
        )
        self.assertTrue(result["accepted"])
        self.assertEqual(result["phase_metrics"]["recommended_distance_scale"], 1.0)

    def test_turn_capture_unwraps_yaw_and_checks_track_width(self):
        profile = candidate_profile()
        half_wheel_travel = math.pi * profile["wheels"]["track_width_m"]
        poses = [0.0, math.pi / 2.0, math.pi, -math.pi / 2.0, 0.0]
        samples = [
            sample(
                index * 2_000,
                yaw_rad=yaw,
                odometry_left=-half_wheel_travel * index / 4.0,
                odometry_right=half_wheel_travel * index / 4.0,
            )
            for index, yaw in enumerate(poses)
        ]
        result = self.capture_result(profile, "turn", samples)
        self.assertTrue(result["accepted"])
        self.assertAlmostEqual(result["phase_metrics"]["wheel_turn_deg"], 360.0)

    def test_square_capture_checks_final_closure(self):
        profile = candidate_profile()
        result = self.capture_result(
            profile,
            "square",
            [sample(0), sample(20_000, x_m=0.2, yaw_rad=math.radians(10.0))],
            expected_side_m=1.0,
        )
        self.assertTrue(result["accepted"])


if __name__ == "__main__":
    unittest.main()
