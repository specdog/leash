# macOS development handoff

Status date: 2026-08-30

## Source of truth

Use the default branch from `https://github.com/specdog/leash.git`. The former
`feat/generic-compute-worker` line contains the implementation history, but new
work should start from `main` after the merge documented by this handoff.

```bash
git clone https://github.com/specdog/leash.git ~/leash
cd ~/leash
git fetch origin
git switch main
git pull --ff-only
git status --short --branch
```

Do not use the older Windows recovery checkout or import historical repository
bundles. They are not development baselines.

## Delivered architecture

- `leash-core` owns deterministic domain and safety contracts.
- `leash-runtime` owns bounded execution lanes and durable evidence.
- `leash-gateway` gives HTTP, MCP, and CLI one typed command path.
- `leash-waveshare` is the sole Waveshare serial/controller owner.
- `leash-cuda` owns bounded advisory compute with parity checks and CPU fallback.
- `leash-ros2` treats Nav2 and ROS inputs as proposals; they never bypass CPU
  safety or write motors directly.
- The generic compute API supports bounded temporal-spatial jobs, cancellation,
  replay-safe status, SSE wakeups, and token-file authentication.
- The navigation API supports one bounded, low-speed mission at a time with an
  existing pilot lease, explicit approval, fresh evidence, cancellation, and a
  verified stop.

CUDA remains advisory. CPU safety, stop, E-stop, collision checks, deadman, and
the physical adapter remain authoritative.

## macOS build and verification

The default build is portable and does not require CUDA or native ROS 2:

```bash
rustup update stable
cargo build --release --locked
cargo test --workspace --all-targets
cargo test --no-default-features --features sim,mcp
npm ci
npm test
cargo run --features mcp --bin leash-schema -- --check
```

Start the non-actuating simulator with:

```bash
cargo run -- run sim-http
```

Native `rclrs`, V4L2, systemd deployment, and Jetson CUDA artifact generation
remain Linux/Jetson responsibilities. macOS can develop and test the portable
contracts, simulator, HTTP/MCP gateway, replay, generic compute CPU path, and
operator tooling.

## Current continuation boundary

The completed generic compute and bounded-navigation work is suitable for
continued development. One later experiment is intentionally not part of the
default branch: treating relative wheel odometry plus a synthetic all-free map
as fresh tracking localization. That changes the meaning of a navigation safety
gate and was not accepted for deployment.

Continue by choosing a truthful localization contract:

1. require a real tracking/SLAM provider and map lineage; or
2. define a separate, explicitly distance- and time-bounded odometry-only mode
   whose map cells remain unknown rather than fabricated free space.

Either design must retain fresh lidar, collision, deadman, stop/E-stop, pilot
lease, verified-zero cleanup, stale-provider cancellation, and external-SLAM
precedence. Run focused tests and a no-motion target proof before any physical
deployment.

## Key references

- `docs/RUNTIME_V2_HANDOFF.md` - runtime architecture and target evidence
- `docs/COMPUTE_API.md` - generic compute contract
- `docs/NAVIGATION_API.md` - bounded navigation contract
- `docs/LOCALIZATION_PROVIDERS.md` - provider freshness and lineage
- `docs/PHYSICAL_NAVIGATION.md` - physical safety policy
- `docs/WINDOWS.md` - native Windows development and optional CUDA verification

Credentials, operator tokens, device addresses, and target state are not part
of this handoff. Re-probe every live system and obtain fresh mission-specific
authorization before physical motion.
