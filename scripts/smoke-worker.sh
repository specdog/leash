#!/usr/bin/env bash
set -euo pipefail

output="$(
  cargo run --quiet -- worker run \
    --name smoke-worker \
    --hold-ms 150 \
    -- node -e 'setInterval(() => {}, 1000)'
)"

WORKER_OUTPUT="$output" node <<'EOF'
const payload = JSON.parse(process.env.WORKER_OUTPUT || "");

if (payload.ok !== true) throw new Error("worker smoke ok was not true");
if (!Array.isArray(payload.statuses) || payload.statuses.length !== 1) {
  throw new Error(`expected one worker status, got ${JSON.stringify(payload.statuses)}`);
}

const worker = payload.statuses[0];
if (worker.name !== "smoke-worker") throw new Error(`unexpected worker name: ${worker.name}`);
if (worker.state !== "running") throw new Error(`unexpected worker state: ${worker.state}`);
if (!Number.isInteger(worker.pid) || worker.pid <= 0) {
  throw new Error(`worker pid was invalid: ${worker.pid}`);
}
if (worker.restart_policy !== "never") {
  throw new Error(`unexpected restart policy: ${worker.restart_policy}`);
}
if (worker.health_check !== "process") {
  throw new Error(`unexpected health check: ${worker.health_check}`);
}
if (worker.required !== true) throw new Error("worker should default to required");
EOF

echo "worker smoke ok"
