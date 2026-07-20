#!/usr/bin/env bash
set -euo pipefail

navigation_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

bash -n "$navigation_dir/field-proof.sh"
PYTHONDONTWRITEBYTECODE=1 PYTHONPATH="$navigation_dir" python3 -m unittest discover \
  -s "$navigation_dir/test" -p 'test_*.py'

for required in \
  '--operator-confirmed' \
  '127\.0\.0\.1|localhost' \
  'planner/cancel' \
  'patrol/stop' \
  'physical_navigation_enabled' \
  'covariance' \
  'resource' \
  'replay.jsonl' \
  'final_motor_stop'; do
  grep -Eq -- "$required" "$navigation_dir/field-proof.sh" || {
    echo "field proof is missing required safety evidence: $required" >&2
    exit 1
  }
done

printf '{"ok":true,"suite":"waveshare-ugv-physical-navigation-no-hardware"}\n'
