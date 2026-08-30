# Runtime v2 handoff

Status date: 2026-08-30

## Current state

- Repository: `C:\Users\ericm\leash`
- Remote: `https://github.com/specdog/leash`
- Branch: `feat/rust-native-runtime-v2`
- Candidate implementation commit: `87d84ef48027e90657b979349aa2cd6d1cbbdf6c`
- Post-E-stop-reset safety correction commit:
  `abebece2dd89382f9fff8049848a4cbbc8354761`.
- Rollout direction/configuration hardening commit:
  `9cc6c685c66b81227d2f56e049310b3c0cb0d9e1`.
- No pull request is open. Nothing has been merged.
- Runtime-v2 issues #192 through #209 are closed. Rollout issue #208 and tracker
  #209 were reopened for the live Stop regression described below, then closed
  again only after the fix, target redeployment, physical proof, and exact-head
  CI passed.
- The exact candidate is live with required CUDA selected, the CPU safety
  supervisor retains final authority, telemetry is zero, and the service is
  reachable independently over Wi-Fi.

## Post-handoff safety correction

A supervised room-exploration command exposed a controller-owner bug after an
approved E-stop reset. The old candidate reused the historical E-stop receipt
for a later Stop instead of writing a new zero command. Stop acknowledgement
timed out; the network E-stop then applied verified zero. No safety gate was
bypassed and no further motion was sent on the old binary.

Commit `abebece2` restricts E-stop coverage of Stop to the period while the
E-stop latch is actually active and reports the newest safety receipt by
applied sequence. The regression test
`stop_after_estop_reset_writes_a_fresh_verified_zero` covers E-stop, approved
reset, drive, and a later fresh verified Stop.

The exact aarch64 candidate `77e01eba...86f0c` was built on the Jetson, deployed
with required CUDA and the accepted direction mapping, and exercised through
that same physical sequence. The Stop receipt advanced from sequence 5 to 106
and verified hardware zero. A subsequent bounded Wi-Fi room exploration
covered approximately 0.805 m of forward wheel odometry with camera and lidar
checks between legs; every leg ended with a fresh verified-zero receipt. The
final receipt is sequence 1059, both raw motor outputs are zero, and the front
lidar minimum is 1.319 m. Exact evidence is in
`../crates/leash-runtime/evidence/jetson-orin-nx-rv2-stop-after-reset-20260830.json`.

The architecture, invariants, and ticket definitions of done are in
[`RUNTIME_V2.md`](RUNTIME_V2.md). The frozen rollout thresholds and execution
record are in [`RUNTIME_V2_ROLLOUT.md`](RUNTIME_V2_ROLLOUT.md).

## Architecture delivered

- `leash-core` owns deterministic synchronous domain types, SI units, typed
  frames, activities, beliefs, proposals, safety authorization, and control
  transitions. It exposes no runtime, transport, CUDA, or ROS implementation
  types.
- `leash-runtime` owns bounded lanes, the wake-driven CPU safety supervisor,
  and the durable lossless evidence journal. Stop and E-stop have priority and
  cannot be delayed by normal proposal or evidence persistence work.
- `leash-replay` owns strict versioned replay fixtures and stable state/effect
  digest verification.
- `leash-gateway` owns the transport-neutral command service used by HTTP,
  MCP, and CLI adapters while retaining the frozen v1 wire contracts.
- `leash-waveshare` owns the only serial open/read/write/reconnect/framing
  path. Applied drive, stop, and E-stop receipts carry ordered request and
  acknowledgement identities and verified-zero state.
- `leash-cuda` owns one CUDA context, checked SM 8.7 artifacts, persistent
  grow-only buffers, bounded jobs, deadlines, cancellation, circuit breaking,
  parity shadowing, and CPU fallback. CUDA never owns motor authority.
- `leash-ros2` owns the native Humble `rclrs` executor, generated-message
  conversion boundary, declared QoS, bounded callback queues, clock
  correlation, and Nav2 proposal path through CPU safety.
- The production `waveshare-ugv` feature composes the CPU safety supervisor,
  durable evidence journal, and sole Waveshare controller owner. The additive
  `/runtime-v2/status` endpoint exposes the private rollout proof without
  changing existing v1 response shapes.

The physical authority chain is:

```text
HTTP / MCP / CLI / Nav2
        -> typed gateway proposal
        -> CPU safety supervisor (final command authority)
        -> Authorized<Drive> or priority zero
        -> sole Waveshare controller owner
        -> serial acknowledgement and durable evidence
```

## Verification record

### Host and CI

Candidate implementation `87d84ef` passed GitHub Actions run `33269950579`:

- format and all-feature clippy with warnings denied;
- workspace all-target/all-feature tests and doctests;
- no-default-feature and every supported feature-matrix build;
- schema, packaging, adapter-contract, and full smoke suites;
- hardware-adapter feature matrix.

The frozen v1 contracts and semantic shadow remain green. The production
fake-I/O composition test proves that drive reaches the controller only after
CPU authorization, stop and E-stop return ordered verified-zero receipts, and
the evidence journal remains healthy with no acknowledgement failures.

### CPU, evidence, CUDA, and replay on Jetson

Exact no-motion evidence is checked in at
`../crates/leash-runtime/evidence/jetson-orin-nx-rv2-16-nomotion-20260829.json`.
It records:

- v1/v2 semantic shadow: 10 of 10 matching events, zero mismatches;
- CPU soak: 10,000 ticks at 100 Hz, zero 10 ms deadline misses, p99 jitter
  224,316 ns, p99 transition latency 58,306 ns;
- evidence soak: 1,000 decisions, 100 stops, 3,202 durable records, zero
  evidence failures, p99 evidence-on stop request 11,648 ns;
- CUDA soak: 20 gates, 960 comparisons, zero parity failures, all 80 injected
  faults fell back to CPU, maximum fallback 60.918 ms, maximum concurrent
  E-stop request 205.509 us;
- 457 resource samples, maximum 2,074 MiB RAM and 54.187 C GPU temperature.

These runs used fake actuators, never opened serial, and make no physical stop
latency claim.

### Native ROS 2 Humble and Nav2

Native Humble run `33269287419` succeeded from exact commit `607f3e3`. It built
the generated messages and `rclrs` boundary, exercised all eight bounded
callbacks with the declared QoS, and reported no boundary errors and
`hardware_access=false`. Its checked evidence is
`../crates/leash-ros2/evidence/github-humble-native-20260829.json`.

The durable Nav2 proof covers restart, ROS partition, stale scan/localization,
cancellation, and E-stop. `/cmd_vel` has no path to serial except through the
CPU safety supervisor and sole Waveshare owner. Issues #206 and #207 are
closed.

## CUDA decisions

CUDA is explicit and truthful, never silently selected. Startup must prove the
requested backend; `LEASH_REQUIRE_ACCELERATOR=true` makes a failed requested
backend fail startup rather than degrade unnoticed.

- Current cognition remains CPU because neither measured resident size beat
  CPU end to end.
- Small voxel, lidar, and collision workloads remain CPU.
- Dense lidar and combined spatial jobs are CUDA candidates after startup
  probe, parity shadow, and injected-failure fallback gates.
- Camera preprocessing is a CUDA candidate at the measured resolutions.
- Collision reduction remains advisory; the CPU safety supervisor remains the
  final authority regardless of acceleration backend.

The checked CUDA artifact contains native SM 8.7 code plus compute 8.7 PTX and
has no production NVRTC path.

## Exact candidate and rollback baseline

The production-equivalent candidate was built on the Orin NX from a canonical
LF archive of `87d84ef`, using Rust/Cargo 1.94.0 and features:

```text
http,mcp,waveshare-ugv,bridge-compat,v4l2-camera,webrtc,cuda,physical-navigation
```

- Source archive SHA-256: `034f3a4d64def73fff896605ab01cf1a92b0f3ce8864b9d15d8f89ad359524c6`
- Vendor archive SHA-256: `43196bc1b4cd31d6acb23fb12b2043199032a350412d94a462f678ea90dc5a08`
- Candidate binary SHA-256: `b062d3b102f4112afc59e7e89e43eea454281e4c449ede881684a66754a0ad48`
- Checked provenance:
  `../crates/leash-runtime/evidence/jetson-orin-nx-rv2-16-candidate-20260829.json`

The exact running baseline was captured read-only in private target state as
archive `20260829T185447Z`:

- live/baseline binary SHA-256:
  `6aca86b0faf761c94bbc825657bdab3a0a7eb7b83d45bc2771eb0fff0c4cc294`;
- two effective environment files and the effective systemd user unit are
  retained with private permissions;
- all eight archived payload checksums pass;
- manifest SHA-256:
  `e424cc8e9ad191b1f2e30fdcfdc1c193e83d06dda4d1429cc91cfcbb0117f332`;
- archive checksum-file SHA-256:
  `518b42b59808682ad2a371798829137aee6d291cd38664e029b57ebdfdf4814c`.

The deploy tool requires an explicit `cpu` or `cuda` backend, installs a
private candidate environment atomically, verifies active and required backend
health plus the runtime-v2 authority/evidence status, and automatically
restores the baseline if any gate fails. The rollback command restores the
binary, unit, and environment byte-for-byte and reruns health, camera, sensor,
zero-motion, ownership, and checksum checks.

The first supervised direction observation established that this chassis maps
positive logical drive backward when the baseline's
`LEASH_DRIVE_INVERT=false` is retained. That attempt was stopped and rolled
back. Candidate deployments must explicitly use `--drive-invert true
--drive-swap false`, verify one short forward pulse under operator observation,
and retain the resulting candidate configuration hash.

## Jetson and safety state

- A saved Wi-Fi profile is active with autoconnect. SSH, HTTP health, and the
  final CUDA deployment succeeded over Wi-Fi, so the USB gadget connection is
  no longer required for management. Network identity and credentials were
  supplied out of band and are intentionally not recorded in the repository.
- Live source: private deployment workspace (path intentionally redacted)
- Live binary: `/home/jetson/.local/bin/leash`
- Candidate and build work are isolated under `/tmp`; private deployment proof
  remains under the user's state directory.
- The live service is active with candidate hash `b062d3b1...d48`, active and
  required CUDA, `LEASH_DRIVE_INVERT=true`, and `LEASH_DRIVE_SWAP=false`.
- The operator confirmed the physical preconditions. The accepted mapping was
  observed moving forward, CPU-supervised hardware exercise retained all
  safety gates, exact rollback passed, and required-CUDA E-stop obtained
  verified zero in 37.622 ms.
- Final telemetry is zero, E-stop is reset, the controller is connected, and
  controller-write, acknowledgement, supervisor, and evidence failures are
  zero. CUDA has no motor authority.
- The guarded soak tool now defaults to one supervised pulse followed by
  stationary monitoring instead of repeatedly cycling the wheels. It retains
  the 0.10 command cap and 250 ms stop/E-stop threshold.

## Rollout completion

The authorized physical rollout completed with these results:

1. the first false-inversion direction attempt moved backward, was stopped with
   verified zero, rejected as evidence, and rolled back;
2. explicit `--drive-invert true --drive-swap false` produced forward motion
   and verified-zero confirmations of 88 ms and 11 ms;
3. the CPU-supervised exercise recorded 44 approved forward commands, zero
   write/acknowledgement/fault/evidence failures, and one safely enforced lidar
   collision rejection;
4. rollback `rollback-20260829T193520Z` restored the captured legacy binary
   byte-for-byte and passed every post-rollback gate;
5. CUDA deployment `deploy-20260829T193638Z` selected the real required CUDA
   backend without moving motor authority from CPU, and its real serial E-stop
   obtained verified zero in 37.622 ms;
6. the candidate remains live, healthy, stopped, and reachable over Wi-Fi.

Exact scrubbed results are in
`../crates/leash-runtime/evidence/jetson-orin-nx-rv2-16-physical-rollout-20260829.json`.
The only remaining administrative sequence is exact-head CI, closing #208,
then closing tracker #209 after verifying all children are closed.

## Commit landmarks

- `0ad9325` freezes the runtime-v1 contracts.
- `1cca7f2` and `3c69c97` add the owned core contracts and deterministic safety
  kernel.
- `16e32b0` and `1b9212c` add bounded orchestration and the wake-driven CPU
  safety owner.
- `ded4b81` adds the sole Waveshare controller.
- `097ba88` adds the shared typed gateway.
- `583416a` adds deterministic replay.
- `05d6987`, `956b667`, and `44ced55` establish the bounded CUDA owner,
  workload gates, and checkpoint-compatible resident cognition.
- `9fb8a83` and `85d00d6` add the native Humble executor and durable Nav2 proof.
- `e614532` records the final no-motion rollout soaks.
- `0952b59` adds guarded atomic deployment and rollback.
- `bbce410` composes runtime-v2 as the production physical authority.
- `87d84ef` exposes and tests the supervised physical rollout proof.
- `4a26fc5` freezes the exact physical candidate and rollback baseline.
- `9cc6c68` makes the observed wheel mapping an explicit, fail-closed
  deployment choice.

No pull request or merge should be inferred from this handoff. The guarded
candidate deployment described above is active on the single authorized UGV.
