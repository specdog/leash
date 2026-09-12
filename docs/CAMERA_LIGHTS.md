# Camera lighting

Camera lighting uses the existing Waveshare controller owner and command queue.
Its only light frame is `{"T":132,"IO4":chassis_pwm,"IO5":camera_pwm}` followed
by a newline; each PWM is an integer from 0 through 255. It creates no serial
connection, pilot lease, wheel command, or gimbal command. The owner retains its
existing startup, disconnect and stop behavior.

The protocol and base/head output mapping were checked against Waveshare's
[official `base_ctrl.py`](https://github.com/waveshareteam/ugv_jetson/blob/main/base_ctrl.py),
specifically `lights_ctrl`, `base_lights_ctrl`, and `head_lights_ctrl`. The new Rust
implementation was written for Leash's controller queue; no Python code was copied.

## Deployment and authority

Lighting is disabled by default. It requires the existing Waveshare physical
profile and physical-actuation permission, an existing policy other than `deny`
or `dry-run`, and `LEASH_ALLOW_CAMERA_LIGHTS=1`. This feature does not change those
existing physical or motion permissions.

Set `LEASH_CAMERA_LIGHT_AUTO=1` to explicitly arm automatic camera lighting at
service startup. That setting authorizes the bounded automatic light policy; it
does not authorize motion. Missing light/physical gates make auto startup fail.
Otherwise the initial mode is `off`, which means automatic writes are disarmed;
startup does not claim or enforce a physically dark lamp.

Mode changes and manual commands additionally require the exact token read from
`LEASH_LIGHTS_AUTH_TOKEN_FILE`, falling back to `LEASH_OPERATOR_AUTH_TOKEN_FILE`.
The file must contain a nonempty UTF-8 token of at most 4096 bytes, with surrounding
whitespace ignored. It is read at startup. No token is included in lighting status,
events, or logs. Without a configured token file, explicitly enabled startup auto
can run, but remote mode changes are refused. Keep tokens out of command lines,
evidence files and commits.

`GET /camera/lights` and MCP `invoke_capability` with
`{"capability":"camera_lights_status"}` return read-only status.
`POST /camera/lights` and MCP `invoke_capability` with
`{"capability":"camera_lights", ...request_fields}` use the capability registry's
`physical-light` class, including its physical, policy and agent-approval gates,
plus the separate exact light token check. HTTP accepts `Authorization: Bearer`
with the token supplied securely by the caller, or the same `token` JSON field
used by MCP. An HTTP header overrides the JSON token. These are not pilot tokens.

The request fields are:

| Field | Meaning |
| --- | --- |
| `mode` | Required: `auto`, `manual`, or `off`. |
| `camera_pwm` | Required in manual mode; 0..255. Auto defaults to 180 and rejects 0. Off always writes 0. |
| `chassis_pwm` | Optional 0..255; retains the configured value, initially 0. Set changes in manual/off mode before arming auto. |
| `token` | Configured light authority; may be supplied via HTTP bearer header. |
| `approval` | Existing capability policy approval field, when required. |

Unknown fields and PWM outside 0..255 are rejected. Example bodies, omitting the
separately supplied authority: `{"mode":"auto"}`,
`{"mode":"manual","camera_pwm":180,"chassis_pwm":0}`, and `{"mode":"off"}`.
Off/manual disarm the automatic policy even if the attempted serial write fails;
the failed write remains visible in status. An off request turns the camera lamp
off and retains chassis PWM unless the request explicitly supplies `chassis_pwm`.

## Automatic policy and evidence

The service measures JPEGs from its existing `/camera/snapshot` path, including
ffmpeg and v4l2 captures. A snapshot consumer must keep requesting frames; auto
does not start its own camera process. MJPEG/WebRTC streams alone do not provide
samples. Decoding runs outside the async executor, with one measurement in flight
and a minimum 200 ms interval, regardless of snapshot request rate. JPEG input is
limited to 4 MiB, 4096 pixels per dimension and a 64 MiB decoder allocation limit.

The measurement is mean decoded grayscale pixel value divided by 255, not lux.
At least three dark observations must span two seconds, with no gap above 1.5
seconds. Luma below 0.18 accumulates darkness; luma at or above 0.28 resets the
pending detection, providing a hysteresis band. The observed scene at about 0.16
therefore qualifies when sustained. Samples older than two seconds, invalid
measurements, and interrupted sampling do not trigger a write. Failed writes
have a ten-second retry cooldown. Default auto requests chassis 0, camera 180.

After one acknowledged light write, auto stays latched on until an authorized
mode change or service restart. It never turns itself off because its own lamp
made the image brighter. Lamp state after restart is unknown until a new write;
even then physical feedback remains unknown.

Status retains up to 64 attempts with requested PWM, controller write sequence,
write completion time, any error, and measured before/after camera observations.
An observation contains dimensions, normalized luma, request/receive times and
the JPEG SHA-256, without image bytes. A before observation must be fresh. An
after observation must belong to the latest write attempt, have a request start
at least 250 ms after acknowledged write completion and arrive within five
seconds. Missing observations remain null. The sensor exposure timestamp is not
known; this is stated explicitly in the observation.

`last_written_*` and a receipt mean the serial owner completed its write and
flush. They do not verify the LED physically lit. `physical_state_known` is always
false and `physical_feedback` is `unknown`. Before/after luma permits a caller to
report a measured image change, but does not by itself establish its cause or
verify the lamp. If snapshot sampling stops, the timestamps become stale and no
new camera or physical feedback is claimed.

## Validation and release boundary

The CPU-only tests use fake controller I/O and encoded test JPEGs. They cover the
exact light-only frame, partial writes, failed write receipts, sustained darkness,
hysteresis, cooldown, off disarming on failure, decoder cadence, measured pixels,
authorization and input limits. They do not operate hardware.

Run host checks sequentially with `-j 2`, preserving the lockfile:

```text
cargo test -p leash-waveshare -j 2 --locked
cargo test -p leash-harness --features waveshare-ugv -j 2 --locked lighting
```

This change is based on installed source `edce2ae`. Before deployment, compare the
running service's complete feature set and source revision. A Linux build must
preserve its existing `cuda`, HTTP/MCP, Waveshare and camera features and CUDA
toolchain environment; a host test binary is not a deployment artifact. Use the
existing Linux build/release process and one build at a time. No hardware result
or deployment is asserted by this document.

The earlier production-equivalent feature set is recorded in
[RUNTIME_V2_HANDOFF.md](RUNTIME_V2_HANDOFF.md). After verifying the current service
still uses those features, the corresponding native Jetson build command is:

```text
cargo build -p leash-harness --bin leash --release --locked -j 2 --no-default-features --features http,mcp,waveshare-ugv,bridge-compat,v4l2-camera,webrtc,cuda,physical-navigation
```

Run it in an isolated source checkout with the existing CUDA build environment.
This is a native Linux/aarch64 recipe; this branch does not establish a Windows
to Jetson cross-compilation toolchain or permit deployment by itself.
