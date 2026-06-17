#!/usr/bin/env bash
set -euo pipefail

set +e
output="$(cargo run --quiet --features waveshare-ugv -- serve http --profile waveshare-ugv --listen 127.0.0.1:18081 2>&1)"
status=$?
set -e

if [[ "$status" -eq 0 ]]; then
  echo "expected physical profile to fail without explicit actuation gate" >&2
  exit 1
fi

if [[ "$output" != *"LEASH_ALLOW_PHYSICAL_ACTUATION"* ]]; then
  echo "expected error to mention LEASH_ALLOW_PHYSICAL_ACTUATION" >&2
  echo "$output" >&2
  exit 1
fi

echo "physical gate smoke ok"

