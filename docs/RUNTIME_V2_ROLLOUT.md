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

implementations/waveshare-ugv/deployment-baseline.sh rollback \
  ARCHIVE --confirm
```

The deployment step must first capture the candidate binary/configuration
hashes and reuse the verified baseline archive. Rollback writes its proof under
that archive and compares the restored binary byte-for-byte.

## Current execution state

- The checked semantic fixture and test are
  `examples/contracts/runtime-v1-v2-shadow.json` and
  `tests/runtime_v2_semantic_diff.rs`.
- Prior target CPU, evidence, CUDA parity/fault, temperature, and RAM artifacts
  remain valid component evidence, but the final 10,000-tick and 20-iteration
  RV2-16 soaks are recorded separately.
- Supervised physical testing, deployment, and rollback are pending explicit
  operator authorization.
