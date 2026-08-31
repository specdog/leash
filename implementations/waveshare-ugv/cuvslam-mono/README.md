# Leash cuVSLAM monocular worker

This bounded Jetson sidecar consumes Leash's shared MJPEG stream and submits
advisory CUDA-accelerated monocular odometry back to Leash. Leash remains the
only camera and motor-device owner. Translation is explicitly unanchored and is
never used as meters or as motion authority.

The image pins NVIDIA cuVSLAM `v17.0.0` for Python 3.10, aarch64, and CUDA 12
by SHA-256. It records no frames, enables no debug dump, uses a read-only root
filesystem, and has a 2 GiB memory limit.

Set these values in a private environment file:

```text
LEASH_CUVSLAM_CALIBRATION_FILE=/absolute/path/to/camera-calibration.json
LEASH_VISUAL_ODOMETRY_TOKEN_FILE=/absolute/path/to/localization-token
```

Leash may reuse its localization ingress token file for the visual-odometry
endpoint. Start with `docker compose --env-file <private-env> up -d --build`.
Verify `GET /visual-odometry` reports `tracking` before relying on the evidence;
`stale`, `lost`, or `failed` are advisory and do not disable LiDAR navigation.
