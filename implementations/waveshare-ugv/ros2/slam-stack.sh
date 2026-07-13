#!/usr/bin/env bash
set -euo pipefail

ros_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
compose_file="$ros_dir/compose.yaml"
env_file="${LEASH_ROS_ENV_FILE:-}"
leash_url="${LEASH_URL:-http://127.0.0.1:8000}"
clock_reference_epoch=""

# shellcheck source=clock-gate.sh
source "$ros_dir/clock-gate.sh"

usage() {
  cat <<'EOF'
Usage: slam-stack.sh [--env-file PATH] [--clock-reference-epoch EPOCH] COMMAND [ARG]

Commands:
  build             Build the pinned ROS 2 Humble image.
  start             Send Leash stop, then start the read-only SLAM stack.
  stop              Send Leash stop, then stop the SLAM stack.
  restart           Stop and start the SLAM stack without restarting Leash.
  status            Show container, ROS node/topic, and Leash provider status.
  logs              Follow container logs.
  new NAME          Create and activate a new stable map lineage.
  save NAME         Save occupancy output, pose graph, and active lineage under /data/maps.
  load NAME         Activate and load an exact saved lineage for localization.

The environment file is private and follows .env.example. This script never
sends a drive command and the container has no serial device mapping. The clock
reference is accepted only when NTP is unavailable and must be generated on a
trusted operator machine immediately before invoking this command.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --env-file)
      [[ $# -ge 2 ]] || { echo "--env-file requires a path" >&2; exit 2; }
      env_file="$2"
      shift 2
      ;;
    --clock-reference-epoch)
      [[ $# -ge 2 ]] || { echo "--clock-reference-epoch requires a value" >&2; exit 2; }
      clock_reference_epoch="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *) break ;;
  esac
done

[[ $# -ge 1 ]] || { usage >&2; exit 2; }
[[ -n "$env_file" && -r "$env_file" ]] || {
  echo "set LEASH_ROS_ENV_FILE or pass --env-file with a readable private file" >&2
  exit 2
}

compose() {
  apply_active_lineage
  docker compose --env-file "$env_file" -f "$compose_file" "$@"
}

stop_leash() {
  curl -fsS -X POST "$leash_url/stop" >/dev/null
}

valid_map_name() {
  [[ "$1" != "active" && "$1" =~ ^[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}$ ]]
}

env_value() {
  python3 - "$env_file" "$1" <<'PY'
import sys
from pathlib import Path

path = Path(sys.argv[1])
key = sys.argv[2]
for raw_line in path.read_text(encoding="utf-8").splitlines():
    line = raw_line.strip()
    if not line or line.startswith("#") or "=" not in line:
        continue
    name, value = line.split("=", 1)
    if name.strip() != key:
        continue
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'":
        value = value[1:-1]
    print(value)
    break
PY
}

lineage_root() {
  local state_dir
  state_dir="$(env_value LEASH_ROS_MAP_STATE_DIR)"
  [[ "$state_dir" = /* ]] || {
    echo "LEASH_ROS_MAP_STATE_DIR must be an absolute path" >&2
    return 1
  }
  printf '%s/maps\n' "$state_dir"
}

active_lineage_path() {
  printf '%s/active.lineage.json\n' "$(lineage_root)"
}

write_lineage() {
  local path="$1"
  local map_id="$2"
  local map_revision="$3"
  python3 - "$path" "$map_id" "$map_revision" <<'PY'
import json
import os
import sys
from pathlib import Path

path = Path(sys.argv[1])
value = {
    "format": "leash-map-lineage-v1",
    "map_id": sys.argv[2],
    "map_revision": sys.argv[3],
    "frame_id": "map",
}
path.parent.mkdir(parents=True, exist_ok=True)
temporary = path.with_suffix(path.suffix + ".tmp")
temporary.write_text(json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n", encoding="utf-8")
os.chmod(temporary, 0o600)
temporary.replace(path)
PY
}

validate_lineage() {
  local path="$1"
  local configured_map_id
  configured_map_id="$(env_value LEASH_ROS_MAP_ID)"
  [[ -n "$configured_map_id" ]] || configured_map_id="waveshare-ugv-map"
  python3 - "$path" "$configured_map_id" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
configured_map_id = sys.argv[2]
try:
    value = json.loads(path.read_text(encoding="utf-8"))
except (OSError, json.JSONDecodeError) as error:
    raise SystemExit(f"invalid map lineage {path}: {error}")
expected = {"format", "map_id", "map_revision", "frame_id"}
if not isinstance(value, dict) or set(value) != expected:
    raise SystemExit(f"invalid map lineage fields: {path}")
if value["format"] != "leash-map-lineage-v1":
    raise SystemExit(f"invalid map lineage format: {path}")
if value["map_id"] != configured_map_id or value["frame_id"] != "map":
    raise SystemExit(f"map lineage does not match configured map identity: {path}")
if not isinstance(value["map_revision"], str) or not value["map_revision"].strip():
    raise SystemExit(f"map lineage revision is empty: {path}")
PY
}

ensure_active_lineage() {
  local active map_id map_revision
  active="$(active_lineage_path)"
  if [[ -f "$active" ]]; then
    validate_lineage "$active"
    return
  fi
  map_id="$(env_value LEASH_ROS_MAP_ID)"
  [[ -n "$map_id" ]] || map_id="waveshare-ugv-map"
  map_revision="$(env_value LEASH_ROS_MAP_REVISION)"
  [[ -n "$map_revision" ]] || {
    echo "LEASH_ROS_MAP_REVISION is required until a map lineage is activated" >&2
    return 1
  }
  write_lineage "$active" "$map_id" "$map_revision"
}

apply_active_lineage() {
  local active
  active="$(active_lineage_path)"
  [[ -f "$active" ]] || return 0
  validate_lineage "$active"
  export LEASH_ROS_MAP_ID
  export LEASH_ROS_MAP_REVISION
  LEASH_ROS_MAP_ID="$(jq -er '.map_id' "$active")"
  LEASH_ROS_MAP_REVISION="$(jq -er '.map_revision' "$active")"
}

command="$1"
shift
case "$command" in
  build)
    compose build --pull=false
    ;;
  start)
    stop_leash
    require_trusted_clock "$clock_reference_epoch"
    ensure_active_lineage
    compose up -d --no-build
    stop_leash
    ;;
  stop)
    stop_leash
    compose stop
    stop_leash
    ;;
  restart)
    stop_leash
    require_trusted_clock "$clock_reference_epoch"
    compose restart slam
    stop_leash
    ;;
  status)
    compose ps
    compose exec -T slam bash -lc \
      'source /opt/ros/humble/setup.bash; ros2 node list; ros2 topic list | grep -E "^/(scan|imu/data_raw|wheel/odometry|odometry/filtered|map|pose|tf|tf_static)$"'
    curl -fsS "$leash_url/localization" | jq .
    ;;
  logs)
    compose logs -f slam
    ;;
  new)
    [[ $# -eq 1 ]] || { echo "new requires one map name" >&2; exit 2; }
    valid_map_name "$1" || { echo "invalid map name" >&2; exit 2; }
    map_id="$(env_value LEASH_ROS_MAP_ID)"
    [[ -n "$map_id" ]] || map_id="waveshare-ugv-map"
    map_revision="$(python3 -c 'import uuid; print(uuid.uuid4().hex)')"
    lineage="$(lineage_root)/$1.lineage.json"
    [[ ! -e "$lineage" ]] || { echo "map lineage already exists: $1" >&2; exit 1; }
    write_lineage "$lineage" "$map_id" "$map_revision"
    install -m 600 "$lineage" "$(active_lineage_path)"
    printf 'activated map lineage %s (%s)\n' "$1" "$map_revision"
    ;;
  save)
    [[ $# -eq 1 ]] || { echo "save requires one map name" >&2; exit 2; }
    valid_map_name "$1" || { echo "invalid map name" >&2; exit 2; }
    ensure_active_lineage
    compose exec -T slam bash -lc \
      "source /opt/ros/humble/setup.bash; ros2 service call /slam_toolbox/serialize_map slam_toolbox/srv/SerializePoseGraph \"{filename: '/data/maps/$1'}\"; ros2 service call /slam_toolbox/save_map slam_toolbox/srv/SaveMap \"{name: {data: '/data/maps/$1'}}\""
    install -m 600 "$(active_lineage_path)" "$(lineage_root)/$1.lineage.json"
    ;;
  load)
    [[ $# -eq 1 ]] || { echo "load requires one map name" >&2; exit 2; }
    valid_map_name "$1" || { echo "invalid map name" >&2; exit 2; }
    lineage="$(lineage_root)/$1.lineage.json"
    validate_lineage "$lineage"
    stop_leash
    install -m 600 "$lineage" "$(active_lineage_path)"
    require_trusted_clock "$clock_reference_epoch"
    compose up -d --force-recreate --no-build slam
    compose exec -T slam bash -lc \
      "source /opt/ros/humble/setup.bash; ros2 service call /slam_toolbox/deserialize_map slam_toolbox/srv/DeserializePoseGraph \"{filename: '/data/maps/$1', match_type: 1, initial_pose: {x: 0.0, y: 0.0, theta: 0.0}}\""
    stop_leash
    ;;
  *)
    echo "unknown command: $command" >&2
    usage >&2
    exit 2
    ;;
esac
