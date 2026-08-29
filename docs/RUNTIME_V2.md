# Rust-native runtime v2

This is the execution contract for turning the current Leash harness into a
Rust-native, type-driven robot runtime with deterministic control, one owner for
each physical device, direct ROS 2 interoperability, and deliberate CUDA
acceleration on Jetson.

The work is tracked as small tickets. A ticket is not done because code exists;
it is done only when every listed proof is checked in and repeatable.

## Current truth

The baseline was captured on 2026-08-29 and is recorded in
`implementations/waveshare-ugv/provenance/runtime-v2-baseline.json`.

- The imported deployed source passes 221 library tests and 4 CLI tests with all
  features enabled.
- The checked-in external JSON schema is generated and current.
- CUDA is present in the code, but it is not the live compute plane. Voxel
  projection JIT-compiles one inline kernel with NVRTC and performs synchronous
  allocation, upload, launch, and readback. Cognition runs the CPU update first,
  then launches a GPU update whose state is not read back into canonical state.
- The live UGV reports `accelerator.active = none` and cognition `backend = cpu`,
  even though CUDA support is compiled and the Jetson exposes a GPU.
- The target is an aarch64 Jetson Orin NX with CUDA 12.9 and compute capability
  8.7. The current Cargo feature selects cudarc's CUDA 12.2 API surface, so
  toolchain compatibility must be proved rather than assumed.

CUDA is therefore part of this plan, but motor arbitration, deadman, e-stop,
command limiting, and the final serial write stay on the CPU. A GPU fault must
never delay or bypass a stop.

The first CUDA gate is now implemented in `leash-cuda`: production code loads a
checked-in SM 8.7 fatbin with a compute 8.7 PTX fallback, and a bounded
single-owner executor keeps its context and persistent device buffers private.
On 2026-08-29, both the CUDA Driver API probe and the Rust `cudarc` executor
probe passed all four kernels on the target Orin NX without opening the compute
circuit or touching motion hardware. Artifact hashes, compiler flags, and the
target result are recorded in the CUDA artifact manifest. CUDA remains
non-authoritative until the timing, fault-injection, and shadow gates in RV2-13
are complete.

The RV2 Waveshare boundary is also available as `leash-waveshare`. A single
named owner thread holds the serial factory and live stream, performs all reads,
writes, framing, and reconnects, and writes a verified zero before accepting
normal work after every connection. Normal commands use a bounded reject-newest
lane; stop and e-stop use the atomic priority mailbox, flush queued work, and
retain separate request-range receipts. Partial writes, malformed telemetry,
disconnect/reconnect, saturation, and owner panic are covered by fake serial
transcripts. This boundary is not connected to the live service yet, so the
existing driver remains the deployment path until the shadow gate.

`leash-runtime` now owns a dedicated `leash-cpu-safety` supervisor thread. It
drives the deterministic kernel from an injected monotonic clock, rejects stop
and e-stop on the normal bounded proposal lane, and routes every zero effect to
the atomic controller safety mailbox. Normal authorized drive submission is
non-blocking; failed or rejected actuator acknowledgements fault the supervisor
and schedule e-stop. Tests bound priority-stop handling under proposal overload
to 50 ms on the host and prove it remains live while an unrelated compute or
gateway worker is stalled. The configured production target remains 100 Hz;
Jetson deadline and fault-injection evidence is still required before Gate C.

The DIMOS-style domain vocabulary now lives in `leash-core`, not in transport
JSON: versioned owned activities, states, intents, observations, framed beliefs,
proposals, effects, and outcomes. Activity transitions are total and distinguish
illegal edges from reversed time; beliefs carry source, typed frame, timestamp,
precision, expiry, and non-empty evidence lineage; competing proposals resolve
deterministically by freshness, priority, then typed ID. Drive remains only a
proposal until the safety kernel produces the unforgeable `Authorized` wrapper.

`leash-gateway` now provides the common edge service for future HTTP, MCP, and
CLI adapters. It strictly decodes owned DTOs, validates operator IDs and drive
ranges before constructing domain commands, waits on typed transition tickets
with an explicit timeout, and renders stable effect DTOs. Stop and e-stop never
enter that normal proposal lane; they acknowledge acceptance by the atomic
safety mailbox immediately. The legacy surfaces retain their frozen wire
contracts until they are switched to this service behind compatibility tests.

## Runtime shape

The core is a synchronous state transition system. It accepts owned, typed
inputs for one logical tick and returns owned, typed effects. It does not know
about Tokio, HTTP, MCP, ROS, CUDA, serial ports, or wall-clock time.

```text
sensor owners -> typed observations -> deterministic core -> typed effects
                                                   |             |
                                                   |             +-> sole controller owner
                                                   +-> compute jobs -> CPU or CUDA executor

HTTP / MCP / CLI <-> gateway conversions <-> commands, queries, and snapshots
ROS 2            <-> ROS conversions     <-> observations and proposals
```

The intended trait vocabulary is deliberately small:

- `Clock`: supplies explicit monotonic time to orchestration.
- `Controller`: `step(&mut self, Tick<Input>) -> Result<Effects<Output>, Error>`
  using owned values and no borrowed futures.
- `SensorSource`: owns a sensor and emits timestamped typed samples.
- `ActuatorSink`: owns an actuator and applies already-authorized commands.
- `ComputeBackend`: accepts closed compute jobs and returns typed results; CUDA
  objects never cross the executor boundary.
- `Gateway`: converts transport DTOs to core commands and core results back to
  wire DTOs.

Large immutable payloads may use `Arc<[T]>` at an orchestration boundary. The
domain API does not expose mutex guards, channels, runtime handles, CUDA device
pointers, ROS messages, JSON values, or lifetimes tied to an adapter.

## Non-negotiable invariants

1. Exactly one task or thread owns each serial port, camera, lidar, and CUDA
   context.
2. All control-path queues are bounded and declare overflow behavior.
3. Stop and e-stop have a CPU-only priority path and cannot wait behind CUDA,
   perception, logging, HTTP, ROS, or normal motion commands.
4. The same input sequence and clock sequence produce the same state, effects,
   and replay digest.
5. HTTP, MCP, CLI, and ROS are adapters; none contains policy or controller
   logic and none writes motors directly.
6. GPU output is authoritative only after parity, fault, and timing gates pass.
   Until then it runs in comparison or shadow mode.
7. A feature or backend is not advertised as available until its real startup
   probe succeeds on the target.

## Tickets and definitions of done

GitHub tracker: [#209](https://github.com/specdog/leash/issues/209). The
implementation tickets are [#192](https://github.com/specdog/leash/issues/192)
through [#208](https://github.com/specdog/leash/issues/208), in RV2 order.

### RV2-00 - Freeze provenance and external contracts

Goal: make the deployed baseline reproducible before moving code.

Done when:

- the source-manifest and deployed-binary hashes are checked in without private
  host or credential data;
- all-features tests pass from a clean build;
- the generated schema check passes;
- golden fixtures cover health, capabilities, telemetry, action evidence,
  cognition boundary, replay, and Waveshare zero-command encoding;
- CI rejects an unreviewed wire-schema or fixture change.

### RV2-01 - Create the workspace and enforce its dependency DAG

Goal: split compilation boundaries without changing behavior.

Target crates: `leash-core`, `leash-runtime`, `leash-replay`, `leash-gateway`,
`leash-waveshare`, `leash-cuda`, `leash-ros2`, plus a `leash-harness` facade and
CLI for compatibility.

Done when:

- every crate builds with documented feature sets on Linux and Windows where
  applicable;
- `leash-core` has no Tokio, HTTP, MCP, ROS, CUDA, serialport, or filesystem
  dependency;
- a dependency-policy test rejects reverse edges and feature leakage;
- the existing CLI, schemas, and 225 baseline tests remain green.

### RV2-02 - Add owned domain types and compile-time API contracts

Goal: make invalid control states hard to construct without complicated
lifetimes.

Done when:

- monotonic timestamps, durations, sequence numbers, frames, SI units,
  normalized drive values, safety state, command IDs, and evidence IDs have
  explicit types and checked constructors;
- `Controller`, `SensorSource`, `ActuatorSink`, and `ComputeBackend` contracts
  compile with owned messages and associated types;
- trybuild tests reject wrong units, wrong frames, unvalidated drive values, and
  direct actuator access from a gateway;
- core public types contain no `Arc<Mutex<_>>`, Tokio types, JSON values, CUDA
  handles, ROS messages, or borrowed async trait methods.

### RV2-03 - Build the deterministic control kernel and replay oracle

Goal: replace independent timer tasks with one explicit transition engine.

Done when:

- a caller-supplied clock drives deadman, arbitration, planning, patrol,
  cognition scheduling, and evidence sequencing;
- each tick returns effects without performing I/O;
- a checked-in scenario covers authorize, drive, obstacle stop, stale sensor,
  deadman, e-stop, reset, and planner cancellation;
- CPU architectures produce the same canonical event digest for that scenario;
- tests can advance time without sleeping and contain no process-global env
  mutation.

### RV2-04 - Add bounded orchestration lanes and explicit ownership

Goal: keep async synchronization outside the deterministic core.

Done when:

- the runtime has documented safety, control, sensor, compute, and observability
  lanes with bounded capacities and overflow policies;
- stop/e-stop preempts normal work and is never dropped;
- sensor overload uses latest-value or bounded-loss semantics by contract;
- backpressure, task panic, slow subscriber, and shutdown tests pass;
- a 100 Hz host control-loop benchmark reports p50, p95, p99, maximum jitter,
  queue depth, and missed deadlines.

### RV2-05 - Make the Waveshare controller a single-owner actor

Goal: remove cloned serial ownership and put every base command through one
controller mailbox.

Done when:

- one owner opens the base serial port and performs command and telemetry I/O;
- command acknowledgements carry command ID, applied sequence, monotonic time,
  and verified-zero evidence;
- gimbal and base commands cannot interleave malformed JSONL frames;
- disconnect, partial write, corrupt telemetry, reconnect, and queue-saturation
  tests fail closed;
- a fake serial transcript proves no code path writes the motor device outside
  the owner.

### RV2-06 - Isolate the CPU safety supervisor

Goal: make safety independent from gateways, planning, ROS, and GPU health.

Done when:

- arbitration, limits, deadman, collision stop, odometry bound, token expiry,
  stop, and e-stop execute in the CPU safety lane;
- every accepted, rejected, superseded, and zero command has lossless evidence;
- injected compute stalls, CUDA faults, ROS loss, and gateway overload do not
  delay the next safety decision;
- stop remains available under every policy denial;
- fault-injection tests state exact stop deadlines and pass on the Jetson.

### RV2-07 - Model activities and beliefs as typed state machines

Goal: give DIMOS-like behavior a Rust-native vocabulary instead of dynamic
JSON and cross-cutting mutex state.

Done when:

- `Activity`, `ActivityState`, `Intent`, `Observation`, `Belief`, `Proposal`,
  `Effect`, and `Outcome` are closed, versioned domain types;
- activity transitions are total and illegal transitions return typed errors;
- beliefs retain source, frame, timestamp, precision, expiry, and lineage;
- planners and cognition can propose effects but only safety can authorize
  actuation;
- replay tests cover start, suspend, cancel, succeed, fail, stale belief, and
  competing proposal behavior.

### RV2-08 - Reduce HTTP, MCP, and CLI to gateway adapters

Goal: keep wire compatibility while removing transport logic from the core.

Done when:

- all three surfaces call the same typed command/query service;
- JSON parsing and schema generation live at the edge;
- contract fixtures from RV2-00 are byte- or semantic-equivalent as specified;
- gateway cancellation cannot cancel an already accepted safety stop;
- no gateway crate depends on a hardware implementation crate.

### RV2-09 - Build reproducible CUDA artifacts for Orin

Goal: remove production-startup NVRTC compilation and make kernel provenance
visible.

Done when:

- CUDA kernels live in `.cu` files with unit-testable host contracts;
- the build produces an SM 8.7 cubin/fatbin for Orin plus a documented PTX
  fallback and records compiler flags, CUDA version, and artifact SHA-256;
- release startup loads prebuilt artifacts and performs no NVRTC compilation;
- the CUDA 12.9 target and cudarc API compatibility are proven in CI or a
  reproducible Jetson build job;
- a no-CUDA build remains supported and contains no CUDA runtime dependency.

### RV2-10 - Add one CUDA executor with persistent memory

Goal: own the CUDA context once and remove per-call allocation and copies.

Done when:

- one worker owns the context, streams, modules, events, and device buffers;
- job queues are bounded and prioritized, with explicit cancellation and
  deadline behavior;
- buffers are pooled or persistent and steady-state jobs allocate no device
  memory;
- safety and controller types cannot contain CUDA handles;
- context loss, launch error, timeout, and executor panic open a circuit breaker
  and return typed failures without crashing the runtime.

### RV2-11 - Move spatial and perception preprocessing to CUDA

Goal: use the GPU for parallel workloads that amortize transfer cost.

Kernels: occupancy-to-voxel projection, lidar transform/filter/binning,
collision-sector reduction for advisory evidence, and camera tensor
normalization when a GPU perception provider consumes it.

Done when:

- every kernel has a scalar CPU reference and randomized parity tests;
- dense inputs remain resident across adjacent jobs where possible;
- empty, maximum-size, NaN, infinity, malformed, and overflow inputs are
  handled without undefined behavior;
- end-to-end benchmarks include transfers and synchronization;
- the CPU safety supervisor independently computes the final collision stop.

### RV2-12 - Make CUDA cognition authoritative

Goal: eliminate the current CPU-then-GPU duplicate update.

Done when:

- exactly one selected backend advances cognition state per tick;
- CUDA sensor, layer, weight, bias, and top-down state stay resident on device;
- only bounded snapshots and checkpoints read back at declared cadences;
- checkpoint restore works across CPU and CUDA backends with a versioned state
  format;
- backend status reports selected, active, degraded, and fallback reasons
  truthfully.

### RV2-13 - Prove CUDA parity, fallback, and useful speed

Goal: enable CUDA only when it is correct and beneficial on the actual UGV.

Done when:

- CPU/CUDA outputs meet declared absolute and relative tolerances over golden
  and randomized workloads;
- a shadow run compares both backends without giving GPU output authority;
- `tegrastats` and Nsight evidence record latency, transfers, memory, GPU load,
  CPU load, power mode, and thermal behavior;
- voxel, lidar, cognition, and camera jobs each have an end-to-end break-even
  result; workloads slower on GPU remain on CPU;
- injected GPU failure falls back within one compute deadline while the CPU
  safety lane continues meeting its deadline.

### RV2-14 - Add a native Rust ROS 2 boundary

Goal: replace the implementation-owned Python bridge with a feature-gated Rust
adapter while keeping ROS out of the core.

Done when:

- `leash-ros2` converts typed Leash scans, IMU, odometry, transforms, maps,
  localization, and paths to and from ROS messages;
- executor ownership, QoS, callback queues, and timestamps are explicit and
  bounded;
- ROS callbacks submit observations or proposals and never call hardware;
- recorded rosbag and Leash replay fixtures produce equivalent domain events;
- builds without a ROS installation remain green.

### RV2-15 - Connect Nav2 as a proposal source, not a motor owner

Goal: talk to ROS controllers and Nav2 without creating a second command path.

Done when:

- goals, feedback, cancellation, path proposals, and velocity proposals have
  typed adapters;
- proposed velocity is revalidated by the Leash safety supervisor and applied
  only by the Waveshare controller owner;
- `/cmd_vel` cannot reach the serial device except through that path;
- stale localization, stale lidar, Nav2 restart, ROS partition, cancelled goal,
  and e-stop all produce bounded zero motion;
- simulation and replay tests prove the full proposal-to-evidence chain.

### RV2-16 - Shadow, benchmark, deploy, and roll back

Goal: replace the live harness only after equivalent behavior is demonstrated.

Done when:

- the v1 and v2 cores consume the same recorded inputs and produce a reviewed
  semantic diff;
- CPU-only and CUDA modes complete a no-motion soak and a supervised hardware
  soak with saved timing evidence;
- control p99, deadline misses, queue high-water marks, stop latency, GPU fault
  recovery, temperature, and memory remain inside written thresholds;
- the deployed binary and config hashes are recorded and health reports the
  active backend accurately;
- rollback restores the captured baseline binary and service configuration with
  one documented command and a post-rollback health proof.

## Phase gates

- Gate A: RV2-00 is complete before public contracts or file layout move.
- Gate B: RV2-01 through RV2-04 are complete before v2 can drive a simulated
  actuator.
- Gate C: RV2-05 and RV2-06 pass shadow and fault tests before any v2 physical
  command is allowed.
- Gate D: RV2-09 through RV2-13 pass parity and Jetson timing proof before CUDA
  becomes authoritative.
- Gate E: RV2-14 and RV2-15 pass replay and partition tests before ROS proposals
  can be authorized for physical motion.
- Gate F: RV2-16 is complete before v2 replaces the current service.

The gate order is intentional. CUDA and ROS improve throughput and integration;
neither is allowed to become a new safety authority.
