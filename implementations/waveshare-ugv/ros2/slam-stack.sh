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
  save NAME         Save occupancy output and serialized pose graph under /data/maps.
  load NAME         Load a serialized pose graph from /data/maps for localization.

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
  docker compose --env-file "$env_file" -f "$compose_file" "$@"
}

stop_leash() {
  curl -fsS -X POST "$leash_url/stop" >/dev/null
}

valid_map_name() {
  [[ "$1" =~ ^[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}$ ]]
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
    compose up -d --build
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
  save)
    [[ $# -eq 1 ]] || { echo "save requires one map name" >&2; exit 2; }
    valid_map_name "$1" || { echo "invalid map name" >&2; exit 2; }
    compose exec -T slam bash -lc \
      "source /opt/ros/humble/setup.bash; ros2 service call /slam_toolbox/serialize_map slam_toolbox/srv/SerializePoseGraph \"{filename: '/data/maps/$1'}\"; ros2 service call /slam_toolbox/save_map slam_toolbox/srv/SaveMap \"{name: {data: '/data/maps/$1'}}\""
    ;;
  load)
    [[ $# -eq 1 ]] || { echo "load requires one map name" >&2; exit 2; }
    valid_map_name "$1" || { echo "invalid map name" >&2; exit 2; }
    stop_leash
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
