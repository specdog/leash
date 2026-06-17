# Source Map

Leash starts from the Rust `robot-harness` built inside the hackathon
`onchain-rover` repo.

## Source Reference

- Repository: `https://github.com/0xSoftBoi/onchain-rover`
- Local source ref used for the first extraction pass:
  `cba2604421249b02de93a52658db0c272dab5ae7`
- Source package: `robot-harness`

## What Becomes Core

- Runtime config and profile selection
- Health, capabilities, telemetry, and sensor snapshots
- Pilot token/session controls
- Speed caps, deadman stop, estop latch/reset, and stop behavior
- Simulation-safe runtime
- MCP stdio and localhost HTTP transports

## What Stays Optional

- Waveshare Ubuntu UGV serial adapter
- Existing bridge/client compatibility routes
- Bot user-service installation helpers

## What Stays Out Of Core

- Application-specific payment flows
- Race or game UI
- Chain contracts and settlement logic
- Public sidecar app code
- Hardware deployment state for a specific venue

## Known Source Baseline Note

The source harness had a lidar unit-test call-site mismatch in this local ref:
`LidarScanWindow::ingest` expects `min_valid_mm` and `mask`, while one test still
called it with only `points`. Leash avoids importing that monolith directly and
keeps the first extraction focused on the reusable runtime and install surface.

