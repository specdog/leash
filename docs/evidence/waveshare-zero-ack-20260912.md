# Zero-drive acknowledgement channel repair

The operator's integration on 2026-09-12 returned HTTP 200 from authorization,
then HTTP 400 from a low-speed zero drive with
`runtime v2 Waveshare acknowledgement timed out`. Verified stop returned
`acknowledged: true`, adapter sample sequence 30763, and an inactive lease.

Read-only inspection of the running service's `GET /runtime-v2/status` found
`connected: true`, zero disconnects and write failures, 6290 writes, zero
supervisor `drives_submitted` and `acknowledgements`, two stop requests, and a
`last_stop_receipt` with `verified_zero: true`. These counters describe the
observed instance, not an acknowledgement for a new command.

## Cause and repair

At source `ef467b17df3dce7019939fdc9baf828f4a00af7a`, the CPU supervisor routes
every authorized zero `Actuate` through `request_safety`. It publishes
`SafetyRequested` with the command ID and safety request sequence. The controller
publishes the resulting cumulative safety receipt. The supervisor records that
receipt in its durable evidence, but does not emit the normal `Acknowledged`
event for it. The adapter waited only for `Acknowledged` and therefore timed out
even when the zero write succeeded.

The adapter now accepts either the existing matching command acknowledgement or
the matching command's `SafetyRequested` event followed by a receipt of that
safety kind whose request interval covers the exact request sequence and whose
`verified_zero` is true. Both paths retain the original 500 ms deadline. It does
not synthesize a normal drive acknowledgement or substitute link health for a
receipt. Verified-stop, lease revocation, and deadman behavior are unchanged.

Nonzero `Actuate` still goes through `WaveshareActuationPort::submit_drive`, the
single serial owner, its command ticket, and the supervisor's matching
`Acknowledged` event. Applied remains the existing controller write/flush
acknowledgement; it is not measured wheel motion. The event reader remains a
latest-value slot, so an overwritten event can still cause a fail-closed timeout.
This inspection does not establish successful nonzero motion or resolve any
physical condition reported by the operator.

The user's uncommitted root `src/runtime.rs` patch was read without modification.
It promotes wheel odometry into a localization provider and does not repair the
acknowledgement channel mismatch. This fix stays in the isolated worktree.

No hardware commands or test cycles were run for the diagnosis above.

## Installed zero-only integration

After deployment of source `a38bf1e071c76ac5951d6fd2e1a8a446639f1162`, one
explicitly authorized zero-only integration ran at 2026-09-12 21:09:00 EDT
(2026-09-13 01:09:00 UTC). The installed binary hash matched
`d58117de7f61811e567cd273ea8febecfa4495cb62e844b7833bf9903e9291cd`.

- Authorization: HTTP 200, `ttl_secs: 5`, `speed_mode: low`.
- Drive: requested left/right 0/0; HTTP 200 in 19.392 ms, `ok: true`, returned
  left/right 0/0, `max_speed: 0.22`, `soft_odometry_limited: false`.
- Finally, verified stop with `reason: operator-request`: HTTP 200 in 87.554 ms,
  `acknowledged: true`, command sequence 2, adapter sample sequence 2374.
  Write completion was Unix ms 1789261740514; the confirming adapter sample was
  received at 1789261740583. The operator lease was inactive afterward.
- Both applied-action endpoints returned HTTP 200 with producer epoch
  1789261501921313288. Entry 2360 records requested/clamped/applied left/right
  0/0, speed scale 0.22, `valid: true`, `armed: true`, safety flags 0. Entry 2361
  records 0/0 at the same scale, `armed: false`, safety flags 16 (verified zero).
- The controller's verified-zero stop receipt covers safety requests 1 through
  2, with applied sequence 482. Disconnects, write failures, and supervisor faults
  remained zero.

The sanitized full receipt is on the host at
`C:/Users/ericm/.local/state/qualia/console-runtime/zero-drive-integration.json`
and on the robot at
`/home/jetson/.local/state/leash/zero-drive-integration/1789261740393.json`.
The bearer was read only into the robot helper's memory and excluded from both
receipts. No nonzero or head commands were issued. This proves removal of the
zero-drive acknowledgement mismatch; it does not demonstrate physical wheel
motion or deadman expiry.
