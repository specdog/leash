#!/usr/bin/env bash
set -euo pipefail

port="${LEASH_MCP_BRIDGE_SMOKE_PORT:-19991}"
base="http://127.0.0.1:$port"
log_file="$(mktemp -t leash-mcp-bridge-smoke.XXXXXX.log)"
state_dir="$(mktemp -d -t leash-mcp-bridge-smoke-state.XXXXXX)"
timeout_secs="${LEASH_SMOKE_TIMEOUT_SECS:-60}"

cleanup() {
  if [[ -n "${server_pid:-}" ]] && kill -0 "$server_pid" 2>/dev/null; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -f "$log_file"
  rm -rf "$state_dir"
}
trap cleanup EXIT

LEASH_STATE_DIR="$state_dir" cargo run --quiet -- serve mcp-http --profile sim --listen "127.0.0.1:$port" >"$log_file" 2>&1 &
server_pid=$!

ready=false
for _ in $(seq 1 $((timeout_secs * 10))); do
  if curl -fsS "$base/mcp/status" >/dev/null 2>&1; then
    ready=true
    break
  fi
  if ! kill -0 "$server_pid" 2>/dev/null; then
    echo "mcp bridge smoke server exited before readiness" >&2
    cat "$log_file" >&2
    exit 1
  fi
  sleep 0.1
done

if [[ "$ready" != true ]]; then
  echo "mcp bridge smoke server was not ready after ${timeout_secs}s" >&2
  cat "$log_file" >&2
  exit 1
fi

LEASH_BRIDGE_URL="$base" node <<'EOF'
const { spawn } = require("node:child_process");
const readline = require("node:readline");

const proc = spawn("cargo", ["run", "--quiet", "--", "mcp", "bridge"], {
  env: process.env,
  stdio: ["pipe", "pipe", "pipe"],
});

const rl = readline.createInterface({ input: proc.stdout });
let stderr = "";
proc.stderr.on("data", (chunk) => {
  stderr += chunk.toString();
});

function fail(message) {
  proc.kill("SIGTERM");
  throw new Error(`${message}\nstderr:\n${stderr}`);
}

function send(message) {
  proc.stdin.write(`${JSON.stringify(message)}\n`);
}

function readLine(timeoutMs = 30000) {
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => {
      reject(new Error(`timed out waiting for MCP bridge response\nstderr:\n${stderr}`));
    }, timeoutMs);
    rl.once("line", (line) => {
      clearTimeout(timeout);
      try {
        resolve(JSON.parse(line));
      } catch (error) {
        reject(error);
      }
    });
    proc.once("exit", () => {
      clearTimeout(timeout);
      reject(new Error(`MCP bridge process closed stdout\nstderr:\n${stderr}`));
    });
  });
}

function parseTextPayload(response, id) {
  if (response.id !== id || !response.result) {
    fail(`bad MCP response: ${JSON.stringify(response)}`);
  }
  const text = response.result.content?.[0]?.text;
  if (!text) {
    fail(`missing text content: ${JSON.stringify(response)}`);
  }
  return JSON.parse(text);
}

async function main() {
  send({
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: {
      protocolVersion: "2025-11-25",
      capabilities: {},
      clientInfo: { name: "leash-bridge-smoke", version: "0.1.0" },
    },
  });
  const init = await readLine();
  if (init.id !== 1 || !init.result) fail(`bad initialize response: ${JSON.stringify(init)}`);

  send({ jsonrpc: "2.0", method: "notifications/initialized", params: {} });
  send({ jsonrpc: "2.0", id: 2, method: "tools/list", params: {} });
  const tools = await readLine();
  const names = new Set(tools.result.tools.map((tool) => tool.name));
  for (const name of ["health", "capabilities", "modules", "observe", "invoke_capability", "stop", "estop", "capture"]) {
    if (!names.has(name)) fail(`missing bridge tool: ${name}`);
  }

  send({
    jsonrpc: "2.0",
    id: 3,
    method: "tools/call",
    params: { name: "health", arguments: {} },
  });
  const health = parseTextPayload(await readLine(), 3);
  if (health.ok !== true || health.tool !== "health" || health.result.profile !== "sim") {
    fail(`bad bridged health payload: ${JSON.stringify(health)}`);
  }

  send({
    jsonrpc: "2.0",
    id: 4,
    method: "tools/call",
    params: { name: "stop", arguments: {} },
  });
  const stop = parseTextPayload(await readLine(), 4);
  if (stop.ok !== true || stop.tool !== "stop" || stop.result.ok !== true) {
    fail(`bad bridged stop payload: ${JSON.stringify(stop)}`);
  }

  console.log("mcp bridge smoke ok");
}

main()
  .catch((error) => {
    console.error(`mcp bridge smoke failed: ${error.message}`);
    process.exitCode = 1;
  })
  .finally(() => {
    proc.stdin.end();
    proc.kill("SIGTERM");
  });
EOF
