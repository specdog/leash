# FLEET.md — Leash UGV corral (PUBLIC-SAFE TEMPLATE)

> NOTE: This repository is a public library. Do NOT put real bot names, IP
> addresses, or credentials in this file. This is a template that documents the
> corral pattern only. Keep your actual fleet inventory in LOCAL/PRIVATE config
> (e.g. the `leash-fleet-corral` Hermes skill under ~/.hermes/skills/...),
> which is not part of this repo and never gets committed here.

## Mental model

```
operator/Mac (Hermes)
  └─ Hermes agent = CONTROL SURFACE
       └─ one named MCP server per bot (`leash mcp bridge` on the Mac)
            └─ forwards tool calls over the network
                 └─ bot: `leash serve mcp-http`  (custom REST /mcp/call)
                      └─ serial /dev/ttyTHS1 @115200 -> ESP32 -> motors/gimbal
```

Hermes never touches the bot serial port. It only speaks MCP tool calls. The
bot runs leash; leash owns the safety gates and the serial write.

## Critical gotcha

- leash's real MCP server is **stdio-only** (`serve_stdio`). A stdio MCP client
  must live on the same box as the bot — impossible for a remote bot.
- `leash serve mcp-http` is a **custom REST** surface, NOT the MCP Streamable
  HTTP protocol: GET /mcp/tools, POST /mcp/call {"tool","args"}, GET /mcp/status.
  Hermes's native_mcp HTTP client expects real MCP JSON-RPC, so it will NOT
  connect to this directly.
- Therefore: Hermes uses a **local Rust stdio MCP bridge** per bot. Each bridge
  proxies tool calls to that bot's `/mcp/call`. Register each bridge in
  ~/.hermes/config.yaml under `mcp_servers`, keyed by bot name. See the
  `leash-fleet-corral` skill for the private fleet inventory.

## Bring-up pattern (run on the bot host)

```bash
# 1) stop any stock robot service that owns the serial port
sudo systemctl stop <stock-robot-service>
# 2) start leash physical MCP surface (needs the actuation gate)
LEASH_ALLOW_PHYSICAL_ACTUATION=1 \
  ./target/release/leash serve mcp-http \
  --profile waveshare-ugv --role <role> \
  --listen 0.0.0.0:9990 --serial-port /dev/ttyTHS1 --deadman-ms 400
```

Preflight: `curl -s http://127.0.0.1:9990/mcp/status`.

## Hermes wiring (on the operator machine)

In ~/.hermes/config.yaml (PRIVATE — not in this repo):

```yaml
mcp_servers:
  <bot-name>:
    command: "leash"
    args: ["mcp", "bridge"]
    env:
      LEASH_BRIDGE_URL: "http://<bot-ip>:9990"
```

After restart, Hermes sees tools like `mcp_<bot-name>_health`,
`mcp_<bot-name>_invoke_capability`, `mcp_<bot-name>_estop`. Naming done.

## Safety reminders

- Physical profile refuses to start without LEASH_ALLOW_PHYSICAL_ACTUATION=1.
- Stop the stock robot service first or leash cannot open /dev/ttyTHS1.
- Deadman = 400 ms; estop latches until estop_reset.
- leash's MCP/HTTP surface has no auth (CORS permissive). Front with SSH tunnel
  or VPN if exposed beyond the bot's own subnet.
- The bot host's own OS login is NOT set by leash; keep credentials in private
  config, never in this repo.
