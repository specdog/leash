#!/usr/bin/env bash
set -euo pipefail

ros_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
duration_secs=1800
interval_secs=2
warmup_secs=120
output="${HOME}/.local/state/leash/waveshare-ugv-ros-soak.json"
env_file="${LEASH_ROS_ENV_FILE:-}"
leash_url="${LEASH_URL:-http://127.0.0.1:8000}"
clock_reference_epoch=""

# shellcheck source=clock-gate.sh
source "$ros_dir/clock-gate.sh"

usage() {
  cat <<'EOF'
Usage: ros-soak.sh --env-file PATH [options]

Options:
  --duration-secs N   Total stationary run time (default: 1800).
  --interval-secs N   Poll interval (default: 2).
  --warmup-secs N     Grace period before tracking is required (default: 120).
  --output PATH       Private JSON proof output.
  --clock-reference-epoch EPOCH
                      Fresh epoch from a trusted operator machine when NTP is unavailable.

The script sends stop before and after, never sends drive, and requires an
unchanged Leash PID/container, available lidar/IMU, tracking localization after
warmup, no container restart/OOM, and bounded target memory.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --env-file) env_file="$2"; shift 2 ;;
    --duration-secs) duration_secs="$2"; shift 2 ;;
    --interval-secs) interval_secs="$2"; shift 2 ;;
    --warmup-secs) warmup_secs="$2"; shift 2 ;;
    --output) output="$2"; shift 2 ;;
    --clock-reference-epoch) clock_reference_epoch="$2"; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

for value in "$duration_secs" "$interval_secs" "$warmup_secs"; do
  [[ "$value" =~ ^[0-9]+$ ]] || { echo "durations must be integers" >&2; exit 2; }
done
(( duration_secs > 0 && interval_secs > 0 && warmup_secs < duration_secs )) || {
  echo "invalid soak timing" >&2
  exit 2
}
[[ -n "$env_file" && -r "$env_file" ]] || { echo "a readable --env-file is required" >&2; exit 2; }
require_trusted_clock "$clock_reference_epoch"

compose() {
  docker compose --env-file "$env_file" -f "$ros_dir/compose.yaml" "$@"
}

mkdir -p "$(dirname "$output")"
chmod 700 "$(dirname "$output")"
curl -fsS -X POST "$leash_url/stop" >/dev/null
trap 'curl -fsS -X POST "$leash_url/stop" >/dev/null || true' EXIT

container_id="$(compose ps -q slam)"
[[ -n "$container_id" ]] || { echo "SLAM container is not running" >&2; exit 1; }
initial_restart_count="$(docker inspect -f '{{.RestartCount}}' "$container_id")"
leash_pid="$(systemctl --user show leash.service -p MainPID --value)"
[[ "$leash_pid" =~ ^[1-9][0-9]*$ ]] || { echo "Leash service has no main PID" >&2; exit 1; }

start_epoch="$(date +%s)"
deadline=$((start_epoch + duration_secs))
polls=0
min_rss_kb=0
max_rss_kb=0
max_cpu_pct=0
max_lidar_age_ms=0
max_imu_age_ms=0

while (( $(date +%s) < deadline )); do
  now_epoch="$(date +%s)"
  current_pid="$(systemctl --user show leash.service -p MainPID --value)"
  [[ "$current_pid" == "$leash_pid" ]] || { echo "Leash service PID changed" >&2; exit 1; }
  [[ "$(systemctl --user is-active leash.service)" == "active" ]] || { echo "Leash service stopped" >&2; exit 1; }

  inspect="$(docker inspect "$container_id")"
  [[ "$(jq -r '.[0].State.Running' <<<"$inspect")" == "true" ]] || { echo "SLAM container stopped" >&2; exit 1; }
  [[ "$(jq -r '.[0].State.OOMKilled' <<<"$inspect")" == "false" ]] || { echo "SLAM container was OOM killed" >&2; exit 1; }
  [[ "$(jq -r '.[0].RestartCount' <<<"$inspect")" == "$initial_restart_count" ]] || { echo "SLAM container restarted" >&2; exit 1; }
  container_pid="$(jq -r '.[0].State.Pid' <<<"$inspect")"
  cgroup_path="$(awk -F: '$1 == "0" {print $3}' "/proc/$container_pid/cgroup")"
  memory_current="/sys/fs/cgroup${cgroup_path}/memory.current"
  if [[ -r "$memory_current" ]]; then
    rss_kb=$(( $(<"$memory_current") / 1024 ))
  else
    rss_kb="$(awk '/VmRSS:/ {print $2}' "/proc/$container_pid/status")"
  fi
  [[ "$rss_kb" =~ ^[0-9]+$ ]] || { echo "cannot read container RSS" >&2; exit 1; }
  (( min_rss_kb == 0 || rss_kb < min_rss_kb )) && min_rss_kb="$rss_kb"
  (( rss_kb > max_rss_kb )) && max_rss_kb="$rss_kb"

  cpu_raw="$(docker stats --no-stream --format '{{.CPUPerc}}' "$container_id" | tr -d '%')"
  cpu_int="${cpu_raw%%.*}"
  [[ "$cpu_int" =~ ^[0-9]+$ ]] && (( cpu_int > max_cpu_pct )) && max_cpu_pct="$cpu_int"

  health="$(curl -fsS "$leash_url/health")"
  sensors="$(curl -fsS "$leash_url/sensors")"
  localization="$(curl -fsS "$leash_url/localization")"
  [[ "$(jq -r '.ok' <<<"$health")" == "true" ]] || { echo "Leash health degraded" >&2; exit 1; }
  [[ "$(jq -r '.sensors.range_scan.status' <<<"$sensors")" == "available" ]] || { echo "lidar unavailable" >&2; exit 1; }
  [[ "$(jq -r '.sensors.imu.status' <<<"$sensors")" == "available" ]] || { echo "IMU unavailable" >&2; exit 1; }

  now_ms=$((now_epoch * 1000))
  lidar_ms="$(jq -r '.sensors.range_scan.last_ms' <<<"$sensors")"
  imu_ms="$(jq -r '.sensors.imu.last_ms' <<<"$sensors")"
  lidar_age=$((now_ms - lidar_ms))
  imu_age=$((now_ms - imu_ms))
  (( lidar_age > max_lidar_age_ms )) && max_lidar_age_ms="$lidar_age"
  (( imu_age > max_imu_age_ms )) && max_imu_age_ms="$imu_age"
  (( lidar_age <= 1000 && imu_age <= 1000 )) || { echo "sensor stream is stale" >&2; exit 1; }

  if (( now_epoch - start_epoch >= warmup_secs )); then
    [[ "$(jq -r '.state' <<<"$localization")" == "tracking" ]] || { echo "localization is not tracking" >&2; exit 1; }
  fi
  ((polls += 1))
  sleep "$interval_secs"
done

curl -fsS -X POST "$leash_url/stop" >/dev/null
jq -n \
  --argjson duration_secs "$duration_secs" \
  --argjson polls "$polls" \
  --argjson leash_pid "$leash_pid" \
  --argjson min_rss_kb "$min_rss_kb" \
  --argjson max_rss_kb "$max_rss_kb" \
  --argjson max_cpu_pct "$max_cpu_pct" \
  --argjson max_lidar_age_ms "$max_lidar_age_ms" \
  --argjson max_imu_age_ms "$max_imu_age_ms" \
  --arg clock_proof_source "$CLOCK_PROOF_SOURCE" \
  --argjson initial_clock_skew_secs "$CLOCK_PROOF_SKEW_SECS" \
  '{ok:true,suite:"waveshare-ugv-read-only-slam",duration_secs:$duration_secs,polls:$polls,leash_pid:$leash_pid,clock:{proof:$clock_proof_source,initial_skew_secs:$initial_clock_skew_secs},container:{restarts:0,oom_killed:false,rss:{min_kb:$min_rss_kb,max_kb:$max_rss_kb,growth_kb:($max_rss_kb-$min_rss_kb)},max_cpu_pct:$max_cpu_pct},sensors:{max_lidar_age_ms:$max_lidar_age_ms,max_imu_age_ms:$max_imu_age_ms},localization:"tracking"}' \
  | tee "$output"
chmod 600 "$output"
