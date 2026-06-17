#!/usr/bin/env bash
set -euo pipefail

port="${LEASH_DAEMON_SMOKE_PORT:-18082}"
base="http://127.0.0.1:$port"
state_dir="$(mktemp -d -t leash-daemon-smoke.XXXXXX)"
name="smoke"

cleanup() {
  LEASH_STATE_DIR="$state_dir" cargo run --quiet -- stop "$name" >/dev/null 2>&1 || true
  rm -rf "$state_dir"
}
trap cleanup EXIT

wait_ready() {
  for _ in $(seq 1 100); do
    if curl -fsS "$base/health" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done
  echo "daemon smoke server was not ready" >&2
  LEASH_STATE_DIR="$state_dir" cargo run --quiet -- log "$name" --lines 80 >&2 || true
  return 1
}

assert_running_status() {
  node -e 'const payload = JSON.parse(require("node:fs").readFileSync(0, "utf8"));
if (payload.ok !== true) throw new Error("status ok was not true");
if (payload.runs.length !== 1) throw new Error(`expected one run, got ${payload.runs.length}`);
if (payload.runs[0].running !== true) throw new Error("run was not running");
if (payload.runs[0].profile !== "sim") throw new Error("run profile was not sim");'
}

assert_empty_status() {
  node -e 'const payload = JSON.parse(require("node:fs").readFileSync(0, "utf8"));
if (payload.ok !== true) throw new Error("status ok was not true");
if (payload.runs.length !== 0) throw new Error(`expected no runs, got ${payload.runs.length}`);'
}

LEASH_STATE_DIR="$state_dir" cargo run --quiet -- run "$name" --daemon --profile sim --listen "127.0.0.1:$port" >/dev/null
wait_ready
LEASH_STATE_DIR="$state_dir" cargo run --quiet -- status "$name" | assert_running_status
LEASH_STATE_DIR="$state_dir" cargo run --quiet -- log "$name" --lines 40 | grep -q "leash http listening"

LEASH_STATE_DIR="$state_dir" cargo run --quiet -- restart "$name" >/dev/null
wait_ready
LEASH_STATE_DIR="$state_dir" cargo run --quiet -- status "$name" | assert_running_status

LEASH_STATE_DIR="$state_dir" cargo run --quiet -- stop "$name" >/dev/null
LEASH_STATE_DIR="$state_dir" cargo run --quiet -- status "$name" | assert_empty_status

echo "daemon smoke ok: $base"
