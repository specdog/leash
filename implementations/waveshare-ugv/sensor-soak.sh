#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: sensor-soak.sh [options]

Stationary, non-driving proof for the Waveshare UGV lidar and IMU implementation.
The script sends stop before and after polling; it never sends a drive command.

Options:
  --duration-secs N       proof duration (default: 600)
  --interval-secs N       poll interval (default: 1)
  --max-sensor-age-ms N   allowed sample age (default: 1500)
  --max-rss-growth-mb N   allowed RSS high-water spread (default: 64)
  --service NAME          systemd user service (default: leash.service)
  --base-url URL          local Leash URL (default: http://127.0.0.1:8000)
  --output PATH           optional JSON proof path
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

duration_secs=600
interval_secs=1
max_sensor_age_ms=1500
max_rss_growth_mb=64
service="leash.service"
base_url="http://127.0.0.1:8000"
output=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --duration-secs) duration_secs="${2:?missing duration}"; shift 2 ;;
    --interval-secs) interval_secs="${2:?missing interval}"; shift 2 ;;
    --max-sensor-age-ms) max_sensor_age_ms="${2:?missing age}"; shift 2 ;;
    --max-rss-growth-mb) max_rss_growth_mb="${2:?missing growth}"; shift 2 ;;
    --service) service="${2:?missing service}"; shift 2 ;;
    --base-url) base_url="${2:?missing URL}"; shift 2 ;;
    --output) output="${2:?missing output}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
done

need curl
need jq
need systemctl

[[ "$duration_secs" =~ ^[0-9]+$ && "$duration_secs" -gt 0 ]] || die "duration must be positive"
[[ "$interval_secs" =~ ^[0-9]+$ && "$interval_secs" -gt 0 ]] || die "interval must be positive"
[[ "$max_sensor_age_ms" =~ ^[0-9]+$ ]] || die "sensor age must be an integer"
[[ "$max_rss_growth_mb" =~ ^[0-9]+$ ]] || die "RSS growth must be an integer"

stop_robot() {
  curl -fsS -X POST "$base_url/stop" >/dev/null || true
}
trap stop_robot EXIT
stop_robot

[[ "$(systemctl --user is-active "$service")" == "active" ]] || die "$service is not active"
pid="$(systemctl --user show "$service" -p MainPID --value)"
[[ "$pid" =~ ^[0-9]+$ && "$pid" -gt 1 && -r "/proc/$pid/status" ]] || die "service PID unavailable"

started_epoch="$(date +%s)"
deadline=$((started_epoch + duration_secs))
polls=0
rss_min_kb=0
rss_max_kb=0
max_lidar_age_ms=0
max_imu_age_ms=0
min_scan_points=0

while (( $(date +%s) < deadline )); do
  [[ "$(systemctl --user is-active "$service")" == "active" ]] || die "$service stopped"
  current_pid="$(systemctl --user show "$service" -p MainPID --value)"
  [[ "$current_pid" == "$pid" ]] || die "$service restarted during proof"

  health="$(curl -fsS "$base_url/health")"
  sensors="$(curl -fsS "$base_url/sensors")"
  [[ "$(jq -r '.ok' <<<"$health")" == "true" ]] || die "health degraded"
  [[ "$(jq -r '.sensors.range_scan.status' <<<"$sensors")" == "available" ]] || die "lidar is not available"
  [[ "$(jq -r '.sensors.imu.status' <<<"$sensors")" == "available" ]] || die "IMU is not available"

  now_ms="$(date +%s%3N)"
  lidar_ms="$(jq -r '.sensors.range_scan.last_ms // 0' <<<"$sensors")"
  imu_ms="$(jq -r '.sensors.imu.last_ms // 0' <<<"$sensors")"
  lidar_age=$((now_ms - lidar_ms))
  imu_age=$((now_ms - imu_ms))
  (( lidar_age >= 0 && lidar_age <= max_sensor_age_ms )) || die "lidar sample age exceeded tolerance"
  (( imu_age >= 0 && imu_age <= max_sensor_age_ms )) || die "IMU sample age exceeded tolerance"
  (( lidar_age > max_lidar_age_ms )) && max_lidar_age_ms="$lidar_age"
  (( imu_age > max_imu_age_ms )) && max_imu_age_ms="$imu_age"

  points="$(jq -r '.sensors.range_scan.sample.ranges_m | length' <<<"$sensors")"
  rate="$(jq -r '.sensors.range_scan.sample.scan_rate_hz // 0' <<<"$sensors")"
  (( points >= 12 )) || die "lidar scan has too few bins"
  jq -en --argjson rate "$rate" '$rate > 0' >/dev/null || die "lidar scan rate is invalid"
  if (( min_scan_points == 0 || points < min_scan_points )); then
    min_scan_points="$points"
  fi

  rss_kb="$(awk '/^VmRSS:/ {print $2}' "/proc/$pid/status")"
  [[ "$rss_kb" =~ ^[0-9]+$ ]] || die "service RSS unavailable"
  if (( rss_min_kb == 0 || rss_kb < rss_min_kb )); then rss_min_kb="$rss_kb"; fi
  if (( rss_kb > rss_max_kb )); then rss_max_kb="$rss_kb"; fi

  polls=$((polls + 1))
  sleep "$interval_secs"
done

rss_growth_kb=$((rss_max_kb - rss_min_kb))
(( rss_growth_kb <= max_rss_growth_mb * 1024 )) || die "RSS growth exceeded tolerance"

proof="$(jq -n \
  --arg suite "waveshare-ugv-stationary-sensors" \
  --argjson duration_secs "$duration_secs" \
  --argjson polls "$polls" \
  --argjson pid "$pid" \
  --argjson rss_min_kb "$rss_min_kb" \
  --argjson rss_max_kb "$rss_max_kb" \
  --argjson rss_growth_kb "$rss_growth_kb" \
  --argjson max_lidar_age_ms "$max_lidar_age_ms" \
  --argjson max_imu_age_ms "$max_imu_age_ms" \
  --argjson min_scan_points "$min_scan_points" \
  '{ok:true, suite:$suite, duration_secs:$duration_secs, polls:$polls, service_pid:$pid, rss:{min_kb:$rss_min_kb,max_kb:$rss_max_kb,growth_kb:$rss_growth_kb}, sensors:{max_lidar_age_ms:$max_lidar_age_ms,max_imu_age_ms:$max_imu_age_ms,min_scan_points:$min_scan_points}}')"

if [[ -n "$output" ]]; then
  umask 077
  mkdir -p "$(dirname "$output")"
  printf '%s\n' "$proof" > "$output"
fi
printf '%s\n' "$proof"
