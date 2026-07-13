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


def capture_duration_secs(samples: list[dict[str, Any]]) -> float:
    first_ms = int(samples[0]["telemetry"]["ts_ms"])
    last_ms = int(samples[-1]["telemetry"]["ts_ms"])
    if last_ms < first_ms:
        raise ValueError("capture sample timestamps are out of order")
    return (last_ms - first_ms) / 1000.0


def maximum_pose_excursion(
    poses: list[dict[str, Any]],
) -> tuple[float, float]:
    first = poses[0]
    max_distance = max(distance(first, current) for current in poses)
    max_yaw_deg = max(
        abs(math.degrees(angle_delta(float(first["yaw_rad"]), float(current["yaw_rad"]))))
        for current in poses
    )
    return max_distance, max_yaw_deg


def straight_lateral_deviation(poses: list[dict[str, Any]]) -> float:
    first = poses[0]
    yaw = float(first["yaw_rad"])
    sin_yaw = math.sin(yaw)
    cos_yaw = math.cos(yaw)
    return max(
        abs(
            -sin_yaw * (float(current["x_m"]) - float(first["x_m"]))
            + cos_yaw * (float(current["y_m"]) - float(first["y_m"]))
        )
        for current in poses
    )


def square_path_metrics(poses: list[dict[str, Any]]) -> dict[str, Any]:
    side_lengths: list[float] = []
    corner_turns: list[float] = []
    current_side = 0.0
    current_turn = 0.0
    mode = "side"
    ordered = True
    localized_travel = 0.0

    for previous, current in zip(poses, poses[1:]):
        step_distance = distance(previous, current)
        turn_deg = math.degrees(
            angle_delta(float(previous["yaw_rad"]), float(current["yaw_rad"]))
        )
        localized_travel += step_distance
        moving = step_distance >= 0.05
        turning = abs(turn_deg) >= 5.0
        if moving and turning:
            ordered = False
        if moving:
            if mode == "turn":
                corner_turns.append(current_turn)
                current_turn = 0.0
                mode = "side"
            current_side += step_distance
        elif turning:
            if mode == "side":
                side_lengths.append(current_side)
                current_side = 0.0
                mode = "turn"
            current_turn += turn_deg
        elif mode == "side":
            current_side += step_distance

    if mode == "side":
        side_lengths.append(current_side)
    else:
        corner_turns.append(current_turn)

    same_direction = bool(corner_turns) and (
        all(turn > 0.0 for turn in corner_turns)
        or all(turn < 0.0 for turn in corner_turns)
    )
    return {
        "side_lengths_m": side_lengths,
        "corner_turns_deg": corner_turns,
        "localized_travel_m": localized_travel,
        "ordered": ordered,
        "same_turn_direction": same_direction,
    }


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


def verified_zero_evidence(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ValueError(f"{label} is missing typed verified zero evidence")
    integer_fields = (
        "command_sequence",
        "write_completed_at_ms",
        "adapter_sample_sequence",
        "confirmation_received_at_ms",
    )
    if any(
        isinstance(value.get(field), bool)
        or not isinstance(value.get(field), int)
        or int(value[field]) < 0
        for field in integer_fields
    ):
        raise ValueError(f"{label} contains invalid verified zero counters")
    if value.get("acknowledged") is not True:
        raise ValueError(f"{label} does not acknowledge verified zero")
    if value.get("statement") != "zero command confirmed":
        raise ValueError(f"{label} has an invalid verified zero statement")
    if not isinstance(value.get("source"), str) or not value["source"].strip():
        raise ValueError(f"{label} has no verified zero source")
    if value["confirmation_received_at_ms"] < value["write_completed_at_ms"]:
        raise ValueError(f"{label} confirms verified zero before the write completed")
    return value


def resource_age_ms(event: dict[str, Any]) -> int:
    resource = event["telemetry"].get("resource")
    if not isinstance(resource, dict):
        raise ValueError("capture sample is missing a resource sample")
    sampled_at_ms = resource.get("sampled_at_ms")
    process_id = resource.get("process_id")
    if (
        isinstance(sampled_at_ms, bool)
        or not isinstance(sampled_at_ms, int)
        or isinstance(process_id, bool)
        or not isinstance(process_id, int)
        or process_id <= 0
    ):
        raise ValueError("capture sample contains an invalid resource sample")
    counters = (resource.get("cpu_time_ticks"), resource.get("memory_rss_bytes"))
    if not any(
        not isinstance(value, bool) and isinstance(value, int) and value >= 0
        for value in counters
    ):
        raise ValueError("capture sample has no measured process resources")
    age_ms = int(event["telemetry"]["ts_ms"]) - sampled_at_ms
    if not 0 <= age_ms <= 1000:
        raise ValueError("capture sample contains a stale resource sample")
    return age_ms


def generic_metrics(
    header: dict[str, Any], samples: list[dict[str, Any]], end: dict[str, Any]
) -> dict[str, Any]:
    initial_zero = verified_zero_evidence(header.get("verified_zero"), "capture start")
    final_zero = verified_zero_evidence(end.get("verified_zero"), "capture end")
    if final_zero["command_sequence"] <= initial_zero["command_sequence"]:
        raise ValueError("capture end verified zero command is not newer than capture start")
    if final_zero["adapter_sample_sequence"] <= initial_zero["adapter_sample_sequence"]:
        raise ValueError("capture end verified zero sample is not newer than capture start")
    calibration_status_recorded = all(
        isinstance(event.get("calibration"), dict)
        and event["calibration"].get("active") is True
        and event["calibration"].get("phase") == header.get("phase")
        and event["calibration"].get("run_index") == header.get("run_index")
        and event["calibration"].get("calibration_sha256")
        == header.get("calibration_sha256")
        for event in samples
    )
    if not calibration_status_recorded:
        raise ValueError("capture is missing the active calibration status on every sample")
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
    resource_ages = [resource_age_ms(event) for event in samples]
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
            calibration_status_recorded,
            len(restarts) == 1,
            max(resource_ages) <= 1000,
            max(lidar_ages) <= 1000,
            max(imu_ages) <= 1000,
            covariance_recorded,
            end.get("ok") is True,
            initial_zero["acknowledged"] is True,
            final_zero["acknowledged"] is True,
        )
    )
    return {
        "accepted": accepted,
        "health_ok": health_ok,
        "tracking": tracking,
        "sensors_available": sensors_ok,
        "container_healthy": container_ok,
        "calibration_status_recorded": calibration_status_recorded,
        "container_restarts": max(restarts) - min(restarts) if restarts else None,
        "max_resource_age_ms": max(resource_ages),
        "max_lidar_age_ms": max(lidar_ages),
        "max_imu_age_ms": max(imu_ages),
        "covariance_recorded": covariance_recorded,
        "stop_events": {"initial": initial_zero, "final": final_zero},
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
    poses = [pose(sample_event) for sample_event in samples]
    first_pose, last_pose = poses[0], poses[-1]
    generic = generic_metrics(header, samples, end)
    replay = replay_metrics(path, end)
    phase = header["phase"]
    wheels = profile["wheels"]
    left_start = float(samples[0]["telemetry"]["odometry_left"])
    right_start = float(samples[0]["telemetry"]["odometry_right"])
    left_delta = float(samples[-1]["telemetry"]["odometry_left"]) - left_start
    right_delta = float(samples[-1]["telemetry"]["odometry_right"]) - right_start
    raw_wheel_distance = abs((left_delta + right_delta) / 2.0)
    wheel_distance = raw_wheel_distance * float(wheels["distance_scale"])
    localized_distance = distance(first_pose, last_pose)
    yaw_error_deg = abs(
        math.degrees(
            angle_delta(float(first_pose["yaw_rad"]), float(last_pose["yaw_rad"]))
        )
    )

    phase_metrics: dict[str, Any]
    if phase == "stationary":
        duration_secs = capture_duration_secs(samples)
        max_excursion_m, max_excursion_deg = maximum_pose_excursion(poses)
        phase_metrics = {
            "duration_secs": duration_secs,
            "max_excursion_m": max_excursion_m,
            "max_excursion_deg": max_excursion_deg,
            "drift_m": max_excursion_m,
            "drift_deg": max_excursion_deg,
            "accepted": duration_secs >= 60.0
            and max_excursion_m <= 0.05
            and max_excursion_deg <= 2.0,
        }
    elif phase == "straight":
        expected = float(header["expected_distance_m"])
        max_lateral_deviation = straight_lateral_deviation(poses)
        phase_metrics = {
            "expected_distance_m": expected,
            "wheel_distance_m": wheel_distance,
            "localized_distance_m": localized_distance,
            "max_lateral_deviation_m": max_lateral_deviation,
            "recommended_distance_scale": (
                expected / raw_wheel_distance if raw_wheel_distance > 0.0 else None
            ),
            "accepted": abs(expected - 1.0) <= 0.001
            and 0.90 <= wheel_distance <= 1.10
            and 0.90 <= localized_distance <= 1.10
            and max_lateral_deviation <= 0.15,
        }
    elif phase == "turn":
        expected = float(header["expected_turn_deg"])
        wheel_turn_signed = math.degrees(
            (right_delta - left_delta)
            * float(wheels["distance_scale"])
            / float(wheels["track_width_m"])
        )
        wheel_turn = abs(wheel_turn_signed)
        turn_steps = [
            math.degrees(
                angle_delta(float(left["yaw_rad"]), float(right["yaw_rad"]))
            )
            for left, right in zip(poses, poses[1:])
        ]
        localized_turn_signed = sum(turn_steps)
        localized_turn = abs(localized_turn_signed)
        direction = 1.0 if localized_turn_signed >= 0.0 else -1.0
        backtracking = sum(abs(step) for step in turn_steps if step * direction < 0.0)
        max_translation, _ = maximum_pose_excursion(poses)
        phase_metrics = {
            "expected_turn_deg": expected,
            "wheel_turn_deg": wheel_turn,
            "wheel_turn_signed_deg": wheel_turn_signed,
            "localized_turn_deg": localized_turn,
            "localized_turn_signed_deg": localized_turn_signed,
            "backtracking_deg": backtracking,
            "max_translation_m": max_translation,
            "wheel_error_deg": abs(wheel_turn - expected),
            "localization_error_deg": abs(localized_turn - expected),
            "recommended_track_width_m": abs(
                (right_delta - left_delta)
                * float(wheels["distance_scale"])
                / math.radians(expected)
            ),
            "accepted": abs(expected - 360.0) <= 0.001
            and abs(wheel_turn - expected) <= 10.0
            and abs(localized_turn - expected) <= 10.0
            and backtracking <= 10.0
            and max_translation <= 0.20,
        }
    elif phase == "square":
        expected_side = float(header["expected_side_m"])
        path_metrics = square_path_metrics(poses)
        side_lengths = path_metrics["side_lengths_m"]
        corner_turns = path_metrics["corner_turns_deg"]
        phase_metrics = {
            "expected_side_m": expected_side,
            **path_metrics,
            "closure_m": localized_distance,
            "closure_deg": yaw_error_deg,
            "accepted": abs(expected_side - 1.0) <= 0.001
            and path_metrics["ordered"]
            and path_metrics["same_turn_direction"]
            and len(side_lengths) == 4
            and all(0.90 <= side <= 1.10 for side in side_lengths)
            and len(corner_turns) == 4
            and all(75.0 <= abs(turn) <= 105.0 for turn in corner_turns)
            and 3.60 <= path_metrics["localized_travel_m"] <= 4.40
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
        if phase == "square" or args.require_complete_series:
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
