# Bot Install

Leash can be installed on a bot host as a user service. The install path is
source-based for Ubuntu UGV and Jetson hosts until Linux aarch64 release
binaries are proven in CI.

## Sim Install

Use this first on any new bot. It proves the binary, service, and HTTP API
without touching motors.

```bash
git clone https://github.com/specdog/leash.git
cd leash
scripts/install-bot.sh --profile sim --listen 127.0.0.1:8000 --start
```

Check:

```bash
systemctl --user status leash.service --no-pager
curl -s http://127.0.0.1:8000/health
curl -s http://127.0.0.1:8000/capabilities
curl -s -X POST http://127.0.0.1:8000/motors/stop
```

## Waveshare Ubuntu UGV Install

Only run this after the bot is physically safe and the stock Waveshare process
is stopped. The serial port is single-owner.

Preflight the resolved config before starting the service:

```bash
leash show-config waveshare-ugv \
  --role courier \
  --listen 0.0.0.0:8000 \
  --serial-port /dev/ttyTHS1 \
  --no-untokened-drive \
  --allow-physical-actuation
```

Check `network_bind`, `physical_actuation_enabled`, and the field `source`
values before moving on.

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

The installer writes:

- `~/.local/bin/leash`
- `~/.config/leash/leash.env`
- `~/.config/systemd/user/leash.service`

Service commands:

```bash
systemctl --user restart leash.service
systemctl --user stop leash.service
journalctl --user -u leash.service -f
```

If the service should start before login, enable lingering once:

```bash
sudo loginctl enable-linger "$USER"
```

## Safety Defaults

- `sim` is the default profile.
- `waveshare-ugv` refuses install without `--allow-physical-actuation`.
- The runtime also refuses physical startup unless
  `LEASH_ALLOW_PHYSICAL_ACTUATION=true` or `LEASH_ALLOW_PHYSICAL_ACTUATION=1`.
- Deadman stop defaults to `400ms`.
- `estop` is latching and requires reset before future drive commands.
