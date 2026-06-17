# Waveshare Ubuntu UGV Example

This adapter matches the original Ubuntu UGV / Jetson Orin NX demo shape:

- ESP32 lower board on `/dev/ttyTHS1`
- 115200 baud newline JSON drive commands
- `{"T":1,"L":left,"R":right}` motor frames

Build with the physical adapter:

```bash
cargo build --release --features waveshare-ugv,bridge-compat
```

Run only after stopping the stock Waveshare service:

```bash
LEASH_ALLOW_PHYSICAL_ACTUATION=1 \
  target/release/leash serve http \
  --profile waveshare-ugv \
  --role courier \
  --listen 0.0.0.0:8000 \
  --serial-port /dev/ttyTHS1 \
  --deadman-ms 400
```

Safety defaults:

- physical profile refuses to start without the explicit env/flag gate
- speed caps are enforced before writing serial frames
- deadman sends zero speed after stale drive commands
- `estop` latches until `estop_reset`
