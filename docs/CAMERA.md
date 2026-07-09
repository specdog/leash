# Camera Capture And Streaming

Leash gives snapshots, MJPEG, native V4L2 capture, and WebRTC one camera
ownership boundary. Only one capture or stream may own the device at a time.
Concurrent callers receive `camera is busy` instead of opening a second encoder
or racing the active stream.

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
the active owner (`snapshot`, `mjpeg`, or `webrtc`), start time, recovery
generation/count, and the last 16 sanitized failure reasons. `/camera/status`
includes the same object as `stream_health`.

`POST /camera/stream/recover` advances the recovery generation. An active MJPEG
or WebRTC stream observes the new generation, stops its capture process, releases
the single-owner guard, and can then be reconnected by the operator. Recovery is
safe to call while idle; it does not touch motors, serial ownership, or actuation
policy.
