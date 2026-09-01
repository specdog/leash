# Contributing to Leash

Leash is MIT-licensed and accepts contributions through GitHub issues and pull requests.

## Human quick start

Prerequisites:

- stable Rust
- Node.js 22+
- Linux builds need `pkg-config` and `libudev-dev`

```bash
git clone https://github.com/specdog/leash.git
cd leash
npm ci
cargo build
cargo run -- run sim-http
```

The default path is simulation-safe and does not require robot hardware.

Create a branch from current `main` before making changes. Keep the branch current with `main`; do not develop new work on a stale stacked branch.

## Required checks

Run the checks that match your change. Before requesting merge, the full repository proof is:

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

CI also runs a feature matrix covering core-only, default, MCP-only, HTTP simulation, hardware-adapter, and all-feature builds.

## Specs

`specs/leash/*.dog` is the human-authored DotDog source. `specs/leash/leash.dag` is the compiled graph used by agents.

When a human changes the spec:

```bash
npx dotdog validate .
npx dotdog compile . -o /tmp/leash.dag
cmp /tmp/leash.dag specs/leash/leash.dag
npx dotdog analyze .
```

If the compiled graph changed intentionally, regenerate `specs/leash/leash.dag` and commit it with the `.dog` source change.

## Safety and hardware changes

Simulation and replay must remain usable without hardware. A new hardware path must be feature-gated and fail closed by default.

Do not add a second motor writer. Leash owns the final device-command boundary. Mapping, localization, planning, ROS 2, CUDA, model providers, and external agents may provide evidence or requests; they do not bypass capability policy, pilot ownership, approval, freshness checks, deadman, collision checks, stop, or E-stop.

Physical claims require actual device evidence. Do not describe a PTY, simulator, replay, or no-hardware test as a physical-hardware proof. Keep private host identity, credentials, device serials, network values, and unsanitized field artifacts out of the repository.

## Pull requests

A mergeable PR should:

- target current `main` unless it is an intentional documented stack;
- be rebased or otherwise brought current before merge;
- describe user-visible and safety-boundary changes;
- include tests or proof for changed behavior;
- keep docs, schemas, examples, and specs synchronized when their contracts change;
- avoid claiming draft or unverified physical behavior as shipped.

Draft PRs may be used for stacked or field-dependent work, but they are not release state.

## Agents

Coding agents should start with [`AGENTS.md`](AGENTS.md), then use the repository's `.agents/skills/` instructions when relevant. Project structure should be queried from the compiled DotDog graph rather than inferred from `.dog` source.

## License

By contributing, you agree that your contribution is provided under the repository's MIT license.
