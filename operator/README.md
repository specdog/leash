# Leash Operator

Dockerized fleet GUI for local UGV operation. The container serves a web UI and
proxies requests to named Leash robots.

```bash
docker build -t leash-operator ./operator
docker run --rm --name leash-operator \
  -p 8787:8787 \
  -v "$PWD/operator/fleet.example.json:/app/config/fleet.json:ro" \
  leash-operator
```

Open `http://localhost:8787`.

Fleet membership is the mounted JSON file. Each robot needs a unique `id`,
display `name`, and Leash HTTP `baseUrl`.

Set `videoTransport` to `mjpeg`, `webrtc`, or `auto`. `webrtc` is strict: the
operator reports a fault when the bot does not advertise WebRTC and will not
silently open the MJPEG relay. `auto` prefers WebRTC and falls back to MJPEG.
Camera Refresh calls the bot's `/camera/stream/recover` endpoint and clears the
operator relay cache.
