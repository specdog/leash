# Release Checklist

Use this checklist for each public Leash release.

## Version And Notes

- Pick a semver version, for example `0.1.0`.
- Update `Cargo.toml` package version.
- Update release notes or changelog text with user-visible changes, install
  changes, safety changes, and known limits.
- Confirm Linux aarch64 or Jetson binaries are not promised until that
  cross-build is proven in CI.

## Local Verification

Run from a clean checkout:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo test --no-default-features --features sim,mcp
cargo package --locked
scripts/smoke-all.sh
```

Use `scripts/smoke-http.sh` when isolating HTTP telemetry, require-token policy
denial, approved drive, and agent input channels. Use
`scripts/smoke-mcp-http.sh` when isolating the localhost MCP HTTP and
`leash mcp ...` CLI tool-call surface. `scripts/smoke-all.sh` also verifies
hosted agent-provider config redacts `LEASH_AGENT_API_KEY` and policy mode
resolves in `show-config` output.

Verify the feature matrix before tagging:

```bash
cargo check --no-default-features --lib
cargo check
cargo check --no-default-features --features mcp
cargo check --no-default-features --features sim,http
cargo check --no-default-features --features sim,http,mcp,waveshare-ugv,bridge-compat
cargo check --all-features
```

Check a bot preflight config before publishing bot-facing notes:

```bash
leash show-config waveshare-ugv \
  --role courier \
  --listen 0.0.0.0:8000 \
  --serial-port /dev/ttyTHS1 \
  --no-untokened-drive \
  --allow-physical-actuation
```

## Crates.io

Dry-run first:

```bash
cargo publish --dry-run
```

Publish only after the dry-run and CI are green:

```bash
cargo publish
```

## Git Tag And Binaries

Create and push an annotated tag:

```bash
git tag -a v0.1.0 -m "v0.1.0"
git push origin v0.1.0
```

The `release` workflow builds:

- `leash-x86_64-unknown-linux-gnu.tar.gz`
- `leash-x86_64-apple-darwin.tar.gz`
- `leash-aarch64-apple-darwin.tar.gz`
- `leash-x86_64-pc-windows-msvc.zip`
- one `.sha256` file per binary archive
- the packaged `leash-harness-<version>.crate`

Before publishing the draft GitHub release:

- Verify every archive contains `leash`, `README.md`, and `LICENSE`.
- Verify each checksum matches its archive.
- Download the Linux archive on a clean host and run `leash --help`.
- Keep Linux aarch64 and Jetson users on the source install path until a
  dedicated cross-build ticket proves that target.
