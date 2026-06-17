# Leash

Leash is a Rust harness runtime for robot control and local-LLM tools.
It ships as the `leash-harness` Rust crate and installs a `leash` binary.

The default runtime is simulation-safe. Physical robot adapters are optional
features and refuse motor actuation unless `LEASH_ALLOW_PHYSICAL_ACTUATION=1`
or `--allow-physical-actuation` is set.

## Install

From source:

```bash
cargo install --path .
```

After publish:

```bash
cargo install leash-harness
```

## Local LLM / MCP

Run a stdio MCP server:

```bash
leash serve mcp --profile sim
```

Use this command in a local LLM client's MCP config. The server exposes:

- `health`
- `capabilities`
- `observe`
- `invoke_capability`
- `stop`
- `estop`
- `capture`

## HTTP Compatibility

Run the HTTP harness:

```bash
leash serve http --profile sim --listen 127.0.0.1:8000
```

Smoke it:

```bash
curl -s http://127.0.0.1:8000/health
curl -s http://127.0.0.1:8000/capabilities
curl -s http://127.0.0.1:8000/telemetry
curl -s -X POST http://127.0.0.1:8000/motors/stop
```

## Waveshare UGV Example

The Waveshare Ubuntu UGV adapter is not part of the default build:

```bash
cargo build --features waveshare-ugv,bridge-compat
LEASH_ALLOW_PHYSICAL_ACTUATION=1 \
  leash serve http \
  --profile waveshare-ugv \
  --listen 0.0.0.0:8000 \
  --serial-port /dev/ttyTHS1
```

Only one process can own the UGV serial port. Stop the stock Waveshare app or
any previous harness before starting the physical profile.

## Install On A Bot

For a source-based bot install with a user systemd service:

```bash
scripts/install-bot.sh --profile sim --listen 127.0.0.1:8000 --start
```

Physical installs are explicit:

```bash
scripts/install-bot.sh \
  --profile waveshare-ugv \
  --role courier \
  --listen 0.0.0.0:8000 \
  --serial-port /dev/ttyTHS1 \
  --no-untokened-drive \
  --allow-physical-actuation \
  --start
```

See [docs/BOT_INSTALL.md](docs/BOT_INSTALL.md).

## Smoke Tests

```bash
scripts/smoke-http.sh
scripts/smoke-mcp.py
scripts/smoke-physical-gate.sh
```

## Provenance

Leash is extracted from the `robot-harness` in
`https://github.com/0xSoftBoi/onchain-rover`, originally built for the Clanker
500 / Onchain Rover hackathon project. The sidecar, x402, race UI, and chain
flows stay in that project and are treated here as examples or integrations.

