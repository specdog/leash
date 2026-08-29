# Runtime v2 lossless evidence journal

Status: implementation, host verification, and the isolated Jetson timing and
fault gate are complete. This is not physical motor stop proof.

## Contract

`leash-runtime` owns one ordered `EvidenceRecord` type. Every record carries a
journal ordinal, monotonic time, source, decision, and optional proposal,
command, evidence, and acknowledgement identities. The recorded decisions are:

- proposal accepted or rejected;
- command accepted, policy-rejected, or superseded by safety;
- zero requested and zero verified;
- actuator acknowledgement applied or failed; and
- journal saturation, configured storage exhaustion, and torn-tail recovery.

The CPU safety owner sends records to a dedicated persistence thread. Producers
only enter a bounded in-memory queue; they never perform file I/O or wait for a
flush. Normal decisions and stop/ack evidence have separate bounded capacities.
Stop and e-stop request the actuator before attempting to enqueue evidence, so
storage cannot block or drop the safety action.

Normal-lane or priority-lane saturation latches the journal unhealthy and uses
a reserved terminal slot to persist `JournalSaturated` once the writer makes
progress. The supervisor rejects further policy work and requests e-stop.
Configured storage limits reserve the final on-disk slot for `StorageFull`.
Unexpected I/O exhaustion is exposed as a writer fault; no implementation can
guarantee a new durable byte after the device itself refuses writes.

## File and recovery semantics

The journal is append-only and versioned (`LEASHEV1`). Records are fixed-width,
carry a frame marker and FNV-1a checksum, and are synchronized by the dedicated
writer in batches. Ordinals must be strictly increasing.

Startup scans complete frames. A partial or checksum-invalid final frame is
treated as a torn tail, truncated to the last complete record, and followed by
a durable `TornTailRecovered` record. Corruption before the final frame fails
open rather than silently discarding later evidence. `EvidenceRecoveryState`
reports both the last complete pre-recovery record and whether a torn tail was
detected.

Only one `EvidenceJournal` may own a journal path at a time. Callers retain the
journal owner, pass its `EvidenceProducer` to
`CpuSafetySupervisor::spawn_with_evidence`, shut down the supervisor, and then
shut down the journal to flush its tail.

## Verification

Focused tests cover command acceptance, policy denial, proposal queue
rejection, controller supersession, verified zero, failed acknowledgement,
writer stall, queue saturation, configured full storage, and restart after a
torn tail.

Run the host/target benchmark with:

```console
cargo run --release -p leash-runtime --example evidence_bench -- --samples 1000
```

The versioned output reports durable record throughput, both queue high-water
marks, evidence failures, stop latency with and without journaling, and p99 stop
latency impact. Baseline and journaled stop samples alternate order to control
warm-up and scheduling bias. The fake actuator measures software request
latency only; it is not physical motor stop proof.

The checked Windows host result is
`crates/leash-runtime/evidence/host-windows-x86_64-evidence-20260829.json`.
It persisted 3,202 records without saturation or evidence failure at 570,776
records/second. Stop p99 was 700 ns without journaling and 1,300 ns with it, a
600 ns measured impact.

The checked Orin result is
`crates/leash-runtime/evidence/jetson-orin-nx-evidence-20260829.json`. In 10 W
mode it persisted 3,202 records without saturation or evidence failure at
110,293 records/second. Alternating stop samples measured p99 of 13,152 ns
without journaling and 11,073 ns with it. The negative 2,079 ns difference is
measurement noise; the result supports no measurable p99 regression, not a
claim that persistence improves stopping. All 24 release-mode runtime tests,
including the 50 ms writer-stall fail-closed deadline, passed on the same
isolated source archive.
