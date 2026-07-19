# Applied-action evidence

Leash exposes the post-safety differential-drive command as completed,
append-only intervals. This is the authoritative supervision surface for
Qualia and offline world-model datasets; requested UI or planner commands are
not substitutes.

Read the bounded history with:

```bash
curl -sS 'http://robot:8000/evidence/action/applied?after_sequence=0&limit=256'
```

`GET /action-evidence` is an equivalent alias. The response schema is
`leash.applied-action-page.v1`; every entry uses
`qualia.applied-action.v1` and contains:

- one boot-scoped `producer_epoch` and a contiguous `action_sequence`;
- a non-empty `[interval_start_ns, interval_end_ns)`;
- requested, safety-clamped, and adapter-accepted wheel values;
- the active speed cap, safety flags, validity, arming, deadman, and collision
  provenance;
- `authority: "leash"`.

Leash seals the current interval every 100 ms even when stopped. This creates
continuous zero-motion evidence without issuing additional motor commands.
Every accepted drive, stop, collision stop, deadman stop, or e-stop first
closes the previous interval and then starts the new post-safety state.

The in-memory history retains 4,096 entries. A nonzero cursor older than the
retained range fails with an explicit overrun error; consumers must stop rather
than skip evidence. A new consumer may use `after_sequence=0` to begin at the
oldest retained entry.

Reading evidence has no motor, planner, authorization, or safety authority.
Leash remains the sole writer to the physical adapter.
