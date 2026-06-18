# leash

> Composable local-LLM and robotics harness. Rust-first, MCP-first, simulation-safe.

Leash is a Rust harness runtime that lets LLM agents control robots through typed modules, safety gates, and a shared capability registry. Run in simulation with zero hardware, or connect physical robots behind explicit actuation gates.

```bash
cargo install leash-harness
leash serve mcp          # MCP stdio for LLM agents
leash serve http         # localhost HTTP + WebSocket
```

## Why Leash

- **Simulation-safe by default.** CI, demos, and smoke tests require zero hardware. Physical actuation is an explicit opt-in gate.
- **MCP-native.** Agents get 7 typed tools (health, capabilities, observe, invoke_capability, stop, estop, capture) over stdio.
- **Safety gates at every layer.** Deadman switch, estop, soft odometry limits, physical actuation gate. Policy-gated capability invocation.
- **Feature-gated hardware.** Waveshare UGV today, MAVLink drone + manipulator planned. No hardware compiles without explicit `--features`.
- **Blueprint catalog.** Runnable sim, MCP, HTTP, and compatibility demos. `leash list` + `leash run <blueprint>`.
- **Module graph with typed streams.** Modules declare inputs, outputs, lifecycle, and health. Coordinator manages startup/shutdown order.

## Quick Start

```bash
# Install
cargo install leash-harness

# Run in simulation (zero hardware)
leash serve mcp --profile sim

# Run with HTTP + WebSocket
leash serve http --profile sim --listen 127.0.0.1:8000

# Check health
leash health --url http://127.0.0.1:8000
```

## MCP Tools

| Tool | Description |
|------|-------------|
| `health` | Harness health and safety state |
| `capabilities` | Endpoints, MCP tools, speed modes |
| `observe` | Latest telemetry frame (odometry, battery, sensors) |
| `invoke_capability` | authorize, drive, stop, estop, estop_reset, speed_mode |
| `stop` | Non-latching zero-speed motor stop |
| `estop` | Latch emergency stop until reset |
| `capture` | Deterministic frame capture |

## HTTP Endpoints

```
GET  /health              Harness health
GET  /capabilities         Endpoints + tools
GET  /telemetry            Latest TelemetryFrame
POST /drive               { token, left, right, speed_mode }
POST /estop                Latch emergency stop
POST /estop/reset          Clear estop
WS   /ws/telemetry         Streaming telemetry frames
```

## Features

| Feature | Description | Default |
|---------|-------------|---------|
| `sim` | Simulation driver (no hardware) | ✓ |
| `http` | HTTP server + WebSocket | ✓ |
| `mcp` | MCP stdio server | ✓ |
| `waveshare-ugv` | Waveshare UGV physical adapter | opt-in |
| `bridge-compat` | Legacy robot bridge compatibility | opt-in |

## Smoke Tests

```bash
scripts/smoke-all.sh
```

`scripts/smoke-all.sh` runs the no-hardware release proof and prints a JSON
summary covering HTTP routes and policy denial, stdio MCP, physical-gate
refusal, daemon lifecycle, graph export, and config preflight checks.

Run narrower checks when you need to isolate one surface:

```bash
scripts/smoke-http.sh
scripts/smoke-mcp.sh
scripts/smoke-physical-gate.sh
scripts/smoke-daemon.sh
```

## Roadmap

See [issues](https://github.com/specdog/leash/issues) for the full plan. Highlights:

- [ ] Module graph with typed streams, lifecycle, and health aggregation
- [ ] Blueprint catalog: `leash list` + `leash run`
- [ ] Replay engine: deterministic sensor record + playback
- [ ] Transport abstraction: in-process, cross-process, network
- [ ] MAVLink drone + manipulator adapters
- [ ] Localhost command center dashboard
- [ ] Spatial memory and perception primitives
- [ ] Patrol and exploration in simulation
- [x] Full no-hardware smoke suite

## License

MIT
