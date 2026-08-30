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
| `GET` | `/mapping/status` | Read the provider-owned fixed-map lineage and health |
| `POST` | `/mapping/lifecycle` | Versioned lifecycle contract; currently returns `501 Not Implemented` |

The caller must first create a short pilot lease with `POST /pilot/authorize`.
Goal submission never creates or extends that lease.

```json
{
  "schema_version": "leash.navigation-goal.v2",
  "mission_id": "bounded-room-pass-1",
  "idempotency_key": "bounded-room-pass-1:plan-1",
  "token": "runtime-supplied-token",
  "approval": true,
  "expected_map": {
    "map_id": "bounded-room",
    "map_revision": "2026-08-30-a",
    "frame_id": "map"
  },
  "frame_id": "map",
  "x_m": 0.8,
  "y_m": 0.2,
  "tolerance_m": 0.2,
  "speed_mode": "low",
  "deadline_ms": 1788086400000
}
```

Identifiers are URL-safe and limited to 160 characters. The deadline must be
in the future and no more than 120 seconds away. Only `low` speed is accepted.
Leash accepts one active HTTP navigation mission at a time and independently
cancels it at the submitted deadline. Repeating the same goal fields with the
same idempotency key returns that mission's stored planner state; changing a
goal field under that key returns `409 Conflict`. Authorization tokens are
used for submission but are not retained in the idempotency registry.

`expected_map` must be copied exactly from `/mapping/status.active_map`. A
lineage change while a mission is active cancels that mission. `grid_revision`
is reported by mapping status but is not pinned by the goal because occupancy
may update without replacing the underlying saved map.

Status uses `leash.navigation-status.v2` and includes the expected and active
map identities, deadline, readiness gates, explicit terminal reason, accepted
goal, path, and available executor feedback. A client must treat any inactive
state other than terminal reason `reached` as a denial or failure and issue a
verified stop. Client disconnect never grants continued authority: Leash
independently stops on lease expiry, deadman, stale providers, collision, map
replacement, or any other safety gate.

## Mapping boundary

`GET /mapping/status` returns `leash.mapping-status.v1` with provider state,
`active_map`, `grid_revision`, provider identity, freshness, and
`lifecycle_control_supported`. Leash does not currently own the ROS service or
process boundary needed to start, stop, save, or load SLAM safely. Therefore
`POST /mapping/lifecycle` accepts the versioned `leash.mapping-lifecycle.v1`
contract but returns `501 Not Implemented` and
`lifecycle_control_supported=false`; it never shells out to host scripts.

## Physical executor boundary

Simulation retains its deterministic grid planner. Physical navigation no
longer synthesizes a two-pose path and drives toward it directly. A physical
runtime must inject a `NavigationExecutor` that owns Nav2 goal, cancel, path,
and feedback exchange while Nav2 velocity remains a proposal to Leash's CPU
safety supervisor and single motor owner. The default physical executor reports
`executor=unsupported` and rejects goals. The existing ROS boundary does not
yet provide an outbound Nav2 action client, so enabling physical compile and
runtime flags alone does not claim goal readiness.
