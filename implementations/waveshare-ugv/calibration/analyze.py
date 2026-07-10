#!/usr/bin/env python3
"""Analyze scrubbed Pinkie calibration captures without ROS or robot hardware."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import pathlib
import sys
from statistics import fmean
from typing import Any

from profile import canonical_bytes, validate

CAPTURE_FORMAT = "leash-waveshare-ugv-calibration-capture-v1"


def angle_delta(left: float, right: float) -> float:
    return math.atan2(math.sin(right - left), math.cos(right - left))


def pose(sample: dict[str, Any]) -> dict[str, Any]:
    value = sample["telemetry"]["localization"].get("pose")
    if not isinstance(value, dict) or not isinstance(value.get("pose"), dict):
        raise ValueError("sample is missing a localized pose")
    covariance = value.get("covariance")
    if not isinstance(covariance, list) or len(covariance) != 9:
        raise ValueError("sample is missing 3x3 localization covariance")
    return value["pose"]


def distance(first: dict[str, Any], last: dict[str, Any]) -> float:
    return math.hypot(float(last["x_m"]) - float(first["x_m"]), float(last["y_m"]) - float(first["y_m"]))


def mask_contains(angle_deg: float, mask: str) -> bool:
    start, end = (float(value) for value in mask.split(":"))
    angle = (angle_deg + 180.0) % 360.0 - 180.0
    start = (start + 180.0) % 360.0 - 180.0
    end = (end + 180.0) % 360.0 - 180.0
    return start <= angle <= end if start <= end else angle >= start or angle <= end


def body_mask_metrics(samples: list[dict[str, Any]], masks: list[str]) -> dict[str, Any]:
    checked = 0
    violations = 0
    for event in samples:
        scan = event["telemetry"]["sensors"]["range_scan"].get("sample")
        if not isinstance(scan, dict):
            continue
        ranges = scan.get("ranges_m", [])
        minimum = float(scan["angle_min_rad"])
        increment = float(scan["angle_increment_rad"])
        for index, value in enumerate(ranges):
            angle_deg = math.degrees(minimum + index * increment)
            if any(mask_contains(angle_deg, mask) for mask in masks):
                checked += 1
                violations += value is not None
    return {
        "configured": bool(masks),
        "masked_bins_checked": checked,
        "masked_bin_violations": violations,
        "accepted": not masks or (checked > 0 and violations == 0),
    }


def residual_imu(samples: list[dict[str, Any]]) -> dict[str, Any]:
    accelerations: list[tuple[float, float, float]] = []
    gyros: list[tuple[float, float, float]] = []
    for event in samples:
        sample = event["telemetry"]["sensors"]["imu"].get("sample")
        if not isinstance(sample, dict):
            continue
        acceleration = sample["linear_acceleration_mps2"]
        gyro = sample["angular_velocity_radps"]
        accelerations.append(tuple(float(acceleration[axis]) for axis in ("x", "y", "z")))
        gyros.append(tuple(float(gyro[axis]) for axis in ("x", "y", "z")))
    if not accelerations:
        raise ValueError("capture contains no IMU samples")
    mean_acceleration = [fmean(value[index] for value in accelerations) for index in range(3)]
    mean_gyro = [fmean(value[index] for value in gyros) for index in range(3)]
    return {
        "mean_acceleration_mps2": mean_acceleration,
        "mean_acceleration_norm_mps2": math.sqrt(sum(value * value for value in mean_acceleration)),
        "mean_angular_velocity_radps": mean_gyro,
        "mean_angular_velocity_norm_radps": math.sqrt(sum(value * value for value in mean_gyro)),
    }


def load_capture(path: pathlib.Path) -> tuple[dict[str, Any], list[dict[str, Any]], dict[str, Any]]:
    events = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
    if len(events) < 3:
        raise ValueError(f"{path} does not contain a complete capture")
    header, end = events[0], events[-1]
    if header.get("kind") != "capture-start" or header.get("format") != CAPTURE_FORMAT:
        raise ValueError(f"{path} has an unsupported capture header")
    samples = [event for event in events[1:-1] if event.get("kind") == "sample"]
    if not samples or end.get("kind") != "capture-end":
        raise ValueError(f"{path} does not contain samples and a capture end")
    return header, samples, end


def generic_metrics(samples: list[dict[str, Any]], end: dict[str, Any]) -> dict[str, Any]:
    health_ok = all(event["health"].get("ok") is True for event in samples)
    tracking = all(event["provider"].get("state") == "tracking" for event in samples)
    sensors_ok = all(
        event["telemetry"]["sensors"][sensor].get("status") == "available"
        for event in samples
        for sensor in ("range_scan", "imu")
    )
    container_ok = all(
        event["container"].get("running") is True
        and event["container"].get("oom_killed") is False
        for event in samples
    )
    restarts = {event["container"].get("restart_count") for event in samples}
    lidar_ages = [
        int(event["telemetry"]["ts_ms"])
        - int(event["telemetry"]["sensors"]["range_scan"]["last_ms"])
        for event in samples
    ]
    imu_ages = [
        int(event["telemetry"]["ts_ms"])
        - int(event["telemetry"]["sensors"]["imu"]["last_ms"])
        for event in samples
    ]
    covariance_recorded = all(
        len(event["telemetry"]["localization"]["pose"]["covariance"]) == 9
        for event in samples
    )
    accepted = all(
        (
            health_ok,
            tracking,
            sensors_ok,
            container_ok,
            len(restarts) == 1,
            max(lidar_ages) <= 1000,
            max(imu_ages) <= 1000,
            covariance_recorded,
            end.get("ok") is True,
            end.get("final_motor_stop") is True,
        )
    )
    return {
        "accepted": accepted,
        "health_ok": health_ok,
        "tracking": tracking,
        "sensors_available": sensors_ok,
        "container_healthy": container_ok,
        "container_restarts": max(restarts) - min(restarts) if restarts else None,
        "max_lidar_age_ms": max(lidar_ages),
        "max_imu_age_ms": max(imu_ages),
        "covariance_recorded": covariance_recorded,
        "stop_events": {"initial": True, "final": end.get("final_motor_stop") is True},
        "resource_samples": len(samples),
    }


def replay_metrics(capture_path: pathlib.Path, end: dict[str, Any]) -> dict[str, Any]:
    metadata = end.get("replay")
    if not isinstance(metadata, dict) or metadata.get("format") != "leash-replay-v1":
        raise ValueError("capture end is missing leash-replay-v1 evidence")
    name = metadata.get("file")
    if not isinstance(name, str) or pathlib.Path(name).name != name:
        raise ValueError("replay evidence must use a scrubbed basename")
    path = capture_path.with_name(name)
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    if digest != metadata.get("sha256"):
        raise ValueError("replay evidence digest does not match capture metadata")
    events = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
    if not events or any(event.get("format") != "leash-replay-v1" for event in events):
        raise ValueError("replay evidence contains an invalid format")
    kinds = {event.get("kind") for event in events}
    if not {"telemetry", "sensors", "camera", "command"}.issubset(kinds):
        raise ValueError("replay evidence is missing required event kinds")
    telemetry_events = [event["data"] for event in events if event.get("kind") == "telemetry"]
    scrubbed = all(
        frame["telemetry"].get("session_id") is None
        and frame["telemetry"].get("workers") == []
        and frame["telemetry"].get("resource") is None
        and frame["telemetry"]["sensors"]["raw_frame"].get("payload") is None
        and frame["telemetry"]["sensors"]["camera"].get("stream_url") is None
        and frame["telemetry"]["sensors"]["camera"].get("snapshot_url") is None
        and frame["command"].get("session_id") is None
        and frame["health"].get("physical_actuation_enabled") is False
        and frame["health"].get("physical_navigation_enabled") is False
        and frame["safety"].get("physical_actuation_enabled") is False
        and frame["safety"].get("physical_navigation_enabled") is False
        for frame in telemetry_events
    )
    return {
        "file": name,
        "sha256": digest,
        "events": len(events),
        "telemetry_frames": len(telemetry_events),
        "scrubbed": scrubbed,
        "accepted": bool(telemetry_events) and scrubbed,
    }


def analyze_one(path: pathlib.Path, profile: dict[str, Any]) -> dict[str, Any]:
    header, samples, end = load_capture(path)
    profile_digest = hashlib.sha256(canonical_bytes(profile)).hexdigest()
    if header.get("profile") != profile["profile"] or header.get("calibration_sha256") != profile_digest:
        raise ValueError(f"{path} was not captured with the supplied calibration profile")
    first_pose, last_pose = pose(samples[0]), pose(samples[-1])
    generic = generic_metrics(samples, end)
    replay = replay_metrics(path, end)
    phase = header["phase"]
    wheels = profile["wheels"]
    left_start = float(samples[0]["telemetry"]["odometry_left"])
    right_start = float(samples[0]["telemetry"]["odometry_right"])
    left_delta = float(samples[-1]["telemetry"]["odometry_left"]) - left_start
    right_delta = float(samples[-1]["telemetry"]["odometry_right"]) - right_start
    wheel_distance = abs((left_delta + right_delta) / 2.0 * float(wheels["distance_scale"]))
    localized_distance = distance(first_pose, last_pose)
    yaw_error_deg = abs(math.degrees(angle_delta(float(first_pose["yaw_rad"]), float(last_pose["yaw_rad"]))))

    phase_metrics: dict[str, Any]
    if phase == "stationary":
        phase_metrics = {
            "duration_secs": header["duration_secs"],
            "drift_m": localized_distance,
            "drift_deg": yaw_error_deg,
            "accepted": header["duration_secs"] >= 60 and localized_distance <= 0.05 and yaw_error_deg <= 2.0,
        }
    elif phase == "straight":
        expected = float(header["expected_distance_m"])
        wheel_error = abs(wheel_distance - expected) / expected
        localization_error = abs(localized_distance - expected) / expected
        phase_metrics = {
            "expected_distance_m": expected,
            "wheel_distance_m": wheel_distance,
            "localized_distance_m": localized_distance,
            "wheel_error_fraction": wheel_error,
            "localization_error_fraction": localization_error,
            "recommended_distance_scale": expected / abs((left_delta + right_delta) / 2.0),
            "accepted": wheel_error <= 0.10 and localization_error <= 0.10,
        }
    elif phase == "turn":
        expected = float(header["expected_turn_deg"])
        wheel_turn = abs(
            math.degrees(
                (right_delta - left_delta)
                * float(wheels["distance_scale"])
                / float(wheels["track_width_m"])
            )
        )
        poses = [pose(sample) for sample in samples]
        localized_turn = abs(
            sum(
                math.degrees(angle_delta(float(left["yaw_rad"]), float(right["yaw_rad"])))
                for left, right in zip(poses, poses[1:])
            )
        )
        phase_metrics = {
            "expected_turn_deg": expected,
            "wheel_turn_deg": wheel_turn,
            "localized_turn_deg": localized_turn,
            "wheel_error_deg": abs(wheel_turn - expected),
            "localization_error_deg": abs(localized_turn - expected),
            "recommended_track_width_m": abs(
                (right_delta - left_delta) * float(wheels["distance_scale"]) / math.radians(expected)
            ),
            "accepted": abs(wheel_turn - expected) <= 10.0 and abs(localized_turn - expected) <= 10.0,
        }
    elif phase == "square":
        expected_side = float(header["expected_side_m"])
        phase_metrics = {
            "expected_side_m": expected_side,
            "closure_m": localized_distance,
            "closure_deg": yaw_error_deg,
            "accepted": abs(expected_side - 1.0) <= 0.001
            and localized_distance <= 0.25
            and yaw_error_deg <= 15.0,
        }
    else:
        raise ValueError(f"unsupported phase {phase}")

    accepted = generic["accepted"] and replay["accepted"] and phase_metrics["accepted"]
    if phase == "stationary":
        mask_metrics = body_mask_metrics(samples, profile["lidar"]["body_masks_deg"])
        accepted = accepted and mask_metrics["accepted"]
    else:
        mask_metrics = body_mask_metrics(samples, profile["lidar"]["body_masks_deg"])
    return {
        "capture": path.name,
        "evidence_sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "phase": phase,
        "run_index": header["run_index"],
        "samples": len(samples),
        "generic": generic,
        "replay": replay,
        "phase_metrics": phase_metrics,
        "body_masks": mask_metrics,
        "imu_residual": residual_imu(samples),
        "accepted": accepted,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--profile", required=True, type=pathlib.Path)
    parser.add_argument("--output", type=pathlib.Path)
    parser.add_argument("--require-complete-series", action="store_true")
    parser.add_argument("captures", nargs="+", type=pathlib.Path)
    args = parser.parse_args()
    try:
        profile = validate(json.loads(args.profile.read_text()), require_values=True)
        runs = [analyze_one(path, profile) for path in args.captures]
        phases = {run["phase"] for run in runs}
        if len(phases) != 1:
            raise ValueError("all captures in one analysis must use the same phase")
        phase = runs[0]["phase"]
        series_complete = phase != "square" or sorted(run["run_index"] for run in runs) == [1, 2, 3]
        accepted = all(run["accepted"] for run in runs)
        if args.require_complete_series:
            accepted = accepted and series_complete
        result = {
            "ok": accepted,
            "format": "leash-waveshare-ugv-calibration-analysis-v1",
            "profile": profile["profile"],
            "phase": phase,
            "series_complete": series_complete,
            "runs": runs,
        }
        rendered = json.dumps(result, indent=2, sort_keys=True) + "\n"
        if args.output:
            args.output.write_text(rendered)
        else:
            print(rendered, end="")
        return 0 if accepted else 1
    except (OSError, json.JSONDecodeError, KeyError, TypeError, ValueError, ZeroDivisionError) as error:
        print(f"calibration analysis error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
