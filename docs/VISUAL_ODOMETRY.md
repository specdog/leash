# Advisory monocular visual odometry

Leash accepts bounded, authenticated visual-odometry evidence without making
that provider an actuation, camera, localization, or metric-distance authority.
The v1 monocular contract always reports translation in tracker scale units and
requires `scale_status` to be `unanchored`.

| Method | Route | Purpose |
| --- | --- | --- |
| `GET` | `/visual-odometry` | Current state, age, last bounded update, and rejection count |
| `POST` | `/visual-odometry` | Submit one authenticated `leash.visual-odometry.v1` update |

Configure `LEASH_VISUAL_ODOMETRY_INGRESS_TOKEN_FILE` with a private regular
token file. If omitted, Leash reuses `LEASH_LOCALIZATION_INGRESS_TOKEN_FILE`.
The POST route returns `503` when neither is configured and `401` for a missing
or incorrect bearer token.

An update includes provider and calibration identity, provider epoch, frame
sequence, wall and monotonic capture timestamps, tracking state, optional pose,
observation count, input rate, processing latency, dropped-frame count, and GPU
backend. Leash rejects oversized identity fields, stale timestamps, non-finite
metrics, out-of-order samples within an epoch, invalid quaternions, tracking
without a pose, and every claim of metric monocular scale. State becomes
`stale` after 1.5 seconds without an accepted update.

This evidence is asynchronous. `tracking` can improve world estimation;
`initializing`, `lost`, `failed`, `stale`, or unavailable visual odometry does
not disable otherwise valid LiDAR and wheel-odometry motion. It also never
weakens Leash policy, collision checks, deadman, STOP, or e-stop.

The concrete Jetson provider is
[`implementations/waveshare-ugv/cuvslam-mono/`](../implementations/waveshare-ugv/cuvslam-mono/).
It consumes the shared MJPEG stream, writes only one small health document to a
bounded tmpfs, and runs with a read-only root filesystem and fixed CPU/memory
limits.
