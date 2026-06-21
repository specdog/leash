# Replay Example

This folder holds deterministic replay fixtures. Replay lets HTTP and MCP observe paths return stable telemetry without live hardware.

```mermaid
flowchart LR
  fixture["sim-basic.jsonl\nobserve fixture"] --> replay["ReplayPlayback"]
  memory["sim-memory.jsonl\nmemory demo fixture"] --> replay
  replay --> harness["profile: replay\nphysical: false"]
  harness --> http["serve http --replay-source"]
  harness --> mcp["serve mcp --replay-source"]
  harness --> memoryCaps["MCP memory capabilities"]
  http --> observe["GET /telemetry"]
  mcp --> tool["observe tool"]
  memoryCaps --> recall["tag/list/query/clear local memory"]
```

## Files

- `sim-basic.jsonl`: small replay recording used by replay smoke tests and examples.
- `sim-memory.jsonl`: short replay recording for demos that tag and recall locations through MCP while observe output stays deterministic.

## Commands

```bash
leash replay examples/replay/sim-basic.jsonl --speed 10
leash serve http --replay-source examples/replay/sim-basic.jsonl
leash serve mcp --replay-source examples/replay/sim-basic.jsonl
LEASH_STATE_DIR="$(mktemp -d)" leash serve mcp-http --replay-source examples/replay/sim-memory.jsonl
```
