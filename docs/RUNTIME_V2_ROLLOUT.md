# Runtime v2 rollout gates

Status date: 2026-08-29

This is the threshold and execution record for RV2-16. Thresholds in this file
must be committed before the final target soaks. A passing no-motion run does
not authorize motor output, deployment, or rollback.

## Safety boundary

- No-motion stages use simulation, in-process fake actuators, and `/tmp` source
  only. They must not open the Waveshare serial device, send a motor command,
  alter the live service, or replace its binary or configuration.
- The live service remains the legacy runtime until an operator explicitly
  authorizes deployment.
- A supervised physical test requires a stationary secured chassis or lifted
  wheels, a reachable physical E-stop, a second observer, and explicit operator
  authorization immediately before the test.
- Deployment and rollback each require explicit operator authorization. The
  baseline archive must verify before either action.

## Frozen thresholds

| Gate | Required result |
| --- | --- |
| v1/v2 semantic shadow | Every recorded event has identical normalized wheel command and E-stop state; zero mismatches |
| CPU no-motion soak | 10,000 ticks at 100 Hz; zero 10 ms deadline misses; p99 completion jitter at most 1 ms; p99 transition latency at most 1 ms; proposal high-water at most 16 of 32; zero rejected proposals |
| Evidence path | Zero evidence failures or saturation; software stop request p99 at most 1 ms; injected writer stall fails closed within 50 ms |
| CUDA no-motion soak | 20 complete gate-probe iterations; 960 total shadow comparisons; zero parity failures; all 80 injected faults fall back to CPU within the 100 ms compute deadline; concurrent E-stop request observed within 10 ms |
| Target resources | Peak GPU temperature below 80 C and peak system RAM below 3,072 MiB during the final CUDA soak |
| Supervised physical stop | Priority stop and E-stop each obtain verified-zero acknowledgement within 250 ms; zero acknowledgement timeouts; serial command identities remain ordered |
| Deployment health | Active binary and effective configuration hashes are recorded; `/health` is OK and reports the backend actually selected by its startup probe; exclusive device ownership passes |
| Rollback | Captured baseline checksums pass; one rollback command restores byte-identical binary and saved service/configuration; service is active and all post-rollback health, capability, sensor, zero, and ownership checks pass |

Temperature and RAM are platform-protection ceilings, not performance targets.
The physical 250 ms limit is end-to-end through the real serial
acknowledgement; the stricter 10 ms and 1 ms limits above cover software
request paths using fake actuators.

## Repeatable commands

From repository source on the target, with a task-specific state directory:

```bash
LEASH_STATE_DIR=/tmp/leash-rv2-16-state \
  cargo test --offline --release --test runtime_v2_semantic_diff -- --nocapture

cargo run --offline --quiet --release -p leash-runtime \
  --example control_loop_bench -- --ticks 10000

cargo run --offline --quiet --release -p leash-runtime \
  --example evidence_bench -- --samples 1000 \
  --journal /tmp/leash-rv2-16-evidence.journal

cargo run --offline --quiet --release -p leash-cuda --features cuda \
  --example jetson_gate_probe
```

The CUDA command is repeated 20 times while `tegrastats` records resources.
All stdout, hashes, source revision, toolchain versions, and aggregate results
belong in the checked evidence JSON.

## Deployment and rollback commands

These commands are deliberately documented but must not be executed without
the authorization and preconditions above. `ARCHIVE` is the private baseline
directory created on the target; it must never be committed.

```bash
implementations/waveshare-ugv/deployment-baseline.sh verify

implementations/waveshare-ugv/deployment-baseline.sh deploy \
  /tmp/leash-runtime-v2-candidate ARCHIVE --accelerator cpu --confirm

implementations/waveshare-ugv/deployment-baseline.sh rollback \
  ARCHIVE --confirm
```

The deployment step captures candidate, active binary, service, and private
configuration hashes and reuses the verified baseline archive. If startup or
health fails it automatically restores that baseline. Rollback writes its proof
under the archive and compares the restored binary byte-for-byte.

## Current execution state

- The checked semantic fixture and test are
  `examples/contracts/runtime-v1-v2-shadow.json` and
  `tests/runtime_v2_semantic_diff.rs`.
- Thresholds were frozen in commit `2e470b2` before measurement. The exact
  `be7ee6d` source archive then passed the 10-event v1/v2 shadow, the
  10,000-tick CPU soak, the 1,000-decision evidence soak, and 20 CUDA gate
  iterations on the Orin NX. CUDA covered 960 shadow comparisons and 80
  injected faults. There were zero deadline misses, parity failures, evidence
  failures, saturations, or rejected proposals.
- CPU p99 jitter was 224,316 ns, CPU p99 transition latency was 58,306 ns,
  evidence-on p99 stop-request latency was 11,648 ns, maximum CUDA fallback was
  60,918,327 ns, and maximum concurrent E-stop request latency was 205,509 ns.
  Peak RAM was 2,074 MiB and peak GPU temperature was 54.187 C.
- Exact hashes and unrounded results are checked in at
  `crates/leash-runtime/evidence/jetson-orin-nx-rv2-16-nomotion-20260829.json`.
  The live binary hash remained the captured baseline and the service remained
  active after the tests.
- The production `waveshare-ugv` feature now composes the CPU safety supervisor,
  lossless evidence journal, and sole Waveshare serial owner for every physical
  drive, stop, and E-stop. `GET /runtime-v2/status` exposes receipt identities,
  queue watermarks, failure counters, and evidence health for the private soak
  record without changing existing v1 responses.
- A production-equivalent aarch64 candidate with
  `http,mcp,waveshare-ugv,bridge-compat,v4l2-camera,webrtc,cuda,physical-navigation`
  built successfully from exact commit `87d84ef` in isolated LF-canonical
  `/tmp` source. Its binary SHA-256 is `b062d3b1...d48`; the source archive is
  `034f3a4d...24c6`. The target release composition test and CI run
  `33269950579` passed. Exact provenance is checked in at
  `crates/leash-runtime/evidence/jetson-orin-nx-rv2-16-candidate-20260829.json`.
- The exact live baseline was captured read-only under the target's private
  state directory at `20260829T185447Z`. Its archive checksums pass, both service
  environment files are retained, and its binary SHA-256 `6aca86b0...294`
  matches the still-running service.
- Supervised physical testing, deployment, and rollback are pending explicit
  operator authorization.
