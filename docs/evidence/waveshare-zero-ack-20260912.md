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

No hardware commands or test cycles were run for this diagnosis. Deployment and
the next bounded integration belong to the operator's coordinating agent.
