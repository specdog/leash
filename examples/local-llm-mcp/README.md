# Local LLM MCP Example

Configure your MCP-capable local LLM client to launch:

```mermaid
flowchart LR
  llm["MCP-capable local LLM client"] --> stdio["stdio MCP transport"]
  stdio --> leash["leash serve mcp --profile sim"]
  leash --> tools["health, capabilities, observe, invoke_capability, stop, estop, capture"]
  tools --> sim["simulation-safe harness"]
```

Launch command:

```bash
leash serve mcp --profile sim
```

The default sim profile allows safe untokened drive commands. For physical
profiles, keep token/session gating and the physical actuation env gate enabled.

Useful tool calls:

- `health`: confirm runtime and safety state.
- `capabilities`: list supported harness actions.
- `observe`: read current telemetry.
- `invoke_capability`: call `authorize`, `drive`, `speed_mode`, `stop`,
  `estop`, or `estop_reset`.
- `capture`: return deterministic capture metadata.
