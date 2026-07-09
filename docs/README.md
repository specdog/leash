# Docs

This folder is for operator and release documentation that should stay close to the code but not live in the top-level README.

```mermaid
flowchart TB
  docs["docs/"] --> bot["BOT_INSTALL.md\nJetson and Waveshare UGV service install"]
  docs --> release["RELEASE.md\nlocal and CI release gates"]
  docs --> source["SOURCE_MAP.md\ncodebase orientation"]
  docs --> mcp["MCP_HTTP.md\nstandard and compatibility HTTP paths"]
  docs --> camera["CAMERA.md\ncapture, stream, recovery, and encoder tuning"]
  docs --> navigation["NAVIGATION.md\nsaved waypoints, patrol zones, and motion events"]
  docs --> adapters["ADAPTERS.md\nRust contracts and second-UGV checklist"]
  docs --> adapterSmoke["ADAPTER_SMOKE_TEMPLATE.md\nreusable pre-fleet proof"]
  docs --> sessions["OPERATOR_SESSIONS.md\nsafe recording and offline GUI replay"]

  bot --> service["systemd user service\n~/.config/systemd/user/leash.service"]
  release --> proof["cargo, smoke scripts, package checks"]
  source --> modules["runtime, HTTP, MCP, replay, safety modules"]
```

## Files

- `BOT_INSTALL.md`: how to install Leash on a bot host as a user service.
- `RELEASE.md`: release checklist, feature matrix, bot preflight, and packaging notes.
- `SOURCE_MAP.md`: quick map from product surface to implementation files.
- `MCP_HTTP.md`: MCP Streamable HTTP requests, safety behavior, and legacy REST compatibility.
- `CAMERA.md`: camera ownership, health and recovery routes, capture settings, and Jetson encoder tuning.
- `NAVIGATION.md`: persistent waypoints and patrol zones, sim/replay execution, operator controls, and passive motion events.
- `ADAPTERS.md`: mobile-base, gimbal, and camera contracts plus the second-UGV implementation checklist.
- `ADAPTER_SMOKE_TEMPLATE.md`: reusable no-hardware, bench, camera, telemetry, soak, and sign-off checklist.
- `OPERATOR_SESSIONS.md`: safe operator event recording and offline GUI timeline replay.
