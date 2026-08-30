# Bounded Navigation HTTP API

Leash exposes a generic goal-level HTTP surface for clients that need durable
mission orchestration without owning a motor refresh loop. The physical
navigation compile/runtime gates, an existing pilot lease, explicit approval,
fresh localization and range data, CPU collision checks, deadman, stop, and
E-stop remain authoritative.

## Routes

| Method | Route | Purpose |
| --- | --- | --- |
| `POST` | `/navigation/goals` | Submit an idempotent bounded planner goal |
| `GET` | `/navigation/status?mission_id=...` | Reconcile current planner state |
| `POST` | `/navigation/goals/:mission_id/cancel` | Cancel the goal and command zero output |
| `POST` | `/motors/stop/verified` | Command and confirm zero output |

The caller must first create a short pilot lease with `POST /pilot/authorize`.
Goal submission never creates or extends that lease.

```json
{
  "schema_version": "leash.navigation-goal.v1",
  "mission_id": "bounded-room-pass-1",
  "idempotency_key": "bounded-room-pass-1:plan-1",
  "token": "runtime-supplied-token",
  "approval": true,
  "frame_id": "odom",
  "x_m": 0.8,
  "y_m": 0.2,
  "tolerance_m": 0.2,
  "speed_mode": "low",
  "deadline_ms": 1788086400000
}
```

Identifiers are URL-safe and limited to 160 characters. The deadline must be
in the future and no more than 120 seconds away. Only `low` speed is accepted.
Repeating an identical request with the same idempotency key returns current
planner state; changing any field under that key returns `409 Conflict`.

Status uses `leash.navigation-status.v1` and includes `ok`, `active`, `status`,
`message`, and the accepted goal. A client must treat any inactive state other
than `reached` as a denial or failure and issue a verified stop. Client
disconnect never grants continued authority: Leash independently stops on
lease expiry, deadman, stale providers, collision, or any other safety gate.
