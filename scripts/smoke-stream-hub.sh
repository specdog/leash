#!/usr/bin/env bash
set -euo pipefail

port="${LEASH_STREAM_HUB_SMOKE_PORT:-18084}"
log_file="$(mktemp -t leash-stream-hub-smoke.XXXXXX.log)"
timeout_secs="${LEASH_SMOKE_TIMEOUT_SECS:-60}"

cleanup() {
  if [[ -n "${server_pid:-}" ]] && kill -0 "$server_pid" 2>/dev/null; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -f "$log_file"
}
trap cleanup EXIT

cargo run --quiet -- serve stream-hub --profile sim --listen "127.0.0.1:$port" >"$log_file" 2>&1 &
server_pid=$!

ready=false
for _ in $(seq 1 $((timeout_secs * 10))); do
  if PORT="$port" node <<'EOF' >/dev/null 2>&1
const net = require("node:net");
const socket = net.createConnection({ host: "127.0.0.1", port: Number(process.env.PORT) });
socket.setTimeout(250);
socket.on("connect", () => socket.end());
socket.on("close", () => process.exit(0));
socket.on("timeout", () => socket.destroy(new Error("timeout")));
socket.on("error", () => process.exit(1));
EOF
  then
    ready=true
    break
  fi
  if ! kill -0 "$server_pid" 2>/dev/null; then
    echo "stream hub smoke server exited before readiness" >&2
    cat "$log_file" >&2
    exit 1
  fi
  sleep 0.1
done

if [[ "$ready" != true ]]; then
  echo "stream hub smoke server was not ready after ${timeout_secs}s" >&2
  cat "$log_file" >&2
  exit 1
fi

LOG_FILE="$log_file" PORT="$port" node <<'EOF'
const fs = require("node:fs");
const net = require("node:net");

const port = Number(process.env.PORT);

function sendLines(lines) {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection({ host: "127.0.0.1", port }, () => {
      for (const line of lines) socket.write(`${line}\n`);
      socket.end();
    });
    socket.setTimeout(2000);
    socket.on("timeout", () => socket.destroy(new Error("tcp jsonl send timeout")));
    socket.on("error", reject);
    socket.on("close", resolve);
  });
}

function frame(stream, seq) {
  return JSON.stringify({
    schema_version: "leash-stream-jsonl-v1",
    stream,
    payload: { seq, source: "smoke-stream-hub" },
  });
}

(async () => {
  const firstLine = fs.readFileSync(process.env.LOG_FILE, "utf8").trim().split(/\n+/)[0];
  const startup = JSON.parse(firstLine);
  if (startup.ok !== true) throw new Error("stream hub startup ok was not true");
  if (startup.transport !== "stream-hub") throw new Error(`unexpected transport: ${startup.transport}`);
  if (startup.profile !== "sim") throw new Error(`unexpected profile: ${startup.profile}`);
  if (startup.listen !== `127.0.0.1:${port}`) throw new Error(`unexpected listen: ${startup.listen}`);
  if (startup.stream_transport !== "local-pubsub") {
    throw new Error(`unexpected stream transport: ${startup.stream_transport}`);
  }

  await sendLines([frame("telemetry", 1), frame("telemetry", 2)]);
  await sendLines([
    JSON.stringify({
      schema_version: "old",
      stream: "telemetry",
      payload: { seq: 0, source: "bad-peer" },
    }),
  ]);
  await sendLines([frame("telemetry", 3)]);
})().catch((error) => {
  console.error(error.stack || String(error));
  process.exit(1);
});
EOF

if ! kill -0 "$server_pid" 2>/dev/null; then
  echo "stream hub smoke server exited after client traffic" >&2
  cat "$log_file" >&2
  exit 1
fi

echo "stream hub smoke ok: 127.0.0.1:$port"
