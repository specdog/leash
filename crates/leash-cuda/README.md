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
