# Read-only directional lidar clearance

`GET /motors/clearance` exposes the existing Waveshare collision and freshness
decision as `leash.motor-clearance.v1`. It reads the latest scan and does not
acquire authority, write to a device, change a gate, or request a drive.

`forward`, `reverse`, and `rotation` report `blocked`, `collision_blocked`,
`reason`, the closest observed range and bearing, and valid/unknown bin counts.
Forward and reverse use the existing 120-degree sectors centered at logical
body 0 and 180 degrees. In-place rotation uses the full scan. Each collision
decision calls the same `scan_blocks_drive` function used for actual drives;
freshness calls the same `with_freshness` function. A stale or unavailable scan
reports blocked even when no collision decision can be made. Unknown bins
remain unknown; this endpoint does not certify a swept footprint or clear space.

Top-level fields include the scan timestamp and age in milliseconds, freshness
deadline, collision threshold, configured yaw/clockwise/body masks, frame ID,
and drive inversion/swap. `grants_motion_authority` is false and `gate_scope` is
limited to lidar collision and freshness: lease, estop, acknowledgement, and
other checks still apply independently.

Read-only inspection on 2026-09-12 found the configured collision threshold
unset (default 0.25 m), freshness unset (default 500 ms), yaw unset (default
180 degrees), clockwise unset (default false), and body mask `45:136`.
The active file was `/home/jetson/.config/leash/leash.env`; the additional
camera-stream environment file had no yaw or mask override. Accepted earlier
motor evidence documents `LEASH_DRIVE_INVERT=true` and swap false as the mapping
from positive logical drive to body forward. Inversion occurs after the lidar
decision, so the collision signs must not be inverted a second time.

The operator subsequently identified the centered camera's forward corner with
the scan near -90 degrees. The requested alignment adds 90 degrees to yaw
(180 to 270), and rotates the existing mask by the same amount (`45:136` to
`135:226`, equivalently `135:-134`). The assembler transforms raw angles before
masking, and the mask normalizes endpoints modulo 360; these paired changes
preserve the exact same inclusive raw masked rays. This is an operator-observed
alignment, not measured metric extrinsics. Configuration application and live
verification belong to the supervising operator; this source change does not
apply that configuration. The endpoint reports the effective configuration
without claiming a calibrated mount.

The current Qualia scan plot maps angle zero to screen right and positive
90 degrees to screen up; its forward arrow also points right. Screen compass
position alone is not a mounting calibration.

Implementation was authored from the local runtime and adapter contracts.
No external implementation was copied. No hardware commands or new test cycle
were used for this telemetry change; the production build is recorded separately.

## Production artifact

Source commit `071fb2fe617448a00b947d56e31a9f77dbef4290` passed one cached
production cross-build in 1 minute 50 seconds, using Rust 1.94.1, `--locked`,
`-j2`, and target `aarch64-unknown-linux-gnu.2.35`. Its Cargo fingerprint
contains all eight installed features: `http`, `mcp`, `waveshare-ugv`,
`bridge-compat`, `v4l2-camera`, `webrtc`, `cuda`, and `physical-navigation`.
ELF inspection confirms AArch64 and maximum required GLIBC 2.34, compatible
with the robot's 2.35. The executable SHA-256 is
`a5a31c2c89b981f94f485a8c8791ae73b1de9cc71d595e6f8dbdb0cfaf86a887`.
The host artifact, exact source archive, scripts, build log, fingerprint, and
ELF inspection are under
`C:/Users/ericm/.local/state/qualia/leash-clearance-cross-071fb2f/`.
Deployment and live endpoint observations are separate from this build receipt.

## Remaining orientation limit

After implementation, a read-only audit of the official LDROBOT ROS2 driver
at `bf668a89baf722a787dadc442860dcbf33a82f5a` found that its
[counterclockwise output reverses raw-angle bin order](https://github.com/ldrobotSensorTeam/ldlidar_stl_ros2/blob/bf668a89baf722a787dadc442860dcbf33a82f5a/src/demo.cpp#L215).
The [LD06 launch configuration](https://github.com/ldrobotSensorTeam/ldlidar_stl_ros2/blob/bf668a89baf722a787dadc442860dcbf33a82f5a/launch/ld06.launch.py)
defaults to that mode. Leash's configured clockwise false preserves raw sign.
A single front-target correspondence establishes yaw alignment but does not
establish the physical left/right handedness of this mounting. Reflection
about the forward axis does not change the symmetric forward/reverse clearance
sectors; lateral turn-away decisions still need a known left/right observation.
No vendor implementation was incorporated and no clockwise setting was changed.
