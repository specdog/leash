#!/usr/bin/env python3
"""Bounded cuVSLAM monocular worker for Leash.

The worker consumes Leash's timestamped MJPEG fan-out, runs cuVSLAM on the
Jetson GPU, and returns advisory visual odometry. It never opens a camera or a
motor device and never persists images.
"""

from __future__ import annotations

import json
import math
import os
import signal
import time
import urllib.request
import uuid
from collections import deque
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO, Iterator


SCHEMA_VERSION = "leash.visual-odometry.v1"
PROVIDER = "cuvslam-mono-17.0.0"
MAX_HEADER_BYTES = 8 * 1024
MAX_JPEG_BYTES = 8 * 1024 * 1024
STATUS_INTERVAL_S = 0.1


@dataclass(frozen=True)
class CameraCalibration:
    calibration_id: str
    source_width: int
    source_height: int
    focal: tuple[float, float]
    principal: tuple[float, float]
    distortion: tuple[float, float, float, float]

    def scaled(self, width: int, height: int) -> "CameraCalibration":
        if width <= 0 or height <= 0:
            raise ValueError("camera dimensions must be positive")
        source_aspect = self.source_width / self.source_height
        target_aspect = width / height
        if abs(source_aspect - target_aspect) > 0.01:
            raise ValueError("camera frame aspect ratio does not match calibration")
        scale_x = width / self.source_width
        scale_y = height / self.source_height
        return CameraCalibration(
            calibration_id=self.calibration_id,
            source_width=width,
            source_height=height,
            focal=(self.focal[0] * scale_x, self.focal[1] * scale_y),
            principal=(self.principal[0] * scale_x, self.principal[1] * scale_y),
            distortion=self.distortion,
        )


@dataclass(frozen=True)
class MjpegFrame:
    jpeg: bytes
    sequence: int
    captured_at_ms: int
    monotonic_ns: int


def load_calibration(path: Path) -> CameraCalibration:
    payload = json.loads(path.read_text(encoding="utf-8"))
    image = payload["image"]
    fisheye = payload["fisheye"]
    matrix = fisheye["camera_matrix"]
    distortion = tuple(float(value) for value in fisheye["distortion"])
    if len(distortion) != 4:
        raise ValueError("cuVSLAM fisheye calibration requires four coefficients")
    calibration = CameraCalibration(
        calibration_id=str(payload["calibration_id"]),
        source_width=int(image["width_px"]),
        source_height=int(image["height_px"]),
        focal=(float(matrix[0][0]), float(matrix[1][1])),
        principal=(float(matrix[0][2]), float(matrix[1][2])),
        distortion=distortion,
    )
    values = (*calibration.focal, *calibration.principal, *calibration.distortion)
    if any(not math.isfinite(value) for value in values):
        raise ValueError("camera calibration contains non-finite values")
    return calibration


def mjpeg_frames(stream: BinaryIO) -> Iterator[MjpegFrame]:
    while True:
        boundary = stream.readline(MAX_HEADER_BYTES)
        if not boundary:
            return
        if not boundary.startswith(b"--"):
            continue
        headers: dict[str, str] = {}
        header_bytes = len(boundary)
        while True:
            line = stream.readline(MAX_HEADER_BYTES)
            if not line:
                return
            header_bytes += len(line)
            if header_bytes > MAX_HEADER_BYTES:
                raise ValueError("MJPEG headers exceed bound")
            if line in (b"\r\n", b"\n"):
                break
            name, separator, value = line.decode("ascii", "strict").partition(":")
            if not separator:
                raise ValueError("malformed MJPEG header")
            headers[name.strip().lower()] = value.strip()
        length = int(headers.get("content-length", "0"))
        if length <= 0 or length > MAX_JPEG_BYTES:
            raise ValueError("MJPEG frame length is missing or out of bounds")
        jpeg = stream.read(length)
        if len(jpeg) != length:
            return
        stream.readline(2)
        yield MjpegFrame(
            jpeg=jpeg,
            sequence=int(headers["x-leash-sequence"]),
            captured_at_ms=int(headers["x-leash-captured-at-ms"]),
            monotonic_ns=int(headers["x-leash-monotonic-ns"]),
        )


def build_tracker(calibration: CameraCalibration):
    import cuvslam

    camera = cuvslam.Camera()
    camera.size = calibration.source_width, calibration.source_height
    camera.focal = calibration.focal
    camera.principal = calibration.principal
    camera.distortion = cuvslam.Distortion(
        cuvslam.Distortion.Model.Fisheye, list(calibration.distortion)
    )
    rig = cuvslam.Rig()
    rig.cameras = [camera]
    config = cuvslam.Tracker.OdometryConfig()
    config.odometry_mode = cuvslam.Tracker.OdometryMode.Mono
    config.use_gpu = True
    config.use_motion_model = True
    config.enable_observations_export = True
    return cuvslam.Tracker(rig, config, None)


def pose_payload(pose) -> dict | None:
    if pose is None:
        return None
    translation = [float(value) for value in pose.translation]
    rotation = [float(value) for value in pose.rotation]
    if len(translation) != 3 or len(rotation) != 4:
        raise ValueError("cuVSLAM returned an invalid pose shape")
    return {
        "translation_scale_units": translation,
        "rotation_xyzw": {
            "x": rotation[0],
            "y": rotation[1],
            "z": rotation[2],
            "w": rotation[3],
        },
    }


def post_status(url: str, token: str, payload: dict) -> None:
    body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
    request = urllib.request.Request(
        url,
        data=body,
        method="POST",
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
        },
    )
    with urllib.request.urlopen(request, timeout=1.0) as response:
        if response.status != 202:
            raise RuntimeError(f"Leash rejected visual odometry with {response.status}")


def write_health(path: Path, payload: dict) -> None:
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps(payload, separators=(",", ":")), encoding="utf-8")
    temporary.replace(path)


def run() -> None:
    import cv2
    import numpy as np

    leash_url = os.environ.get("LEASH_URL", "http://host.docker.internal:8000").rstrip("/")
    camera_url = os.environ.get("LEASH_CAMERA_URL", f"{leash_url}/camera/stream.mjpg")
    status_url = f"{leash_url}/visual-odometry"
    token = Path(os.environ["LEASH_VISUAL_ODOMETRY_TOKEN_FILE"]).read_text(
        encoding="utf-8"
    ).strip()
    calibration = load_calibration(Path(os.environ["LEASH_CUVSLAM_CALIBRATION_FILE"]))
    health_path = Path(os.environ.get("LEASH_CUVSLAM_HEALTH_FILE", "/run/cuvslam-health.json"))
    stopped = False

    def stop(_signum, _frame) -> None:
        nonlocal stopped
        stopped = True

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    retry_s = 0.25
    while not stopped:
        try:
            with urllib.request.urlopen(camera_url, timeout=5.0) as stream:
                # The Leash camera hub resets its frame sequence after a service
                # restart. A new epoch makes that reset explicit to ingress.
                provider_epoch = uuid.uuid4().hex
                tracker = None
                saw_pose = False
                last_sequence = 0
                last_monotonic_ns = 0
                last_status_at = 0.0
                dropped_frames = 0
                frame_times: deque[float] = deque(maxlen=31)
                for frame in mjpeg_frames(stream):
                    if stopped:
                        return
                    if frame.sequence <= last_sequence or frame.monotonic_ns <= last_monotonic_ns:
                        continue
                    if last_sequence:
                        dropped_frames += max(0, frame.sequence - last_sequence - 1)
                    last_sequence = frame.sequence
                    last_monotonic_ns = frame.monotonic_ns
                    image = cv2.imdecode(np.frombuffer(frame.jpeg, dtype=np.uint8), cv2.IMREAD_GRAYSCALE)
                    if image is None:
                        continue
                    if tracker is None:
                        height, width = image.shape
                        tracker = build_tracker(calibration.scaled(width, height))
                    started = time.perf_counter()
                    odometry, _slam = tracker.track(frame.monotonic_ns, [image])
                    processing_ms = (time.perf_counter() - started) * 1_000.0
                    world_from_rig = odometry.world_from_rig
                    saw_pose = saw_pose or world_from_rig is not None
                    observations = len(tracker.get_last_observations(0))
                    now = time.monotonic()
                    frame_times.append(now)
                    if now - last_status_at < STATUS_INTERVAL_S:
                        continue
                    input_fps = 0.0
                    if len(frame_times) > 1:
                        input_fps = (len(frame_times) - 1) / (frame_times[-1] - frame_times[0])
                    state = "tracking" if world_from_rig is not None else ("lost" if saw_pose else "initializing")
                    payload = {
                        "schema_version": SCHEMA_VERSION,
                        "provider": PROVIDER,
                        "provider_epoch": provider_epoch,
                        "sequence": frame.sequence,
                        "captured_at_ms": frame.captured_at_ms,
                        "monotonic_ns": frame.monotonic_ns,
                        "calibration_id": calibration.calibration_id,
                        "state": state,
                        "pose": pose_payload(world_from_rig),
                        "tracked_observations": observations,
                        "input_fps": input_fps,
                        "processing_ms": processing_ms,
                        "dropped_frames": dropped_frames,
                        "gpu_backend": "cuda",
                        "scale_status": "unanchored",
                    }
                    post_status(status_url, token, payload)
                    write_health(health_path, payload)
                    last_status_at = now
                raise RuntimeError("Leash camera stream ended")
        except Exception as error:
            write_health(
                health_path,
                {"state": "degraded", "error": str(error)[:512], "ts_ms": int(time.time() * 1_000)},
            )
            if stopped:
                return
            time.sleep(retry_s)
            retry_s = min(retry_s * 2.0, 5.0)
        else:
            retry_s = 0.25


if __name__ == "__main__":
    run()
