# Bridge Compatibility

Leash can stand in as a simulated or hardware-backed robot runtime for an
existing bridge/client that already speaks the harness HTTP/WebSocket contract.

```mermaid
flowchart LR
  bridge["Existing bridge or client"] --> routes["Bridge-compatible HTTP routes"]
  routes --> leash["Leash HTTP runtime"]
  leash --> sim["sim profile"]
  leash --> ugv["optional waveshare-ugv profile"]
  leash --> telemetry["telemetry, sensors, camera status, WebSocket"]
```

Run Leash in compatibility mode:

```bash
cargo run --features bridge-compat -- \
  serve http --profile sim --role guard --listen 127.0.0.1:8000
```

Then point the bridge/client at `http://127.0.0.1:8000`.

Compatibility routes currently include:

- `GET /health`
- `GET /capabilities`
- `GET /telemetry`
- `GET /sensors`
- `GET /camera/status`
- `POST /pilot/authorize`
- `POST /pilot/speed-mode`
- `POST /drive`
- `POST /motors/drive`
- `POST /motors/stop`
- `POST /estop`
- `POST /estop/reset`
- `GET /stream`
- `WS /ws/telemetry`
