# Saved navigation and motion events

Leash persists saved waypoints and patrol zones in a versioned
`leash-navigation-v1` JSON store next to the per-run spatial-memory file. A
waypoint has a stable ID, display name, frame, position, tolerance, and created
and updated timestamps. A patrol zone groups ordered waypoint IDs with an
optional polygon boundary of at least three points.

The capability registry exposes:

- `waypoint_create`, `waypoint_list`, `waypoint_update`, `waypoint_delete`
- `patrol_zone_create`, `patrol_zone_list`, `patrol_zone_update`, `patrol_zone_delete`
- `start_patrol_zone`, `stop_patrol`, `patrol_status`

Deleting a waypoint that a zone references is rejected. IDs accept only ASCII
letters, numbers, `-`, and `_`; coordinates must be finite; tolerances must be
positive. Zone creation and updates reject missing or duplicate waypoint IDs.

```bash
leash mcp call invoke_capability --json \
  '{"capability":"waypoint_create","id":"entry","name":"Entry","x_m":0.25,"y_m":0.0}'
leash mcp call invoke_capability --json \
  '{"capability":"patrol_zone_create","id":"front","name":"Front","waypoint_ids":["entry"],"boundary":[{"x_m":0.0,"y_m":0.0},{"x_m":0.5,"y_m":0.0},{"x_m":0.5,"y_m":0.5}]}'
leash mcp call invoke_capability --json \
  '{"capability":"start_patrol_zone","zone_id":"front","speed_mode":"low"}'
```

Simulation uses the local planner. Replay selects the ordered zone path and
reports active status without issuing drive commands. Physical mobile-base
profiles reject zone starts by default; the independent compile/runtime gate,
freshness requirements, policy lease, and reusable checklist are in
[`PHYSICAL_NAVIGATION.md`](PHYSICAL_NAVIGATION.md). A latched e-stop always
rejects a new start. `POST /patrol/stop` and e-stop cancel the active patrol
before any later control action.

The HTTP surface provides `GET /waypoints`, `GET /patrol/zones`,
`POST /patrol/zones/:zone_id/start`, `GET /patrol/status`, and
`POST /patrol/stop`. The operator shows every configured zone, its waypoint
count, start/stop controls, and the current zone status.

External motion workers use the same `leash-worker-frame-v1` envelope as
perception workers. `motion` input payloads contain an image observation;
`motion-events` outputs contain passive `MotionEvent` records and have no
control fields. `TelemetryFrame.motion_events` carries these observations to
HTTP, stream, operator, recording, and replay consumers. The checked-in
`examples/workers/sim-motion-output.json` fixture demonstrates the wire shape.
