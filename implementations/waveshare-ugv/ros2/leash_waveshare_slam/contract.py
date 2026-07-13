"""Pure contract conversion used by the ROS bridge and no-hardware tests."""

from __future__ import annotations

import math
from dataclasses import dataclass
from typing import Any

from .lineage import grid_revision

LOCALIZATION_HEARTBEAT_INTERVAL_S = 0.5


def localization_update_due(
    last_pose_ts_ms: int | None,
    pose_ts_ms: int,
    last_posted_at_s: float | None,
    now_s: float,
) -> bool:
    """Post new poses immediately and heartbeat unchanged stationary poses."""
    if last_pose_ts_ms != pose_ts_ms or last_posted_at_s is None:
        return True
    return now_s - last_posted_at_s >= LOCALIZATION_HEARTBEAT_INTERVAL_S


def normalize_angle(value: float) -> float:
    return math.atan2(math.sin(value), math.cos(value))


@dataclass
class DifferentialOdometry:
    track_width_m: float
    wheel_scale: float = 1.0
    x_m: float = 0.0
    y_m: float = 0.0
    yaw_rad: float = 0.0
    _left_m: float | None = None
    _right_m: float | None = None
    _ts_ms: int | None = None

    def __post_init__(self) -> None:
        if not math.isfinite(self.track_width_m) or self.track_width_m <= 0.0:
            raise ValueError("wheel track must be a positive finite measurement")
        if not math.isfinite(self.wheel_scale) or self.wheel_scale <= 0.0:
            raise ValueError("wheel scale must be a positive finite value")

    def update(self, left_m: float, right_m: float, ts_ms: int) -> dict[str, float]:
        left_m = float(left_m) * self.wheel_scale
        right_m = float(right_m) * self.wheel_scale
        ts_ms = int(ts_ms)
        if not all(math.isfinite(value) for value in (left_m, right_m)):
            raise ValueError("wheel odometry must be finite")

        if self._left_m is None or self._right_m is None or self._ts_ms is None:
            self._left_m = left_m
            self._right_m = right_m
            self._ts_ms = ts_ms
            return self._sample(ts_ms, 0.0, 0.0)

        dt_s = (ts_ms - self._ts_ms) / 1000.0
        if dt_s <= 0.0:
            return self._sample(ts_ms, 0.0, 0.0)

        delta_left = left_m - self._left_m
        delta_right = right_m - self._right_m
        self._left_m = left_m
        self._right_m = right_m
        self._ts_ms = ts_ms

        distance = (delta_left + delta_right) / 2.0
        delta_yaw = (delta_right - delta_left) / self.track_width_m
        midpoint_yaw = self.yaw_rad + delta_yaw / 2.0
        self.x_m += distance * math.cos(midpoint_yaw)
        self.y_m += distance * math.sin(midpoint_yaw)
        self.yaw_rad = normalize_angle(self.yaw_rad + delta_yaw)
        return self._sample(ts_ms, distance / dt_s, delta_yaw / dt_s)

    def _sample(self, ts_ms: int, linear_x_mps: float, angular_z_radps: float) -> dict[str, float]:
        return {
            "ts_ms": ts_ms,
            "x_m": self.x_m,
            "y_m": self.y_m,
            "yaw_rad": self.yaw_rad,
            "linear_x_mps": linear_x_mps,
            "angular_z_radps": angular_z_radps,
        }


def covariance_3x3(covariance_6x6: list[float]) -> list[float]:
    if len(covariance_6x6) != 36:
        raise ValueError("ROS pose covariance must contain 36 values")
    indices = (0, 1, 5)
    result = [float(covariance_6x6[row * 6 + column]) for row in indices for column in indices]
    if not all(math.isfinite(value) for value in result):
        raise ValueError("pose covariance must be finite")
    if any(result[index] < 0.0 for index in (0, 4, 8)):
        raise ValueError("pose covariance diagonal must be non-negative")
    return result


def laser_scan_contract(status: dict[str, Any]) -> dict[str, Any] | None:
    if status.get("status") != "available" or not status.get("sample"):
        return None
    sample = status["sample"]
    ranges = [math.inf if value is None else float(value) for value in sample["ranges_m"]]
    intensities = [
        0.0 if value is None else float(value)
        for value in sample.get("intensities", [])
    ]
    if intensities and len(intensities) != len(ranges):
        raise ValueError("scan intensity count differs from range count")
    scan_rate = sample.get("scan_rate_hz")
    return {
        "ts_ms": int(sample["ts_ms"]),
        "frame_id": str(sample["frame_id"]),
        "angle_min": float(sample["angle_min_rad"]),
        "angle_max": float(sample["angle_max_rad"]),
        "angle_increment": float(sample["angle_increment_rad"]),
        "range_min": float(sample["range_min_m"]),
        "range_max": float(sample["range_max_m"]),
        "ranges": ranges,
        "intensities": intensities,
        "scan_time": 0.0 if not scan_rate else 1.0 / float(scan_rate),
    }


def imu_contract(status: dict[str, Any]) -> dict[str, Any] | None:
    if status.get("status") != "available" or not status.get("sample"):
        return None
    sample = status["sample"]
    angular = sample["angular_velocity_radps"]
    acceleration = sample["linear_acceleration_mps2"]
    values = {
        "ts_ms": int(sample["ts_ms"]),
        "frame_id": str(sample["frame_id"]),
        "angular_velocity": tuple(float(angular[axis]) for axis in ("x", "y", "z")),
        "linear_acceleration": tuple(
            float(acceleration[axis]) for axis in ("x", "y", "z")
        ),
        "orientation": sample.get("orientation_xyzw"),
    }
    numeric = (*values["angular_velocity"], *values["linear_acceleration"])
    if not all(math.isfinite(value) for value in numeric):
        raise ValueError("IMU values must be finite")
    return values


def build_localization_update(
    sequence: int,
    provider_instance_id: str,
    map_id: str,
    map_revision: str,
    map_sample: dict[str, Any],
    pose_sample: dict[str, Any],
    path_sample: dict[str, Any] | None = None,
) -> dict[str, Any]:
    provider_instance_id = provider_instance_id.strip()
    map_id = map_id.strip()
    map_revision = map_revision.strip()
    if not provider_instance_id:
        raise ValueError("provider instance id cannot be empty")
    if not map_id:
        raise ValueError("map id cannot be empty")
    if not map_revision:
        raise ValueError("map revision cannot be empty")
    width = int(map_sample["width"])
    height = int(map_sample["height"])
    cells = [int(value) for value in map_sample["cells"]]
    if width <= 0 or height <= 0 or len(cells) != width * height:
        raise ValueError("occupancy dimensions do not match cells")
    if any(value < -1 or value > 100 for value in cells):
        raise ValueError("occupancy cells must be in [-1, 100]")

    frame_id = str(map_sample["frame_id"])
    if frame_id != str(pose_sample["frame_id"]):
        raise ValueError("map and pose frame ids differ")
    ts_ms = int(pose_sample["ts_ms"])
    map_ts_ms = int(map_sample["ts_ms"])
    origin = {
        "ts_ms": map_ts_ms,
        "frame_id": frame_id,
        "x_m": float(map_sample["origin"]["x_m"]),
        "y_m": float(map_sample["origin"]["y_m"]),
        "yaw_rad": float(map_sample["origin"]["yaw_rad"]),
    }
    current_grid_revision = grid_revision(map_sample)
    metadata = {
        "ts_ms": map_ts_ms,
        "map_id": map_id,
        "map_revision": map_revision,
        "grid_revision": current_grid_revision,
        "frame_id": frame_id,
        "width": width,
        "height": height,
        "resolution_m": float(map_sample["resolution_m"]),
        "origin": origin,
        "cell_order": "row-major",
    }
    pose = {
        "ts_ms": ts_ms,
        "frame_id": frame_id,
        "x_m": float(pose_sample["x_m"]),
        "y_m": float(pose_sample["y_m"]),
        "yaw_rad": float(pose_sample["yaw_rad"]),
    }
    costs = [255 if value < 0 else min(254, round(value * 2.54)) for value in cells]
    obstacle_height_m = 0.25
    voxel_depth = max(1, math.ceil(obstacle_height_m / float(map_sample["resolution_m"])))
    voxels = [
        {"x": index % width, "y": index // width, "z": z, "occupancy": value}
        for index, value in enumerate(cells)
        if value > 0
        for z in range(voxel_depth)
    ]
    path = {"ts_ms": 0, "frame_id": "", "poses": []}
    if path_sample is not None and path_sample.get("poses"):
        path_frame = str(path_sample["frame_id"])
        if path_frame != frame_id:
            raise ValueError("planner path and map frame ids differ")
        path_poses = []
        for item in path_sample["poses"]:
            if str(item["frame_id"]) != frame_id:
                raise ValueError("planner path pose and map frame ids differ")
            values = (
                float(item["x_m"]),
                float(item["y_m"]),
                float(item["yaw_rad"]),
            )
            if not all(math.isfinite(value) for value in values):
                raise ValueError("planner path pose must be finite")
            path_poses.append(
                {
                    "ts_ms": int(item["ts_ms"]),
                    "frame_id": frame_id,
                    "x_m": values[0],
                    "y_m": values[1],
                    "yaw_rad": values[2],
                }
            )
        path = {
            "ts_ms": int(path_sample["ts_ms"]),
            "frame_id": frame_id,
            "poses": path_poses,
        }
    grid_common = {
        "ts_ms": map_ts_ms,
        "frame_id": frame_id,
        "width": width,
        "height": height,
        "resolution_m": float(map_sample["resolution_m"]),
        "origin": origin,
        "metadata": metadata,
    }
    return {
        "version": "leash-localization-provider-v2",
        "provider_instance_id": provider_instance_id,
        "sequence": int(sequence),
        "localization": {
            "version": "leash-localization-v1",
            "ts_ms": ts_ms,
            "map": {
                "map_id": map_id,
                "map_revision": map_revision,
                "frame_id": frame_id,
            },
            "pose": {
                "pose": pose,
                "covariance": covariance_3x3(list(pose_sample["covariance"])),
            },
            "health": {
                "status": "tracking",
                "last_update_ms": ts_ms,
                "message": "SLAM Toolbox tracking through the Waveshare ROS adapter",
                "error": None,
            },
        },
        "map": metadata,
        "occupancy_grid": {**grid_common, "cells": cells},
        "costmap": {**grid_common, "costs": costs},
        "path": path,
        "voxel_grid": {
            "version": "leash-voxel-grid-v1",
            "ts_ms": map_ts_ms,
            "frame_id": frame_id,
            "width": width,
            "height": height,
            "depth": voxel_depth,
            "resolution_m": float(map_sample["resolution_m"]),
            "origin": origin,
            "origin_z_m": 0.0,
            "source": "projected-occupancy",
            "observed_3d": False,
            "voxels": voxels,
        },
    }
