#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  deployment-baseline.sh capture --source-revision VALUE --build-features LIST [options]
  deployment-baseline.sh verify [options]
  deployment-baseline.sh deploy CANDIDATE ARCHIVE --accelerator cpu|cuda --confirm [options]
  deployment-baseline.sh rollback ARCHIVE --confirm [options]

Capture or verify a private Waveshare UGV deployment baseline, or restore one.
Run this script on the UGV host. Verify, deploy, and rollback send stop commands;
capture is read-only. No action ever sends drive commands.

Options:
  --service NAME          systemd user service (default: leash.service)
  --base-url URL          local Leash HTTP URL (default: http://127.0.0.1:8000)
  --source-dir PATH       deployed source (default: resolved ~/leash-current)
  --source-revision TEXT  git revision plus local patch identity; required by capture
  --build-features LIST   exact Cargo feature list; required by capture
  --accelerator BACKEND   required active backend for deploy: cpu or cuda
  --drive-invert BOOL     explicitly set candidate wheel-direction inversion
  --drive-swap BOOL       explicitly set candidate left/right wheel swapping
  --output PATH           capture destination (default: private state directory)
  --confirm               required for deploy and rollback
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
candidate=""
env_files=()
accelerator=""
drive_invert=""
drive_swap=""

if [[ "$action" == "deploy" ]]; then
  candidate="${1:-}"
  archive="${2:-}"
  [[ -n "$candidate" && "$candidate" != --* ]] || die "deploy requires a candidate binary"
  [[ -n "$archive" && "$archive" != --* ]] || die "deploy requires a baseline archive path"
  shift 2
elif [[ "$action" == "rollback" ]]; then
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
    --accelerator)
      accelerator="${2:?--accelerator requires a value}"
      shift 2
      ;;
    --drive-invert)
      drive_invert="${2:?--drive-invert requires true or false}"
      shift 2
      ;;
    --drive-swap)
      drive_swap="${2:?--drive-swap requires true or false}"
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

for command in cmp curl fuser install jq readlink sha256sum systemctl; do
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
  mapfile -t env_files < <(service_property EnvironmentFiles | awk '{print $1}' | sed '/^$/d')
  [[ "${#env_files[@]}" -gt 0 ]] || die "service environment is unavailable"
  env_file="${env_files[0]}"
  [[ -x "$binary" ]] || die "service binary is not executable"
  [[ -f "$service_file" ]] || die "service unit is unavailable"
  local configured_env
  for configured_env in "${env_files[@]}"; do
    [[ -f "$configured_env" ]] || die "service environment is unavailable: $configured_env"
  done
}

env_value() {
  local key="$1"
  local configured_env
  for configured_env in "${env_files[@]}"; do
    sed -n "s/^${key}=//p" "$configured_env"
  done | tail -1
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
  systemctl --user cat "$service" >"$output/leash.service.effective"
  mkdir -m 0700 "$output/environment"
  : >"$output/environment-files.txt"
  local configured_env env_index env_copy
  for env_index in "${!env_files[@]}"; do
    configured_env="${env_files[$env_index]}"
    env_copy="$(printf '%03d.env' "$env_index")"
    install -m 0600 "$configured_env" "$output/environment/$env_copy"
    printf '%s=%s\n' "$env_copy" "$configured_env" >>"$output/environment-files.txt"
  done
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
    printf 'environment_file_count=%s\n' "${#env_files[@]}"
  } >"$output/manifest.txt"

  (
    cd "$output"
    sha256sum leash leash.service leash.service.effective leash.env source.tar.gz \
      environment-files.txt environment/*.env >archive.sha256
  )
  find "$output" -type f -exec chmod 0600 {} +
  chmod 0700 "$output" "$output/environment"
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

deploy() {
  [[ "$confirm" == "true" ]] || die "deploy requires --confirm"
  [[ "$accelerator" =~ ^(cpu|cuda)$ ]] || die "deploy requires --accelerator cpu|cuda"
  [[ -z "$drive_invert" || "$drive_invert" =~ ^(true|false)$ ]] \
    || die "--drive-invert must be true or false"
  [[ -z "$drive_swap" || "$drive_swap" =~ ^(true|false)$ ]] \
    || die "--drive-swap must be true or false"
  candidate="$(readlink -f "$candidate")"
  archive="$(readlink -f "$archive")"
  [[ -x "$candidate" ]] || die "candidate binary is not executable"
  [[ -d "$archive" ]] || die "baseline archive does not exist"
  for file in leash leash.service leash.env archive.sha256 manifest.txt; do
    [[ -f "$archive/$file" ]] || die "baseline archive is missing $file"
  done
  (cd "$archive" && sha256sum --check archive.sha256)
  service_paths
  cmp "$archive/leash" "$binary" || die "active binary does not match the verified baseline"
  local configured_env
  for configured_env in "${env_files[@]:1}"; do
    if grep -Eq '^(LEASH_ACCELERATOR|LEASH_REQUIRE_ACCELERATOR)=' "$configured_env"; then
      die "accelerator selection is overridden by a later service environment file"
    fi
    if [[ -n "$drive_invert" ]] && grep -Eq '^LEASH_DRIVE_INVERT=' "$configured_env"; then
      die "drive inversion is overridden by a later service environment file"
    fi
    if [[ -n "$drive_swap" ]] && grep -Eq '^LEASH_DRIVE_SWAP=' "$configured_env"; then
      die "drive swapping is overridden by a later service environment file"
    fi
  done

  local proof rollback_on_exit=0
  proof="$archive/deploy-$(date -u +%Y%m%dT%H%M%SZ)"
  mkdir -m 0700 "$proof"
  stop_now >"$proof/stop-before.json"
  device_ownership >"$proof/device-ownership-before.txt"
  sha256sum "$candidate" >"$proof/candidate.sha256"
  sha256sum "$binary" "$service_file" "$env_file" >"$proof/before.sha256"
  "$candidate" --version >"$proof/candidate-version.txt"
  awk -v backend="$accelerator" -v drive_invert="$drive_invert" -v drive_swap="$drive_swap" '
    BEGIN { accelerator_seen=0; required_seen=0; invert_seen=0; swap_seen=0 }
    /^LEASH_ACCELERATOR=/ {
      if (!accelerator_seen) print "LEASH_ACCELERATOR=" backend
      accelerator_seen=1
      next
    }
    /^LEASH_REQUIRE_ACCELERATOR=/ {
      if (!required_seen) print "LEASH_REQUIRE_ACCELERATOR=true"
      required_seen=1
      next
    }
    /^LEASH_DRIVE_INVERT=/ {
      if (drive_invert != "" && !invert_seen) print "LEASH_DRIVE_INVERT=" drive_invert
      else if (drive_invert == "") print
      invert_seen=1
      next
    }
    /^LEASH_DRIVE_SWAP=/ {
      if (drive_swap != "" && !swap_seen) print "LEASH_DRIVE_SWAP=" drive_swap
      else if (drive_swap == "") print
      swap_seen=1
      next
    }
    { print }
    END {
      if (!accelerator_seen) print "LEASH_ACCELERATOR=" backend
      if (!required_seen) print "LEASH_REQUIRE_ACCELERATOR=true"
      if (drive_invert != "" && !invert_seen) print "LEASH_DRIVE_INVERT=" drive_invert
      if (drive_swap != "" && !swap_seen) print "LEASH_DRIVE_SWAP=" drive_swap
    }
  ' "$env_file" >"$proof/leash.env.candidate"
  chmod 0600 "$proof/leash.env.candidate"
  sha256sum "$proof/leash.env.candidate" >"$proof/candidate-config.sha256"

  deploy_cleanup() {
    if [[ "$rollback_on_exit" == 1 ]]; then
      systemctl --user stop "$service" >/dev/null 2>&1 || true
      install -m 0755 "$archive/leash" "$binary"
      install -m 0644 "$archive/leash.service" "$service_file"
      install -m 0600 "$archive/leash.env" "$env_file"
      systemctl --user daemon-reload >/dev/null 2>&1 || true
      systemctl --user start "$service" >/dev/null 2>&1 || true
    fi
  }
  trap deploy_cleanup EXIT
  rollback_on_exit=1
  systemctl --user stop "$service"
  install -m 0755 "$candidate" "$binary"
  install -m 0600 "$proof/leash.env.candidate" "$env_file"
  systemctl --user start "$service"
  wait_for_health

  endpoint /health >"$proof/health.json"
  grep -Eq '"ok"[[:space:]]*:[[:space:]]*true' "$proof/health.json" \
    || die "candidate health did not report ok=true"
  jq -e --arg backend "$accelerator" \
    '.accelerator.requested == $backend and .accelerator.active == $backend and .accelerator.required == true' \
    "$proof/health.json" >/dev/null || die "candidate did not activate the required backend"
  endpoint /capabilities >"$proof/capabilities.json"
  endpoint /cognition/status >"$proof/cognition-status.json"
  endpoint /runtime-v2/status >"$proof/runtime-v2-status.json"
  jq -e --arg backend "$accelerator" \
    '.backend_status.selected == $backend and .backend_status.active == $backend and .backend_status.degraded == false and .capabilities.motor_authority == false' \
    "$proof/cognition-status.json" >/dev/null || die "candidate cognition backend is not healthy"
  jq -e \
    '.available == true and .cpu_final_authority == true and .control_authority == "cpu-safety-supervisor" and .motor_authority == "waveshare-controller-owner" and .controller.connected == true and .supervisor.faulted == false and .supervisor.evidence.healthy == true' \
    "$proof/runtime-v2-status.json" >/dev/null || die "candidate Runtime v2 authority is not healthy"
  endpoint /camera/status >"$proof/camera-status.json"
  endpoint /sensors >"$proof/sensors.json"
  stop_now >"$proof/stop-after.json"
  device_ownership >"$proof/device-ownership-after.txt"
  sha256sum "$binary" "$service_file" "$env_file" >"$proof/after.sha256"
  cmp "$candidate" "$binary" || die "active binary differs from the candidate"
  systemctl --user is-active --quiet "$service" || die "$service did not remain active"

  rollback_on_exit=0
  trap - EXIT
  chmod 0600 "$proof"/*
  printf '%s\n' "$proof"
}

case "$action" in
  capture) capture ;;
  verify) verify ;;
  deploy) deploy ;;
  rollback) rollback ;;
  *) die "unknown action: $action" ;;
esac
