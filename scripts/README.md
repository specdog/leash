# Scripts

This folder contains shell smoke tests and the bot installer. Scripts are intended to be run from the repository root.

```mermaid
flowchart TB
  all["smoke-all.sh\naggregate no-hardware release proof"] --> schema["leash-schema --check\nchecked-in JSON Schemas"]
  all --> http["smoke-http.sh\nHTTP routes, telemetry streams, policy denial"]
  all --> mcp["smoke-mcp.sh\nstdio MCP initialize, tools, health"]
  all --> mcphttp["smoke-mcp-http.sh\nlocalhost MCP HTTP, CLI, planner, and patrol calls"]
  all --> mcpbridge["smoke-mcp-bridge.sh\nRust stdio bridge to MCP HTTP"]
  all --> streamhub["smoke-stream-hub.sh\nTCP JSONL stream hub"]
  all --> worker["smoke-worker.sh\nexternal worker lifecycle"]
  all --> replayhttp["smoke-replay-http.sh\nHTTP replay observe"]
  all --> replaymcp["smoke-replay-mcp.sh\nMCP replay observe"]
  all --> physical["smoke-physical-gate.sh\nphysical profile refuses without gate"]
  all --> daemon["smoke-daemon.sh\ndaemon lifecycle, status, logs"]
  install["install-bot.sh\nbuild and install leash.service"] --> bot["Jetson or bot host"]
```

## Files

- `install-bot.sh`: builds a Leash binary and installs a user `systemd` service plus env file on a bot host.
- `smoke-all.sh`: aggregate no-hardware release proof; CI runs this and checks generated schemas.
- `smoke-http.sh`: HTTP, WebSocket/SSE, visualization frame, map/costmap contracts, guarded planner goal/status/cancel, external clients, agent input, capture, drive, and policy checks.
- `smoke-mcp.sh`: stdio MCP initialization and tool calls.
- `smoke-mcp-http.sh`: localhost MCP HTTP routes, `leash mcp` CLI calls, sim planner set/status calls, and sim patrol start/status/stop calls.
- `smoke-mcp-bridge.sh`: Rust stdio MCP bridge against a localhost `leash serve mcp-http` bot surface.
- `smoke-stream-hub.sh`: starts the localhost TCP JSONL stream hub, sends valid frames, and proves an invalid peer does not kill the listener.
- `smoke-worker.sh`: starts an explicit child process through the worker supervisor, verifies JSON status, and lets the supervisor stop it.
- `smoke-replay-http.sh`: replay mode over HTTP.
- `smoke-replay-mcp.sh`: replay mode over MCP.
- `smoke-physical-gate.sh`: proves physical startup fails without explicit actuation.
- `smoke-daemon.sh`: daemon start/status/log/restart/stop path.
