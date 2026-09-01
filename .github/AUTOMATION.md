# GitHub automation

This folder turns a proposed Leash change into verified Rust artifacts. Pull
requests and pushes run the safety-oriented build matrix; version tags add the
packaging and draft-release path.

```mermaid
flowchart LR
  change["Push or pull request"] --> ci["ci.yml"]
  ci --> quality["fmt + clippy"]
  ci --> tests["all-target tests"]
  ci --> matrix["core · MCP · HTTP · hardware · all features"]
  ci --> contracts["schema freshness"]
  ci --> smoke["no-hardware smoke proof"]
  quality --> merge{"All checks pass?"}
  tests --> merge
  matrix --> merge
  contracts --> merge
  smoke --> merge
  merge -- yes --> main["main"]
```

```mermaid
flowchart LR
  tag["Push v*.*.* tag"] --> release["release.yml"]
  release --> crate["Cargo crate"]
  release --> linux["Linux binary + SHA-256"]
  release --> mac["Intel + Apple Silicon binaries"]
  release --> windows["Windows binary + SHA-256"]
  crate --> draft["Draft GitHub release"]
  linux --> draft
  mac --> draft
  windows --> draft
```

## How to use it

Before opening a pull request, run the same core checks locally:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo run --features mcp --bin leash-schema -- --check
scripts/smoke-all.sh
```

- [`workflows/ci.yml`](workflows/ci.yml) runs on every push and pull request.
- [`workflows/release.yml`](workflows/release.yml) runs for version tags or a
  manual dispatch and produces a draft release for review.
- Runtime behavior belongs in `src/`; operator and protocol guidance belongs in
  `docs/`. This folder should contain repository automation only.
