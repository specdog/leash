#!/usr/bin/env bash
set -euo pipefail

node <<'EOF'
const { spawn } = require("node:child_process");
const readline = require("node:readline");

const proc = spawn(
  "cargo",
  [
    "run",
    "--quiet",
    "--",
    "serve",
    "mcp",
    "--replay-source",
    "examples/replay/sim-basic.jsonl",
  ],
  { stdio: ["pipe", "pipe", "pipe"] }
);

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

function readLine(timeoutMs = 10000) {
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => {
      reject(new Error(`timed out waiting for MCP response\nstderr:\n${stderr}`));
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
      reject(new Error(`MCP process closed stdout\nstderr:\n${stderr}`));
    });
  });
}

function textPayload(response) {
  if (!response.result || !response.result.content || !response.result.content[0]) {
    fail(`bad tool response: ${JSON.stringify(response)}`);
  }
  return JSON.parse(response.result.content[0].text);
}

async function main() {
  send({
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: {
      protocolVersion: "2025-11-25",
      capabilities: {},
      clientInfo: { name: "leash-replay-smoke", version: "0.1.0" },
    },
  });
  const init = await readLine();
  if (init.id !== 1 || !init.result) fail(`bad initialize response: ${JSON.stringify(init)}`);

  send({ jsonrpc: "2.0", method: "notifications/initialized", params: {} });

  send({
    jsonrpc: "2.0",
    id: 2,
    method: "tools/call",
    params: { name: "health", arguments: {} },
  });
  const health = textPayload(await readLine());
  if (health.mode !== "replay" || health.replay !== true || health.profile !== "replay") {
    fail(`bad replay health payload: ${JSON.stringify(health)}`);
  }
  if (health.physical_actuation_enabled !== false) {
    fail(`replay MCP health exposed physical actuation: ${JSON.stringify(health)}`);
  }

  send({
    jsonrpc: "2.0",
    id: 3,
    method: "tools/call",
    params: { name: "observe", arguments: {} },
  });
  const telemetry = textPayload(await readLine());
  if (telemetry.profile !== "replay" || telemetry.source !== "replay") {
    fail(`bad replay observe payload: ${JSON.stringify(telemetry)}`);
  }
  if (telemetry.vision?.status !== "ok" || telemetry.vision.detections?.[0]?.label !== "replay-fixture") {
    fail(`missing replay fake detection: ${JSON.stringify(telemetry.vision)}`);
  }

  console.log("replay mcp smoke ok");
}

main()
  .catch((error) => {
    console.error(`replay mcp smoke failed: ${error.message}`);
    process.exitCode = 1;
  })
  .finally(() => {
    proc.stdin.end();
    proc.kill("SIGTERM");
  });
EOF
