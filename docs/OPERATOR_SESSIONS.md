# Operator Session Recording And Replay

Leash Operator can capture a small, portable record of fleet summaries, safe
operator ownership metadata, joystick commands, camera failures, camera
recovery requests, and frame health. The same file can drive the GUI in an
offline debug mode without contacting a robot.

## Record

1. Start Leash Operator normally and open `http://localhost:8787/?debug=1`.
2. Select **Record** before reproducing the control or camera problem.
3. Select **Stop & Export** to download an
   `leash-operator-session-<timestamp>.json` file.

Recording does not alter drive, e-stop, token, or physical-actuation behavior.
The browser captures responses only after the existing request paths succeed.

## Replay Offline

1. Open `http://localhost:8787/?debug=1` and select **Load**.
2. Choose an operator-session JSON file.
3. Scrub the timeline or select **Play**.

Loading a recording replaces the visible fleet with recorded robots, closes
live camera connections, stops live polling, and disables all actuation
controls. Summary and telemetry events update health, battery, motion, drive,
and operator ownership. Camera failure, recovery, and frame-health events update
the camera state and recent event log. Recorded sessions do not contain frames;
the camera pane reports the recorded stream state.

## File Contract And Privacy

The versioned format is `leash-operator-session-v1`. Its Rust types are
`OperatorSessionRecording`, `OperatorSessionRobot`, and `OperatorSessionEvent`;
their generated schemas live in `schemas/`.

Robot base URLs and raw operator tokens are excluded. The recorder rejects
event objects containing exact sensitive keys such as `token`, `password`,
`secret`, `credential`, or `base_url`. Hashed `owner_id`, remaining ownership
TTL, and speed mode are safe to retain. Fleet and robot display names are kept
so a local operator can identify a recording; review those labels before
sharing a file outside the fleet team.

The committed fixture at `examples/replay/operator-session.json` uses only
generic names and offline data. Validate the complete path with:

```bash
bash scripts/smoke-operator-session.sh
```
