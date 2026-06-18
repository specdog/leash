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
  "LEASH_ALLOW_UNTOKENED_DRIVE",
  "LEASH_CONFIG",
  "LEASH_LISTEN",
  "LEASH_PROFILE",
  "LEASH_REQUIRE_ACCELERATOR",
  "LEASH_ROLE",
]) {
  delete baseEnv[key];
}

const checks = [
  {
    name: "http-routes-and-policy",
    argv: ["bash", "scripts/smoke-http.sh"],
    proof: "HTTP routes, capture, authorized drive, and drive-denial policy passed",
  },
  {
    name: "mcp-stdio",
    argv: ["bash", "scripts/smoke-mcp.sh"],
    proof: "stdio MCP initialization, tool list, and health call passed",
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
      for (const required of ["sim-http", "sim-mcp", "bridge-compat-http", "waveshare-ugv-http"]) {
        if (!names.includes(required)) {
          throw new Error(`missing stack ${required}`);
        }
      }
      const physical = stacks.find((stack) => stack.name === "waveshare-ugv-http");
      if (!physical.hardware_required) {
        throw new Error("waveshare stack did not declare hardware_required");
      }
      return `listed ${stacks.length} built-in stacks`;
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
    env: baseEnv,
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
