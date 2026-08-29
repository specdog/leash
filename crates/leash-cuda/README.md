# leash-cuda

This crate owns Leash's CUDA artifact contract. Release builds load a checked-in
fatbin and do not compile inline CUDA source with NVRTC during startup.

The current artifact contains native SM 8.7 cubin and compute 8.7 PTX for the
Jetson Orin NX. It was built twice with CUDA 12.9; both outputs had the same
SHA-256 recorded in `kernels/prebuilt/sm_87/manifest.json`.

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
single-owner `cudarc` executor and compares all four jobs to their CPU oracle:

```bash
cargo run --release --features cuda --example jetson_executor_probe
```
