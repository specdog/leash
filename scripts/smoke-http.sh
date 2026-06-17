#!/usr/bin/env bash
set -euo pipefail

port="${LEASH_SMOKE_PORT:-18080}"
base="http://127.0.0.1:$port"
log_file="$(mktemp -t leash-http-smoke.XXXXXX.log)"
timeout_secs="${LEASH_SMOKE_TIMEOUT_SECS:-60}"

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

ready=false
for _ in $(seq 1 $((timeout_secs * 10))); do
  if curl -fsS "$base/health" >/dev/null 2>&1; then
    ready=true
    break
  fi
  if ! kill -0 "$server_pid" 2>/dev/null; then
    echo "http smoke server exited before readiness" >&2
    cat "$log_file" >&2
    exit 1
  fi
  sleep 0.1
done

if [[ "$ready" != true ]]; then
  echo "http smoke server was not ready after ${timeout_secs}s" >&2
  cat "$log_file" >&2
  exit 1
fi

curl -fsS "$base/health" | python3 -m json.tool >/dev/null
curl -fsS "$base/capabilities" | python3 -m json.tool >/dev/null
curl -fsS "$base/telemetry" | python3 -m json.tool >/dev/null
curl -fsS "$base/sensors" | python3 -m json.tool >/dev/null
curl -fsS -X POST "$base/motors/stop" | python3 -m json.tool >/dev/null

echo "http smoke ok: $base"
