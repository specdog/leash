#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  deployment-baseline.sh capture --source-revision VALUE --build-features LIST [options]
  deployment-baseline.sh verify [options]
  deployment-baseline.sh rollback ARCHIVE --confirm [options]

Capture or verify a private Waveshare UGV deployment baseline, or restore one.
Run this script on the UGV host. It sends stop commands but never drive commands.

Options:
  --service NAME          systemd user service (default: leash.service)
  --base-url URL          local Leash HTTP URL (default: http://127.0.0.1:8000)
  --source-dir PATH       deployed source (default: resolved ~/leash-current)
  --source-revision TEXT  git revision plus local patch identity; required by capture
  --build-features LIST   exact Cargo feature list; required by capture
  --output PATH           capture destination (default: private state directory)
  --confirm               required for rollback
  -h, --help              show this help
EOF
}

die() {
  echo "error: $*" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || die "$1 is required"
}

action="${1:-}"
if [[ "$action" == "-h" || "$action" == "--help" || -z "$action" ]]; then
  usage
  exit 0
fi
shift

service="leash.service"
base_url="http://127.0.0.1:8000"
source_dir="$(readlink -f "$HOME/leash-current" 2>/dev/null || true)"
source_revision=""
build_features=""
output=""
confirm="false"
archive=""

if [[ "$action" == "rollback" ]]; then
  archive="${1:-}"
  [[ -n "$archive" && "$archive" != --* ]] || die "rollback requires an archive path"
  shift
fi

while [[ $# -gt 0 ]]; do
  case "$1" in
    --service)
      service="${2:?--service requires a value}"
      shift 2
      ;;
    --base-url)
      base_url="${2:?--base-url requires a value}"
      shift 2
      ;;
    --source-dir)
      source_dir="${2:?--source-dir requires a value}"
      shift 2
      ;;
    --source-revision)
      source_revision="${2:?--source-revision requires a value}"
      shift 2
      ;;
    --build-features)
      build_features="${2:?--build-features requires a value}"
      shift 2
      ;;
    --output)
      output="${2:?--output requires a value}"
      shift 2
      ;;
    --confirm)
      confirm="true"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown argument: $1"
      ;;
  esac
done

for command in curl fuser sha256sum systemctl; do
  need "$command"
done

service_property() {
  systemctl --user show "$service" -p "$1" --value
}

service_paths() {
  main_pid="$(service_property MainPID)"
  [[ "$main_pid" =~ ^[1-9][0-9]*$ ]] || die "$service has no running MainPID"
  binary="$(readlink -f "/proc/$main_pid/exe")"
  service_file="$(service_property FragmentPath)"
  env_file="$(service_property EnvironmentFiles | awk '{print $1}')"
  [[ -x "$binary" ]] || die "service binary is not executable"
  [[ -f "$service_file" ]] || die "service unit is unavailable"
  [[ -f "$env_file" ]] || die "service environment is unavailable"
}

env_value() {
  local key="$1"
  sed -n "s/^${key}=//p" "$env_file" | tail -1
}

endpoint() {
  local path="$1"
  curl --fail --silent --show-error --max-time 5 "$base_url$path"
}

stop_now() {
  local response
  response="$(curl --fail --silent --show-error --max-time 5 -X POST "$base_url/stop")"
  grep -Eq '"left"[[:space:]]*:[[:space:]]*0([,.}]|\.0)' <<<"$response" || die "stop response did not prove left zero"
  grep -Eq '"right"[[:space:]]*:[[:space:]]*0([,.}]|\.0)' <<<"$response" || die "stop response did not prove right zero"
  printf '%s\n' "$response"
}

device_ownership() {
  service_paths
  local cgroup cgroup_file allowed serial camera device owners pid foreign=0
  cgroup="$(service_property ControlGroup)"
  cgroup_file="/sys/fs/cgroup${cgroup}/cgroup.procs"
  allowed="$main_pid"
  if [[ -r "$cgroup_file" ]]; then
    allowed="$(tr '\n' ' ' <"$cgroup_file")"
  fi
  serial="$(env_value LEASH_SERIAL_PORT)"
  camera="$(env_value LEASH_CAMERA_DEVICE)"

  printf 'SERVICE_PID=%s\n' "$main_pid"
  for device in "$serial" "$camera"; do
    [[ -n "$device" ]] || continue
    if [[ ! -e "$device" ]]; then
      printf 'DEVICE=%s STATUS=missing\n' "$device"
      continue
    fi
    owners="$(fuser "$device" 2>/dev/null | tr '\n' ' ' || true)"
    printf 'DEVICE=%s OWNERS=%s\n' "$device" "${owners:-none}"
    for pid in $owners; do
      if ! grep -Eq "(^|[[:space:]])${pid}([[:space:]]|$)" <<<"$allowed"; then
        printf 'FOREIGN_OWNER device=%s pid=%s\n' "$device" "$pid"
        foreign=1
      fi
    done
  done
  [[ "$foreign" == 0 ]] || die "a configured device has a foreign owner"
}

wait_for_health() {
  local attempt
  for attempt in {1..30}; do
    if endpoint /health >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  die "Leash health did not recover within 30 seconds"
}

capture() {
  [[ -n "$source_revision" ]] || die "capture requires --source-revision"
  [[ -n "$build_features" ]] || die "capture requires --build-features"
  [[ -d "$source_dir" ]] || die "source directory is unavailable; pass --source-dir"
  service_paths

  local stamp state_root
  stamp="$(date -u +%Y%m%dT%H%M%SZ)"
  state_root="${XDG_STATE_HOME:-$HOME/.local/state}/leash/waveshare-ugv-baselines"
  output="${output:-$state_root/$stamp}"
  umask 077
  mkdir -p "$output"
  chmod 0700 "$output"

  install -m 0755 "$binary" "$output/leash"
  install -m 0644 "$service_file" "$output/leash.service"
  install -m 0600 "$env_file" "$output/leash.env"
  awk '
    /^[[:space:]]*#/ || /^[[:space:]]*$/ { print; next }
    /^[A-Za-z_][A-Za-z0-9_]*=/ { sub(/=.*/, "=<redacted>"); print; next }
    { print "<redacted-line>" }
  ' "$env_file" >"$output/leash.env.redacted"

  tar \
    --exclude=.git \
    --exclude=.env \
    --exclude=.env.local \
    --exclude=node_modules \
    --exclude=target \
    --exclude=vendor \
    -czf "$output/source.tar.gz" \
    -C "$source_dir" .

  (
    cd "$source_dir"
    find . -type f \
      -not -path './.git/*' \
      -not -path './node_modules/*' \
      -not -path './target/*' \
      -not -path './vendor/*' \
      -not -name '.env' \
      -not -name '.env.local' \
      -print0 | sort -z | xargs -0 sha256sum
  ) >"$output/source-files.sha256"

  endpoint /health >"$output/health.json"
  endpoint /capabilities >"$output/capabilities.json"
  endpoint /camera/status >"$output/camera-status.json"
  endpoint /sensors >"$output/sensors.json"
  device_ownership >"$output/device-ownership.txt"
  systemctl --user show "$service" \
    -p ActiveState -p SubState -p MainPID -p FragmentPath -p ExecStart -p EnvironmentFiles \
    >"$output/service-properties.txt"

  {
    printf 'captured_at=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    printf 'leash_version=%s\n' "$("$binary" --version)"
    printf 'binary_sha256=%s\n' "$(sha256sum "$binary" | awk '{print $1}')"
    printf 'source_revision=%s\n' "$source_revision"
    printf 'build_features=%s\n' "$build_features"
    printf 'source_snapshot=source.tar.gz\n'
    printf 'service=%s\n' "$service"
  } >"$output/manifest.txt"

  (
    cd "$output"
    sha256sum leash leash.service leash.env source.tar.gz >archive.sha256
  )
  chmod 0600 "$output"/*
  chmod 0700 "$output/leash"
  printf '%s\n' "$output"
}

verify() {
  service_paths
  systemctl --user is-active --quiet "$service" || die "$service is not active"
  endpoint /health
  printf '\n'
  endpoint /capabilities >/dev/null
  endpoint /camera/status >/dev/null
  endpoint /sensors >/dev/null
  stop_now
  device_ownership
}

rollback() {
  [[ "$confirm" == "true" ]] || die "rollback requires --confirm"
  archive="$(readlink -f "$archive")"
  [[ -d "$archive" ]] || die "archive does not exist"
  for file in leash leash.service leash.env archive.sha256 manifest.txt; do
    [[ -f "$archive/$file" ]] || die "archive is missing $file"
  done
  (cd "$archive" && sha256sum --check archive.sha256)
  service_paths

  local proof restart_on_exit=0
  proof="$archive/rollback-$(date -u +%Y%m%dT%H%M%SZ)"
  mkdir -m 0700 "$proof"
  stop_now >"$proof/stop-before.json"
  device_ownership >"$proof/device-ownership-before.txt"

  rollback_cleanup() {
    if [[ "$restart_on_exit" == 1 ]]; then
      systemctl --user start "$service" >/dev/null 2>&1 || true
    fi
  }
  trap rollback_cleanup EXIT
  restart_on_exit=1
  systemctl --user stop "$service"
  install -m 0755 "$archive/leash" "$binary"
  install -m 0644 "$archive/leash.service" "$service_file"
  install -m 0600 "$archive/leash.env" "$env_file"
  systemctl --user daemon-reload
  systemctl --user start "$service"
  wait_for_health
  restart_on_exit=0
  trap - EXIT

  endpoint /health >"$proof/health.json"
  endpoint /capabilities >"$proof/capabilities.json"
  endpoint /camera/status >"$proof/camera-status.json"
  endpoint /sensors >"$proof/sensors.json"
  stop_now >"$proof/stop-after.json"
  device_ownership >"$proof/device-ownership-after.txt"
  sha256sum "$binary" >"$proof/binary.sha256"
  cmp "$archive/leash" "$binary" || die "restored binary differs from archive"
  systemctl --user is-active --quiet "$service" || die "$service did not remain active"
  chmod 0600 "$proof"/*
  printf '%s\n' "$proof"
}

case "$action" in
  capture) capture ;;
  verify) verify ;;
  rollback) rollback ;;
  *) die "unknown action: $action" ;;
esac
