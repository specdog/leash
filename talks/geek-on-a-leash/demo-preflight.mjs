#!/usr/bin/env node

import fs from "node:fs/promises";

function parseArgs(argv) {
  const options = {
    baseUrl: process.env.LEASH_BASE_URL || "http://127.0.0.1:8000",
    motion: false,
    json: false,
    output: null,
    timeoutMs: 4000,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--base-url") options.baseUrl = argv[++index];
    else if (argument === "--motion") options.motion = true;
    else if (argument === "--json") options.json = true;
    else if (argument === "--output") options.output = argv[++index];
    else if (argument === "--timeout-ms") options.timeoutMs = Number(argv[++index]);
    else if (argument === "--help") {
      console.log(`Usage: node demo-preflight.mjs [options]

Read-only Pinkie/Leash presentation preflight.

  --base-url URL    Leash HTTP base URL (default: LEASH_BASE_URL or localhost)
  --motion          Require an active operator token in addition to observation gates
  --json            Print the structured report as JSON
  --output FILE     Write the redacted structured report to FILE
  --timeout-ms N    Per-request timeout in milliseconds (default: 4000)`);
      process.exit(0);
    } else throw new Error(`Unknown argument: ${argument}`);
  }
  if (!Number.isFinite(options.timeoutMs) || options.timeoutMs <= 0) {
    throw new Error("--timeout-ms must be a positive number");
  }
  return options;
}

async function fetchJson(baseUrl, route, timeoutMs) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(new URL(route, `${baseUrl.replace(/\/$/, "")}/`), {
      headers: { Accept: "application/json" },
      signal: controller.signal,
    });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return await response.json();
  } finally {
    clearTimeout(timer);
  }
}

function gate(id, label, ok, detail, severity = "required") {
  return { id, label, ok: Boolean(ok), severity, detail };
}

function ageMs(timestamp) {
  if (!Number.isFinite(timestamp)) return null;
  return Math.max(0, Date.now() - timestamp);
}

function summarize(options, payloads, transport = {}) {
  const { health, sensors, runtime, mapping, visualOdometry } = payloads;
  const sensorState = sensors?.sensors || {};
  const lidarAge = ageMs(sensorState.range_scan?.last_ms);
  const imuAge = ageMs(sensorState.imu?.last_ms);
  const failures = runtime?.supervisor?.metrics || {};
  const evidence = runtime?.supervisor?.evidence || {};
  const gates = [
    gate("service", "Leash live service", health?.ok && health?.mode === "live" && !health?.replay, health ? `${health.role || "unknown"} / ${health.profile || "unknown"}` : (transport.health || "unavailable")),
    gate("estop", "E-stop clear", health?.estop === false, health ? (health.estop ? "latched" : "clear") : "unavailable"),
    gate("deadman", "Deadman healthy", health?.deadman_ok === true, health ? (health.deadman_ok ? "healthy" : "stale") : "unavailable"),
    gate("actuation", "Physical actuation gate", health?.physical_actuation_enabled === true, health ? (health.physical_actuation_enabled ? "enabled" : "locked") : "unavailable"),
    gate("controller", "Controller owner connected", runtime?.available && runtime?.controller?.connected && runtime?.motor_authority === "waveshare-controller-owner", runtime?.motor_authority || "unavailable"),
    gate("authority", "CPU remains final authority", runtime?.cpu_final_authority === true && runtime?.control_authority === "cpu-safety-supervisor", runtime?.control_authority || "unavailable"),
    gate("supervisor", "Safety supervisor healthy", runtime?.supervisor?.faulted === false && runtime?.supervisor?.closed === false, runtime ? (runtime.supervisor?.last_fault || "healthy") : "unavailable"),
    gate("evidence", "Evidence journal healthy", evidence.healthy === true && evidence.storage_full === false && evidence.saturated === false && !evidence.writer_fault, runtime ? `${evidence.durable_records ?? 0} durable records` : "unavailable"),
    gate("failures", "No safety-path failures", Boolean(runtime) && [failures.acknowledgement_failures, failures.evidence_failures, failures.faults, failures.worker_panics, runtime?.controller?.metrics?.write_failures].every((value) => Number(value || 0) === 0), runtime ? "ack/evidence/fault/panic/write counters are zero" : "unavailable"),
    gate("camera", "Camera fresh", sensorState.camera?.status === "available" && sensorState.camera?.health === "healthy", sensorState.camera?.health || sensorState.camera?.status || "unavailable"),
    gate("lidar", "LiDAR fresh", sensorState.range_scan?.status === "available" && lidarAge !== null && lidarAge < 2000, lidarAge === null ? "no timestamp" : `${lidarAge} ms old`),
    gate("imu", "IMU fresh", sensorState.imu?.status === "available" && imuAge !== null && imuAge < 2000, imuAge === null ? "no timestamp" : `${imuAge} ms old`),
    gate("odometry", "Odometry available", sensorState.odometry?.status === "available", sensorState.odometry?.status || "unavailable"),
    gate("battery", "Battery above 30%", Number(sensorState.battery?.level_pct || 0) >= 30, `${sensorState.battery?.level_pct ?? "?"}%`),
    gate("cuda", "CUDA probe active", health?.accelerator?.active === "cuda" && health?.accelerator?.available === true, health?.accelerator?.message || "unavailable"),
  ];
  if (options.motion) {
    gates.push(gate("operator-token", "Operator token active", health?.operator_token?.active === true, health?.operator_token?.active ? `${health.operator_token.owner_id || "owner"} / ${health.operator_token.speed_mode || "bounded"}` : "authorize deliberately before motion"));
  }
  gates.push(
    gate("mapping", "Mapping state", mapping?.ok === true, mapping?.message || mapping?.state || "unavailable", "advisory"),
    gate("visual-odometry", "Visual odometry", visualOdometry?.ok === true, visualOdometry?.message || visualOdometry?.state || "unavailable", "advisory"),
  );
  const required = gates.filter((item) => item.severity === "required");
  return {
    schema_version: "leash.talk-preflight.v1",
    captured_at: new Date().toISOString(),
    mode: options.motion ? "motion" : "observe",
    role: health?.role || null,
    profile: health?.profile || null,
    ready: required.every((item) => item.ok),
    gates,
    transport,
    snapshot: {
      battery_pct: sensorState.battery?.level_pct ?? null,
      camera: sensorState.camera?.health ?? sensorState.camera?.status ?? null,
      lidar_age_ms: lidarAge,
      imu_age_ms: imuAge,
      accelerator: health?.accelerator?.active ?? null,
      durable_evidence_records: evidence.durable_records ?? null,
      mapping_state: mapping?.state ?? null,
      visual_odometry_state: visualOdometry?.state ?? null,
    },
  };
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  const routes = {
    health: "/health",
    sensors: "/sensors",
    runtime: "/runtime-v2/status",
    mapping: "/mapping/status",
    visualOdometry: "/visual-odometry",
  };
  const entries = await Promise.all(
    Object.entries(routes).map(async ([name, route]) => {
      try {
        return [name, { ok: true, value: await fetchJson(options.baseUrl, route, options.timeoutMs) }];
      } catch (error) {
        return [name, { ok: false, error: error instanceof Error ? error.message : String(error) }];
      }
    }),
  );
  const transport = Object.fromEntries(entries.map(([name, result]) => [name, result.ok ? "ok" : result.error]));
  const payloads = Object.fromEntries(entries.map(([name, result]) => [name, result.ok ? result.value : null]));
  const report = summarize(options, payloads, transport);
  if (options.output) {
    await fs.writeFile(options.output, `${JSON.stringify(report, null, 2)}\n`, { mode: 0o644 });
  }
  if (options.json) {
    console.log(JSON.stringify(report, null, 2));
  } else {
    for (const item of report.gates) {
      const marker = item.severity === "advisory" ? (item.ok ? "INFO" : "WARN") : (item.ok ? "PASS" : "FAIL");
      console.log(`${marker.padEnd(4)}  ${item.label.padEnd(31)} ${item.detail}`);
    }
    console.log(`\n${report.ready ? "READY" : "BLOCKED"} for ${report.mode} presentation flow`);
  }
  process.exitCode = report.ready ? 0 : 1;
}

main().catch((error) => {
  console.error(`Preflight failed: ${error.message}`);
  process.exitCode = 2;
});
