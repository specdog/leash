#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

cargo test --quiet adapter::tests

for contract in "trait MobileBaseAdapter" "trait GimbalAdapter" "trait CameraAdapter" "impl MobileBaseAdapter for WaveshareUgvDriver" "impl GimbalAdapter for WaveshareUgvDriver"; do
  if ! grep -R -q -- "$contract" src implementations/waveshare-ugv; then
    echo "missing adapter contract proof: $contract" >&2
    exit 1
  fi
done

sensor_script="implementations/waveshare-ugv/sensor-soak.sh"
bash -n "$sensor_script"
sensor_help="$(bash "$sensor_script" --help)"
for option in "--duration-secs" "--max-sensor-age-ms" "--max-rss-growth-mb" "--output"; do
  if ! grep -Fq -- "$option" <<<"$sensor_help"; then
    echo "sensor soak help missing: $option" >&2
    exit 1
  fi
done

for fixture in "examples/waveshare-ugv/sensor-fixture.json" "examples/replay/waveshare-ugv-sensors.jsonl"; do
  [[ -s "$fixture" ]] || {
    echo "missing Waveshare sensor fixture: $fixture" >&2
    exit 1
  }
done

for heading in "## No-hardware proof" "## Bench preflight" "## Gimbal and camera" "## Telemetry and soak" "## Evidence and sign-off"; do
  if ! grep -Fq -- "$heading" docs/ADAPTER_SMOKE_TEMPLATE.md; then
    echo "adapter smoke template missing: $heading" >&2
    exit 1
  fi
done

baseline_script="implementations/waveshare-ugv/deployment-baseline.sh"
bash -n "$baseline_script"
baseline_help="$(bash "$baseline_script" --help)"
for command in "capture" "verify" "rollback" "--source-revision" "--build-features" "--confirm"; do
  if ! grep -Fq -- "$command" <<<"$baseline_help"; then
    echo "deployment baseline help missing: $command" >&2
    exit 1
  fi
done

for heading in "## Deployment baseline and rollback" "## USB bring-up without committed identity" "## LD06 lidar and base IMU" "### Stationary proof" "calibration/"; do
  if ! grep -Fq -- "$heading" implementations/waveshare-ugv/README.md; then
    echo "UGV implementation guide missing: $heading" >&2
    exit 1
  fi
done

implementations/waveshare-ugv/ros2/verify.sh
implementations/waveshare-ugv/calibration/verify.sh
implementations/waveshare-ugv/navigation/verify.sh

if grep -R -E -q -- '(^|[^0-9])10\.[0-9]+\.[0-9]+\.[0-9]+([^0-9]|$)|(^|[^0-9])192\.168\.[0-9]+\.[0-9]+([^0-9]|$)' docs/ADAPTERS.md docs/ADAPTER_SMOKE_TEMPLATE.md implementations/waveshare-ugv; then
  echo "adapter docs contain a private address" >&2
  exit 1
fi

printf '{"ok":true,"contracts":3,"waveshare_traits":2,"template_sections":5,"deployment_baseline":true,"sensor_soak":true,"sensor_fixtures":2,"ros2_slam_adapter":true,"ugv_calibration":true,"ugv_physical_navigation":true}\n'
