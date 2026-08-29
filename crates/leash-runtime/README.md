# leash-runtime

Bounded orchestration lanes and the dedicated CPU safety supervisor for the
Leash v2 deterministic core.

Run the 100 Hz host timing probe in release mode:

```console
cargo run --release -p leash-runtime --example control_loop_bench -- --ticks 1000
```

It emits one versioned JSON record containing p50/p95/p99/maximum completion
jitter, transition latency, deadline misses, and proposal queue capacity,
high-water mark, rejection count, and final depth. This is a host diagnostic;
the Jetson timing and fault-injection record is a separate deployment gate.
