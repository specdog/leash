#!/usr/bin/env bash
set -euo pipefail

port="${LEASH_SMOKE_PORT:-18080}"
base="http://127.0.0.1:$port"
log_file="$(mktemp -t leash-http-smoke.XXXXXX.log)"

cleanup() {
  if [[ -n "${server_pid:-}" ]] && kill -0 "$server_pid" 2>/dev/null; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -f "$log_file"
}
trap cleanup EXIT

cargo run --quiet -- serve http --profile sim --listen "127.0.0.1:$port" >"$log_file" 2>&1 &
server_pid=$!

for _ in $(seq 1 80); do
  if curl -fsS "$base/health" >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done

curl -fsS "$base/health" | python3 -m json.tool >/dev/null
curl -fsS "$base/capabilities" | python3 -m json.tool >/dev/null
curl -fsS "$base/telemetry" | python3 -m json.tool >/dev/null
curl -fsS "$base/sensors" | python3 -m json.tool >/dev/null
curl -fsS -X POST "$base/motors/stop" | python3 -m json.tool >/dev/null

echo "http smoke ok: $base"

