# Runtime v2 handoff

Status date: 2026-08-29

## Repository state

- Repository: `C:\Users\ericm\leash`
- Remote: `https://github.com/specdog/leash`
- Branch: `feat/rust-native-runtime-v2`
- Implementation head before this handoff document:
  `4de7d8cddb8d287956d2989a9e56d6aaa99e36ce`
- Branch is pushed and synchronized with origin.
- The branch is 25 commits ahead of `main` at `0426ab1`.
- Tracker: [#209](https://github.com/specdog/leash/issues/209)
- Implementation tickets: [#192](https://github.com/specdog/leash/issues/192)
  through [#208](https://github.com/specdog/leash/issues/208).
- No pull request has been opened, nothing has been merged, and nothing has
  been deployed.

The detailed architecture and every ticket definition of done are in
[`docs/RUNTIME_V2.md`](RUNTIME_V2.md). Do not mark a ticket complete merely
because its host-side code exists; retain the hardware, timing, replay, and
deployment gates written there.

## What is implemented

### Provenance and compatibility

- The deployed baseline, source manifest, binary digest, target CUDA version,
  and UGV identity are captured in
  `implementations/waveshare-ugv/provenance/runtime-v2-baseline.json`.
- Existing wire schemas and golden fixtures are frozen.
- The imported legacy runtime still passes 221 library tests, 4 CLI tests, and
  2 v1 contract tests.
- The live service still uses the legacy runtime. Runtime v2 has not replaced
  it.

### Workspace and owned APIs

The workspace now contains:

- `leash-core`: synchronous owned domain types, SI units, typed frames,
  activities, beliefs, proposals, safety authorization, and deterministic
  control transitions.
- `leash-runtime`: bounded lanes and a dedicated wake-driven CPU safety owner.
- `leash-replay`: strict versioned replay scenarios and stable state/effect
  digest verification.
- `leash-gateway`: one transport-neutral typed command service for future HTTP,
  MCP, and CLI adapters.
- `leash-waveshare`: one owner for serial open/read/write/reconnect/framing,
  with priority stop/e-stop and verified-zero receipts.
- `leash-cuda`: checked-in SM 8.7 fatbin, CPU references, one CUDA context
  owner, persistent grow-only buffers, bounded jobs, deadlines, cancellation,
  circuit breaking, and CPU fallback.
- `leash-ros2`: ROS-installation-free message contracts, checked conversions,
  explicit clock/QoS/queue semantics, and Nav2 proposal dispatch through CPU
  safety.

Core public APIs do not expose Tokio handles, mutex guards, JSON values, CUDA
handles, ROS messages, or borrowed async lifetimes.

### Determinism and replay

The checked scenario is
`crates/leash-replay/fixtures/control-safety-v1.json`.

Frozen results:

- Final state digest: `14161983491101435343`
- Ordered event digest: `922937285294098901`

The scenario covers authorization, planning, drive, obstacle stop, deadman,
stale evidence, e-stop, failed and approved reset, explicit stop, and lease
expiry. It runs without I/O, threads, sleeps, ROS, hardware, or CUDA.

### CPU control timing

The safety owner wakes immediately for normal proposals, stop, e-stop, and
shutdown. A fixed periodic tick remains for deadman and lease expiration.

Orin NX release result over 1,000 ticks at 100 Hz:

- Deadline misses over 10 ms: 0
- p99 completion jitter: 129,390 ns
- p99 transition latency: 59,202 ns
- Proposal queue high-water: 1 of 32

Full provenance and unrounded values:
`crates/leash-runtime/evidence/jetson-orin-nx-20260829.json`.

This used an in-process fake actuator. It is not physical stop-latency proof.

### CUDA result and current policy

Production startup no longer has an NVRTC path. The checked fatbin contains
native SM 8.7 code plus compute 8.7 PTX. The target Orin loaded it through
`cudarc` 0.19.8 against CUDA 12.9, and all six kernels matched their CPU
references. Commit `a18d454` removed the legacy application's remaining inline
voxel/cognition compilers, routed its probe through the single CUDA owner, and
passed the release root selection test from isolated `/tmp` source. Source hash
verification now canonicalizes LF/CRLF so the same artifact contract passes on
Windows and aarch64 Linux.

Commit `956b667` adds advisory collision-sector reduction and a combined
spatial job that uploads one dense lidar scan once for both kernels. Its exact
archive passed 15 release unit tests, 3 artifact tests, and 208 fixed, empty,
one-million-element, and randomized CPU/CUDA jobs on the Orin with the circuit
closed. Two CUDA 12.9 builds were byte-identical; the five-symbol fatbin is
36,032 bytes with SHA-256 `839ac2f5...b77540f`.

Commits `e23b88a` and `44ced55` make explicitly selected CUDA cognition
canonical and resident. The final six-symbol fatbin is 39,080 bytes with
SHA-256 `c96e10ac...a640a4`. The exact Orin archive passed 100 randomized
resident ticks, 312 total executor jobs, bidirectional CPU/CUDA checkpoint
restore, and a root runtime test proving one sequence advance per layer.
Steady root ticks read at most 36 metric bytes; the 4 KiB layer-2 state returns
at 20 Hz and the 45,056-byte full state at the 60-second checkpoint cadence.

The bias-controlled end-to-end benchmark alternated 100 CPU and CUDA samples,
recorded first-use buffer growth separately, and included queueing, transfer,
kernel, synchronization, and readback.

| Workload | Current backend decision | Reason |
| --- | --- | --- |
| Voxel, 204,800 outputs | CPU | CUDA p50 was 6.41 times slower |
| Voxel, 2,560,000 outputs | CPU | CUDA p50 was 4.43 times slower |
| Lidar, 720 points | CPU | CUDA p50 was 3.14 times slower |
| Lidar, 10,000 points | CUDA candidate | 1.64x p50 speedup and better p95 |
| Collision, 720 points | CPU | CUDA p50 was slower |
| Collision, 10,000 points | Advisory only | 6.42x p50 but CUDA p99 regressed |
| Combined spatial, 10,000 points | CUDA candidate | 2.74x p50 and better p95/p99 |
| Camera, 320x240 RGB | CUDA candidate | 1.23x p50 and better p95/p99 in this run |
| Camera, 640x480 RGB | CUDA candidate | 2.10x p50 and better p95/p99 |
| Cognition, 4,096 values | CPU | Upload/readback dominates |
| Cognition, 65,536 values | CPU | Upload/readback dominates |
| Resident cognition, 3x4,096 values | CPU | CUDA p50 was 1.85 times slower |
| Resident cognition, 3x65,536 values | CPU | CUDA p50 was 1.24 times slower |

During the measured RV2-11 run, maximum GPU load was 76%, maximum GPU
temperature was 52.468 C, and maximum board input was 7,707 mW in 10 W mode.
Exact evidence is in
`crates/leash-cuda/evidence/jetson-orin-nx-rv2-11-20260829.json`.

CUDA is not enabled by default. Explicit CUDA cognition is now authoritative
and checkpoint-compatible, but the measured policy remains CPU because neither
resident size beat CPU. Large spatial/lidar and camera remain candidates only
after a startup probe, shadow comparison, and injected-failure fallback proof.

### ROS and Nav2

`leash-ros2` has checked conversions in both directions for scans, IMU, planar
odometry, planar transforms, occupancy maps, localization, and paths. It also
contains explicit clock correlation, QoS declarations, latest-value sensor
queues, bounded reject-newest proposal queues, typed goals/feedback/cancel, and
kinematically validated `cmd_vel` proposals.

The integration proof is:

```text
ROS velocity -> typed proposal -> Nav2 freshness checks
             -> CPU safety supervisor -> Authorized<Drive>
             -> sole Waveshare owner -> fake serial transcript
```

A disconnected Nav2 source requests a priority stop and produces verified-zero
evidence. The ROS crate has no serial, gateway, or CUDA dependency.

The actual `rclrs` executor and rosbag equivalence fixture are not implemented.

## Verification commands

Run from the repository root:

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo run --quiet --features mcp --bin leash-schema -- --check
git diff --check
```

At handoff, all commands passed. The workspace run contains 310 unit,
integration, contract, and compile-fail doctests.

Host control timing probe:

```powershell
cargo run --release -p leash-runtime --example control_loop_bench -- --ticks 1000
```

Jetson CUDA benchmark, from a sourced package with CUDA available:

```bash
cargo run --release --features cuda --example jetson_benchmark -- --iterations 100
```

The CUDA benchmark must run on the Jetson; a normal no-CUDA workspace build
must remain green on developer machines and CI.

## Remaining work, in recommended order

### 1. Make evidence lossless before broader integration (completed after handoff)

Issue: [RV2-06 / #198](https://github.com/specdog/leash/issues/198)

The supervisor's observability event surface is latest-value. Transition
tickets, safety mailbox counters, and controller receipts retain important
identity, but there is not yet a bounded lossless persistence path for every
accepted, rejected, superseded, and verified-zero decision.

Definition of done for the next change:

- Introduce one ordered evidence record type with proposal, command, evidence,
  source, monotonic time, decision, and acknowledgement identity.
- Use a bounded non-blocking producer path from the safety owner to a dedicated
  persistence owner.
- Saturation fails closed and is itself durably observable; it never blocks or
  drops a stop/e-stop.
- Restart recovers the last complete record and detects a torn tail.
- Tests cover acceptance, denial, queue rejection, supersession, zero, failed
  acknowledgement, writer stall, full storage, and restart.
- Record throughput and stop-latency impact on the Orin.

Post-handoff result: implemented and verified on the host and isolated Orin
source tree. The checked format, failure semantics, commands, and evidence are
in [`docs/RUNTIME_V2_EVIDENCE.md`](RUNTIME_V2_EVIDENCE.md) and
`crates/leash-runtime/evidence/*-evidence-20260829.json`. This remains software
request-latency evidence with a fake actuator, not physical motor stop proof.

### 2. Switch legacy HTTP, MCP, and CLI behind the typed gateway

Issue: [RV2-08 / #200](https://github.com/specdog/leash/issues/200)

`leash-gateway` exists, but the legacy surfaces do not call it yet.

Definition of done:

- All three adapters call the same `CommandService`.
- Frozen v1 fixtures remain byte- or semantic-equivalent as specified.
- Stop/e-stop still bypass normal transition waiting.
- No transport server or gateway gains a hardware dependency.

### 3. Finish CUDA shadow and failure gates

Issue: [#205](https://github.com/specdog/leash/issues/205)

- Do not move voxel projection or current cognition to CUDA based on the
  measured results.
- Shadow selected workloads and compare outputs without GPU authority.
- Inject context loss, launch error, timeout, and executor panic.
- Prove CPU fallback completes within one compute deadline while the safety
  lane retains its deadline.
- Advertise CUDA active only after the actual startup probe succeeds.

### 4. Build the target `rclrs` executor and rosbag proof

Issues: [#206](https://github.com/specdog/leash/issues/206) and
[#207](https://github.com/specdog/leash/issues/207)

Perform this in a sourced ROS 2 environment. Keep generated ROS message types
and executor ownership outside `leash-core`.

Definition of done:

- One owned `rclrs` executor maps generated messages to the checked
  `leash-ros2` DTO/domain boundary.
- Callback queue capacities and QoS match the declared contracts.
- Recorded rosbag and Leash fixtures yield equivalent domain events.
- Nav2 restart, ROS partition, stale scan/localization, cancellation, and
  e-stop all request bounded zero motion.
- `/cmd_vel` has no route to serial except through the CPU safety supervisor
  and sole Waveshare owner.
- Builds without ROS remain green.

### 5. Shadow, supervised hardware test, deploy, and roll back

Issue: [RV2-16 / #208](https://github.com/specdog/leash/issues/208)

This requires explicit operator authorization. Do not infer it from this
handoff.

- Run v1 and v2 against the same recorded inputs and review the semantic diff.
- Complete CPU-only and CUDA no-motion soaks.
- Complete supervised physical stop/fault tests with written deadlines.
- Record deployed binary/config hashes and truthful backend health.
- Prove rollback restores the captured baseline and health.

## Safety and target state

- Jetson target: `jetson@192.168.55.1` over the USB gadget connection.
- Do not commit credentials. Obtain the password from the operator when needed.
- The live source is under `/home/jetson/leash-qualia-combined-1f004d6`.
- The live binary is `/home/jetson/.local/bin/leash`.
- Runtime v2 target work performed during this branch is under `/tmp` only.
- Temporary benchmark directories include
  `/tmp/leash-runtime-bench-74a585f`,
  `/tmp/leash-cuda-bench-2392d53`, and
  `/tmp/leash-cuda-bench-64f8dbd`, plus `/tmp/leash-956b667`.
  RV2-12 exact-commit work used `/tmp/leash-44ced55`.
- No runtime v2 test opened the base serial port, sent a motor command, changed
  the live service, or modified its configuration.
- Do not remove or replace the live binary/service without an exact backup,
  explicit operator approval, and a tested rollback command.

## Useful commit landmarks

- `0ad9325` freezes runtime v1 contract fixtures.
- `1cca7f2` introduces owned core domain contracts.
- `3c69c97` adds the deterministic safety kernel.
- `16e32b0` adds bounded orchestration lanes.
- `aa7c506` adds the reproducible Orin CUDA artifact.
- `05d6987` adds the bounded single-owner CUDA executor.
- `ded4b81` adds the single-owner Waveshare controller.
- `1b9212c` isolates the CPU safety supervisor.
- `dac4bfa` adds activities, beliefs, proposals, and outcomes.
- `097ba88` adds the shared typed gateway service.
- `15abf54` adds the ROS/Nav2 boundary and integration proof.
- `583416a` adds the standalone replay oracle.
- `74a585f` makes the safety owner wake-driven and adds the timing probe.
- `0de4962` records Orin CPU timing.
- `64f8dbd` removes ordering bias from the CUDA benchmark.
- `4de7d8c` records CUDA break-even and tegrastats evidence.
- `956b667` adds collision reduction, dense spatial reuse, and randomized parity.
- `e23b88a` makes explicitly selected CUDA cognition resident and canonical.
- `44ced55` proves root CUDA cognition checkpoint restore on the Orin.

Issue comments with implementation evidence were added to #195, #196, #205,
#206, and #207. Leave the CUDA authority, ROS target, physical safety, and
deployment tickets open until their complete definitions of done are met.
