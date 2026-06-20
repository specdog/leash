# Replay Example

This folder holds deterministic replay fixtures. Replay lets HTTP and MCP observe paths return stable telemetry without live hardware.

```mermaid
flowchart LR
  fixture["sim-basic.jsonl\nleash-replay-v1 events"] --> replay["ReplayPlayback"]
  replay --> harness["profile: replay\nphysical: false"]
  harness --> http["serve http --replay-source"]
  harness --> mcp["serve mcp --replay-source"]
  http --> observe["GET /telemetry"]
  mcp --> tool["observe tool"]
```

## Files

- `sim-basic.jsonl`: small replay recording used by replay smoke tests and examples.

## Commands

```bash
leash replay examples/replay/sim-basic.jsonl --speed 10
leash serve http --replay-source examples/replay/sim-basic.jsonl
leash serve mcp --replay-source examples/replay/sim-basic.jsonl
```
