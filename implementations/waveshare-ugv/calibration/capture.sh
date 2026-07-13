#!/usr/bin/env bash
set -euo pipefail

calibration_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ros_dir="$(cd "$calibration_dir/../ros2" && pwd)"
# shellcheck source=../ros2/clock-gate.sh
source "$ros_dir/clock-gate.sh"

phase=""
profile=""
env_file="${LEASH_ROS_ENV_FILE:-}"
pilot_token_file=""
output=""
replay_output=""
duration_secs=60
interval_ms=200
run_index=1
expected_distance_m="null"
expected_turn_deg=360
expected_side_m="null"
operator_confirmed=false
clock_reference_epoch=""
leash_url="${LEASH_URL:-http://127.0.0.1:8000}"

usage() {
  cat <<'EOF'
Usage: capture.sh --phase PHASE --profile FILE --env-file FILE --pilot-token-file FILE --output FILE --replay-output FILE [options]

Phases: stationary, straight, turn, square

Options:
  --duration-secs N            Capture duration (default: 60).
  --interval-ms N              Poll interval (default: 200).
  --run-index N                Run number, required as 1..3 for square evidence.
  --expected-distance-m N      Ground-truth distance for a straight run.
  --expected-turn-deg N        Ground-truth rotation (default: 360).
  --expected-side-m N          Ground-truth square side length (must be 1.0).
  --clock-reference-epoch N    Trusted operator epoch when NTP is unavailable.
  --pilot-token-file FILE      Private file containing the active pilot token.
  --replay-output FILE         New scrubbed leash-replay-v1 JSONL output.
  --operator-confirmed         Assert clear floor, present spotter, and reachable E-stop.

This recorder never issues drive, motor, or velocity commands. It enters one
bounded calibration phase, records its status, and exits through verified zero.
A present operator must drive only through the phase owner token.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --phase) phase="$2"; shift 2 ;;
    --profile) profile="$2"; shift 2 ;;
    --env-file) env_file="$2"; shift 2 ;;
    --pilot-token-file) pilot_token_file="$2"; shift 2 ;;
    --output) output="$2"; shift 2 ;;
    --replay-output) replay_output="$2"; shift 2 ;;
    --duration-secs) duration_secs="$2"; shift 2 ;;
    --interval-ms) interval_ms="$2"; shift 2 ;;
    --run-index) run_index="$2"; shift 2 ;;
    --expected-distance-m) expected_distance_m="$2"; shift 2 ;;
    --expected-turn-deg) expected_turn_deg="$2"; shift 2 ;;
    --expected-side-m) expected_side_m="$2"; shift 2 ;;
    --clock-reference-epoch) clock_reference_epoch="$2"; shift 2 ;;
    --operator-confirmed) operator_confirmed=true; shift ;;
    --help|-h) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ "$phase" =~ ^(stationary|straight|turn|square)$ ]] || { echo "invalid --phase" >&2; exit 2; }
[[ -n "$profile" && -r "$profile" ]] || { echo "a readable --profile is required" >&2; exit 2; }
[[ -n "$env_file" && -r "$env_file" ]] || { echo "a readable --env-file is required" >&2; exit 2; }
[[ -n "$pilot_token_file" && -r "$pilot_token_file" ]] || { echo "a readable --pilot-token-file is required" >&2; exit 2; }
[[ -n "$output" && ! -e "$output" ]] || { echo "--output must be a new file" >&2; exit 2; }
[[ -n "$replay_output" && ! -e "$replay_output" ]] || { echo "--replay-output must be a new file" >&2; exit 2; }
[[ "$output" != "$replay_output" ]] || { echo "capture and replay outputs must differ" >&2; exit 2; }
[[ "$duration_secs" =~ ^[1-9][0-9]*$ && "$interval_ms" =~ ^[1-9][0-9]*$ ]] || {
  echo "duration and interval must be positive integers" >&2
  exit 2
}
[[ "$run_index" =~ ^[1-9][0-9]*$ ]] || { echo "run index must be positive" >&2; exit 2; }
if [[ "$phase" == "stationary" && "$duration_secs" -lt 60 ]]; then
  echo "stationary acceptance capture must run for at least 60 seconds" >&2
  exit 2
fi
if [[ "$phase" == "straight" ]]; then
  [[ "$expected_distance_m" =~ ^[0-9]+([.][0-9]+)?$ ]] || {
    echo "straight capture requires --expected-distance-m" >&2
    exit 2
  }
  awk -v value="$expected_distance_m" 'BEGIN {exit !(value >= 0.999 && value <= 1.001)}' || {
    echo "issue #166 acceptance requires a measured one-meter straight run" >&2
    exit 2
  }
fi
if [[ "$phase" == "square" && ( "$run_index" -lt 1 || "$run_index" -gt 3 ) ]]; then
  echo "square --run-index must be 1, 2, or 3" >&2
  exit 2
fi
if [[ "$phase" == "square" ]]; then
  [[ "$expected_side_m" =~ ^[0-9]+([.][0-9]+)?$ ]] || {
    echo "square capture requires --expected-side-m" >&2
    exit 2
  }
  awk -v value="$expected_side_m" 'BEGIN {exit !(value >= 0.999 && value <= 1.001)}' || {
    echo "issue #166 acceptance requires one-meter square sides" >&2
    exit 2
  }
fi
[[ "$expected_turn_deg" =~ ^[0-9]+([.][0-9]+)?$ ]] || {
  echo "expected turn must be a positive number" >&2
  exit 2
}
awk -v value="$expected_turn_deg" 'BEGIN {exit !(value > 0)}' || {
  echo "expected turn must be positive" >&2
  exit 2
}
if [[ "$operator_confirmed" != true ]]; then
  echo "calibration capture requires --operator-confirmed after an on-site safety check" >&2
  exit 1
fi

python3 "$calibration_dir/profile.py" "$profile" validate --require-values >/dev/null
profile_digest="$(python3 "$calibration_dir/profile.py" "$profile" digest --require-values)"
profile_name="$(jq -r '.profile' "$profile")"
pilot_token="$(tr -d '\r\n' < "$pilot_token_file")"
[[ -n "$pilot_token" ]] || { echo "pilot token file is empty" >&2; exit 2; }
require_trusted_clock "$clock_reference_epoch"

compose() {
  docker compose --env-file "$env_file" -f "$ros_dir/compose.yaml" "$@"
}

stop_leash() {
  curl -fsS -X POST "$leash_url/stop" >/dev/null
}

verified_stop() {
  local reason="$1"
  curl -fsS -X POST \
    -H 'Content-Type: application/json' \
    --data "{\"reason\":\"$reason\"}" \
    "$leash_url/stop/verified"
}

calibration_enter() {
  local payload
  payload="$(jq -nc \
    --arg token "$pilot_token" \
    --arg calibration_sha256 "$profile_digest" \
    --arg phase "$phase" \
    --argjson run_index "$run_index" \
    '{token:$token,approval:true,calibration_sha256:$calibration_sha256,phase:$phase,run_index:$run_index}')"
  curl -fsS -X POST -H 'Content-Type: application/json' --data "$payload" \
    "$leash_url/calibration/enter"
}

calibration_status() {
  curl -fsS "$leash_url/calibration/status"
}

calibration_exit() {
  local payload
  payload="$(jq -nc --arg token "$pilot_token" '{token:$token}')"
  curl -fsS -X POST -H 'Content-Type: application/json' --data "$payload" \
    "$leash_url/calibration/exit"
}

mkdir -p "$(dirname "$output")"
chmod 700 "$(dirname "$output")"
mkdir -p "$(dirname "$replay_output")"
chmod 700 "$(dirname "$replay_output")"
umask 077
completed=false
calibration_entered=false
polls=0
stream_pid=""
raw_stream=""

finish() {
  local exit_code=$?
  local final_zero="null"
  if [[ -n "$stream_pid" ]]; then
    kill "$stream_pid" 2>/dev/null || true
    wait "$stream_pid" 2>/dev/null || true
  fi
  if [[ "$completed" != true ]]; then
    if [[ "$calibration_entered" == true ]]; then
      final_zero="$(calibration_exit 2>/dev/null)" || final_zero="null"
      calibration_entered=false
    else
      final_zero="$(verified_stop calibration-exit 2>/dev/null)" || final_zero="null"
    fi
    [[ "$final_zero" != "null" ]] || stop_leash || true
  else
    stop_leash || true
  fi
  if [[ "$completed" != true && -e "$output" ]]; then
    jq -nc --argjson exit_code "$exit_code" --argjson verified_zero "$final_zero" \
      '{kind:"capture-end",ok:false,reason:"aborted",exit_code:$exit_code,verified_zero:$verified_zero}' >> "$output" || true
  fi
  chmod 600 "$output" 2>/dev/null || true
  chmod 600 "$replay_output" 2>/dev/null || true
  [[ -z "$raw_stream" ]] || rm -f "$raw_stream"
}
trap finish EXIT
trap 'exit 130' INT TERM

entry_response="$(calibration_enter)"
calibration_entered=true
entry_verified_zero="$(jq -c '.verified_zero' <<<"$entry_response")"
entry_calibration_status="$(jq -c '.status' <<<"$entry_response")"
container_id="$(compose ps -q slam)"
[[ -n "$container_id" ]] || { echo "SLAM container is not running" >&2; exit 1; }
leash_pid="$(systemctl --user show leash.service -p MainPID --value)"
[[ "$leash_pid" =~ ^[1-9][0-9]*$ ]] || { echo "Leash service has no main PID" >&2; exit 1; }
initial_restart_count="$(docker inspect -f '{{.RestartCount}}' "$container_id")"
start_epoch_ms="$(($(date +%s) * 1000))"

jq -nc \
  --arg phase "$phase" \
  --arg profile "$profile_name" \
  --arg calibration_sha256 "$profile_digest" \
  --arg clock_proof "$CLOCK_PROOF_SOURCE" \
  --argjson clock_skew_secs "$CLOCK_PROOF_SKEW_SECS" \
  --argjson duration_secs "$duration_secs" \
  --argjson interval_ms "$interval_ms" \
  --argjson run_index "$run_index" \
  --argjson expected_distance_m "$expected_distance_m" \
  --argjson expected_turn_deg "$expected_turn_deg" \
  --argjson expected_side_m "$expected_side_m" \
  --argjson verified_zero "$entry_verified_zero" \
  --argjson calibration "$entry_calibration_status" \
  '{kind:"capture-start",format:"leash-waveshare-ugv-calibration-capture-v1",phase:$phase,profile:$profile,calibration_sha256:$calibration_sha256,clock:{proof:$clock_proof,initial_skew_secs:$clock_skew_secs},duration_secs:$duration_secs,interval_ms:$interval_ms,run_index:$run_index,expected_distance_m:$expected_distance_m,expected_turn_deg:$expected_turn_deg,expected_side_m:$expected_side_m,actuation_source:"external-supervised-operator",recorder_issues_motion:false,verified_zero:$verified_zero,calibration:$calibration}' \
  > "$output"

raw_stream="$(mktemp "${TMPDIR:-/tmp}/leash-calibration-stream.XXXXXX")"
curl -fsSN "$leash_url/events/telemetry" > "$raw_stream" &
stream_pid=$!
sleep 1
kill -0 "$stream_pid" 2>/dev/null || { echo "telemetry stream did not start" >&2; exit 1; }

deadline_ms=$((start_epoch_ms + duration_secs * 1000))
while (( $(($(date +%s) * 1000)) < deadline_ms )); do
  now_epoch_ms="$(($(date +%s) * 1000))"
  elapsed_ms=$((now_epoch_ms - start_epoch_ms))
  current_pid="$(systemctl --user show leash.service -p MainPID --value)"
  [[ "$current_pid" == "$leash_pid" ]] || { echo "Leash service PID changed" >&2; exit 1; }
  [[ "$(systemctl --user is-active leash.service)" == "active" ]] || { echo "Leash service stopped" >&2; exit 1; }

  health="$(curl -fsS "$leash_url/health")"
  telemetry="$(curl -fsS "$leash_url/telemetry")"
  provider="$(curl -fsS "$leash_url/localization")"
  calibration="$(calibration_status)"
  inspect="$(docker inspect "$container_id")"
  container_state="$(jq '.[0] | {running:.State.Running,oom_killed:.State.OOMKilled,restart_count:.RestartCount}' <<<"$inspect")"
  cpu_pct="$(docker stats --no-stream --format '{{.CPUPerc}}' "$container_id" | tr -d '%')"
  memory_usage="$(docker stats --no-stream --format '{{.MemUsage}}' "$container_id")"

  [[ "$(jq -r '.ok' <<<"$health")" == "true" ]] || { echo "Leash health degraded" >&2; exit 1; }
  [[ "$(jq -r '.sensors.range_scan.status' <<<"$telemetry")" == "available" ]] || { echo "lidar unavailable" >&2; exit 1; }
  [[ "$(jq -r '.sensors.imu.status' <<<"$telemetry")" == "available" ]] || { echo "IMU unavailable" >&2; exit 1; }
  [[ "$(jq -r '.state' <<<"$provider")" == "tracking" ]] || { echo "localization is not tracking" >&2; exit 1; }
  jq -e --arg phase "$phase" --arg digest "$profile_digest" --argjson run_index "$run_index" \
    '.active == true and .phase == $phase and .run_index == $run_index and .calibration_sha256 == $digest' \
    <<<"$calibration" >/dev/null || { echo "calibration lease changed or expired" >&2; exit 1; }
  [[ "$(jq -r '.running' <<<"$container_state")" == "true" ]] || { echo "SLAM container stopped" >&2; exit 1; }
  [[ "$(jq -r '.oom_killed' <<<"$container_state")" == "false" ]] || { echo "SLAM container was OOM killed" >&2; exit 1; }
  [[ "$(jq -r '.restart_count' <<<"$container_state")" == "$initial_restart_count" ]] || { echo "SLAM container restarted" >&2; exit 1; }
  if [[ "$phase" == "stationary" ]]; then
    jq -e '.left_cmd == 0 and .right_cmd == 0' <<<"$telemetry" >/dev/null || {
      echo "stationary capture observed a non-zero motor command" >&2
      exit 1
    }
  fi

  jq -nc \
    --argjson elapsed_ms "$elapsed_ms" \
    --argjson health "$health" \
    --argjson telemetry "$telemetry" \
    --argjson provider "$provider" \
    --argjson calibration "$calibration" \
    --argjson container "$container_state" \
    --argjson cpu_pct "$cpu_pct" \
    --arg memory_usage "$memory_usage" \
    '{kind:"sample",elapsed_ms:$elapsed_ms,health:($health|{ok,mode,role,profile,physical,physical_navigation_enabled}),telemetry:($telemetry|{ts_ms,left_cmd,right_cmd,odometry_left,odometry_right,deadman_ok,estop,stopped_by_deadman,soft_odometry_limited,speed_mode,max_speed,sensors:{odometry:.sensors.odometry,range_scan:.sensors.range_scan,imu:.sensors.imu},localization,localization_provider,resource,motion_events}),provider:($provider|{provider,state,sequence,generation,last_update_ms,last_received_ms,stale_after_ms,message,error}),calibration:$calibration,container:($container+{cpu_pct:$cpu_pct,memory_usage:$memory_usage})}' \
    >> "$output"
  ((polls += 1))
  sleep "$(awk -v milliseconds="$interval_ms" 'BEGIN {printf "%.3f", milliseconds / 1000}')"
done

exit_verified_zero="$(calibration_exit)"
calibration_entered=false
sleep 1
final_telemetry="$(curl -fsS "$leash_url/telemetry")"
jq -e '.left_cmd == 0 and .right_cmd == 0' <<<"$final_telemetry" >/dev/null || {
  echo "final motor stop was not observed" >&2
  exit 1
}
kill "$stream_pid" 2>/dev/null || true
wait "$stream_pid" 2>/dev/null || true
stream_pid=""
python3 "$calibration_dir/replay.py" "$raw_stream" "$replay_output" >/dev/null
replay_sha256="$(sha256sum "$replay_output" | awk '{print $1}')"
replay_name="$(basename "$replay_output")"
jq -nc --argjson polls "$polls" --argjson telemetry "$final_telemetry" --argjson verified_zero "$exit_verified_zero" --arg replay "$replay_name" --arg replay_sha256 "$replay_sha256" \
  '{kind:"capture-end",ok:true,polls:$polls,verified_zero:$verified_zero,final_command:{left_cmd:$telemetry.left_cmd,right_cmd:$telemetry.right_cmd,estop:$telemetry.estop},replay:{file:$replay,sha256:$replay_sha256,format:"leash-replay-v1"}}' \
  >> "$output"
completed=true
chmod 600 "$output"
chmod 600 "$replay_output"
printf '{"ok":true,"phase":"%s","polls":%s,"output":"%s","replay":"%s","recorder_issues_motion":false}\n' \
  "$phase" "$polls" "$output" "$replay_output"
