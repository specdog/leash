# AGENTS.md — Leash

Leash is a safety-gated Rust robotics runtime. Work from current `main` on a branch. Do not commit directly to `main`.

## Sources of truth

- Implementation behavior: current code, tests, schemas, and merged docs on `main`.
- Project/spec graph: `specs/leash/leash.dag`, queried through DotDog MCP.
- Human spec source: `specs/leash/*.dog`.

Do not read or edit `.dog` files as an agent. For project structure, run:

```bash
npm ci
npx dotdog serve
```

Use DotDog MCP tools to query the compiled `.dag`. If the graph and implementation disagree, report the mismatch instead of inventing a spec change.

## Workspace

The repository is an active Rust workspace, not a planned skeleton:

- `crates/leash-core` — reusable contracts and core types
- `crates/leash-runtime` — runtime orchestration
- `crates/leash-cuda` — bounded advisory CUDA compute with CPU parity/fallback
- `crates/leash-gateway` — gateway boundary
- `crates/leash-replay` — replay support
- `crates/leash-ros2` — ROS 2 proposal/provider boundary
- `crates/leash-waveshare` — reusable Waveshare adapter code
- `src/` — top-level CLI/HTTP/MCP harness integration
- `implementations/waveshare-ugv/` — concrete robot deployment, calibration, and field proof

## Safety rules

Never bypass or weaken Leash's sole-writer device boundary. External agents, ROS 2, planners, localization, mapping, model providers, and CUDA results are requests or evidence only.

Physical motion remains behind explicit compile/runtime gates plus the normal token/approval/freshness/deadman/collision/stop/E-stop policy. Simulation and replay must remain non-actuating.

Never claim physical verification unless a real device was exercised and the evidence supports the claim.

## Verification

Load `.agents/skills/verify/SKILL.md` when verifying public CLI/HTTP behavior.

Before merge, run the checks relevant to the change. The full proof is:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo test -p leash-core --doc
cargo test --no-default-features --features sim,mcp
npm test
cargo run --features mcp --bin leash-schema -- --check
cargo package --workspace --locked
scripts/smoke-all.sh
```

Do not merge a stale draft branch. Reconcile it with current `main`, rerun verification, and update docs/contracts before requesting merge.

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the human contribution workflow and `specs/leash/AGENTS.md` for DotDog-specific agent rules.
