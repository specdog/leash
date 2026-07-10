#!/usr/bin/env bash
set -euo pipefail
export PYTHONDONTWRITEBYTECODE=1

ros_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$ros_dir/../../.." && pwd)"
cd "$repo_root"

dockerfile="$ros_dir/Dockerfile"
compose="$ros_dir/compose.yaml"
bridge="$ros_dir/leash_waveshare_slam/bridge.py"

grep -Eq '^FROM ros:humble-ros-base-jammy@sha256:[0-9a-f]{64}$' "$dockerfile"
[[ "$(wc -l < "$ros_dir/packages.lock" | tr -d ' ')" -ge 300 ]]
grep -Fq 'done < /tmp/leash-ros-packages.lock' "$dockerfile"
for package in robot-localization slam-toolbox tf2-ros ros2launch launch-ros nav-msgs sensor-msgs geometry-msgs; do
  grep -Eq "ros-humble-${package}=\\\$\\{[A-Z0-9_]+_VERSION\\}" "$dockerfile"
done

for unsafe in 'privileged:' 'devices:' 'network_mode:.*host' '/dev/' 'pid:.*host'; do
  if grep -Eq "$unsafe" "$compose"; then
    echo "ROS compose grants forbidden host/device access: $unsafe" >&2
    exit 1
  fi
done
for required in 'read_only: true' 'cap_drop:' 'no-new-privileges:true' 'LEASH_ROS_READ_ONLY: "1"'; do
  grep -Fq -- "$required" "$compose"
done

for forbidden in '/drive' '/motors' '/stop' 'cmd_vel' 'geometry_msgs.msg import Twist'; do
  if grep -R -Fq -- "$forbidden" "$ros_dir/leash_waveshare_slam" "$ros_dir/launch"; then
    echo "ROS adapter contains a forbidden motor path: $forbidden" >&2
    exit 1
  fi
done

python3 - "$bridge" "$ros_dir/leash_waveshare_slam/contract.py" "$ros_dir/launch/bringup.launch.py" <<'PY'
import ast
import pathlib
import sys

for path in sys.argv[1:]:
    ast.parse(pathlib.Path(path).read_text(encoding="utf-8"), filename=path)
PY
PYTHONPATH="$ros_dir" python3 -m unittest discover -s "$ros_dir/test" -p 'test_*.py'
python3 - "$ros_dir/package.xml" <<'PY'
import sys
import xml.etree.ElementTree as ET

root = ET.parse(sys.argv[1]).getroot()
if root.findtext("name") != "leash_waveshare_slam":
    raise SystemExit("unexpected ROS package name")
dependencies = {node.text for node in root.findall("exec_depend")}
required = {"robot_localization", "slam_toolbox", "rclpy", "tf2_ros"}
if not required.issubset(dependencies):
    raise SystemExit(f"missing ROS dependencies: {sorted(required - dependencies)}")
PY

if command -v ruby >/dev/null 2>&1; then
  ruby -e 'require "yaml"; ARGV.each { |path| YAML.safe_load(File.read(path), aliases: true) }' \
    "$compose" "$ros_dir/config/ekf.yaml" "$ros_dir/config/slam_toolbox.yaml"
fi

bash -n \
  "$ros_dir/bin/container_health.sh" \
  "$ros_dir/clock-gate.sh" \
  "$ros_dir/slam-stack.sh" \
  "$ros_dir/ros-soak.sh"

grep -Fq -- '--clock-reference-epoch' "$ros_dir/slam-stack.sh"
grep -Fq -- '--clock-reference-epoch' "$ros_dir/ros-soak.sh"
grep -Fq -- 'delta <= 5' "$ros_dir/clock-gate.sh"
grep -Fq -- 'compose up -d --no-build' "$ros_dir/slam-stack.sh"
if grep -Fq -- 'compose up -d --build' "$ros_dir/slam-stack.sh"; then
  echo "SLAM start must use the prebuilt pinned image without an online rebuild" >&2
  exit 1
fi

printf '{"ok":true,"ros_distro":"humble","bridge_fixture":true,"motor_access":false,"device_access":false}\n'
