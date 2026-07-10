#!/usr/bin/env bash
set -euo pipefail

node <<'EOF'
const { spawn } = require("node:child_process");
const readline = require("node:readline");

const proc = spawn("cargo", ["run", "--quiet", "--", "serve", "mcp", "--profile", "sim"], {
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

async function main() {
  send({
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: {
      protocolVersion: "2025-11-25",
      capabilities: {},
      clientInfo: { name: "leash-smoke", version: "0.1.0" },
    },
  });
  const init = await readLine();
  if (init.id !== 1 || !init.result) fail(`bad initialize response: ${JSON.stringify(init)}`);

  send({ jsonrpc: "2.0", method: "notifications/initialized", params: {} });
  send({ jsonrpc: "2.0", id: 2, method: "tools/list", params: {} });
  const tools = await readLine();
  const names = new Set(tools.result.tools.map((tool) => tool.name));
  for (const name of ["health", "capabilities", "observe", "invoke_capability", "stop", "estop", "capture"]) {
    if (!names.has(name)) fail(`missing tool: ${name}`);
  }

  send({
    jsonrpc: "2.0",
    id: 3,
    method: "tools/call",
    params: { name: "health", arguments: {} },
  });
  const health = await readLine();
  if (health.id !== 3 || !health.result) fail(`bad health response: ${JSON.stringify(health)}`);
  const payload = JSON.parse(health.result.content[0].text);
  if (payload.ok !== true || payload.profile !== "sim") {
    fail(`bad health payload: ${JSON.stringify(payload)}`);
  }
  if (!Array.isArray(payload.modules) || payload.modules.length < 3) {
    fail(`missing health modules: ${JSON.stringify(payload)}`);
  }
  if (!payload.modules.every((module) => module.state === "running")) {
    fail(`bad module states: ${JSON.stringify(payload.modules)}`);
  }

  send({
    jsonrpc: "2.0",
    id: 4,
    method: "tools/call",
    params: { name: "observe", arguments: {} },
  });
  const observe = await readLine();
  if (observe.id !== 4 || !observe.result) fail(`bad observe response: ${JSON.stringify(observe)}`);
  const telemetry = JSON.parse(observe.result.content[0].text);
  if (telemetry.profile !== "sim" || telemetry.vision?.status !== "ok") {
    fail(`bad observe payload: ${JSON.stringify(telemetry)}`);
  }
  if (!Array.isArray(telemetry.vision.detections) || telemetry.vision.detections[0]?.label !== "sim-fixture") {
    fail(`missing fake detection in observe payload: ${JSON.stringify(telemetry.vision)}`);
  }
  if (telemetry.sensors?.range_scan?.status !== "available" || telemetry.sensors?.range_scan?.sample?.frame_id !== "base_scan") {
    fail(`missing range scan in observe payload: ${JSON.stringify(telemetry.sensors)}`);
  }
  if (telemetry.sensors?.imu?.status !== "available" || telemetry.sensors?.imu?.sample?.frame_id !== "base_link") {
    fail(`missing IMU in observe payload: ${JSON.stringify(telemetry.sensors)}`);
  }
  if (telemetry.localization?.version !== "leash-localization-v1" || telemetry.localization?.health?.status !== "tracking") {
    fail(`missing localization in observe payload: ${JSON.stringify(telemetry.localization)}`);
  }
  if (telemetry.map?.map_id !== telemetry.localization.map.map_id) {
    fail(`map identity did not match localization: ${JSON.stringify(telemetry.map)}`);
  }
  if (!Array.isArray(telemetry.occupancy_grid?.cells) || telemetry.occupancy_grid.cells.length !== telemetry.occupancy_grid.metadata.width * telemetry.occupancy_grid.metadata.height) {
    fail(`invalid occupancy grid in observe payload: ${JSON.stringify(telemetry.occupancy_grid)}`);
  }
  if (!Array.isArray(telemetry.costmap?.costs) || telemetry.costmap.costs.length !== telemetry.costmap.metadata.width * telemetry.costmap.metadata.height) {
    fail(`invalid costmap in observe payload: ${JSON.stringify(telemetry.costmap)}`);
  }

  console.log("mcp smoke ok");
}

main()
  .catch((error) => {
    console.error(`mcp smoke failed: ${error.message}`);
    process.exitCode = 1;
  })
  .finally(() => {
    proc.stdin.end();
    proc.kill("SIGTERM");
  });
EOF
