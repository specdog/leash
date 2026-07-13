from __future__ import annotations

import copy
import json
import pathlib
import unittest

from replay import normalize_and_scrub


def telemetry_stream_frame(ts_ms: int) -> dict:
    repository = pathlib.Path(__file__).resolve().parents[4]
    event = next(
        json.loads(line)
        for line in (repository / "examples/replay/sim-mapping.jsonl").read_text().splitlines()
        if json.loads(line)["kind"] == "telemetry"
    )
    frame = copy.deepcopy(event["data"])
    frame["ts_ms"] = ts_ms
    frame["telemetry"]["ts_ms"] = ts_ms
    return frame


def path_fixture(ts_ms: int) -> dict:
    return {
        "ts_ms": ts_ms,
        "frame_id": "map",
        "poses": [
            {
                "ts_ms": ts_ms,
                "frame_id": "map",
                "x_m": 0.25,
                "y_m": -0.5,
                "yaw_rad": 0.125,
            }
        ],
    }


def voxel_fixture(ts_ms: int) -> dict:
    return {
        "version": "leash-voxel-grid-v1",
        "ts_ms": ts_ms,
        "frame_id": "map",
        "width": 2,
        "height": 2,
        "depth": 1,
        "resolution_m": 0.1,
        "origin": {
            "ts_ms": ts_ms,
            "frame_id": "map",
            "x_m": -0.1,
            "y_m": -0.1,
            "yaw_rad": 0.0,
        },
        "origin_z_m": 0.0,
        "source": "costmap-extruded",
        "observed_3d": False,
        "voxels": [{"x": 1, "y": 0, "z": 0, "occupancy": 100}],
    }


class ReplayTests(unittest.TestCase):
    def test_normalization_rebases_path_and_voxel_timestamps(self):
        frame = telemetry_stream_frame(ts_ms=1_700_000_000_000)
        frame["telemetry"]["path"] = path_fixture(1_700_000_000_000)
        frame["telemetry"]["voxel_grid"] = voxel_fixture(1_700_000_000_000)

        normalized = normalize_and_scrub(frame, 25)

        self.assertEqual(normalized["telemetry"]["path"]["ts_ms"], 25)
        self.assertEqual(normalized["telemetry"]["path"]["poses"][0]["ts_ms"], 25)
        self.assertEqual(normalized["telemetry"]["voxel_grid"]["ts_ms"], 25)
        self.assertEqual(normalized["telemetry"]["voxel_grid"]["origin"]["ts_ms"], 25)
        self.assertEqual(normalized["telemetry"]["voxel_grid"]["source"], "costmap-extruded")
        self.assertFalse(normalized["telemetry"]["voxel_grid"]["observed_3d"])
        self.assertEqual(normalized["visualization"]["path"], normalized["telemetry"]["path"])
        self.assertEqual(
            normalized["visualization"]["voxel_grid"],
            normalized["telemetry"]["voxel_grid"],
        )

    def test_normalization_preserves_observed_3d_provenance(self):
        frame = telemetry_stream_frame(ts_ms=1_700_000_000_000)
        frame["telemetry"]["voxel_grid"] = voxel_fixture(1_700_000_000_000)
        frame["telemetry"]["voxel_grid"]["observed_3d"] = True

        normalized = normalize_and_scrub(frame, 25)

        self.assertTrue(normalized["telemetry"]["voxel_grid"]["observed_3d"])
        self.assertTrue(normalized["visualization"]["voxel_grid"]["observed_3d"])


if __name__ == "__main__":
    unittest.main()
