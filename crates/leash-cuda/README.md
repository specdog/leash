# leash-cuda

This crate owns Leash's CUDA artifact contract. Release builds load a checked-in
fatbin and do not compile inline CUDA source with NVRTC during startup.

The current artifact contains native SM 8.7 cubin and compute 8.7 PTX for the
Jetson Orin NX. It was built twice with CUDA 12.9; both outputs had the same
SHA-256 recorded in `kernels/prebuilt/sm_87/manifest.json`.
The source digest is calculated after canonicalizing line endings to LF, so the
same Git source verifies on Windows and Linux build hosts.

The optional `cuda` feature compiles the driver loader and module probe. The
default build has no CUDA dependency. To deliberately rebuild on an Orin CUDA
build host:

```bash
LEASH_CUDA_REBUILD=1 NVCC=/usr/local/cuda/bin/nvcc cargo build -p leash-cuda --features cuda
```

Rebuilding is a development/release operation. Production service startup only
loads the prebuilt module.

The target-side no-motion driver probe can be built without a Rust toolchain:

```bash
g++ -std=c++17 tests/jetson_driver_probe.cpp -I/usr/local/cuda/include \
  -L/usr/local/cuda/lib64 -lcuda -o /tmp/leash-cuda-probe
/tmp/leash-cuda-probe kernels/prebuilt/sm_87/leash_kernels.fatbin
```

With Rust installed on the target, the same check exercises the bounded
single-owner `cudarc` executor and compares all six kernels to their CPU
oracles over fixed, empty, maximum-size, and deterministic randomized inputs:

```bash
cargo run --release --features cuda --example jetson_executor_probe
```

Measure end-to-end CPU/CUDA break-even behavior on the Jetson (including the
executor queue, host/device transfers, synchronization, and readback):

```bash
cargo run --release --features cuda --example jetson_benchmark -- --iterations 20
```

The benchmark performs parity checks before timing and emits a versioned JSON
record for small and large voxel, lidar, advisory collision, camera, and
cognition workloads. The combined spatial profile uploads one lidar scan and
runs transform plus collision reduction against the same resident buffer.
Resident cognition profiles keep activations, weights, and biases on device;
steady ticks transfer sensor/top-down inputs and fixed-size metrics only. Full
state is read back only for the versioned checkpoint gate.
The latest measured Orin NX result and conservative backend decisions are
recorded in `evidence/jetson-orin-nx-rv2-12-20260829.json`.
