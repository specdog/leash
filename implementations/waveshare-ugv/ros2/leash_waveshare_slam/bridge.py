"""ROS 2 bridge: Leash sensors out, generic localization updates back in."""

from __future__ import annotations

import json
import math
import os
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

import rclpy
from geometry_msgs.msg import PoseWithCovarianceStamped, TransformStamped
from nav_msgs.msg import OccupancyGrid, Odometry
from rclpy.node import Node
from rclpy.qos import qos_profile_sensor_data
from sensor_msgs.msg import Imu, LaserScan
from tf2_ros.static_transform_broadcaster import StaticTransformBroadcaster

from .contract import (
    DifferentialOdometry,
    build_localization_update,
    imu_contract,
    laser_scan_contract,
    localization_update_due,
)


def env_float(name: str) -> float:
    raw = os.environ.get(name)
    if raw is None:
        raise RuntimeError(f"{name} is required")
    value = float(raw)
    if not math.isfinite(value):
        raise RuntimeError(f"{name} must be finite")
    return value


def quaternion_from_yaw(yaw: float) -> tuple[float, float, float, float]:
    return 0.0, 0.0, math.sin(yaw / 2.0), math.cos(yaw / 2.0)


def yaw_from_quaternion(x: float, y: float, z: float, w: float) -> float:
    return math.atan2(2.0 * (w * z + x * y), 1.0 - 2.0 * (y * y + z * z))


class LeashRosBridge(Node):
    def __init__(self) -> None:
        super().__init__("leash_ros_bridge")
        if os.environ.get("LEASH_ROS_READ_ONLY") != "1":
            raise RuntimeError("LEASH_ROS_READ_ONLY=1 is required")

        self._leash_url = os.environ.get(
            "LEASH_URL", "http://host.docker.internal:8000"
        ).rstrip("/")
        token_file = Path(os.environ["LEASH_LOCALIZATION_TOKEN_FILE"])
        self._token = token_file.read_text(encoding="utf-8").strip()
        if not self._token:
            raise RuntimeError("localization token file is empty")
        self._map_id = os.environ.get("LEASH_ROS_MAP_ID", "waveshare-ugv-map")
        self._odometry = DifferentialOdometry(
            env_float("LEASH_ROS_WHEEL_TRACK_M"),
            env_float("LEASH_ROS_WHEEL_SCALE"),
        )
        self._sequence = 0
        self._sequence_initialized = False
        self._map_sample: dict[str, Any] | None = None
        self._pose_sample: dict[str, Any] | None = None
        self._last_posted_pose_ms: int | None = None
        self._last_posted_at_s: float | None = None
        self._last_error_log_s = 0.0

        self._scan_publisher = self.create_publisher(
            LaserScan, "/scan", qos_profile_sensor_data
        )
        self._imu_publisher = self.create_publisher(
            Imu, "/imu/data_raw", qos_profile_sensor_data
        )
        self._odom_publisher = self.create_publisher(
            Odometry, "/wheel/odometry", qos_profile_sensor_data
        )
        self.create_subscription(OccupancyGrid, "/map", self._on_map, 1)
        self.create_subscription(
            PoseWithCovarianceStamped, "/pose", self._on_pose, 10
        )
        self._static_broadcaster = StaticTransformBroadcaster(self)
        self._publish_static_transforms()
        self.create_timer(0.1, self._poll_leash)
        self.create_timer(0.2, self._post_localization)

    @staticmethod
    def _stamp(message: Any, ts_ms: int) -> None:
        message.sec = int(ts_ms // 1000)
        message.nanosec = int(ts_ms % 1000) * 1_000_000

    @staticmethod
    def _ts_ms(stamp: Any) -> int:
        return int(stamp.sec) * 1000 + int(stamp.nanosec) // 1_000_000

    def _request_json(
        self, path: str, payload: dict[str, Any] | None = None
    ) -> dict[str, Any]:
        data = None if payload is None else json.dumps(payload).encode("utf-8")
        request = urllib.request.Request(
            f"{self._leash_url}{path}",
            data=data,
            headers={
                "Accept": "application/json",
                "Content-Type": "application/json",
                "Authorization": f"Bearer {self._token}",
            },
            method="GET" if payload is None else "POST",
        )
        with urllib.request.urlopen(request, timeout=0.5) as response:
            return json.load(response)

    def _poll_leash(self) -> None:
        try:
            telemetry = self._request_json("/telemetry")
            sensors = telemetry["sensors"]
            self._publish_scan(sensors.get("range_scan", {}))
            self._publish_imu(sensors.get("imu", {}))
            self._publish_odometry(sensors.get("odometry", {}), int(telemetry["ts_ms"]))
        except (KeyError, TypeError, ValueError, OSError, urllib.error.URLError) as error:
            self._log_error_throttled(f"Leash sensor poll failed: {error}")

    def _publish_scan(self, status: dict[str, Any]) -> None:
        sample = laser_scan_contract(status)
        if sample is None:
            return
        message = LaserScan()
        self._stamp(message.header.stamp, int(sample["ts_ms"]))
        message.header.frame_id = str(sample["frame_id"])
        message.angle_min = sample["angle_min"]
        message.angle_max = sample["angle_max"]
        message.angle_increment = sample["angle_increment"]
        message.scan_time = sample["scan_time"]
        if message.scan_time:
            message.time_increment = message.scan_time / max(1, len(sample["ranges"]))
        message.range_min = sample["range_min"]
        message.range_max = sample["range_max"]
        message.ranges = sample["ranges"]
        message.intensities = sample["intensities"]
        self._scan_publisher.publish(message)

    def _publish_imu(self, status: dict[str, Any]) -> None:
        sample = imu_contract(status)
        if sample is None:
            return
        message = Imu()
        self._stamp(message.header.stamp, int(sample["ts_ms"]))
        message.header.frame_id = str(sample["frame_id"])
        message.angular_velocity.x, message.angular_velocity.y, message.angular_velocity.z = (
            sample["angular_velocity"]
        )
        (
            message.linear_acceleration.x,
            message.linear_acceleration.y,
            message.linear_acceleration.z,
        ) = sample["linear_acceleration"]
        orientation = sample["orientation"]
        if orientation is None:
            message.orientation_covariance[0] = -1.0
        else:
            message.orientation.x = float(orientation["x"])
            message.orientation.y = float(orientation["y"])
            message.orientation.z = float(orientation["z"])
            message.orientation.w = float(orientation["w"])
            message.orientation_covariance[0] = 0.04
            message.orientation_covariance[4] = 0.04
            message.orientation_covariance[8] = 0.08
        message.angular_velocity_covariance[0] = 0.02
        message.angular_velocity_covariance[4] = 0.02
        message.angular_velocity_covariance[8] = 0.02
        message.linear_acceleration_covariance[0] = 0.20
        message.linear_acceleration_covariance[4] = 0.20
        message.linear_acceleration_covariance[8] = 0.20
        self._imu_publisher.publish(message)

    def _publish_odometry(self, status: dict[str, Any], ts_ms: int) -> None:
        if status.get("status") != "available":
            return
        if status.get("left_m") is None or status.get("right_m") is None:
            return
        sample = self._odometry.update(status["left_m"], status["right_m"], ts_ms)
        message = Odometry()
        self._stamp(message.header.stamp, ts_ms)
        message.header.frame_id = "odom"
        message.child_frame_id = "base_link"
        message.pose.pose.position.x = sample["x_m"]
        message.pose.pose.position.y = sample["y_m"]
        qx, qy, qz, qw = quaternion_from_yaw(sample["yaw_rad"])
        message.pose.pose.orientation.x = qx
        message.pose.pose.orientation.y = qy
        message.pose.pose.orientation.z = qz
        message.pose.pose.orientation.w = qw
        message.pose.covariance[0] = 0.04
        message.pose.covariance[7] = 0.04
        message.pose.covariance[35] = 0.09
        message.twist.twist.linear.x = sample["linear_x_mps"]
        message.twist.twist.angular.z = sample["angular_z_radps"]
        message.twist.covariance[0] = 0.04
        message.twist.covariance[35] = 0.09
        self._odom_publisher.publish(message)

    def _publish_static_transforms(self) -> None:
        now = self.get_clock().now().to_msg()
        transforms = [
            self._static_transform(
                now,
                "base_link",
                "base_scan",
                "LEASH_ROS_SCAN",
            ),
            self._static_transform(
                now,
                "base_link",
                "camera_link",
                "LEASH_ROS_CAMERA",
            ),
        ]
        self._static_broadcaster.sendTransform(transforms)

    @staticmethod
    def _static_transform(
        stamp: Any, parent: str, child: str, prefix: str
    ) -> TransformStamped:
        message = TransformStamped()
        message.header.stamp = stamp
        message.header.frame_id = parent
        message.child_frame_id = child
        message.transform.translation.x = env_float(f"{prefix}_X_M")
        message.transform.translation.y = env_float(f"{prefix}_Y_M")
        message.transform.translation.z = env_float(f"{prefix}_Z_M")
        qx, qy, qz, qw = quaternion_from_yaw(env_float(f"{prefix}_YAW_RAD"))
        message.transform.rotation.x = qx
        message.transform.rotation.y = qy
        message.transform.rotation.z = qz
        message.transform.rotation.w = qw
        return message

    def _on_map(self, message: OccupancyGrid) -> None:
        origin = message.info.origin
        self._map_sample = {
            "ts_ms": self._ts_ms(message.header.stamp),
            "frame_id": message.header.frame_id or "map",
            "width": message.info.width,
            "height": message.info.height,
            "resolution_m": message.info.resolution,
            "origin": {
                "x_m": origin.position.x,
                "y_m": origin.position.y,
                "yaw_rad": yaw_from_quaternion(
                    origin.orientation.x,
                    origin.orientation.y,
                    origin.orientation.z,
                    origin.orientation.w,
                ),
            },
            "cells": list(message.data),
        }

    def _on_pose(self, message: PoseWithCovarianceStamped) -> None:
        pose = message.pose.pose
        self._pose_sample = {
            "ts_ms": self._ts_ms(message.header.stamp),
            "frame_id": message.header.frame_id or "map",
            "x_m": pose.position.x,
            "y_m": pose.position.y,
            "yaw_rad": yaw_from_quaternion(
                pose.orientation.x,
                pose.orientation.y,
                pose.orientation.z,
                pose.orientation.w,
            ),
            "covariance": list(message.pose.covariance),
        }

    def _post_localization(self) -> None:
        if self._map_sample is None or self._pose_sample is None:
            return
        pose_ts_ms = int(self._pose_sample["ts_ms"])
        now_s = time.monotonic()
        if not localization_update_due(
            self._last_posted_pose_ms,
            pose_ts_ms,
            self._last_posted_at_s,
            now_s,
        ):
            return
        try:
            if not self._sequence_initialized:
                status = self._request_json("/localization")
                self._sequence = max(self._sequence, int(status.get("sequence") or 0))
                self._sequence_initialized = True
            self._sequence += 1
            update = build_localization_update(
                self._sequence, self._map_id, self._map_sample, self._pose_sample
            )
            self._request_json("/localization/update", update)
            self._last_posted_pose_ms = pose_ts_ms
            self._last_posted_at_s = now_s
        except (KeyError, TypeError, ValueError, OSError, urllib.error.URLError) as error:
            self._log_error_throttled(f"localization update failed: {error}")

    def _log_error_throttled(self, message: str) -> None:
        now = time.monotonic()
        if now - self._last_error_log_s >= 5.0:
            self.get_logger().error(message)
            self._last_error_log_s = now


def main() -> None:
    rclpy.init()
    node = LeashRosBridge()
    try:
        rclpy.spin(node)
    except KeyboardInterrupt:
        pass
    finally:
        node.destroy_node()
        if rclpy.ok():
            rclpy.shutdown()
