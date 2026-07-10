#!/usr/bin/env bash
set -euo pipefail
export PYTHONDONTWRITEBYTECODE=1

calibration_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

bash -n \
  "$calibration_dir/capture.sh" \
  "$calibration_dir/map-reload-proof.sh"

python3 "$calibration_dir/profile.py" "$calibration_dir/pinkie-v1.json" validate >/dev/null
PYTHONPATH="$calibration_dir" python3 -m unittest discover \
  -s "$calibration_dir/test" -p 'test_*.py'

for script in "$calibration_dir/capture.sh" "$calibration_dir/map-reload-proof.sh"; do
  for forbidden in '/drive' '/motors' 'cmd_vel' '/patrol' '/navigation/goals'; do
    if grep -Fq -- "$forbidden" "$script"; then
      echo "calibration recorder contains a forbidden actuation path: $forbidden" >&2
      exit 1
    fi
  done
done

grep -Fq -- '--replay-output' "$calibration_dir/capture.sh"
grep -Fq -- 'leash-replay-v1' "$calibration_dir/replay.py"
jq -r 'select(.kind == "telemetry") | "data: " + (.data | tojson)' \
  "$calibration_dir/../../../examples/replay/sim-mapping.jsonl" \
  | python3 "$calibration_dir/replay.py" /dev/stdin - \
  | cargo run --quiet -- replay /dev/stdin --speed 100 >/dev/null

stop_post_count="$(grep -hE 'curl .* -X POST .*\$leash_url/stop' "$calibration_dir"/*.sh | wc -l | tr -d ' ')"
[[ "$stop_post_count" -ge 2 ]]
printf '{"ok":true,"profile":"unmeasured","offline_analysis":true,"recorder_issues_motion":false}\n'
