# Camera Capture And Streaming

Native V4L2 snapshots and MJPEG streams share one persistent camera owner. The
first request starts capture; subsequent callers consume the latest actual JPEG
through a watch channel. A slow stream skips frames without blocking capture.
FFmpeg and WebRTC retain exclusive device ownership and cannot open a second
device owner while native capture is active.

Frames must be at most 500 ms old. When V4L2 supplies a monotonic timestamp, this
bound includes time spent in the driver queue. Otherwise it bounds time since
dequeue, and the snapshot explicitly reports `dequeue-only-exposure-unknown` in
`X-Leash-Camera-Timestamp-Basis`. Snapshot headers also expose the unchanged
driver sequence, timestamp in microseconds, timestamp flags, acquisition-start
time and dequeue time. Requests never replace those times with a cache-hit time.
The native worker drains the device continuously and owns no client output queue.

## Build

The FFmpeg snapshot and MJPEG paths need the `http` feature and an `ffmpeg`
binary on the bot. Native V4L2 MJPEG and WebRTC are explicit features:

```bash
cargo build --release --no-default-features \
  --features http,mcp,waveshare-ugv,bridge-compat,v4l2-camera,webrtc
```

## Capture settings

Add camera values to `~/.config/leash/leash.env`, then restart `leash.service`.

| Variable | Meaning | Default |
|---|---|---|
| `LEASH_CAMERA_DEVICE` | V4L2 device path | `/dev/video0` |
| `LEASH_CAMERA_BACKEND` | `auto`/native V4L2 or `ffmpeg` | `auto` |
| `LEASH_CAMERA_INPUT_FORMAT` | V4L2/FFmpeg input format such as `mjpeg` | `mjpeg` for native V4L2 |
| `LEASH_CAMERA_VIDEO_SIZE` | Requested `WIDTHxHEIGHT` | native V4L2: `1280x720`; FFmpeg: device default |
| `LEASH_CAMERA_FRAMERATE` | Requested frames per second | native V4L2: `30`; WebRTC pacing: `5` |
| `LEASH_CAMERA_STREAM_CODEC` | MJPEG relay codec; `copy` avoids re-encoding MJPEG input | encode MJPEG |
| `LEASH_CAMERA_MJPEG_QUALITY` | FFmpeg MJPEG `-q:v` value | `5` |

Start conservatively on a Jetson or USB camera, for example `1280x720` at `10`
FPS. Confirm supported modes with `v4l2-ctl --list-formats-ext -d /dev/video0`;
unsupported size/FPS combinations may be rounded or rejected by the driver.

## WebRTC encoder settings

| Variable | Meaning | Default |
|---|---|---|
| `LEASH_WEBRTC_ENABLED` | `false`, `0`, `no`, `off`, or `disabled` removes WebRTC advertisement and rejects signaling | enabled when compiled |
| `LEASH_WEBRTC_ENCODER` | FFmpeg H.264 encoder | `libx264` |
| `LEASH_WEBRTC_GOP` | keyframe interval | camera FPS, otherwise `5` |
| `LEASH_WEBRTC_X264_PRESET` | x264 speed/quality preset | `ultrafast` |
| `LEASH_WEBRTC_BITRATE` | target bitrate passed as `-b:v` | encoder default |
| `LEASH_WEBRTC_MAXRATE` | maximum bitrate passed as `-maxrate` | encoder default |
| `LEASH_WEBRTC_BUFSIZE` | rate-control buffer passed as `-bufsize` | encoder default |
| `LEASH_WEBRTC_STUN_URL` | optional ICE STUN URL | none; LAN/local ICE only |

Software-safe baseline:

```dotenv
LEASH_CAMERA_VIDEO_SIZE=1280x720
LEASH_CAMERA_FRAMERATE=10
LEASH_WEBRTC_ENCODER=libx264
LEASH_WEBRTC_X264_PRESET=ultrafast
LEASH_WEBRTC_GOP=10
LEASH_WEBRTC_BITRATE=1500k
LEASH_WEBRTC_MAXRATE=1800k
LEASH_WEBRTC_BUFSIZE=3000k
```

On Jetson, choose an encoder only after confirming it exists in
`ffmpeg -hide_banner -encoders`. Hardware encoder names vary with the installed
JetPack/FFmpeg build; Leash passes `LEASH_WEBRTC_ENCODER` through and reports a
stream failure if FFmpeg cannot start it. Keep `-bf 0`, a short GOP, and bounded
bitrate for low latency. If a hardware encoder is unstable, return to `libx264`
before changing resolution or FPS so only one variable changes at a time.

## Health and recovery

```bash
curl -s http://127.0.0.1:8000/camera/stream/health
curl -s -X POST http://127.0.0.1:8000/camera/stream/recover
```

`GET /camera/stream/health` (also `/camera/health`) reports device availability,
the active owner (`v4l2-shared`, `snapshot`, `mjpeg`, or `webrtc`), start time, recovery
generation/count, and the last 16 sanitized failure reasons. `/camera/status`
includes the same object as `stream_health`.

`POST /camera/stream/recover` advances the recovery generation. The native owner,
active FFmpeg MJPEG stream, or WebRTC stream observes the new generation, closes
capture, and releases the owner guard. Native capture is restarted by the next
snapshot or stream subscription after the old owner exits. Recovery is
safe to call while idle; it does not touch motors, serial ownership, or actuation
policy.

## Camera-only gimbal authority

`POST /camera/aim` (alias `/gimbal/aim`) accepts a camera-only bearer when
`LEASH_ALLOW_CAMERA_AIM=1` is enabled on the existing Waveshare physical profile
with its existing physical-actuation permission and policy gates. The bearer is
read at startup from `LEASH_CAMERA_AIM_AUTH_TOKEN_FILE`, falling back to
`LEASH_OPERATOR_AUTH_TOKEN_FILE`; no credential is included in status or logs.
This path never creates or refreshes a pilot lease and never emits a wheel frame.
Existing valid pilot-token callers remain supported. The camera bearer does not
grant wheel authority or alter any wheel gate.

Supply HTTP `Authorization: Bearer` privately, with a JSON body containing
`pan_deg` and `tilt_deg`, optional `speed` and `accel`, and `approval` if required
by the existing policy. MCP uses `invoke_capability` with
`capability: camera_aim` and the same fields, supplying its authority as `token`.
Pan must be finite and within -180..180 degrees; tilt within -30..90. Requests
outside those bounds are rejected instead of clamped. Speed defaults to 8 and
must be 1..40; acceleration defaults to 4 and must be 1..20, in the controller's
units. Requests are serialized and must start at least 250 ms apart. These are
bounded operating settings, not measurements of actual gimbal velocity.

The only frame is the existing T133 command with `X`, `Y`, `SPD`, and `ACC`,
queued through the existing single serial owner. The runtime estop check and
the owner's estop latch both refuse camera commands while latched, including
commands already queued when the owner observes the latch.

Success retains `ok`, `pan_deg`, `tilt_deg`, `speed`, and `accel`, and adds
`authority` (`camera-only-bearer` on this path) plus `serial_receipt` containing
`controller_sequence`, `write_completed_at_ms`, and `physical_feedback: unknown`.
It means the owner completed a serial write/flush, not that the requested physical
pose was verified. No head command occurs merely by enabling this authority.

`GET /camera/aim` retains the `gimbal` object. `gimbal.pose` remains null until a
successful command; afterward it explicitly reports `source: last-commanded`
and its write receipt. `gimbal.physical_pose_known` remains false. The new
`gimbal.dedicated_authority` reports `enabled`, `token_configured`,
`grants_wheel_authority: false`, request interval, last attempt time, and last
write error. The limits/defaults are also published separately. A UI must not
turn an unknown physical pose into an assumed centered gimbal.
