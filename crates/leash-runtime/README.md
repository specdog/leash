# leash-runtime

Bounded orchestration lanes and the dedicated CPU safety supervisor for the
Leash v2 deterministic core.

The lossless evidence path is an append-only, checksummed journal with bounded
normal and priority ingress queues and a dedicated persistence owner. Use
`EvidenceJournal::producer` with `CpuSafetySupervisor::spawn_with_evidence`.
Saturation fails closed, while stop/e-stop reaches the actuator before any
evidence enqueue. Recovery truncates a torn final frame and records the event.
The format and failure contract are documented in
[`docs/RUNTIME_V2_EVIDENCE.md`](../../docs/RUNTIME_V2_EVIDENCE.md).

Run the 100 Hz host timing probe in release mode:

```console
cargo run --release -p leash-runtime --example control_loop_bench -- --ticks 1000
```

It emits one versioned JSON record containing p50/p95/p99/maximum completion
jitter, transition latency, deadline misses, and proposal queue capacity,
high-water mark, rejection count, and final depth. This is a host diagnostic;
the Jetson timing and fault-injection record is a separate deployment gate.

Measure evidence throughput and software stop-latency impact with:

```console
cargo run --release -p leash-runtime --example evidence_bench -- --samples 1000
```
