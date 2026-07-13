#!/usr/bin/env python3
"""Validate a versioned Waveshare UGV calibration profile and emit safe env values."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import pathlib
import re
import sys
from typing import Any

SCHEMA = "leash-waveshare-ugv-calibration-v1"
PROFILE_RE = re.compile(r"^[a-z0-9][a-z0-9._-]{0,63}$")
AXIS_RE = re.compile(r"^[+-]?[xyz]$")


def fail(message: str) -> None:
    raise ValueError(message)


def finite(value: Any, field: str, *, positive: bool = False) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        fail(f"{field} must be a number")
    result = float(value)
    if not math.isfinite(result) or (positive and result <= 0.0):
        qualifier = "positive " if positive else "finite "
        fail(f"{field} must be a {qualifier}number")
    return result


def vector(value: Any, field: str) -> list[float]:
    if not isinstance(value, list) or len(value) != 3:
        fail(f"{field} must contain three numbers")
    return [finite(component, f"{field}[{index}]") for index, component in enumerate(value)]


def transform(value: Any, field: str) -> dict[str, float]:
    if not isinstance(value, dict):
        fail(f"{field} must be an object")
    return {
        key: finite(value.get(key), f"{field}.{key}")
        for key in ("x_m", "y_m", "z_m", "yaw_rad")
    }


def validate(
    profile: Any, *, require_accepted: bool = False, require_values: bool = False
) -> dict[str, Any]:
    if not isinstance(profile, dict) or profile.get("schema") != SCHEMA:
        fail(f"schema must be {SCHEMA}")
    name = profile.get("profile")
    if not isinstance(name, str) or not PROFILE_RE.fullmatch(name):
        fail("profile must be a short lowercase identifier")
    status = profile.get("status")
    if status not in {"unmeasured", "candidate", "accepted"}:
        fail("status must be unmeasured, candidate, or accepted")
    if require_accepted and status != "accepted":
        fail("profile is not accepted; physical evidence must replace every placeholder first")
    if status == "unmeasured":
        if require_values:
            fail("profile is unmeasured; fill every value and mark it candidate first")
        return profile

    measurement = profile.get("measurement")
    if not isinstance(measurement, dict):
        fail("measurement must be an object")
    if measurement.get("procedure_revision") != "issue-166-v1":
        fail("measurement.procedure_revision must be issue-166-v1")
    measured_at = measurement.get("measured_at")
    if not isinstance(measured_at, str) or not measured_at.strip():
        fail("measurement.measured_at is required for a candidate or accepted profile")
    if "evidence_sha256" in measurement:
        fail("measurement.evidence_sha256 is legacy; use acceptance_manifest_sha256")
    manifest_digest = measurement.get("acceptance_manifest_sha256")
    if status == "accepted" and manifest_digest is None:
        fail("an accepted profile requires acceptance_manifest_sha256")
    if manifest_digest is not None and (
        not isinstance(manifest_digest, str)
        or not re.fullmatch(r"[0-9a-f]{64}", manifest_digest)
    ):
        fail("measurement.acceptance_manifest_sha256 must be a lowercase SHA-256 digest or null")

    wheels = profile.get("wheels")
    if not isinstance(wheels, dict):
        fail("wheels must be an object")
    finite(wheels.get("track_width_m"), "wheels.track_width_m", positive=True)
    finite(wheels.get("distance_scale"), "wheels.distance_scale", positive=True)

    lidar = profile.get("lidar")
    if not isinstance(lidar, dict):
        fail("lidar must be an object")
    transform(lidar.get("transform"), "lidar.transform")
    finite(lidar.get("yaw_offset_deg"), "lidar.yaw_offset_deg")
    if not isinstance(lidar.get("clockwise"), bool):
        fail("lidar.clockwise must be a boolean")
    masks = lidar.get("body_masks_deg")
    if not isinstance(masks, list):
        fail("lidar.body_masks_deg must be a list")
    for index, mask in enumerate(masks):
        if not isinstance(mask, str) or mask.count(":") != 1:
            fail(f"lidar.body_masks_deg[{index}] must be START:END")
        for endpoint in mask.split(":"):
            finite(float(endpoint), f"lidar.body_masks_deg[{index}]")

    camera = profile.get("camera")
    if not isinstance(camera, dict):
        fail("camera must be an object")
    transform(camera.get("transform"), "camera.transform")

    imu = profile.get("imu")
    if not isinstance(imu, dict):
        fail("imu must be an object")
    finite(imu.get("accel_lsb_per_g"), "imu.accel_lsb_per_g", positive=True)
    finite(imu.get("gyro_dps_per_lsb"), "imu.gyro_dps_per_lsb", positive=True)
    axis_map = imu.get("axis_map")
    if not isinstance(axis_map, str):
        fail("imu.axis_map must be a string")
    axes = [axis.strip() for axis in axis_map.split(",")]
    if len(axes) != 3 or any(not AXIS_RE.fullmatch(axis) for axis in axes):
        fail("imu.axis_map must contain three signed x/y/z axes")
    if len({axis[-1] for axis in axes}) != 3:
        fail("imu.axis_map must use each source axis once")
    vector(imu.get("accel_bias_mps2"), "imu.accel_bias_mps2")
    vector(imu.get("gyro_bias_radps"), "imu.gyro_bias_radps")
    return profile


def canonical_bytes(profile: dict[str, Any]) -> bytes:
    values = {
        "schema": profile["schema"],
        "profile": profile["profile"],
        "procedure_revision": profile["measurement"]["procedure_revision"],
        "wheels": profile["wheels"],
        "lidar": profile["lidar"],
        "camera": profile["camera"],
        "imu": profile["imu"],
    }
    return (json.dumps(values, sort_keys=True, separators=(",", ":")) + "\n").encode()


def env_lines(profile: dict[str, Any], target: str) -> list[str]:
    validate(profile, require_values=True)
    wheels = profile["wheels"]
    lidar = profile["lidar"]
    camera = profile["camera"]
    imu = profile["imu"]
    if target == "ros":
        values = {
            "LEASH_ROS_WHEEL_TRACK_M": wheels["track_width_m"],
            "LEASH_ROS_WHEEL_SCALE": wheels["distance_scale"],
            "LEASH_ROS_SCAN_X_M": lidar["transform"]["x_m"],
            "LEASH_ROS_SCAN_Y_M": lidar["transform"]["y_m"],
            "LEASH_ROS_SCAN_Z_M": lidar["transform"]["z_m"],
            "LEASH_ROS_SCAN_YAW_RAD": lidar["transform"]["yaw_rad"],
            "LEASH_ROS_CAMERA_X_M": camera["transform"]["x_m"],
            "LEASH_ROS_CAMERA_Y_M": camera["transform"]["y_m"],
            "LEASH_ROS_CAMERA_Z_M": camera["transform"]["z_m"],
            "LEASH_ROS_CAMERA_YAW_RAD": camera["transform"]["yaw_rad"],
        }
    else:
        values = {
            "LEASH_UGV_LIDAR_YAW_OFFSET_DEG": lidar["yaw_offset_deg"],
            "LEASH_UGV_LIDAR_CLOCKWISE": str(lidar["clockwise"]).lower(),
            "LEASH_UGV_LIDAR_BODY_MASKS_DEG": ",".join(lidar["body_masks_deg"]),
            "LEASH_UGV_IMU_ACCEL_LSB_PER_G": imu["accel_lsb_per_g"],
            "LEASH_UGV_IMU_GYRO_DPS_PER_LSB": imu["gyro_dps_per_lsb"],
            "LEASH_UGV_IMU_AXIS_MAP": imu["axis_map"],
            "LEASH_UGV_IMU_ACCEL_BIAS_MPS2": ",".join(map(str, imu["accel_bias_mps2"])),
            "LEASH_UGV_IMU_GYRO_BIAS_RADPS": ",".join(map(str, imu["gyro_bias_radps"])),
        }
    return [f"{key}={value}" for key, value in values.items()]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("profile", type=pathlib.Path)
    parser.add_argument("command", choices=("validate", "digest", "emit-env"))
    parser.add_argument("--require-accepted", action="store_true")
    parser.add_argument("--require-values", action="store_true")
    parser.add_argument("--target", choices=("leash", "ros"))
    args = parser.parse_args()
    try:
        profile = validate(
            json.loads(args.profile.read_text()),
            require_accepted=args.require_accepted,
            require_values=args.require_values,
        )
        if args.command == "digest":
            print(hashlib.sha256(canonical_bytes(profile)).hexdigest())
        elif args.command == "emit-env":
            if args.target is None:
                fail("emit-env requires --target leash or --target ros")
            print("\n".join(env_lines(profile, args.target)))
        else:
            print(json.dumps({"ok": True, "profile": profile["profile"], "status": profile["status"]}))
    except (OSError, json.JSONDecodeError, ValueError) as error:
        print(f"calibration profile error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
