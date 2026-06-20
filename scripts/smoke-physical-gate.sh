#!/usr/bin/env bash
set -euo pipefail

check_refuses_without_gate() {
  local label="$1"
  shift

  set +e
  output="$("$@" 2>&1)"
  status=$?
  set -e

  if [[ "$status" -eq 0 ]]; then
    echo "expected $label physical profile to fail without explicit actuation gate" >&2
    exit 1
  fi

  if [[ "$output" != *"LEASH_ALLOW_PHYSICAL_ACTUATION"* ]]; then
    echo "expected $label error to mention LEASH_ALLOW_PHYSICAL_ACTUATION" >&2
    echo "$output" >&2
    exit 1
  fi
}

check_refuses_without_gate "waveshare-ugv" \
  cargo run --quiet --features waveshare-ugv -- serve http --profile waveshare-ugv --listen 127.0.0.1:18081

check_refuses_without_gate "mavlink-drone" \
  cargo run --quiet --features mavlink-drone -- serve http --profile mavlink-drone --listen 127.0.0.1:18082 --mavlink-endpoint udp://127.0.0.1:14550

check_refuses_without_gate "manipulator" \
  cargo run --quiet --features manipulator -- serve http --profile manipulator --listen 127.0.0.1:18083

echo "physical gate smoke ok"
