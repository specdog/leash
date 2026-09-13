# Automatic camera lighting on Pinkie

Installed source `b95ab3866ef7487f4894bae1d53aedca63e2327b` on the robot at
2026-09-12 19:46 America/New_York. The aarch64 binary SHA-256 is
`3722ad35249faa7250fca526d451559d24c86375903bb08e3edd20918e4c5223`.
It retains HTTP, MCP, Waveshare, bridge compatibility, native V4L2, WebRTC,
CUDA and physical navigation build features. Existing controller permissions
were retained; no wheel or head commands were issued by this work.

The installed user service has a `camera-light.conf` drop-in enabling
`LEASH_ALLOW_CAMERA_LIGHTS=1` and `LEASH_CAMERA_LIGHT_AUTO=1`. Its existing
environment files, credentials and service command were retained. The previous
binary is backed up under the robot's local state directory. CUDA remained active
after the service restart.

Live `GET /camera/lights` reported automatic darkness detection and an error-free
controller write at Unix milliseconds 1789256782129: camera PWM 180, chassis PWM 0,
controller sequence 12. The automatic latch was set. Mean decoded camera luma
was 0.1571279233 before the write and 0.1924895578 afterward, both from actual
640 by 480 JPEGs. This measurement is normalized pixel intensity, not lux.

The operator subsequently reported seeing the light. That is operator observation;
the controller API still correctly reports physical feedback as unknown because
it has no lamp feedback sensor.

An outstanding camera issue was observed after deployment: simultaneous snapshot
and stream consumers contend for the camera, causing HTTP 400 busy responses.
The shared native capture path is being repaired separately. Lighting itself
activated automatically; successful visual tracking or autonomous driving is not
claimed by this evidence.

## Shared camera deployment, 2026-09-12

The contention above was resolved by source
`bf5b6bb88761e1b88df747c3ca9898918a15685c`. The deployed aarch64 binary SHA-256 is
`65685ebc3be0f30cba3323d8c48b4bf6139bd6637ac74cc0c5f548423b685233`.
The cross-build retained all eight production features listed above. Deployment
verified both executable hashes, backed up the actual installed binary, replaced
only the executable atomically, and ran `systemctl --user restart leash.service`.
The backup is `/home/jetson/.local/state/leash/camera-sharing/20260912-200410/leash`.

Readback reported `ok: true`, accelerator `active: cuda` and `required: true`,
with no active operator lease. The effective service unit, effective environment,
`leash.env`, and `camera-stream.env` had identical SHA-256 fingerprints before
and after replacement. Existing drop-ins, automatic-light settings, credentials,
controller permissions and service arguments were retained. No motion, head,
stop, authorization or camera-recovery API call was made for this deployment.

With an additional MJPEG stream held open while the console remained open, three
actual snapshot responses returned HTTP 200:

| V4L2 sequence | Snapshot latency | JPEG bytes | Dequeued Unix ms |
| --- | --- | --- | --- |
| 0 | 2.168 ms | 14,848 | 1789257863873 |
| 2 | 2.489 ms | 14,756 | 1789257864073 |
| 3 | 2.412 ms | 14,764 | 1789257864273 |

Median snapshot latency was 2.412 ms after the shared owner had initialized.
All three JPEGs had valid start/end markers and distinct SHA-256 hashes. Their
headers reported `X-Leash-Camera-Timestamp-Basis: v4l2-monotonic` and driver flags
73729. Raw driver timestamps were 12143408073, 12143808040 and 12144008046
microseconds. These driver times are monotonic; the separate acquisition-start
and dequeue headers are Unix milliseconds. Repeated cache reads retain the
original sequence and times. The native path rejects frames older than 500 ms,
including measured driver-queue age when the monotonic timestamp is supplied.

`GET /camera/stream/health` reported one `v4l2-shared` owner and an empty failure
history. Lighting remained enabled in `auto` mode and measured current luma
0.2099209431 from sequence 3's actual JPEG. No additional light write was reported
at that readback because the scene was already above the dark threshold. Physical
lamp feedback remained `unknown`.

The shared `/camera/snapshot` feed is available to the console and pretrained
camera observer concurrently with MJPEG. The observer owner was notified of
recovery after restart. This establishes camera-feed availability; it does not
claim a model output, successful tracking or autonomous movement.

The complete deployment receipt, including camera headers, JPEG hashes and
configuration fingerprints, is retained on the robot at
`/home/jetson/.local/state/leash/camera-sharing/20260912-200410/receipt.json` and on
the dev host at
`C:/Users/ericm/.local/state/qualia/leash-camera-sharing-bf5b6bb/receipt.json`.
Cross-build and ELF compatibility records are beside the host artifact under
`C:/Users/ericm/.local/state/qualia/leash-camera-cross-bf5b6bb/`.
No further test cycle was run; these are deployment readbacks.
