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

The robot remains stopped. The proof saves occupancy output, pose graph, and
lineage, records exact sizes and hashes, recreates only the read-only ROS
container, reloads the exact lineage, and requires a new provider instance and
generation without changing the Leash service PID or stopped grid revision.
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
verified_stop() {
  local reason="$1"
  curl -fsS -X POST \
    -H 'Content-Type: application/json' \
    --data "{\"reason\":\"$reason\"}" \
    "$leash_url/stop/verified"
}
wait_for_tracking() {
  local deadline status
  deadline=$(( $(date +%s) + timeout_secs ))
  while (( $(date +%s) < deadline )); do
    status="$(curl -fsS "$leash_url/localization" 2>/dev/null || true)"
    if [[ -n "$status" && "$(jq -r '.state // ""' <<<"$status")" == "tracking" ]]; then
      return 0
    fi
    sleep 1
  done
  echo "localization did not return to tracking" >&2
  return 1
}
telemetry_snapshot() {
  local telemetry
  telemetry="$(curl -fsS "$leash_url/telemetry")"
  jq -e '
    .left_cmd == 0 and .right_cmd == 0 and
    (.map.map_id | type == "string" and length > 0) and
    (.map.map_revision | type == "string" and length > 0) and
    (.map.grid_revision | type == "string" and length > 0) and
    (.map.frame_id | type == "string" and length > 0) and
    .localization_provider.state == "tracking" and
    (.localization_provider.provider_instance_id | type == "string" and length > 0) and
    (.localization_provider.generation | type == "number" and . > 0) and
    (.localization_provider.last_received_ms | type == "number") and
    (.localization_provider.stale_after_ms | type == "number" and . > 0) and
    (.ts_ms - .localization_provider.last_received_ms >= 0) and
    (.ts_ms - .localization_provider.last_received_ms <= .localization_provider.stale_after_ms) and
    (.localization.pose.covariance | type == "array" and length == 9) and
    (.localization.pose.pose.ts_ms | type == "number") and
    (.ts_ms - .localization.pose.pose.ts_ms >= 0) and
    (.ts_ms - .localization.pose.pose.ts_ms <= .localization_provider.stale_after_ms)
  ' <<<"$telemetry" >/dev/null || {
    echo "telemetry does not contain a fresh stopped map/provider/pose snapshot" >&2
    return 1
  }
  jq -c '{
    captured_at_ms:.ts_ms,
    map:.map,
    provider:.localization_provider,
    pose:.localization.pose,
    command:{left_cmd:.left_cmd,right_cmd:.right_cmd}
  }' <<<"$telemetry"
}
artifact_metadata() {
  compose exec -T slam python3 - "$map_name" <<'PY'
import hashlib
import json
import sys
from pathlib import Path

name = sys.argv[1]
root = Path("/data/maps")
paths = [
    root / f"{name}.posegraph",
    root / f"{name}.data",
    root / f"{name}.yaml",
    root / f"{name}.pgm",
    root / f"{name}.lineage.json",
]
records = []
for path in paths:
    payload = path.read_bytes()
    if not payload:
        raise SystemExit(f"saved map artifact is empty: {path}")
    records.append({
        "file": path.name,
        "size_bytes": len(payload),
        "sha256": hashlib.sha256(payload).hexdigest(),
    })
print(json.dumps(records, sort_keys=True, separators=(",", ":")))
PY
}
saved_lineage() {
  compose exec -T slam python3 - "$map_name" <<'PY'
import json
import sys
from pathlib import Path

path = Path("/data/maps") / f"{sys.argv[1]}.lineage.json"
value = json.loads(path.read_text(encoding="utf-8"))
expected = {"format", "map_id", "map_revision", "frame_id"}
if not isinstance(value, dict) or set(value) != expected:
    raise SystemExit(f"invalid saved lineage fields: {path}")
print(json.dumps(value, sort_keys=True, separators=(",", ":")))
PY
}

mkdir -p "$(dirname "$output")"
chmod 700 "$(dirname "$output")"
umask 077
trap 'stop_leash || true' EXIT
entry_verified_zero="$(verified_stop map-reload-entry)"

leash_pid="$(systemctl --user show leash.service -p MainPID --value)"
[[ "$leash_pid" =~ ^[1-9][0-9]*$ ]] || { echo "Leash service has no main PID" >&2; exit 1; }
wait_for_tracking
before="$(telemetry_snapshot)"

stack save "$map_name" >/dev/null
artifacts_before="$(artifact_metadata)"
lineage="$(saved_lineage)"

jq -e -n \
  --argjson lineage "$lineage" \
  --argjson before "$before" \
  '$lineage.map_id == $before.map.map_id and
   $lineage.map_revision == $before.map.map_revision and
   $lineage.frame_id == $before.map.frame_id' >/dev/null || {
  echo "saved lineage does not match the active map identity" >&2
  exit 1
}

stop_leash
stack load "$map_name" >/dev/null
wait_for_tracking
after="$(telemetry_snapshot)"
artifacts_after="$(artifact_metadata)"
current_pid="$(systemctl --user show leash.service -p MainPID --value)"
[[ "$current_pid" == "$leash_pid" ]] || { echo "Leash service PID changed" >&2; exit 1; }

jq -e -n \
  --argjson lineage "$lineage" \
  --argjson before "$before" \
  --argjson after "$after" \
  '$lineage.map_id == $after.map.map_id and
   $lineage.map_revision == $after.map.map_revision and
   $lineage.frame_id == $after.map.frame_id and
   $before.map.grid_revision == $after.map.grid_revision' >/dev/null || {
  echo "map lineage or stopped grid revision changed across reload" >&2
  exit 1
}
jq -e -n \
  --argjson before "$before" \
  --argjson after "$after" \
  '$before.provider.provider_instance_id != $after.provider.provider_instance_id and
   $after.provider.generation > $before.provider.generation' >/dev/null || {
  echo "provider instance and generation did not advance across reload" >&2
  exit 1
}
jq -e -n \
  --argjson before "$artifacts_before" \
  --argjson after "$artifacts_after" \
  '$before == $after' >/dev/null || {
  echo "saved map artifact size or hash changed across reload" >&2
  exit 1
}

container_id="$(compose ps -q slam)"
container_state="$(docker inspect "$container_id" | jq '.[0] | {running:.State.Running,oom_killed:.State.OOMKilled,restart_count:.RestartCount}')"
[[ "$(jq -r '.running' <<<"$container_state")" == "true" && "$(jq -r '.oom_killed' <<<"$container_state")" == "false" ]] || {
  echo "SLAM container is unhealthy after reload" >&2
  exit 1
}
exit_verified_zero="$(verified_stop map-reload-exit)"

jq -n \
  --arg profile "$profile_name" \
  --arg calibration_sha256 "$profile_digest" \
  --arg map_name "$map_name" \
  --arg clock_proof "$CLOCK_PROOF_SOURCE" \
  --argjson clock_skew_secs "$CLOCK_PROOF_SKEW_SECS" \
  --argjson leash_pid "$leash_pid" \
  --argjson lineage "$lineage" \
  --argjson artifacts_before "$artifacts_before" \
  --argjson artifacts_after "$artifacts_after" \
  --argjson before "$before" \
  --argjson after "$after" \
  --argjson container "$container_state" \
  --argjson entry_verified_zero "$entry_verified_zero" \
  --argjson exit_verified_zero "$exit_verified_zero" \
  '{
    ok:true,
    format:"leash-waveshare-ugv-map-reload-proof-v2",
    profile:$profile,
    calibration_sha256:$calibration_sha256,
    map_name:$map_name,
    clock:{proof:$clock_proof,initial_skew_secs:$clock_skew_secs},
    lineage:$lineage,
    saved_artifacts:{before:$artifacts_before,after:$artifacts_after},
    before:$before,
    after:$after,
    leash:{
      pid:$leash_pid,
      unchanged:true,
      entry_verified_zero:$entry_verified_zero,
      exit_verified_zero:$exit_verified_zero
    },
    container:$container,
    recorder_issues_motion:false
  }' > "$output"
chmod 600 "$output"
printf '{"ok":true,"map_name":"%s","output":"%s","recorder_issues_motion":false}\n' "$map_name" "$output"
