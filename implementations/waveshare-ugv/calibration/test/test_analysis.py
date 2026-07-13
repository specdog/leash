from __future__ import annotations

import json
import math
import pathlib
import subprocess
import sys
import tempfile
import unittest

from analyze import analyze_one
from support import candidate_profile, sample, square_samples, write_capture


class AnalysisTests(unittest.TestCase):
    def analyze(self, phase: str, samples: list[dict], **kwargs) -> dict:
        profile = candidate_profile()
        with tempfile.TemporaryDirectory() as directory:
            path = write_capture(
                pathlib.Path(directory),
                "capture.jsonl",
                profile,
                phase,
                samples,
                **kwargs,
            )
            return analyze_one(path, profile)

    def test_stationary_rejects_intermediate_excursion_that_returns_to_origin(self):
        result = self.analyze(
            "stationary",
            [sample(0), sample(30_000, x_m=0.06), sample(60_000)],
        )

        self.assertFalse(result["accepted"])
        self.assertGreater(result["phase_metrics"]["max_excursion_m"], 0.05)

    def test_stationary_uses_sample_timestamps_for_duration(self):
        result = self.analyze(
            "stationary",
            [sample(0), sample(59_000)],
            duration_secs=60,
        )

        self.assertFalse(result["accepted"])
        self.assertEqual(result["phase_metrics"]["duration_secs"], 59.0)

    def test_straight_requires_one_meter_ground_truth(self):
        result = self.analyze(
            "straight",
            [sample(0), sample(10_000, x_m=0.8)],
            expected_distance_m=0.8,
        )

        self.assertFalse(result["accepted"])

    def test_straight_rejects_lateral_excursion(self):
        result = self.analyze(
            "straight",
            [
                sample(0),
                sample(5_000, x_m=0.5, y_m=0.16, odometry_left=0.5, odometry_right=0.5),
                sample(10_000, x_m=1.0, odometry_left=1.0, odometry_right=1.0),
            ],
            expected_distance_m=1.0,
        )

        self.assertFalse(result["accepted"])
        self.assertGreater(result["phase_metrics"]["max_lateral_deviation_m"], 0.15)

    def test_turn_rejects_more_than_ten_degrees_backtracking(self):
        profile = candidate_profile()
        half_wheel_travel = math.pi * profile["wheels"]["track_width_m"]
        yaws = [0.0, 90.0, 70.0, 180.0, 270.0, 360.0]
        samples = [
            sample(
                index * 1_000,
                yaw_rad=math.radians(yaw),
                odometry_left=-half_wheel_travel * index / (len(yaws) - 1),
                odometry_right=half_wheel_travel * index / (len(yaws) - 1),
            )
            for index, yaw in enumerate(yaws)
        ]
        result = self.analyze("turn", samples)

        self.assertFalse(result["accepted"])
        self.assertGreater(result["phase_metrics"]["backtracking_deg"], 10.0)

    def test_turn_rejects_translation_excursion(self):
        profile = candidate_profile()
        half_wheel_travel = math.pi * profile["wheels"]["track_width_m"]
        points = [
            (0.0, 0.0),
            (90.0, 0.25),
            (180.0, 0.0),
            (270.0, 0.0),
            (360.0, 0.0),
        ]
        samples = [
            sample(
                index * 1_000,
                x_m=x_m,
                yaw_rad=math.radians(yaw),
                odometry_left=-half_wheel_travel * index / (len(points) - 1),
                odometry_right=half_wheel_travel * index / (len(points) - 1),
            )
            for index, (yaw, x_m) in enumerate(points)
        ]
        result = self.analyze("turn", samples)

        self.assertFalse(result["accepted"])
        self.assertGreater(result["phase_metrics"]["max_translation_m"], 0.20)

    def test_square_rejects_zero_length_loop(self):
        result = self.analyze(
            "square",
            [sample(0), sample(20_000)],
            expected_side_m=1.0,
        )

        self.assertFalse(result["accepted"])

    def test_square_accepts_four_ordered_sides_and_corners(self):
        result = self.analyze(
            "square",
            square_samples(),
            expected_side_m=1.0,
        )

        self.assertTrue(result["accepted"])
        self.assertEqual(len(result["phase_metrics"]["side_lengths_m"]), 4)
        self.assertEqual(len(result["phase_metrics"]["corner_turns_deg"]), 4)

    def test_square_rejects_mixed_turn_directions(self):
        result = self.analyze(
            "square",
            square_samples(turn_degrees=(90.0, 90.0, -90.0, 90.0)),
            expected_side_m=1.0,
        )

        self.assertFalse(result["accepted"])

    def test_analysis_rejects_null_resource_samples(self):
        with self.assertRaisesRegex(ValueError, "resource"):
            self.analyze(
                "stationary",
                [sample(0), sample(60_000, resource=None)],
            )

    def test_analysis_rejects_stale_resource_samples(self):
        stale = {
            "sampled_at_ms": 1_700_000_000_000,
            "process_id": 4242,
            "cpu_time_ticks": 100,
            "memory_rss_bytes": 64 * 1024 * 1024,
        }
        with self.assertRaisesRegex(ValueError, "stale resource"):
            self.analyze(
                "stationary",
                [sample(0), sample(60_000, resource=stale)],
            )

    def test_square_series_requires_runs_one_two_and_three(self):
        profile = candidate_profile()
        repository = pathlib.Path(__file__).resolve().parents[4]
        analyzer = repository / "implementations/waveshare-ugv/calibration/analyze.py"
        with tempfile.TemporaryDirectory() as directory:
            directory_path = pathlib.Path(directory)
            profile_path = directory_path / "profile.json"
            profile_path.write_text(json.dumps(profile))
            run = write_capture(
                directory_path,
                "square-1.jsonl",
                profile,
                "square",
                square_samples(),
                run_index=1,
                expected_side_m=1.0,
            )

            result = subprocess.run(
                [sys.executable, str(analyzer), "--profile", str(profile_path), str(run)],
                capture_output=True,
                text=True,
                check=False,
            )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn('"series_complete": false', result.stdout)

    def test_square_series_accepts_exact_runs_one_two_and_three(self):
        profile = candidate_profile()
        repository = pathlib.Path(__file__).resolve().parents[4]
        analyzer = repository / "implementations/waveshare-ugv/calibration/analyze.py"
        with tempfile.TemporaryDirectory() as directory:
            directory_path = pathlib.Path(directory)
            profile_path = directory_path / "profile.json"
            profile_path.write_text(json.dumps(profile))
            runs = [
                write_capture(
                    directory_path,
                    f"square-{run_index}.jsonl",
                    profile,
                    "square",
                    square_samples(),
                    run_index=run_index,
                    expected_side_m=1.0,
                )
                for run_index in (1, 2, 3)
            ]

            result = subprocess.run(
                [
                    sys.executable,
                    str(analyzer),
                    "--profile",
                    str(profile_path),
                    *(str(run) for run in runs),
                ],
                capture_output=True,
                text=True,
                check=False,
            )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('"series_complete": true', result.stdout)


if __name__ == "__main__":
    unittest.main()
