#!/usr/bin/env bash

# Require either kernel-reported NTP synchronization or a fresh epoch supplied
# by a trusted operator machine. Callers may include CLOCK_PROOF_* in private
# proof output; no host identity or network address is recorded.
require_trusted_clock() {
  local reference_epoch="${1:-}"
  local now_epoch delta

  if [[ "$(timedatectl show -p NTPSynchronized --value 2>/dev/null)" == "yes" ]]; then
    CLOCK_PROOF_SOURCE="ntp"
    CLOCK_PROOF_SKEW_SECS=0
    return 0
  fi

  [[ "$reference_epoch" =~ ^[0-9]{10,}$ ]] || {
    echo "target clock is not NTP-synchronized; pass a current --clock-reference-epoch from a trusted operator machine" >&2
    return 1
  }
  now_epoch="$(date +%s)"
  delta=$((now_epoch - reference_epoch))
  (( delta < 0 )) && delta=$((-delta))
  (( delta <= 5 )) || {
    echo "target clock differs from the trusted reference by ${delta}s (maximum: 5s)" >&2
    return 1
  }

  CLOCK_PROOF_SOURCE="trusted-reference"
  CLOCK_PROOF_SKEW_SECS="$delta"
}
