# Examples

This folder contains runnable examples and fixtures for the main operating modes. Examples should stay safe by default unless their README explicitly calls out physical hardware requirements.

```mermaid
flowchart TB
  examples["examples/"] --> bridge["bridge-compat/\nlegacy HTTP/WebSocket bridge shape"]
  examples --> local["local-llm-mcp/\nMCP client setup for local agents"]
  examples --> network["network-transport/\nTCP JSONL stream frame boundary"]
  examples --> replay["replay/\ndeterministic JSONL fixtures"]
  examples --> workers["workers/\nexternal worker frame fixtures"]
  examples --> ugv["waveshare-ugv/\nphysical Jetson/Waveshare adapter notes"]

  bridge --> http["Leash HTTP runtime"]
  local --> mcp["Leash MCP stdio"]
  network --> tcp["localhost TCP JSONL\nno hardware"]
  replay --> deterministic["Replay profile\nno hardware"]
  ugv --> physical["Physical profile\nexplicit actuation gate"]
```

## Folders

- `bridge-compat/`: route compatibility for clients that already speak the robot bridge API.
- `local-llm-mcp/`: how to connect an MCP-capable local LLM client.
- `network-transport/`: TCP JSONL stream frame contract for external module processes.
- `replay/`: checked-in replay fixtures for deterministic observe paths and memory demos.
- `workers/`: versioned no-hardware perception input and passive motion-event output fixtures.
- `waveshare-ugv/`: physical adapter notes for the Jetson/Waveshare UGV.
