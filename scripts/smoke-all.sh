#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

REPO_ROOT="$repo_root" node <<'EOF'
const { spawnSync } = require("node:child_process");

const repoRoot = process.env.REPO_ROOT;
const startedAt = new Date().toISOString();

const baseEnv = { ...process.env, LEASH_ALLOW_PHYSICAL_ACTUATION: "0" };
for (const key of [
  "LEASH_ACCELERATOR",
  "LEASH_AGENT_API_KEY",
  "LEASH_AGENT_BASE_URL",
  "LEASH_AGENT_MODEL",
  "LEASH_AGENT_PROVIDER",
  "LEASH_AGENT_TIMEOUT_MS",
  "LEASH_ALLOW_UNTOKENED_DRIVE",
  "LEASH_CONFIG",
  "LEASH_LISTEN",
  "LEASH_MAVLINK_ENDPOINT",
  "LEASH_POLICY_MODE",
  "LEASH_PROFILE",
  "LEASH_REPLAY_SOURCE",
  "LEASH_REPLAY_SPEED",
  "LEASH_REQUIRE_ACCELERATOR",
  "LEASH_ROLE",
  "LEASH_STREAM_TRANSPORT",
]) {
  delete baseEnv[key];
}

const checks = [
  {
    name: "message-schemas-current",
    argv: ["cargo", "run", "--quiet", "--features", "mcp", "--bin", "leash-schema", "--", "--check"],
    proof: "checked-in JSON Schemas matched Rust wire message types",
  },
  {
    name: "http-routes-and-policy",
    argv: ["bash", "scripts/smoke-http.sh"],
    proof: "HTTP routes, WebSocket/SSE telemetry with visualization map/costmap frames, external clients, agent input, capture, authorized drive, and drive-denial policy passed",
  },
  {
    name: "mcp-stdio",
    argv: ["bash", "scripts/smoke-mcp.sh"],
    proof: "stdio MCP initialization, tool list, health call, and fake-detection observe passed",
  },
  {
    name: "mcp-http-cli",
    argv: ["bash", "scripts/smoke-mcp-http.sh"],
    proof: "HTTP MCP tool list, health/stop calls, CLI status/modules, key=value/JSON direct calls, planner, patrol, and spatial-memory calls passed",
  },
  {
    name: "stream-hub",
    argv: ["bash", "scripts/smoke-stream-hub.sh"],
    proof: "TCP JSONL stream hub accepted valid frames and kept serving after an invalid peer",
  },
  {
    name: "replay-http-observe",
    argv: ["bash", "scripts/smoke-replay-http.sh"],
    proof: "HTTP replay health, capabilities, telemetry observe, and fake detections passed",
  },
  {
    name: "replay-mcp-observe",
    argv: ["bash", "scripts/smoke-replay-mcp.sh"],
    proof: "MCP replay health, observe, and fake detections passed",
  },
  {
    name: "physical-gate",
    argv: ["bash", "scripts/smoke-physical-gate.sh"],
    proof: "physical profile refused to start without the explicit actuation gate",
  },
  {
    name: "daemon-lifecycle",
    argv: ["bash", "scripts/smoke-daemon.sh"],
    proof: "stack daemon start, status, log, restart, and stop passed",
  },
  {
    name: "stack-catalog",
    argv: ["cargo", "run", "--quiet", "--", "list", "--format", "json"],
    validate: (stdout) => {
      const stacks = JSON.parse(stdout);
      const names = stacks.map((stack) => stack.name);
      for (const required of ["sim-http", "sim-mcp", "sim-stream-hub", "bridge-compat-http", "waveshare-ugv-http"]) {
        if (!names.includes(required)) {
          throw new Error(`missing stack ${required}`);
        }
      }
      const streamHub = stacks.find((stack) => stack.name === "sim-stream-hub");
      if (!streamHub || streamHub.transport.kind !== "stream-hub") {
        throw new Error("sim-stream-hub stack did not declare stream-hub transport");
      }
      const physical = stacks.find((stack) => stack.name === "waveshare-ugv-http");
      if (!physical.hardware_required) {
        throw new Error("waveshare stack did not declare hardware_required");
      }
      if (!physical.adapter || physical.adapter.category !== "mobile-base") {
        throw new Error("waveshare stack did not declare mobile-base adapter metadata");
      }
      if (!physical.adapter.required_gates.includes("physical-actuation")) {
        throw new Error("waveshare stack did not declare physical-actuation gate");
      }
      return `listed ${stacks.length} built-in stacks`;
    },
  },
  {
    name: "stack-catalog-mavlink-drone",
    argv: ["cargo", "run", "--quiet", "--features", "mavlink-drone", "--", "list", "--format", "json"],
    validate: (stdout) => {
      const stacks = JSON.parse(stdout);
      const byName = new Map(stacks.map((stack) => [stack.name, stack]));
      for (const required of ["mavlink-drone-sim", "mavlink-drone-replay", "mavlink-drone-http"]) {
        if (!byName.has(required)) {
          throw new Error(`missing stack ${required}`);
        }
      }
      const sim = byName.get("mavlink-drone-sim");
      const replay = byName.get("mavlink-drone-replay");
      const physical = byName.get("mavlink-drone-http");
      if (sim.hardware_required || replay.hardware_required) {
        throw new Error("mavlink sim/replay stacks should not require hardware");
      }
      if (!physical.hardware_required) {
        throw new Error("mavlink physical stack did not declare hardware_required");
      }
      if (sim.adapter.category !== "drone" || replay.adapter.category !== "drone" || physical.adapter.category !== "drone") {
        throw new Error("mavlink stacks did not declare drone adapter metadata");
      }
      if (!sim.adapter.capabilities.includes("drone_arm") || !replay.adapter.capabilities.includes("drone_fly_to")) {
        throw new Error("mavlink sim/replay stacks did not expose drone capabilities");
      }
      if (!physical.adapter.required_gates.includes("physical-actuation")) {
        throw new Error("mavlink physical stack did not declare physical-actuation gate");
      }
      return "mavlink drone sim/replay/physical stack metadata resolved without hardware";
    },
  },
  {
    name: "stack-catalog-manipulator",
    argv: ["cargo", "run", "--quiet", "--features", "manipulator", "--", "list", "--format", "json"],
    validate: (stdout) => {
      const stacks = JSON.parse(stdout);
      const byName = new Map(stacks.map((stack) => [stack.name, stack]));
      for (const required of ["manipulator-sim", "manipulator-replay", "manipulator-http"]) {
        if (!byName.has(required)) {
          throw new Error(`missing stack ${required}`);
        }
      }
      const sim = byName.get("manipulator-sim");
      const replay = byName.get("manipulator-replay");
      const physical = byName.get("manipulator-http");
      if (sim.hardware_required || replay.hardware_required) {
        throw new Error("manipulator sim/replay stacks should not require hardware");
      }
      if (!physical.hardware_required) {
        throw new Error("manipulator physical stack did not declare hardware_required");
      }
      if (sim.adapter.category !== "manipulator" || replay.adapter.category !== "manipulator" || physical.adapter.category !== "manipulator") {
        throw new Error("manipulator stacks did not declare manipulator adapter metadata");
      }
      if (!sim.adapter.capabilities.includes("manipulator_joint_state") || !replay.adapter.capabilities.includes("manipulator_pose_command")) {
        throw new Error("manipulator sim/replay stacks did not expose manipulator capabilities");
      }
      if (!physical.adapter.required_gates.includes("physical-actuation")) {
        throw new Error("manipulator physical stack did not declare physical-actuation gate");
      }
      return "manipulator sim/replay/physical stack metadata resolved without hardware";
    },
  },
  {
    name: "graph-sim-json",
    argv: ["cargo", "run", "--quiet", "--", "graph", "sim", "--format", "json"],
    validate: (stdout) => {
      const graph = JSON.parse(stdout);
      if (!Array.isArray(graph.modules) || graph.modules.length < 3) {
        throw new Error("sim graph did not include the expected modules");
      }
      return `sim graph exported ${graph.modules.length} modules`;
    },
  },
  {
    name: "graph-physical-dot",
    argv: ["cargo", "run", "--quiet", "--", "graph", "waveshare-ugv", "--format", "dot"],
    validate: (stdout) => {
      if (!stdout.includes("digraph leash_module_graph")) {
        throw new Error("physical graph DOT output did not include a graph header");
      }
      if (!stdout.includes("physical")) {
        throw new Error("physical graph DOT output did not mark physical modules");
      }
      return "physical graph exported DOT";
    },
  },
  {
    name: "graph-stream-transport",
    argv: [
      "cargo",
      "run",
      "--quiet",
      "--",
      "graph",
      "sim",
      "--stream-transport",
      "memory",
    ],
    validate: (stdout) => {
      const graph = JSON.parse(stdout);
      const streams = graph.modules.flatMap((module) => [
        ...module.inputs,
        ...module.outputs,
      ]);
      if (!streams.length || streams.some((stream) => stream.transport !== "memory")) {
        throw new Error("graph did not apply selected stream transport");
      }
      return "graph stream transport selection applied";
    },
  },
  {
    name: "config-stack-sim-http",
    argv: ["cargo", "run", "--quiet", "--", "show-config", "sim-http"],
    validate: (stdout) => {
      const config = JSON.parse(stdout);
      if (config.profile !== "sim" || config.network_bind !== "127.0.0.1:8000") {
        throw new Error("sim-http stack did not resolve expected profile and bind");
      }
      const listen = config.fields.find((field) => field.name === "listen");
      if (!listen || listen.source !== "stack-default") {
        throw new Error("sim-http listen source was not stack-default");
      }
      if (config.stream_transport !== "local-pubsub") {
        throw new Error("default stream transport was not local-pubsub");
      }
      return "sim-http stack config resolved";
    },
  },
  {
    name: "config-physical-preflight",
    argv: [
      "cargo",
      "run",
      "--quiet",
      "--",
      "show-config",
      "waveshare-ugv",
      "--listen",
      "0.0.0.0:8000",
      "--allow-physical-actuation",
    ],
    validate: (stdout) => {
      const config = JSON.parse(stdout);
      if (!config.physical || !config.physical_actuation_enabled) {
        throw new Error("physical preflight did not enable the explicit actuation gate");
      }
      if (config.network_bind !== "0.0.0.0:8000") {
        throw new Error(`unexpected network bind: ${config.network_bind}`);
      }
      return "physical preflight config resolved";
    },
  },
  {
    name: "config-accelerator-cpu",
    argv: [
      "cargo",
      "run",
      "--quiet",
      "--",
      "show-config",
      "--accelerator",
      "cpu",
      "--require-accelerator",
    ],
    validate: (stdout) => {
      const config = JSON.parse(stdout);
      if (config.accelerator !== "cpu" || config.require_accelerator !== true) {
        throw new Error("CPU accelerator requirement was not preserved");
      }
      return "CPU accelerator requirement resolved";
    },
  },
  {
    name: "config-accelerator-fallback",
    argv: ["cargo", "run", "--quiet", "--", "show-config", "--accelerator", "cuda"],
    validate: (stdout) => {
      const config = JSON.parse(stdout);
      if (config.accelerator !== "cuda" || config.require_accelerator !== false) {
        throw new Error("accelerator fallback config was not preserved");
      }
      return "accelerator fallback config resolved without hardware";
    },
  },
  {
    name: "config-agent-hosted-redaction",
    env: { LEASH_AGENT_API_KEY: "super-secret" },
    argv: [
      "cargo",
      "run",
      "--quiet",
      "--",
      "show-config",
      "--agent-provider",
      "openai-compatible-http",
      "--agent-base-url",
      "https://example.test/v1",
      "--policy-mode",
      "require-approval",
    ],
    validate: (stdout) => {
      if (stdout.includes("super-secret")) {
        throw new Error("agent API key leaked into show-config output");
      }
      const config = JSON.parse(stdout);
      if (config.agent_provider !== "openai-compatible-http") {
        throw new Error("hosted agent provider did not resolve");
      }
      if (Object.prototype.hasOwnProperty.call(config, "agent_api_key")) {
        throw new Error("agent_api_key should not be serialized at the top level");
      }
      const key = config.fields.find((field) => field.name === "agent_api_key");
      if (!key || key.value !== "<redacted>" || key.source !== "env:LEASH_AGENT_API_KEY") {
        throw new Error("agent API key field was not redacted with env source");
      }
      const policy = config.fields.find((field) => field.name === "policy_mode");
      if (config.policy_mode !== "require-approval" || !policy || policy.value !== "require-approval" || policy.source !== "cli") {
        throw new Error("policy mode did not resolve from CLI");
      }
      return "hosted agent config resolved with redacted API key and policy mode";
    },
  },
  {
    name: "replay-fixture",
    argv: [
      "cargo",
      "run",
      "--quiet",
      "--",
      "replay",
      "examples/replay/sim-basic.jsonl",
      "--speed",
      "100",
    ],
    validate: (stdout) => {
      const events = stdout
        .trim()
        .split(/\n+/)
        .filter(Boolean)
        .map((line) => JSON.parse(line));
      if (events.length !== 8) {
        throw new Error(`expected 8 replay events, got ${events.length}`);
      }
      if (!events.every((event) => event.format === "leash-replay-v1")) {
        throw new Error("replay fixture emitted an unexpected format");
      }
      if (!events.some((event) => event.kind === "telemetry")) {
        throw new Error("replay fixture did not emit telemetry");
      }
      return "deterministic replay fixture emitted telemetry, sensors, camera, and command events";
    },
  },
  {
    name: "config-replay-source",
    argv: [
      "cargo",
      "run",
      "--quiet",
      "--",
      "show-config",
      "--replay-source",
      "examples/replay/sim-basic.jsonl",
    ],
    validate: (stdout) => {
      const config = JSON.parse(stdout);
      if (config.profile !== "replay" || config.physical !== false) {
        throw new Error("replay source did not resolve to non-physical replay profile");
      }
      const replaySource = config.fields.find((field) => field.name === "replay_source");
      if (!replaySource || replaySource.attention !== "replay") {
        throw new Error("replay_source field was not marked as replay");
      }
      return "replay source config resolved as non-physical";
    },
  },
];

function snippet(text) {
  const trimmed = String(text || "").trim();
  if (trimmed.length <= 4000) return trimmed;
  return `${trimmed.slice(0, 1800)}\n...\n${trimmed.slice(-1800)}`;
}

function runCheck(check) {
  const start = Date.now();
  const proc = spawnSync(check.argv[0], check.argv.slice(1), {
    cwd: repoRoot,
    env: { ...baseEnv, ...(check.env || {}) },
    encoding: "utf8",
    timeout: 180000,
  });
  const result = {
    name: check.name,
    ok: proc.status === 0,
    duration_ms: Date.now() - start,
    command: check.argv.join(" "),
  };

  if (proc.error) {
    result.ok = false;
    result.error = proc.error.message;
  } else if (proc.status !== 0) {
    result.error = `exit ${proc.status}${proc.signal ? ` (${proc.signal})` : ""}`;
  } else if (check.validate) {
    try {
      result.proof = check.validate(proc.stdout);
    } catch (error) {
      result.ok = false;
      result.error = error.message;
    }
  } else {
    result.proof = check.proof || snippet(proc.stdout).split(/\r?\n/).pop();
  }

  if (!result.ok) {
    result.stdout = snippet(proc.stdout);
    result.stderr = snippet(proc.stderr);
  }

  return result;
}

const results = checks.map(runCheck);
const summary = {
  ok: results.every((result) => result.ok),
  suite: "leash-no-hardware",
  started_at: startedAt,
  finished_at: new Date().toISOString(),
  checks: results,
};

console.log(JSON.stringify(summary, null, 2));
process.exit(summary.ok ? 0 : 1);
EOF
