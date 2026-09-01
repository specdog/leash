# Source and claim notes

Deck source commit: `566bc569b24bf5f392291b142469282fcdfac2b3` from `specdog/leash` `origin/main`.

## Primary code

- Canonical rule and architecture: `README.md`
- Validated newtypes, `Candidate<C>`, `Authorized<C>`, safety gate, and compile-fail contracts: `crates/leash-core/src/drive.rs`
- `Frame<Tag>`, `Pose2<Tag>`, and `PhantomData` frame markers: `crates/leash-core/src/frame.rs`
- `ActuationPort`, associated types, supervisor bounds, priority safety path, and 10 ms default tick: `crates/leash-runtime/src/supervisor.rs`
- Bounded lane implementation: `crates/leash-runtime/src/lane.rs`
- `ControllerIo` supertraits and blanket implementation, Waveshare adapter, and verified acknowledgements: `crates/leash-waveshare/src/lib.rs`
- Transport-neutral command/query traits: `crates/leash-gateway/src/lib.rs`
- ROS2/Nav2 typed proposals and conversions: `crates/leash-ros2/src/lib.rs`
- CUDA shadow gate and fallback: `crates/leash-cuda/src/gate.rs` and `crates/leash-cuda/README.md`

## Measured evidence

- `crates/leash-runtime/evidence/jetson-orin-nx-rv2-16-nomotion-20260829.json` — 58,306 ns p99 transition latency at 100 Hz with zero deadline misses.
- `crates/leash-runtime/evidence/jetson-orin-nx-evidence-20260829.json` — 110,293 durable records per second in the recorded evidence run.
- `crates/leash-runtime/evidence/jetson-orin-nx-rv2-16-physical-rollout-20260829.json` — 37.622 ms physical E-stop acknowledgement and verified zero; CPU final authority with CUDA active but no motor authority.
- `crates/leash-cuda/evidence/jetson-orin-nx-rv2-13-20260829.json` — CUDA shadow comparison and fallback evidence.

## Visual provenance

- `waveshare-ugv-rover-front.jpg` and `waveshare-ugv-rover-angle.jpg`: official Waveshare UGV Rover product imagery from <https://www.waveshare.com/product/ai/robots/ugv-rover-pt-jetson-orin-ai-kit.htm>.
- `pinkie-live-camera-2026-09-01.jpg`: read-only Pinkie camera snapshot captured during preparation on 2026-09-01.
- `qualia-world-current.png` and `hermes-terminal-current.png`: user-provided current-state screenshots used only in the two-slide Qualia handoff section.
- `fallback-title.svg`: original talk artwork in this directory.

Each slide's external claim and asset citations are also embedded in its speaker notes under `[Sources]`.
