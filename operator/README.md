# Leash Operator

Dockerized fleet GUI for local UGV operation. The container serves a web UI and
proxies requests to named Leash robots.

```bash
cp operator/fleet.example.json operator/fleet.local.json
# Edit fleet.local.json with private bot names and addresses, then validate it.
node operator/server.js --check-config operator/fleet.local.json

docker build -t leash-operator ./operator
docker run --rm --name leash-operator \
  -p 8787:8787 \
  -v "$PWD/operator/fleet.local.json:/app/config/fleet.json:ro" \
  leash-operator
```

Open `http://localhost:8787`.

Fleet membership is the mounted JSON file. `fleet.local.json` and
`operator/config/fleet.json` are ignored so real names, addresses, and notes do
not enter source control. The public example uses documentation-only TEST-NET
addresses. [`fleet.schema.json`](fleet.schema.json) is the machine-readable
contract; the built-in validator additionally rejects duplicate IDs,
credentials in URLs, URL paths, and unknown fields with exact JSON paths.

Each robot requires a unique `id` and an HTTP(S) origin in `baseUrl`. Optional
root fields are `fleetName`, `pollMs`, and `snapshotMs`; intervals must be
integers from 100 through 60000 milliseconds. Optional robot fields are `name`,
`role`, `location`, `notes`, and `videoTransport`.

Set `videoTransport` to `mjpeg`, `webrtc`, or `auto`. `webrtc` is strict: the
operator reports a fault when the bot does not advertise WebRTC and will not
silently open the MJPEG relay. `auto` prefers WebRTC and falls back to MJPEG.
Camera Refresh calls the bot's `/camera/stream/recover` endpoint and clears the
operator relay cache.

## Operator token ownership

Authorization is single-owner and last-writer-wins. A successful authorize
request stops motion owned by a different token, invalidates every older token,
and grants the new token until its TTL expires. Health responses expose only a
stable hashed `owner_id`, remaining TTL, and speed mode; raw bearer tokens are
never returned in health or telemetry. The GUI labels the matching browser
token as `mine` and shows the recent ownership state for every bot.
