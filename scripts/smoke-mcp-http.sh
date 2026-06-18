#!/usr/bin/env bash
set -euo pipefail

port="${LEASH_MCP_HTTP_SMOKE_PORT:-19990}"
base="http://127.0.0.1:$port"
log_file="$(mktemp -t leash-mcp-http-smoke.XXXXXX.log)"
timeout_secs="${LEASH_SMOKE_TIMEOUT_SECS:-60}"

cleanup() {
  if [[ -n "${server_pid:-}" ]] && kill -0 "$server_pid" 2>/dev/null; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -f "$log_file"
}
trap cleanup EXIT

cargo run --quiet -- serve mcp-http --profile sim --listen "127.0.0.1:$port" >"$log_file" 2>&1 &
server_pid=$!

ready=false
for _ in $(seq 1 $((timeout_secs * 10))); do
  if curl -fsS "$base/mcp/status" >/dev/null 2>&1; then
    ready=true
    break
  fi
  if ! kill -0 "$server_pid" 2>/dev/null; then
    echo "mcp-http smoke server exited before readiness" >&2
    cat "$log_file" >&2
    exit 1
  fi
  sleep 0.1
done

if [[ "$ready" != true ]]; then
  echo "mcp-http smoke server was not ready after ${timeout_secs}s" >&2
  cat "$log_file" >&2
  exit 1
fi

assert_tools() {
  node -e 'const payload = JSON.parse(require("node:fs").readFileSync(0, "utf8"));
if (payload.ok !== true) throw new Error("tools ok was not true");
const names = new Set(payload.tools.map((tool) => tool.name));
for (const name of ["health", "capabilities", "modules", "observe", "invoke_capability", "stop", "estop", "capture"]) {
  if (!names.has(name)) throw new Error(`missing MCP tool: ${name}`);
}'
}

assert_status() {
  node -e 'const payload = JSON.parse(require("node:fs").readFileSync(0, "utf8"));
if (payload.ok !== true) throw new Error("status ok was not true");
if (payload.transport !== "mcp-http") throw new Error(`unexpected transport: ${payload.transport}`);
if (payload.profile !== "sim") throw new Error(`unexpected profile: ${payload.profile}`);
if (payload.tool_count < 8) throw new Error(`unexpected tool count: ${payload.tool_count}`);'
}

assert_health_call() {
  node -e 'const payload = JSON.parse(require("node:fs").readFileSync(0, "utf8"));
if (payload.ok !== true || payload.tool !== "health") throw new Error("health call wrapper was invalid");
if (!payload.result || payload.result.ok !== true) throw new Error("health result ok was not true");
if (payload.result.profile !== "sim") throw new Error(`unexpected health profile: ${payload.result.profile}`);
if (!Array.isArray(payload.result.modules) || payload.result.modules.length < 3) throw new Error("health modules were missing");'
}

assert_stop_call() {
  node -e 'const payload = JSON.parse(require("node:fs").readFileSync(0, "utf8"));
if (payload.ok !== true || payload.tool !== "stop") throw new Error("stop call wrapper was invalid");
if (!payload.result || payload.result.ok !== true) throw new Error("stop result ok was not true");
if (payload.result.left !== 0 || payload.result.right !== 0) throw new Error("stop result did not zero motors");'
}

assert_key_value_call() {
  node -e 'const payload = JSON.parse(require("node:fs").readFileSync(0, "utf8"));
if (payload.ok !== true || payload.tool !== "invoke_capability") throw new Error("key=value call wrapper was invalid");
if (!payload.result || payload.result.ok !== true) throw new Error("key=value call result ok was not true");
if (payload.result.ttl_secs !== 30) throw new Error(`unexpected ttl_secs: ${payload.result.ttl_secs}`);
if (JSON.stringify(payload).includes("mcp-smoke-token")) throw new Error("key=value call leaked token");'
}

assert_json_call() {
  node -e 'const payload = JSON.parse(require("node:fs").readFileSync(0, "utf8"));
if (payload.ok !== true || payload.tool !== "invoke_capability") throw new Error("JSON call wrapper was invalid");
if (!payload.result || payload.result.ok !== true) throw new Error("JSON call result ok was not true");
if (payload.result.speed_mode !== "low") throw new Error(`unexpected speed mode: ${payload.result.speed_mode}`);'
}

assert_module_map() {
  node -e 'const payload = JSON.parse(require("node:fs").readFileSync(0, "utf8"));
if (payload.ok !== true) throw new Error("module map ok was not true");
const runtime = payload.modules.find((module) => module.module === "harness-runtime");
if (!runtime || !runtime.tools.includes("invoke_capability")) throw new Error("runtime tool mapping missing invoke_capability");
const driver = payload.modules.find((module) => module.module.endsWith("-driver"));
if (!driver || !driver.tools.includes("stop")) throw new Error("driver tool mapping missing stop");
if (JSON.stringify(payload).includes("mcp-smoke-token")) throw new Error("module map leaked token");'
}

curl -fsS "$base/mcp/tools" | assert_tools
curl -fsS "$base/mcp/status" | assert_status
curl -fsS -X POST "$base/mcp/call" \
  -H "content-type: application/json" \
  --data '{"tool":"health","args":{}}' | assert_health_call
curl -fsS -X POST "$base/mcp/call" \
  -H "content-type: application/json" \
  --data '{"tool":"stop","args":{}}' | assert_stop_call

cargo run --quiet -- mcp list-tools | assert_tools
cargo run --quiet -- mcp status --url "$base" | assert_status
cargo run --quiet -- mcp modules --profile sim | assert_module_map
cargo run --quiet -- mcp call --profile sim invoke_capability \
  capability=authorize token=mcp-smoke-token ttl_secs=30 speed_mode=low | assert_key_value_call
cargo run --quiet -- mcp call --profile sim --json '{"capability":"speed_mode","speed_mode":"low"}' \
  invoke_capability | assert_json_call

echo "mcp-http smoke ok: $base"
