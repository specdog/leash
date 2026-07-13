from __future__ import annotations

import copy
import hashlib
import json
import math
import pathlib

from profile import canonical_bytes
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
    *,
    x_m: float = 0.0,
    y_m: float = 0.0,
    yaw_rad: float = 0.0,
    odometry_left: float | None = None,
    odometry_right: float | None = None,
    resource: dict | None | bool = True,
) -> dict:
    ts_ms = 1_700_000_000_000 + elapsed_ms
    pose = {
        "pose": {
            "ts_ms": ts_ms,
            "frame_id": "map",
            "x_m": x_m,
            "y_m": y_m,
            "yaw_rad": yaw_rad,
        },
        "covariance": [0.01, 0.0, 0.0, 0.0, 0.01, 0.0, 0.0, 0.0, 0.01],
    }
    if resource is True:
        resource = {
            "sampled_at_ms": ts_ms,
            "process_id": 4242,
            "cpu_time_ticks": 100 + elapsed_ms,
            "memory_rss_bytes": 64 * 1024 * 1024,
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
            "resource": resource,
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


def square_samples(
    *,
    side_lengths: tuple[float, float, float, float] = (1.0, 1.0, 1.0, 1.0),
    turn_degrees: tuple[float, float, float, float] = (90.0, 90.0, 90.0, 90.0),
) -> list[dict]:
    samples = [sample(0)]
    elapsed_ms = 0
    x_m = 0.0
    y_m = 0.0
    yaw_rad = 0.0
    odometry = 0.0
    for side, turn in zip(side_lengths, turn_degrees):
        elapsed_ms += 4_000
        x_m += side * math.cos(yaw_rad)
        y_m += side * math.sin(yaw_rad)
        odometry += side
        samples.append(
            sample(
                elapsed_ms,
                x_m=x_m,
                y_m=y_m,
                yaw_rad=yaw_rad,
                odometry_left=odometry,
                odometry_right=odometry,
            )
        )
        elapsed_ms += 1_000
        yaw_rad += math.radians(turn)
        samples.append(
            sample(
                elapsed_ms,
                x_m=x_m,
                y_m=y_m,
                yaw_rad=yaw_rad,
                odometry_left=odometry,
                odometry_right=odometry,
            )
        )
    return samples


def write_capture(
    directory: pathlib.Path,
    name: str,
    profile: dict,
    phase: str,
    samples: list[dict],
    *,
    run_index: int = 1,
    duration_secs: int = 60,
    expected_distance_m: float | None = None,
    expected_turn_deg: float = 360.0,
    expected_side_m: float | None = None,
    include_calibration_status: bool = True,
) -> pathlib.Path:
    digest = hashlib.sha256(canonical_bytes(profile)).hexdigest()
    capture_samples = copy.deepcopy(samples)
    if include_calibration_status:
        for event in capture_samples:
            event["calibration"] = {
                "active": True,
                "phase": phase,
                "run_index": run_index,
                "calibration_sha256": digest,
            }

    repository = pathlib.Path(__file__).resolve().parents[4]
    source_event = next(
        json.loads(line)
        for line in (repository / "examples/replay/sim-mapping.jsonl").read_text().splitlines()
        if json.loads(line)["kind"] == "telemetry"
    )
    replay = replay_events([source_event["data"]])
    replay_text = "\n".join(json.dumps(event) for event in replay) + "\n"
    replay_name = f"{pathlib.Path(name).stem}-replay.jsonl"
    replay_digest = hashlib.sha256(replay_text.encode()).hexdigest()
    events = [
        {
            "kind": "capture-start",
            "format": "leash-waveshare-ugv-calibration-capture-v1",
            "phase": phase,
            "profile": profile["profile"],
            "calibration_sha256": digest,
            "duration_secs": duration_secs,
            "run_index": run_index,
            "expected_distance_m": expected_distance_m,
            "expected_turn_deg": expected_turn_deg,
            "expected_side_m": expected_side_m,
            "verified_zero": verified_zero(run_index * 2 - 1, run_index * 10),
        },
        *capture_samples,
        {
            "kind": "capture-end",
            "ok": True,
            "verified_zero": verified_zero(run_index * 2, run_index * 10 + 1),
            "replay": {
                "file": replay_name,
                "sha256": replay_digest,
                "format": "leash-replay-v1",
            },
        },
    ]
    path = directory / name
    path.write_text("\n".join(json.dumps(event) for event in events) + "\n")
    path.with_name(replay_name).write_text(replay_text)
    return path
