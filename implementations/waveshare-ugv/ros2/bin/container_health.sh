#!/usr/bin/env bash
set -euo pipefail

set +u
source /opt/ros/humble/setup.bash
set -u
nodes="$(ros2 node list 2>/dev/null)"
grep -qx '/leash_ros_bridge' <<<"$nodes"
grep -qx '/ekf_filter_node' <<<"$nodes"
grep -qx '/slam_toolbox' <<<"$nodes"

python3 - <<'PY'
import json
import os
import urllib.request

url = os.environ.get("LEASH_URL", "http://host.docker.internal:8000").rstrip("/")
with urllib.request.urlopen(f"{url}/health", timeout=2.0) as response:
    health = json.load(response)
if not health.get("ok"):
    raise SystemExit("Leash health is degraded")
PY
