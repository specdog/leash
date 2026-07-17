#!/usr/bin/env bash
set -euo pipefail

calibration_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ros_dir="$(cd "$calibration_dir/../ros2" && pwd)"
# shellcheck source=../ros2/clock-gate.sh
source "$ros_dir/clock-gate.sh"

profile=""
env_file="${LEASH_ROS_ENV_FILE:-}"
map_name=""
output=""
clock_reference_epoch=""
timeout_secs=120
leash_url="${LEASH_URL:-http://127.0.0.1:8000}"

usage() {
  cat <<'EOF'
Usage: map-reload-proof.sh --profile FILE --env-file FILE --map-name NAME --output FILE [options]

Options:
  --timeout-secs N             Tracking recovery timeout (default: 120).
  --clock-reference-epoch N    Trusted operator epoch when NTP is unavailable.

The robot remains stopped. The proof saves occupancy output and pose graph,
restarts only the read-only ROS container, reloads the graph, and requires Leash
tracking to recover without changing the Leash service PID.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --profile) profile="$2"; shift 2 ;;
    --env-file) env_file="$2"; shift 2 ;;
    --map-name) map_name="$2"; shift 2 ;;
    --output) output="$2"; shift 2 ;;
    --timeout-secs) timeout_secs="$2"; shift 2 ;;
    --clock-reference-epoch) clock_reference_epoch="$2"; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ -n "$profile" && -r "$profile" ]] || { echo "a readable --profile is required" >&2; exit 2; }
[[ -n "$env_file" && -r "$env_file" ]] || { echo "a readable --env-file is required" >&2; exit 2; }
[[ "$map_name" =~ ^[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}$ ]] || { echo "invalid --map-name" >&2; exit 2; }
[[ -n "$output" && ! -e "$output" ]] || { echo "--output must be a new file" >&2; exit 2; }
[[ "$timeout_secs" =~ ^[1-9][0-9]*$ ]] || { echo "timeout must be positive" >&2; exit 2; }

python3 "$calibration_dir/profile.py" "$profile" validate --require-values >/dev/null
profile_digest="$(python3 "$calibration_dir/profile.py" "$profile" digest --require-values)"
profile_name="$(jq -r '.profile' "$profile")"
require_trusted_clock "$clock_reference_epoch"

stack_args=(--env-file "$env_file")
[[ -n "$clock_reference_epoch" ]] && stack_args+=(--clock-reference-epoch "$clock_reference_epoch")
stack() {
  "$ros_dir/slam-stack.sh" "${stack_args[@]}" "$@"
}
compose() {
  docker compose --env-file "$env_file" -f "$ros_dir/compose.yaml" "$@"
}
stop_leash() {
  curl -fsS -X POST "$leash_url/stop" >/dev/null
}
wait_for_tracking() {
  local deadline=$(( $(date +%s) + timeout_secs ))
  while (( $(date +%s) < deadline )); do
    if [[ "$(curl -fsS "$leash_url/localization" | jq -r '.state')" == "tracking" ]]; then
      return 0
    fi
    sleep 1
  done
  echo "localization did not return to tracking" >&2
  return 1
}

mkdir -p "$(dirname "$output")"
chmod 700 "$(dirname "$output")"
umask 077
trap 'stop_leash || true' EXIT
stop_leash

leash_pid="$(systemctl --user show leash.service -p MainPID --value)"
[[ "$leash_pid" =~ ^[1-9][0-9]*$ ]] || { echo "Leash service has no main PID" >&2; exit 1; }
wait_for_tracking
before="$(curl -fsS "$leash_url/telemetry")"
jq -e '.localization.pose.covariance | length == 9' <<<"$before" >/dev/null || {
  echo "pre-save localization covariance is unavailable" >&2
  exit 1
}

stack save "$map_name" >/dev/null
for suffix in posegraph data yaml pgm; do
  compose exec -T slam test -s "/data/maps/${map_name}.${suffix}" || {
    echo "saved map is missing ${map_name}.${suffix}" >&2
    exit 1
  }
done

stop_leash
compose restart slam >/dev/null
stop_leash
wait_for_tracking
stack load "$map_name" >/dev/null
wait_for_tracking
after="$(curl -fsS "$leash_url/telemetry")"
current_pid="$(systemctl --user show leash.service -p MainPID --value)"
[[ "$current_pid" == "$leash_pid" ]] || { echo "Leash service PID changed" >&2; exit 1; }
jq -e '.localization.pose.covariance | length == 9' <<<"$after" >/dev/null || {
  echo "post-load localization covariance is unavailable" >&2
  exit 1
}
before_map_id="$(jq -r '.localization.map.map_id' <<<"$before")"
after_map_id="$(jq -r '.localization.map.map_id' <<<"$after")"
[[ -n "$before_map_id" && "$before_map_id" == "$after_map_id" ]] || {
  echo "map identity changed across reload" >&2
  exit 1
}
container_id="$(compose ps -q slam)"
container_state="$(docker inspect "$container_id" | jq '.[0] | {running:.State.Running,oom_killed:.State.OOMKilled,restart_count:.RestartCount}')"
[[ "$(jq -r '.running' <<<"$container_state")" == "true" && "$(jq -r '.oom_killed' <<<"$container_state")" == "false" ]] || {
  echo "SLAM container is unhealthy after reload" >&2
  exit 1
}
stop_leash

jq -n \
  --arg profile "$profile_name" \
  --arg calibration_sha256 "$profile_digest" \
  --arg map_name "$map_name" \
  --arg clock_proof "$CLOCK_PROOF_SOURCE" \
  --argjson clock_skew_secs "$CLOCK_PROOF_SKEW_SECS" \
  --argjson leash_pid "$leash_pid" \
  --argjson before "$before" \
  --argjson after "$after" \
  --argjson container "$container_state" \
  '{ok:true,format:"leash-waveshare-ugv-map-reload-proof-v1",profile:$profile,calibration_sha256:$calibration_sha256,map_name:$map_name,clock:{proof:$clock_proof,initial_skew_secs:$clock_skew_secs},leash:{pid:$leash_pid,unchanged:true,final_motor_stop:true},saved_files:["posegraph","data","yaml","pgm"],before:{map:$before.localization.map,pose:$before.localization.pose},after:{map:$after.localization.map,pose:$after.localization.pose},container:$container,recorder_issues_motion:false}' \
  > "$output"
chmod 600 "$output"
printf '{"ok":true,"map_name":"%s","output":"%s","recorder_issues_motion":false}\n' "$map_name" "$output"
