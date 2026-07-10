#!/usr/bin/env bash
set -euo pipefail

navigation_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
replay_converter="$navigation_dir/../calibration/replay.py"

phase=""
run_id=""
token_file=""
output_dir=""
goal_x=""
goal_y=""
tolerance_m="0.15"
zone_id=""
timeout_secs=120
operator_confirmed=false
leash_url="${LEASH_URL:-http://127.0.0.1:8000}"
leash_bin="${LEASH_BIN:-leash}"
capture_pid=""
completed=false

usage() {
  cat <<'EOF'
Usage: field-proof.sh --phase PHASE --run-id ID --token-file FILE --output-dir DIR --operator-confirmed [options]

Phases:
  half-meter  One goal whose measured start-to-target distance is 0.45 through 0.55 m.
  map-goal    One of the three consecutive supervised map-frame goal proofs.
  patrol      One complete pass through an explicitly bounded saved patrol zone.

Goal options: --goal-x M --goal-y M [--tolerance-m M]
Patrol option: --zone-id ID
Other options: --timeout-secs N --leash-url URL --leash-bin PATH

--operator-confirmed asserts that the floor is clear, a spotter is present, and
the physical E-stop is reachable. The script refuses non-loopback URLs, keeps
the token out of artifacts, starts and ends with stop, and captures a scrubbed
normal Leash replay plus resource and final-stop evidence.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --phase) phase="$2"; shift 2 ;;
    --run-id) run_id="$2"; shift 2 ;;
    --token-file) token_file="$2"; shift 2 ;;
    --output-dir) output_dir="$2"; shift 2 ;;
    --goal-x) goal_x="$2"; shift 2 ;;
    --goal-y) goal_y="$2"; shift 2 ;;
    --tolerance-m) tolerance_m="$2"; shift 2 ;;
    --zone-id) zone_id="$2"; shift 2 ;;
    --timeout-secs) timeout_secs="$2"; shift 2 ;;
    --leash-url) leash_url="$2"; shift 2 ;;
    --leash-bin) leash_bin="$2"; shift 2 ;;
    --operator-confirmed) operator_confirmed=true; shift ;;
    --help|-h) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

die() {
  echo "physical navigation proof error: $*" >&2
  exit 1
}

post_empty() {
  curl -fsS -X POST "$leash_url/$1" >/dev/null
}

stop_all() {
  set +e
  post_empty planner/cancel
  post_empty patrol/stop
  post_empty stop
  if [[ -n "$capture_pid" ]]; then
    kill "$capture_pid" 2>/dev/null
    wait "$capture_pid" 2>/dev/null
  fi
  set -e
}

cleanup() {
  if [[ "$completed" != true ]]; then
    stop_all
  fi
}
trap cleanup EXIT INT TERM

[[ "$phase" =~ ^(half-meter|map-goal|patrol)$ ]] || { usage >&2; exit 2; }
[[ "$run_id" =~ ^[A-Za-z0-9_-]{1,64}$ ]] || die "--run-id must be a safe 1 through 64 character identifier"
[[ -n "$token_file" && -f "$token_file" && -r "$token_file" ]] || die "a readable --token-file is required"
[[ -n "$output_dir" && ! -e "$output_dir" ]] || die "--output-dir must not already exist"
[[ "$timeout_secs" =~ ^[1-9][0-9]*$ ]] || die "--timeout-secs must be a positive integer"
[[ "$operator_confirmed" == true ]] || die "--operator-confirmed is required for every physical run"
[[ "$leash_url" =~ ^http://(127\.0\.0\.1|localhost)(:[0-9]+)?$ ]] || die "--leash-url must be loopback HTTP"
[[ -x "$replay_converter" ]] || die "replay converter is missing"
for command in curl jq python3 sha256sum stat; do
  command -v "$command" >/dev/null || die "$command is required"
done
command -v "$leash_bin" >/dev/null || [[ -x "$leash_bin" ]] || die "Leash binary is not executable"

token_mode="$(stat -c '%a' "$token_file")"
[[ "$token_mode" == "600" || "$token_mode" == "400" ]] || die "token file permissions must be 0600 or 0400"
token="$(tr -d '\r\n' < "$token_file")"
[[ -n "$token" ]] || die "token file is empty"

if [[ "$phase" == "patrol" ]]; then
  [[ "$zone_id" =~ ^[A-Za-z0-9_-]{1,64}$ ]] || die "patrol requires a safe --zone-id"
  [[ -z "$goal_x" && -z "$goal_y" ]] || die "patrol does not accept goal coordinates"
else
  [[ "$goal_x" =~ ^-?[0-9]+([.][0-9]+)?$ && "$goal_y" =~ ^-?[0-9]+([.][0-9]+)?$ ]] || die "goal phase requires numeric --goal-x and --goal-y"
  [[ "$tolerance_m" =~ ^[0-9]+([.][0-9]+)?$ ]] || die "--tolerance-m must be positive"
  awk -v value="$tolerance_m" 'BEGIN {exit !(value > 0 && value <= 0.15)}' || die "acceptance tolerance must be greater than zero and no more than 0.15 m"
fi

umask 077
mkdir -p "$output_dir"
raw_sse="$output_dir/telemetry.sse"
samples_json="$output_dir/samples.json"
replay_jsonl="$output_dir/replay.jsonl"
preflight_json="$output_dir/preflight.json"
start_json="$output_dir/start.json"
terminal_json="$output_dir/terminal.json"
final_json="$output_dir/final-telemetry.json"
summary_json="$output_dir/summary.json"
replay_validation="$output_dir/replay-validation.txt"
started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

post_empty stop
curl -fsS "$leash_url/health" > "$output_dir/health.json"
curl -fsS "$leash_url/telemetry" > "$preflight_json"

jq -e '
  .profile == "waveshare-ugv" and
  .physical_actuation_enabled == true and
  .physical_navigation_enabled == true and
  .estop == false and
  .deadman_ok == true
' "$output_dir/health.json" >/dev/null || die "health did not prove the gated Waveshare UGV profile"

jq -e '
  .left_cmd == 0 and .right_cmd == 0 and
  .soft_odometry_limited == false and .estop == false and .deadman_ok == true and
  .resource != null and
  .sensors.range_scan.status == "available" and
  .localization.health.status == "tracking" and
  .localization_provider.state == "tracking" and
  (.localization.pose.covariance | length) == 9 and
  (.localization.pose.covariance[0] >= 0 and (.localization.pose.covariance[0] | sqrt) <= 0.15) and
  (.localization.pose.covariance[4] >= 0 and (.localization.pose.covariance[4] | sqrt) <= 0.15) and
  (.localization.pose.covariance[8] >= 0 and (.localization.pose.covariance[8] | sqrt) <= 0.17453292519943295)
' "$preflight_json" >/dev/null || die "preflight lacks stop, resource, lidar, localization, or covariance proof"

if [[ "$phase" == "half-meter" ]]; then
  initial_x="$(jq -r '.localization.pose.pose.x_m' "$preflight_json")"
  initial_y="$(jq -r '.localization.pose.pose.y_m' "$preflight_json")"
  awk -v x0="$initial_x" -v y0="$initial_y" -v x1="$goal_x" -v y1="$goal_y" \
    'BEGIN {distance=sqrt((x1-x0)^2+(y1-y0)^2); exit !(distance >= 0.45 && distance <= 0.55)}' || \
    die "half-meter target must be 0.45 through 0.55 m from the preflight pose"
fi

if [[ "$phase" == "patrol" ]]; then
  curl -fsS "$leash_url/patrol/zones" > "$output_dir/zones.json"
  curl -fsS "$leash_url/waypoints" > "$output_dir/waypoints.json"
  jq -e --arg zone "$zone_id" '
    any(.zones[]; .id == $zone and (.boundary | length) >= 3 and (.waypoint_ids | length) >= 1)
  ' "$output_dir/zones.json" >/dev/null || die "saved patrol zone is missing a polygon boundary or waypoints"
fi

curl -fsSN "$leash_url/events/telemetry" > "$raw_sse" &
capture_pid=$!
sleep 1
kill -0 "$capture_pid" 2>/dev/null || die "telemetry capture did not stay running"

jq -n --arg token "$token" --argjson ttl "$((timeout_secs + 30))" \
  '{token:$token,ttl_secs:$ttl,speed_mode:"low"}' | \
  curl -fsS -X POST "$leash_url/pilot/authorize" -H 'content-type: application/json' --data-binary @- >/dev/null

if [[ "$phase" == "patrol" ]]; then
  jq -n --arg token "$token" '{token:$token,approval:true,speed_mode:"low"}' | \
    curl -fsS -X POST "$leash_url/patrol/zones/$zone_id/start" \
      -H 'content-type: application/json' --data-binary @- > "$start_json"
  jq -e '.active == true and .speed_mode == "low" and .zone_id != null' "$start_json" >/dev/null || die "patrol did not start through the low-speed gate"
  status_endpoint="patrol/status"
  success_status="completed"
else
  jq -n --arg token "$token" --argjson x "$goal_x" --argjson y "$goal_y" --argjson tolerance "$tolerance_m" \
    '{token:$token,approval:true,frame_id:"map",x_m:$x,y_m:$y,tolerance_m:$tolerance,speed_mode:"low"}' | \
    curl -fsS -X POST "$leash_url/planner/goal" \
      -H 'content-type: application/json' --data-binary @- > "$start_json"
  jq -e '.active == true and .goal.speed_mode == "low"' "$start_json" >/dev/null || die "goal did not start through the low-speed gate"
  status_endpoint="planner/status"
  success_status="reached"
fi

deadline=$((SECONDS + timeout_secs))
while (( SECONDS < deadline )); do
  curl -fsS "$leash_url/$status_endpoint" > "$terminal_json"
  status="$(jq -r '.status' "$terminal_json")"
  active="$(jq -r '.active' "$terminal_json")"
  if [[ "$active" == "false" && "$status" == "$success_status" ]]; then
    break
  fi
  if [[ "$active" == "false" && "$status" != "active" ]]; then
    die "navigation stopped with status '$status'"
  fi
  sleep 0.2
done

jq -e --arg status "$success_status" '.active == false and .status == $status' "$terminal_json" >/dev/null || die "navigation proof timed out"
post_empty patrol/stop
post_empty planner/cancel
post_empty stop
sleep 1
curl -fsS "$leash_url/telemetry" > "$final_json"
stop_all
capture_pid=""

jq -Rn '
  [inputs
    | select(startswith("data:"))
    | sub("^data:[ ]?"; "")
    | fromjson
    | select(.kind == "telemetry")]
' < "$raw_sse" > "$samples_json"
jq -e 'length > 0' "$samples_json" >/dev/null || die "telemetry stream produced no frames"
jq -e '
  all(.[].telemetry;
    (.left_cmd | fabs) <= 0.22 and
    (.right_cmd | fabs) <= 0.22 and
    ((.left_cmd == 0 and .right_cmd == 0) or .speed_mode == "low") and
    .estop == false and
    .resource != null)
' "$samples_json" >/dev/null || die "capture violated the low-speed, E-stop, or resource evidence contract"
jq -e '.left_cmd == 0 and .right_cmd == 0' "$final_json" >/dev/null || die "final telemetry did not prove zero speed"

python3 "$replay_converter" "$raw_sse" "$replay_jsonl" >/dev/null
"$leash_bin" replay "$replay_jsonl" --speed 100 > "$replay_validation"

final_error="null"
if [[ "$phase" != "patrol" ]]; then
  final_x="$(jq -r '.localization.pose.pose.x_m' "$final_json")"
  final_y="$(jq -r '.localization.pose.pose.y_m' "$final_json")"
  final_error="$(awk -v x0="$final_x" -v y0="$final_y" -v x1="$goal_x" -v y1="$goal_y" 'BEGIN {printf "%.6f", sqrt((x1-x0)^2+(y1-y0)^2)}')"
  awk -v error="$final_error" -v tolerance="$tolerance_m" 'BEGIN {exit !(error <= tolerance)}' || die "final pose error exceeds tolerance"
fi

capture_sha="$(sha256sum "$raw_sse" | awk '{print $1}')"
replay_sha="$(sha256sum "$replay_jsonl" | awk '{print $1}')"
finished_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
jq -n \
  --arg phase "$phase" \
  --arg run_id "$run_id" \
  --arg started_at "$started_at" \
  --arg finished_at "$finished_at" \
  --arg capture_sha256 "$capture_sha" \
  --arg replay_sha256 "$replay_sha" \
  --arg zone_id "$zone_id" \
  --argjson goal_x "${goal_x:-null}" \
  --argjson goal_y "${goal_y:-null}" \
  --argjson tolerance_m "${tolerance_m:-null}" \
  --argjson final_error_m "$final_error" \
  --argjson initial "$(jq '.localization.pose.pose' "$preflight_json")" \
  --argjson final "$(jq '.localization.pose.pose' "$final_json")" \
  --argjson terminal "$(cat "$terminal_json")" \
  --argjson samples "$(jq 'length' "$samples_json")" \
  --argjson resource_samples "$(jq '[.[].telemetry.resource | select(. != null)] | length' "$samples_json")" \
  '{ok:true,format:"leash-waveshare-ugv-navigation-proof-v1",phase:$phase,run_id:$run_id,started_at:$started_at,finished_at:$finished_at,target:{frame_id:"map",x_m:$goal_x,y_m:$goal_y,tolerance_m:$tolerance_m,zone_id:(if $zone_id == "" then null else $zone_id end)},initial_pose:$initial,final_pose:$final,final_error_m:$final_error_m,terminal:$terminal,safety:{operator_confirmed:true,clear_floor:true,spotter_present:true,estop_reachable:true,low_speed_cap:0.22,final_motor_stop:true},artifacts:{capture_sha256:$capture_sha256,replay_sha256:$replay_sha256,replay_validated:true},samples:$samples,resource_samples:$resource_samples}' \
  > "$summary_json"

chmod 0600 "$output_dir"/*
completed=true
printf '%s\n' "$summary_json"
