---
name: verify
summary: Drive Leash CLI and HTTP surfaces for runtime verification.
---

# Verify Leash at its public surface

1. Build the binary needed for the changed surface, for example `cargo build --all-features --bin leash`.
2. Inspect launch flags with `target/debug/leash <command> --help`.
3. For HTTP changes, launch an isolated server on an unused loopback port, for example:
   `target/debug/leash serve http --profile sim --listen 127.0.0.1:38081`
4. Drive the changed route with `curl`, capturing response bodies and status codes. Include a malformed body or wrong-method probe.
5. Stop the server after capture.

For calibration analyzer changes, create scrubbed JSONL fixtures in `/tmp`, then drive the public CLI directly:
`python3 implementations/waveshare-ugv/calibration/analyze.py --profile PROFILE --output RESULT CAPTURE...`.
Capture both a valid result and an invalid capture's non-zero exit status.

For acceptance changes, drive `acceptance.py build`, then run `validate` and
`digest` against the emitted file. Probe an unreviewed body artifact and inspect
the default versus watchdog-proven readiness fields.

Physical Waveshare flows require a real serial device. macOS pseudo-terminals fail the `serialport` open path with `Not a typewriter`, so do not claim the physical happy path was exercised when only a PTY was available.
