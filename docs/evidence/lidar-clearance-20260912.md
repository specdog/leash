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
