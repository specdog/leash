#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

cargo test --quiet adapter::tests

for contract in "trait MobileBaseAdapter" "trait GimbalAdapter" "trait CameraAdapter" "impl MobileBaseAdapter for WaveshareUgvDriver" "impl GimbalAdapter for WaveshareUgvDriver"; do
  if ! grep -R -q -- "$contract" src; then
    echo "missing adapter contract proof: $contract" >&2
    exit 1
  fi
done

for heading in "## No-hardware proof" "## Bench preflight" "## Gimbal and camera" "## Telemetry and soak" "## Evidence and sign-off"; do
  if ! grep -Fq -- "$heading" docs/ADAPTER_SMOKE_TEMPLATE.md; then
    echo "adapter smoke template missing: $heading" >&2
    exit 1
  fi
done

if grep -E -q -- '(^|[^0-9])10\.[0-9]+\.[0-9]+\.[0-9]+([^0-9]|$)|(^|[^0-9])192\.168\.[0-9]+\.[0-9]+([^0-9]|$)' docs/ADAPTERS.md docs/ADAPTER_SMOKE_TEMPLATE.md; then
  echo "adapter docs contain a private address" >&2
  exit 1
fi

printf '{"ok":true,"contracts":3,"waveshare_traits":2,"template_sections":5}\n'
