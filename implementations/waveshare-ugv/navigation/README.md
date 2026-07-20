# Pinkie supervised physical-navigation proof

This directory is the concrete Waveshare UGV implementation proof for Leash
issue #168. It is not a second navigation library. Leash continues to own the
generic goal, patrol, policy, deadman, obstacle, soft-distance, stop, E-stop,
telemetry, and replay contracts; these scripts only collect Pinkie's private
field evidence.

Do not run a motion phase until the read-only SLAM proof and measured calibration
have passed. A changed SSH host key, unmeasured calibration, stale localization,
missing resource sampling, or an unclear floor is a stop condition.

## Runtime gate

The Pinkie HTTP service must be built with `waveshare-ugv,physical-navigation`
and started with all of these runtime gates:

```text
--allow-physical-actuation
--allow-physical-navigation
--no-untokened-drive
--policy-mode require-approval
--resource-sampling
```

The normal Leash HTTP transport provides `POST /planner/goal`,
`GET /planner/status`, `POST /planner/cancel`, and the saved patrol-zone routes.
Every physical start still requires a live token and `approval=true`; HTTP does
not bypass the capability guard. Keep the service and proof script on loopback.

Create a private token file without putting the token in shell history:

```bash
install -m 600 /dev/null ~/.config/leash/pinkie-navigation-token
${EDITOR:-vi} ~/.config/leash/pinkie-navigation-token
```

## Acceptance runs

Each command refuses to run without `--operator-confirmed`, which means a clear
floor, present spotter, and reachable physical E-stop. It sends stop before and
after, enforces the low-speed/covariance gates, captures telemetry and resources,
builds a scrubbed `leash-replay-v1` file, validates that replay through the normal
Leash binary, and requires a final zero-speed frame.

First run one target 0.45 through 0.55 m from the live preflight pose:

```bash
implementations/waveshare-ugv/navigation/field-proof.sh \
  --phase half-meter --run-id half-meter-1 \
  --goal-x X --goal-y Y --tolerance-m 0.15 \
  --token-file ~/.config/leash/pinkie-navigation-token \
  --output-dir ~/.local/state/leash/navigation/half-meter-1 \
  --operator-confirmed
```

Then run three consecutive map goals, using unique run IDs and coordinates:

```bash
implementations/waveshare-ugv/navigation/field-proof.sh \
  --phase map-goal --run-id map-goal-1 \
  --goal-x X --goal-y Y --tolerance-m 0.15 \
  --token-file ~/.config/leash/pinkie-navigation-token \
  --output-dir ~/.local/state/leash/navigation/map-goal-1 \
  --operator-confirmed
```

Finally run one saved zone with a polygon boundary and map-scoped waypoints. The
runtime visits each waypoint once and stops after the terminal waypoint:

```bash
implementations/waveshare-ugv/navigation/field-proof.sh \
  --phase patrol --run-id bounded-patrol-1 --zone-id safe-zone \
  --token-file ~/.config/leash/pinkie-navigation-token \
  --output-dir ~/.local/state/leash/navigation/bounded-patrol-1 \
  --operator-confirmed
```

Validate the complete private evidence set:

```bash
python3 implementations/waveshare-ugv/navigation/verify.py \
  ~/.local/state/leash/navigation/half-meter-1/summary.json \
  ~/.local/state/leash/navigation/map-goal-{1,2,3}/summary.json \
  ~/.local/state/leash/navigation/bounded-patrol-1/summary.json
```

Keep the captures private. Link only scrubbed summaries and hashes to the issue.
No browser map or web RViz surface is required by this workflow.

## No-hardware gate

```bash
implementations/waveshare-ugv/navigation/verify.sh
```
