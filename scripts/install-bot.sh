#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/install-bot.sh [options]

Build and install the leash binary plus a user systemd service on a bot host.

Options:
  --role NAME                    Bot role label. Default: robot
  --profile sim|waveshare-ugv    Runtime profile. Default: sim
  --listen ADDR:PORT             HTTP listen address. Default: 127.0.0.1:8000
  --serial-port PATH             Physical serial port. Default: /dev/ttyTHS1
  --serial-baud BAUD             Physical serial baud. Default: 115200
  --deadman-ms MS                Deadman stop timeout. Default: 400
  --soft-odometry-limit-m M      Forward odometry soft limit. Default: 0
  --drive-invert                 Invert left/right motor signs.
  --drive-swap                   Swap left/right motor outputs.
  --no-untokened-drive           Require pilot token for drive commands.
  --allow-physical-actuation     Permit physical profile to actuate motors.
  --start                        Enable and start the user service.
  --force-env                    Rewrite an existing env file.
  -h, --help                     Show this help.

Examples:
  scripts/install-bot.sh --profile sim --start

  scripts/install-bot.sh \
    --profile waveshare-ugv \
    --role courier \
    --listen 0.0.0.0:8000 \
    --serial-port /dev/ttyTHS1 \
    --no-untokened-drive \
    --allow-physical-actuation \
    --start
EOF
}

role="robot"
profile="sim"
listen="127.0.0.1:8000"
serial_port="/dev/ttyTHS1"
serial_baud="115200"
deadman_ms="400"
soft_odometry_limit_m="0"
drive_invert="false"
drive_swap="false"
allow_untokened_drive="true"
allow_physical_actuation="false"
start_service="false"
force_env="false"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --role)
      role="${2:?--role requires a value}"
      shift 2
      ;;
    --profile)
      profile="${2:?--profile requires a value}"
      shift 2
      ;;
    --listen)
      listen="${2:?--listen requires a value}"
      shift 2
      ;;
    --serial-port)
      serial_port="${2:?--serial-port requires a value}"
      shift 2
      ;;
    --serial-baud)
      serial_baud="${2:?--serial-baud requires a value}"
      shift 2
      ;;
    --deadman-ms)
      deadman_ms="${2:?--deadman-ms requires a value}"
      shift 2
      ;;
    --soft-odometry-limit-m)
      soft_odometry_limit_m="${2:?--soft-odometry-limit-m requires a value}"
      shift 2
      ;;
    --drive-invert)
      drive_invert="true"
      shift
      ;;
    --drive-swap)
      drive_swap="true"
      shift
      ;;
    --no-untokened-drive)
      allow_untokened_drive="false"
      shift
      ;;
    --allow-physical-actuation)
      allow_physical_actuation="true"
      shift
      ;;
    --start)
      start_service="true"
      shift
      ;;
    --force-env)
      force_env="true"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "$profile" in
  sim)
    cargo_features="http,mcp"
    ;;
  waveshare-ugv)
    cargo_features="http,mcp,waveshare-ugv,bridge-compat"
    if [[ "$allow_physical_actuation" != "true" ]]; then
      cat >&2 <<'EOF'
Refusing to install a physical profile without --allow-physical-actuation.
Use sim for dry runs, or rerun with the explicit physical gate after the bot is safe.
EOF
      exit 2
    fi
    ;;
  *)
    echo "unsupported profile: $profile" >&2
    exit 2
    ;;
esac

repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin_dir="$HOME/.local/bin"
config_dir="$HOME/.config/leash"
systemd_dir="$HOME/.config/systemd/user"
env_file="$config_dir/leash.env"
service_file="$systemd_dir/leash.service"

command -v cargo >/dev/null || {
  echo "cargo is required on this bot host" >&2
  exit 1
}

cd "$repo_dir"
cargo build --release --no-default-features --features "$cargo_features"

mkdir -p "$bin_dir" "$config_dir" "$systemd_dir"
install -m 0755 "$repo_dir/target/release/leash" "$bin_dir/leash"

if [[ -e "$env_file" && "$force_env" != "true" ]]; then
  echo "Keeping existing $env_file. Use --force-env to rewrite it."
else
  cat >"$env_file" <<EOF
LEASH_ROLE=$role
LEASH_PROFILE=$profile
LEASH_STREAM_TRANSPORT=local-pubsub
LEASH_LISTEN=$listen
LEASH_ALLOW_UNTOKENED_DRIVE=$allow_untokened_drive
LEASH_ALLOW_PHYSICAL_ACTUATION=$allow_physical_actuation
LEASH_DEADMAN_MS=$deadman_ms
LEASH_SOFT_ODOMETRY_LIMIT_M=$soft_odometry_limit_m
LEASH_SERIAL_PORT=$serial_port
LEASH_SERIAL_BAUD=$serial_baud
LEASH_DRIVE_INVERT=$drive_invert
LEASH_DRIVE_SWAP=$drive_swap
EOF
fi

cat >"$service_file" <<'EOF'
[Unit]
Description=Leash robot harness
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
EnvironmentFile=%h/.config/leash/leash.env
ExecStart=%h/.local/bin/leash serve http
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
EOF

systemctl --user daemon-reload

if [[ "$start_service" == "true" ]]; then
  systemctl --user enable --now leash.service
fi

cat <<EOF
Installed leash:
  binary:  $bin_dir/leash
  env:     $env_file
  service: $service_file

Check:
  systemctl --user status leash.service --no-pager
  curl -s http://${listen}/health
EOF
