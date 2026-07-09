# CLI Binary

This folder contains the `leash` command-line binary. The binary is intentionally thin: it parses commands, resolves config, starts library surfaces, and prints machine-readable output where useful.

```mermaid
flowchart TB
  cli["leash CLI\nsrc/bin/leash.rs"] --> list["list\nbuilt-in stacks"]
  cli --> serve["serve\nhttp, mcp, mcp-http"]
  cli --> run["run\nstack foreground or daemon"]
  cli --> daemon["status, log, restart, stop"]
  cli --> graphcmd["graph\nmodule graph export"]
  cli --> config["show-config\nresolved config and sources"]
  cli --> replay["record, replay"]
  cli --> agent["agent-send, agent-interactive"]
  cli --> mcp["mcp\nHTTP MCP helpers and stdio bridge"]
  cli --> safety["health, stop\nremote HTTP helpers"]

  serve --> lib["leash_harness library"]
  run --> lib
  graphcmd --> lib
  config --> lib
```

## File

- `leash.rs`: Clap command definitions and command handlers for all CLI surfaces.

## Common Commands

```bash
leash list
leash run sim-http
leash serve http --profile sim
leash show-config waveshare-ugv --allow-physical-actuation
leash health --url http://127.0.0.1:8000
leash stop --url http://127.0.0.1:8000
```
