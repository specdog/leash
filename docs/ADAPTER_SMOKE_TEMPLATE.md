# Adapter smoke checklist: `<adapter-name>`

Copy this file into a private validation record. Replace placeholders without
committing fleet addresses, credentials, serial numbers, or private bot names.

## Build identity

- Adapter/profile: `<adapter/profile>`
- Commit: `<git-sha>`
- Host image: `<image-version>`
- Operator: `<name-or-team>`
- Date/time: `<ISO-8601>`

## No-hardware proof

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `cargo test --all-features`
- [ ] `cargo run --features mcp --bin leash-schema -- --check`
- [ ] `cargo package --locked`
- [ ] `bash scripts/smoke-all.sh`
- [ ] Physical profile refuses startup without the actuation gate.

## Bench preflight

- [ ] Wheels/actuators lifted or otherwise made safe.
- [ ] Correct adapter feature, profile, device, and baud/transport selected.
- [ ] Adapter metadata lists `physical-actuation` and token/approval policy.
- [ ] Drive signs, left/right ordering, swap, and inversion verified at low speed.
- [ ] Stop writes zero commands and deadman stops an abandoned command.
- [ ] E-stop is reachable, latches immediately, cancels patrol/planner motion,
      and requires the approved reset flow.
- [ ] A new operator token stops and invalidates the previous owner.

## Gimbal and camera

- [ ] Unsupported gimbal fails closed; supported pan/tilt limits are verified.
- [ ] Snapshot and stream share the camera ownership guard without contention.
- [ ] Snapshot returns `image/jpeg` and stream content type matches its boundary.
- [ ] Stream recovery releases the device and increments recovery health state.
- [ ] Operator video mode does not silently fall back when strict WebRTC is set.

## Telemetry and soak

- [ ] Battery, odometry, command, safety, worker, and motion fields are present.
- [ ] Health history and camera failures remain visible after recovery.
- [ ] Public payloads contain no bearer token, process command, environment,
      credential, or deployment-only path.
- [ ] Sustained low-speed drive/stop/camera test completed for `<duration>`.
- [ ] CPU, memory, temperature, reconnect, and error counts stayed acceptable.

## Evidence and sign-off

| Proof | Result | Artifact or note |
|---|---|---|
| CI run | `<pass/fail>` | `<link-or-id>` |
| No-hardware smoke | `<pass/fail>` | `<artifact>` |
| Bench stop/e-stop | `<pass/fail>` | `<artifact>` |
| Camera/recovery | `<pass/fail>` | `<artifact>` |
| Telemetry/soak | `<pass/fail>` | `<artifact>` |

- [ ] Adapter owner approved.
- [ ] Safety reviewer approved.
- [ ] Fleet configuration remains private.
