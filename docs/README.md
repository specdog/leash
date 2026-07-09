# Docs

This folder is for operator and release documentation that should stay close to the code but not live in the top-level README.

```mermaid
flowchart TB
  docs["docs/"] --> bot["BOT_INSTALL.md\nJetson and Waveshare UGV service install"]
  docs --> release["RELEASE.md\nlocal and CI release gates"]
  docs --> source["SOURCE_MAP.md\ncodebase orientation"]
  docs --> mcp["MCP_HTTP.md\nstandard and compatibility HTTP paths"]
  docs --> camera["CAMERA.md\ncapture, stream, recovery, and encoder tuning"]

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
