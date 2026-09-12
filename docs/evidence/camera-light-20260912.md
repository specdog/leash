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
