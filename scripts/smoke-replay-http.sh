#!/usr/bin/env bash
set -euo pipefail

port="${LEASH_REPLAY_SMOKE_PORT:-18081}"
base="http://127.0.0.1:$port"
log_file="$(mktemp -t leash-replay-http-smoke.XXXXXX.log)"
timeout_secs="${LEASH_SMOKE_TIMEOUT_SECS:-60}"

cleanup() {
  if [[ -n "${server_pid:-}" ]] && kill -0 "$server_pid" 2>/dev/null; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -f "$log_file"
}
trap cleanup EXIT

cargo run --quiet -- serve http \
  --replay-source examples/replay/sim-basic.jsonl \
  --listen "127.0.0.1:$port" >"$log_file" 2>&1 &
server_pid=$!

ready=false
for _ in $(seq 1 $((timeout_secs * 10))); do
  if curl -fsS "$base/health" >/dev/null 2>&1; then
    ready=true
    break
  fi
  if ! kill -0 "$server_pid" 2>/dev/null; then
    echo "replay http smoke server exited before readiness" >&2
    cat "$log_file" >&2
    exit 1
  fi
  sleep 0.1
done

if [[ "$ready" != true ]]; then
  echo "replay http smoke server was not ready after ${timeout_secs}s" >&2
  cat "$log_file" >&2
  exit 1
fi

assert_replay_health() {
  node -e 'const payload = JSON.parse(require("node:fs").readFileSync(0, "utf8"));
if (payload.ok !== true) throw new Error("health ok was not true");
if (payload.mode !== "replay" || payload.replay !== true || payload.profile !== "replay") {
  throw new Error(`health was not replay mode: ${JSON.stringify(payload)}`);
}
if (payload.physical_actuation_enabled !== false) {
  throw new Error("replay health exposed physical actuation");
}'
}

assert_replay_capabilities() {
  node -e 'const payload = JSON.parse(require("node:fs").readFileSync(0, "utf8"));
if (payload.mode !== "replay" || payload.replay !== true || payload.profile !== "replay") {
  throw new Error(`capabilities were not replay mode: ${JSON.stringify(payload)}`);
}
if (payload.physical !== false) throw new Error("replay capabilities were physical");'
}

assert_replay_telemetry() {
  node -e 'const payload = JSON.parse(require("node:fs").readFileSync(0, "utf8"));
if (payload.profile !== "replay" || payload.source !== "replay") {
  throw new Error(`telemetry was not replay sourced: ${JSON.stringify(payload)}`);
}
if (!payload.sensors || payload.sensors.raw_frame.source !== "replay") {
  throw new Error("telemetry sensors were not replay sourced");
}'
}

curl -fsS "$base/health" | assert_replay_health
curl -fsS "$base/capabilities" | assert_replay_capabilities
curl -fsS "$base/telemetry" | assert_replay_telemetry

echo "replay http smoke ok: $base"
