# GitHub Workflows

This folder defines the remote validation and release path.

```mermaid
flowchart LR
  push["push or pull_request"] --> ci["ci.yml"]
  ci --> rust["rust job\nfmt, clippy, tests, cargo package, smoke-all"]
  ci --> matrix["feature-matrix job\ncore, default, MCP, HTTP sim, hardware, all features"]

  tag["v* tag or workflow_dispatch"] --> release["release.yml"]
  release --> crate["crate artifact"]
  release --> binaries["release binaries\nLinux, macOS, Windows"]
  release --> draft["draft GitHub release\nfor version tags"]
```

## Files

- `ci.yml`: required correctness path for ordinary changes.
- `release.yml`: packaging path for crates.io artifacts, binary archives, checksums, and draft GitHub releases.
