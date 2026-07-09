const fs = require("fs");
const http = require("http");
const path = require("path");

const PORT = Number(process.env.PORT || 8787);
const ROOT = __dirname;
const PUBLIC = path.join(ROOT, "public");
const DEFAULT_CONFIG = path.join(ROOT, "fleet.example.json");
const MOUNTED_CONFIG = "/app/config/fleet.json";
const CONFIG_PATH =
  process.env.LEASH_FLEET_CONFIG ||
  (fs.existsSync(MOUNTED_CONFIG) ? MOUNTED_CONFIG : DEFAULT_CONFIG);
const SNAPSHOT_MIN_INTERVAL_MS = Number(process.env.LEASH_OPERATOR_SNAPSHOT_MIN_INTERVAL_MS || 100);
const SNAPSHOT_ERROR_LOG_MS = Number(process.env.LEASH_OPERATOR_SNAPSHOT_ERROR_LOG_MS || 5000);
const SNAPSHOT_WARMER_ENABLED = process.env.LEASH_OPERATOR_ENABLE_SNAPSHOT_WARMER === "1";
const STREAM_IDLE_CLOSE_MS = Number(process.env.LEASH_OPERATOR_STREAM_IDLE_CLOSE_MS || 750);
const STREAM_STALL_CLOSE_MS = Number(process.env.LEASH_OPERATOR_STREAM_STALL_CLOSE_MS || 6000);

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".js": "application/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
};

const FLEET_KEYS = new Set(["fleetName", "pollMs", "snapshotMs", "robots"]);
const ROBOT_KEYS = new Set([
  "id",
  "name",
  "role",
  "baseUrl",
  "location",
  "notes",
  "videoTransport",
]);
const VIDEO_TRANSPORTS = new Set(["auto", "mjpeg", "webrtc"]);

function loadFleet(configPath = CONFIG_PATH) {
  let config;
  try {
    config = JSON.parse(fs.readFileSync(configPath, "utf8"));
  } catch (error) {
    throw new Error(`fleet config '${configPath}' could not be read: ${error.message}`);
  }
  const errors = validateFleetConfig(config);
  if (errors.length > 0) {
    throw new Error(`fleet config '${configPath}' is invalid:\n- ${errors.join("\n- ")}`);
  }
  return {
    fleetName: config.fleetName || "Leash Fleet",
    pollMs: Number(config.pollMs || 2500),
    snapshotMs: Number(config.snapshotMs || 3000),
    robots: config.robots.map((robot) => ({
      id: robot.id.trim(),
      name: (robot.name || robot.id).trim(),
      role: (robot.role || "robot").trim(),
      baseUrl: robot.baseUrl.trim().replace(/\/+$/, ""),
      location: robot.location?.trim() || "",
      notes: robot.notes?.trim() || "",
      videoTransport: robot.videoTransport || "auto",
    })),
  };
}

function validateFleetConfig(config) {
  const errors = [];
  if (!config || typeof config !== "object" || Array.isArray(config)) {
    return ["root: expected an object"];
  }
  rejectUnknownKeys(config, FLEET_KEYS, "root", errors);
  optionalNonEmptyString(config, "fleetName", "fleetName", errors);
  optionalInterval(config, "pollMs", errors);
  optionalInterval(config, "snapshotMs", errors);
  if (!Array.isArray(config.robots) || config.robots.length === 0) {
    errors.push("robots: expected a non-empty array");
    return errors;
  }

  const ids = new Set();
  config.robots.forEach((robot, index) => {
    const prefix = `robots[${index}]`;
    if (!robot || typeof robot !== "object" || Array.isArray(robot)) {
      errors.push(`${prefix}: expected an object`);
      return;
    }
    rejectUnknownKeys(robot, ROBOT_KEYS, prefix, errors);
    requiredNonEmptyString(robot, "id", `${prefix}.id`, errors);
    requiredNonEmptyString(robot, "baseUrl", `${prefix}.baseUrl`, errors);
    for (const key of ["name", "role", "location", "notes"]) {
      optionalNonEmptyString(robot, key, `${prefix}.${key}`, errors);
    }
    if (typeof robot.id === "string" && robot.id.trim()) {
      const id = robot.id.trim();
      if (ids.has(id)) errors.push(`${prefix}.id: duplicate robot id '${id}'`);
      ids.add(id);
    }
    if (robot.videoTransport != null && !VIDEO_TRANSPORTS.has(robot.videoTransport)) {
      errors.push(
        `${prefix}.videoTransport: expected one of ${[...VIDEO_TRANSPORTS].join(", ")}`,
      );
    }
    if (typeof robot.baseUrl === "string" && robot.baseUrl.trim()) {
      validateBaseUrl(robot.baseUrl, `${prefix}.baseUrl`, errors);
    }
  });
  return errors;
}

function rejectUnknownKeys(object, allowed, prefix, errors) {
  for (const key of Object.keys(object)) {
    if (!allowed.has(key)) errors.push(`${prefix}.${key}: unknown field`);
  }
}

function requiredNonEmptyString(object, key, pathName, errors) {
  if (typeof object[key] !== "string" || !object[key].trim()) {
    errors.push(`${pathName}: expected a non-empty string`);
  }
}

function optionalNonEmptyString(object, key, pathName, errors) {
  if (object[key] != null && (typeof object[key] !== "string" || !object[key].trim())) {
    errors.push(`${pathName}: expected a non-empty string when provided`);
  }
}

function optionalInterval(object, key, errors) {
  if (
    object[key] != null &&
    (!Number.isInteger(object[key]) || object[key] < 100 || object[key] > 60_000)
  ) {
    errors.push(`${key}: expected an integer from 100 through 60000 milliseconds`);
  }
}

function validateBaseUrl(value, pathName, errors) {
  try {
    const url = new URL(value);
    if (!["http:", "https:"].includes(url.protocol)) {
      errors.push(`${pathName}: expected an http or https URL`);
    }
    if (url.username || url.password) errors.push(`${pathName}: credentials are not allowed`);
    if (url.pathname !== "/" || url.search || url.hash) {
      errors.push(`${pathName}: expected an origin URL without a path, query, or fragment`);
    }
  } catch (_error) {
    errors.push(`${pathName}: expected a valid absolute URL`);
  }
}

function json(response, status, payload) {
  const body = JSON.stringify(payload);
  response.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
  });
  response.end(body);
}

function text(response, status, body) {
  response.writeHead(status, {
    "content-type": "text/plain; charset=utf-8",
    "cache-control": "no-store",
  });
  response.end(body);
}

function robotById(id) {
  return loadFleet().robots.find((robot) => robot.id === id);
}

const snapshotCache = new Map();
const streamRelays = new Map();

function snapshotEntry(robot) {
  if (!snapshotCache.has(robot.id)) {
    snapshotCache.set(robot.id, {
      image: null,
      contentType: "image/jpeg",
      updatedAt: 0,
      seq: 0,
      inflight: null,
      lastErrorAt: 0,
      lastError: null,
      streamBuffer: Buffer.alloc(0),
    });
  }
  return snapshotCache.get(robot.id);
}

function readBody(request) {
  return new Promise((resolve, reject) => {
    let body = "";
    request.setEncoding("utf8");
    request.on("data", (chunk) => {
      body += chunk;
      if (body.length > 64 * 1024) {
        request.destroy();
        reject(new Error("request body too large"));
      }
    });
    request.on("end", () => {
      if (!body.trim()) {
        resolve({});
        return;
      }
      try {
        resolve(JSON.parse(body));
      } catch (error) {
        reject(error);
      }
    });
    request.on("error", reject);
  });
}

async function robotFetch(robot, route, options = {}) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 8000);
  try {
    return await fetch(`${robot.baseUrl}${route}`, {
      ...options,
      signal: controller.signal,
      headers: {
        ...(options.body ? { "content-type": "application/json" } : {}),
        ...(options.headers || {}),
      },
    });
  } finally {
    clearTimeout(timeout);
  }
}

function streamRelay(robot) {
  if (!streamRelays.has(robot.id)) {
    streamRelays.set(robot.id, {
      robot,
      clients: new Set(),
      contentType: null,
      controller: null,
      running: false,
      idleTimer: null,
      stalled: false,
    });
  }
  const relay = streamRelays.get(robot.id);
  relay.robot = robot;
  return relay;
}

function writeStreamHeaders(client, contentType) {
  if (client.headersSent || client.response.destroyed) return;
  client.response.writeHead(200, {
    "content-type": contentType || "multipart/x-mixed-replace; boundary=leashframe",
    "cache-control": "no-store, no-cache, must-revalidate",
  });
  client.headersSent = true;
}

function closeRelayClients(relay, status, message) {
  for (const client of relay.clients) {
    if (client.response.destroyed) continue;
    if (client.headersSent) {
      client.response.end();
    } else {
      text(client.response, status, message);
    }
  }
  relay.clients.clear();
}

function scheduleRelayClose(relay) {
  if (relay.clients.size > 0 || relay.idleTimer) return;
  relay.idleTimer = setTimeout(() => {
    relay.idleTimer = null;
    if (relay.clients.size === 0 && relay.controller) {
      relay.controller.abort();
    }
  }, STREAM_IDLE_CLOSE_MS);
  relay.idleTimer.unref?.();
}

function resetCameraRelay(robot) {
  const relay = streamRelays.get(robot.id);
  if (relay) {
    if (relay.idleTimer) {
      clearTimeout(relay.idleTimer);
      relay.idleTimer = null;
    }
    if (relay.controller) {
      relay.controller.abort();
    }
    for (const client of relay.clients) {
      if (!client.response.destroyed) client.response.end();
    }
    relay.clients.clear();
    relay.contentType = null;
    relay.running = false;
  }
  const entry = snapshotCache.get(robot.id);
  if (entry) {
    entry.inflight = null;
    entry.streamBuffer = Buffer.alloc(0);
  }
}

function rememberStreamChunk(robot, chunk) {
  const entry = snapshotEntry(robot);
  const incoming = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
  let buffer = entry.streamBuffer.length
    ? Buffer.concat([entry.streamBuffer, incoming])
    : incoming;
  if (buffer.length > 2 * 1024 * 1024) {
    buffer = buffer.subarray(buffer.length - 2 * 1024 * 1024);
  }

  const start = buffer.indexOf(Buffer.from([0xff, 0xd8]));
  if (start < 0) {
    entry.streamBuffer = buffer;
    return;
  }
  const end = buffer.indexOf(Buffer.from([0xff, 0xd9]), start + 2);
  if (end < 0) {
    entry.streamBuffer = buffer.subarray(start);
    return;
  }

  const image = buffer.subarray(start, end + 2);
  if (image.length > 512) {
    entry.image = Buffer.from(image);
    entry.contentType = "image/jpeg";
    entry.updatedAt = Date.now();
    entry.seq += 1;
    entry.lastError = null;
  }
  entry.streamBuffer = buffer.subarray(end + 2);
}

async function ensureRelayRunning(relay) {
  if (relay.running) return;
  relay.running = true;
  relay.controller = new AbortController();
  relay.stalled = false;
  let stallTimer = null;
  const armStallTimer = () => {
    if (stallTimer) clearTimeout(stallTimer);
    if (STREAM_STALL_CLOSE_MS <= 0 || relay.clients.size === 0) return;
    stallTimer = setTimeout(() => {
      relay.stalled = true;
      relay.controller?.abort();
    }, STREAM_STALL_CLOSE_MS);
    stallTimer.unref?.();
  };
  try {
    const upstream = await fetch(`${relay.robot.baseUrl}/camera/stream.mjpg`, {
      signal: relay.controller.signal,
    });
    if (!upstream.ok) {
      const body = await upstream.text();
      closeRelayClients(
        relay,
        upstream.status,
        body || `robot '${relay.robot.id}' stream returned HTTP ${upstream.status}`,
      );
      return;
    }
    relay.contentType =
      upstream.headers.get("content-type") ||
      "multipart/x-mixed-replace; boundary=leashframe";
    for (const client of relay.clients) {
      writeStreamHeaders(client, relay.contentType);
    }
    armStallTimer();
    for await (const chunk of upstream.body) {
      armStallTimer();
      rememberStreamChunk(relay.robot, chunk);
      for (const client of [...relay.clients]) {
        if (client.response.destroyed) {
          relay.clients.delete(client);
          continue;
        }
        writeStreamHeaders(client, relay.contentType);
        client.response.write(chunk);
      }
      if (relay.clients.size === 0) {
        relay.controller.abort();
        break;
      }
    }
    for (const client of relay.clients) {
      if (!client.response.destroyed) client.response.end();
    }
    relay.clients.clear();
  } catch (error) {
    if (error.name === "AbortError" && relay.stalled) {
      closeRelayClients(relay, 504, `robot '${relay.robot.id}' stream stalled`);
    } else if (error.name !== "AbortError") {
      closeRelayClients(relay, 502, `robot '${relay.robot.id}' stream unavailable: ${error.message}`);
    }
  } finally {
    if (stallTimer) clearTimeout(stallTimer);
    relay.contentType = null;
    relay.controller = null;
    relay.running = false;
    relay.stalled = false;
  }
}

async function proxyStream(response, robot) {
  const relay = streamRelay(robot);
  if (relay.idleTimer) {
    clearTimeout(relay.idleTimer);
    relay.idleTimer = null;
  }
  const client = { response, headersSent: false };
  relay.clients.add(client);
  response.on("close", () => {
    relay.clients.delete(client);
    scheduleRelayClose(relay);
  });
  if (relay.contentType) {
    writeStreamHeaders(client, relay.contentType);
  }
  ensureRelayRunning(relay).catch((error) => {
    if (!response.destroyed) {
      text(response, 502, `robot '${robot.id}' stream unavailable: ${error.message}`);
    }
  });
}

async function proxyJson(response, robot, route, options = {}) {
  try {
    const upstream = await robotFetch(robot, route, options);
    const body = await upstream.text();
    response.writeHead(upstream.status, {
      "content-type":
        upstream.headers.get("content-type") || "application/json; charset=utf-8",
      "cache-control": "no-store",
    });
    response.end(body);
  } catch (error) {
    json(response, 502, {
      ok: false,
      error: `robot '${robot.id}' unavailable: ${error.message}`,
    });
  }
}

async function robotJson(robot, route, options = {}) {
  const upstream = await robotFetch(robot, route, options);
  const body = await upstream.text();
  let payload;
  try {
    payload = JSON.parse(body);
  } catch (error) {
    throw new Error(`${route} returned non-JSON HTTP ${upstream.status}: ${body.slice(0, 160)}`);
  }
  if (!upstream.ok || payload.ok === false) {
    throw new Error(payload.error || `${route} returned HTTP ${upstream.status}`);
  }
  return payload;
}

async function proxySnapshot(response, robot, options = {}) {
  const entry = snapshotEntry(robot);
  const ageMs = Date.now() - entry.updatedAt;
  const relay = streamRelays.get(robot.id);
  const streamActive = Boolean(relay?.running || relay?.clients.size);
  if (entry.image) {
    if (!options.cacheOnly && !streamActive && ageMs > SNAPSHOT_MIN_INTERVAL_MS) {
      refreshSnapshotCache(robot).catch(() => {});
    }
    sendSnapshot(response, entry, ageMs);
    return;
  }

  if (options.cacheOnly) {
    text(response, 503, `robot '${robot.id}' has no cached frame yet`);
    return;
  }

  if (streamActive) {
    text(response, 503, `robot '${robot.id}' stream has no cached frame yet`);
    return;
  }

  try {
    await refreshSnapshotCache(robot);
    sendSnapshot(response, snapshotEntry(robot), 0);
  } catch (error) {
    text(response, 502, `robot '${robot.id}' snapshot unavailable: ${error.message}`);
  }
}

function sendSnapshot(response, entry, ageMs) {
  response.writeHead(200, {
    "content-type": entry.contentType,
    "content-length": entry.image.length,
    "cache-control": "no-store",
    "x-leash-snapshot-age-ms": String(Math.max(0, Math.round(ageMs))),
    "x-leash-snapshot-seq": String(entry.seq),
    "x-leash-snapshot-refreshing": String(Boolean(entry.inflight)),
  });
  response.end(entry.image);
}

async function refreshSnapshotCache(robot) {
  const entry = snapshotEntry(robot);
  if (entry.inflight) {
    return entry.inflight;
  }

  entry.inflight = (async () => {
    try {
      const upstream = await robotFetch(robot, "/camera/snapshot");
      const body = Buffer.from(await upstream.arrayBuffer());
      if (!upstream.ok || body.length === 0) {
        throw new Error(body.toString("utf8").slice(0, 240) || `HTTP ${upstream.status}`);
      }
      entry.image = body;
      entry.contentType = upstream.headers.get("content-type") || "image/jpeg";
      entry.updatedAt = Date.now();
      entry.seq += 1;
      entry.lastError = null;
    } catch (error) {
      entry.lastError = error;
      const now = Date.now();
      if (now - entry.lastErrorAt > SNAPSHOT_ERROR_LOG_MS) {
        entry.lastErrorAt = now;
        console.warn(`snapshot refresh failed for ${robot.id}: ${error.message}`);
      }
      throw error;
    } finally {
      entry.inflight = null;
    }
  })();

  return entry.inflight;
}

function startSnapshotWarmers() {
  const fleet = loadFleet();
  const delayMs = Math.max(0, Number(process.env.LEASH_OPERATOR_SNAPSHOT_PUMP_DELAY_MS || 25));
  for (const robot of fleet.robots) {
    pumpSnapshot(robot, delayMs);
  }
}

function pumpSnapshot(robot, delayMs) {
  refreshSnapshotCache(robot)
    .catch(() => {})
    .finally(() => {
      const timeout = setTimeout(() => pumpSnapshot(robot, delayMs), delayMs);
      timeout.unref?.();
    });
}

function serveStatic(response, urlPath) {
  const requested = urlPath === "/" ? "/index.html" : urlPath;
  const filePath = path.normalize(path.join(PUBLIC, requested));
  if (!filePath.startsWith(PUBLIC)) {
    text(response, 403, "forbidden");
    return;
  }
  fs.readFile(filePath, (error, data) => {
    if (error) {
      text(response, 404, "not found");
      return;
    }
    response.writeHead(200, {
      "content-type": MIME[path.extname(filePath)] || "application/octet-stream",
      "cache-control": "no-store",
    });
    response.end(data);
  });
}

async function handleApi(request, response, url) {
  if (request.method === "GET" && url.pathname === "/api/fleet") {
    json(response, 200, { ok: true, ...loadFleet() });
    return;
  }

  const match = url.pathname.match(/^\/api\/robots\/([^/]+)\/([^/]+)$/);
  if (!match) {
    json(response, 404, { ok: false, error: "unknown api route" });
    return;
  }

  const robot = robotById(decodeURIComponent(match[1]));
  const action = match[2];
  if (!robot) {
    json(response, 404, { ok: false, error: "unknown robot" });
    return;
  }

  if (request.method === "GET" && action === "health") {
    await proxyJson(response, robot, "/health");
    return;
  }
  if (request.method === "GET" && action === "telemetry") {
    await proxyJson(response, robot, "/telemetry");
    return;
  }
  if (request.method === "GET" && action === "camera") {
    await proxyJson(response, robot, "/camera/status");
    return;
  }
  if (request.method === "GET" && action === "camera-health") {
    await proxyJson(response, robot, "/camera/stream/health");
    return;
  }
  if (request.method === "GET" && action === "summary") {
    try {
      const [health, telemetry, camera, zones, patrol] = await Promise.all([
        robotJson(robot, "/health"),
        robotJson(robot, "/telemetry"),
        robotJson(robot, "/camera/status"),
        robotJson(robot, "/patrol/zones").catch((error) => ({
          ok: false,
          count: 0,
          zones: [],
          error: error.message,
        })),
        robotJson(robot, "/patrol/status").catch((error) => ({
          ok: false,
          active: false,
          status: "unavailable",
          error: error.message,
        })),
      ]);
      json(response, 200, { ok: true, health, telemetry, camera, zones, patrol });
    } catch (error) {
      json(response, 502, {
        ok: false,
        error: `robot '${robot.id}' unavailable: ${error.message}`,
      });
    }
    return;
  }
  if (request.method === "GET" && action === "snapshot") {
    await proxySnapshot(response, robot, {
      cacheOnly: url.searchParams.get("cache") === "1",
    });
    return;
  }
  if (request.method === "GET" && action === "stream.mjpg") {
    if (robot.videoTransport === "webrtc") {
      json(response, 409, {
        ok: false,
        error: "mjpeg stream disabled for this robot",
      });
      return;
    }
    await proxyStream(response, robot);
    return;
  }

  if (request.method !== "POST") {
    json(response, 405, { ok: false, error: "method not allowed" });
    return;
  }

  const body = await readBody(request);
  const post = (route, payload = body) =>
    proxyJson(response, robot, route, {
      method: "POST",
      body: JSON.stringify(payload),
    });

  if (action === "authorize") {
    await post("/pilot/authorize");
  } else if (action === "aim") {
    await post("/camera/aim");
  } else if (action === "camera-refresh") {
    resetCameraRelay(robot);
    await post("/camera/stream/recover", {});
  } else if (action === "drive") {
    await post("/drive");
  } else if (action === "patrol-start") {
    if (!body.zone_id || typeof body.zone_id !== "string") {
      json(response, 400, { ok: false, error: "zone_id is required" });
      return;
    }
    await post(`/patrol/zones/${encodeURIComponent(body.zone_id)}/start`, {
      speed_mode: body.speed_mode || "low",
    });
  } else if (action === "patrol-stop") {
    await post("/patrol/stop", {});
  } else if (action === "stop") {
    await post("/stop", {});
  } else if (action === "estop") {
    await post("/estop", {});
  } else if (action === "estop-reset") {
    await post("/estop/reset");
  } else {
    json(response, 404, { ok: false, error: "unknown robot action" });
  }
}

function startServer() {
  loadFleet();
  const server = http.createServer(async (request, response) => {
    const url = new URL(request.url, `http://${request.headers.host || "localhost"}`);
    try {
      if (url.pathname.startsWith("/api/")) {
        await handleApi(request, response, url);
      } else {
        serveStatic(response, url.pathname);
      }
    } catch (error) {
      json(response, 500, { ok: false, error: error.message });
    }
  });

  server.listen(PORT, "0.0.0.0", () => {
    console.log(`Leash operator listening on http://0.0.0.0:${PORT}`);
    console.log(`Fleet config: ${CONFIG_PATH}`);
    if (SNAPSHOT_WARMER_ENABLED) startSnapshotWarmers();
  });
  return server;
}

function checkConfigFromCli() {
  const index = process.argv.indexOf("--check-config");
  if (index < 0) return false;
  const requested = process.argv[index + 1];
  if (!requested) throw new Error("--check-config requires a JSON file path");
  const configPath = path.resolve(process.cwd(), requested);
  const fleet = loadFleet(configPath);
  console.log(JSON.stringify({ ok: true, configPath, robots: fleet.robots.length }));
  return true;
}

if (require.main === module) {
  try {
    if (!checkConfigFromCli()) startServer();
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}

module.exports = { loadFleet, startServer, validateFleetConfig };
