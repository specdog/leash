# leash-core

`leash-core` is the synchronous, deterministic domain boundary for Leash
runtime v2. It owns no device and performs no I/O.

The crate accepts owned inputs stamped with explicit monotonic time and returns
owned, fixed-capacity effects. Units, frames, normalized drive values, command
identity, and authorization evidence are distinct Rust types.

Deliberate exclusions:

- Tokio and async traits
- serde and JSON
- HTTP and MCP
- ROS messages or executors
- CUDA contexts or device pointers
- serial ports, filesystems, and process environment
- mutexes and channels

Those facilities belong to orchestration and gateway crates. This crate remains
portable, replayable, and suitable for virtual-time tests.

`ControlKernel` is the first v2 transition engine. Every call supplies a
monotonic timestamp and increasing sequence. It returns at most eight effects
in an inline array and performs no I/O. Its golden scenario fixes both state and
event digests while exercising authorization, drive, obstacle stop, stale
evidence, deadman, e-stop/reset, planner cancellation, and lease expiry.
