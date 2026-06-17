# Leash

Leash is a Rust harness runtime for robot control and local-LLM tools.
It ships as the `leash-harness` Rust crate and installs a `leash` binary.

The default runtime is simulation-safe. Physical robot adapters are optional
features and refuse motor actuation unless `LEASH_ALLOW_PHYSICAL_ACTUATION=1`
or `--allow-physical-actuation` is set.

## Install

From the current repository checkout:

```bash
cargo install --path .
```

From GitHub source:

```bash
cargo install --git https://github.com/specdog/leash leash-harness
```

After crates.io publish:

```bash
cargo install leash-harness
```

From a GitHub release archive:

```bash
version=v0.1.0
target=x86_64-unknown-linux-gnu
curl -L -o "leash-$target.tar.gz" \
  "https://github.com/specdog/leash/releases/download/$version/leash-$target.tar.gz"
tar -xzf "leash-$target.tar.gz"
install -m 0755 "leash-$target/leash" "$HOME/.local/bin/leash"
```

Release binaries start with common desktop targets: Linux x86_64, macOS
x86_64, macOS arm64, and Windows x86_64. Ubuntu UGV and Jetson installs should
use the source install path until Linux aarch64 cross-builds are proven.

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

## Daemon Runs

Start a simulation HTTP run in the background:

```bash
leash run --daemon --profile sim --listen 127.0.0.1:8000
```

Inspect and manage it:

```bash
leash status
leash log
leash restart
leash stop
```

Run records and logs are stored under `LEASH_STATE_DIR`, or the XDG state
directory when `LEASH_STATE_DIR` is unset. Use a run name when multiple local
runtimes are active:

```bash
leash run bench --daemon --profile sim --listen 127.0.0.1:8010
leash status bench
leash stop bench
```

## Accelerator Selection

The runtime defaults to no accelerator and remains CPU-safe in CI:

```bash
leash show-config --accelerator cpu --require-accelerator
leash show-config --accelerator cuda
```

Health and capabilities include an accelerator probe inventory. The CPU backend
is always available; the `cuda` backend is a feature-gated placeholder that
reports compile/probe status until a real device backend is attached. Standard
builds do not require GPU hardware or vendor SDKs.

## Inspect Configuration

Check the resolved runtime config before starting a service:

```bash
leash show-config
leash show-config waveshare-ugv --listen 0.0.0.0:8000 --allow-physical-actuation
```

The output includes each field's source, physical-actuation flags, and network
bind address. Precedence is default, config file, blueprint default,
environment, then CLI. If present, `--config` or `LEASH_CONFIG` points at a JSON
config file.

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

Release steps are tracked in [docs/RELEASE.md](docs/RELEASE.md).

## Smoke Tests

```bash
scripts/smoke-http.sh
scripts/smoke-mcp.sh
scripts/smoke-physical-gate.sh
scripts/smoke-daemon.sh
```

## Provenance

Leash is extracted from the `robot-harness` in
`https://github.com/0xSoftBoi/onchain-rover`, originally built for the Clanker
500 / Onchain Rover hackathon project. The sidecar, x402, race UI, and chain
flows stay in that project and are treated here as examples or integrations.
