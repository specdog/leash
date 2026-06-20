const baseUrl = (process.env.LEASH_URL || "http://127.0.0.1:8000").replace(/\/$/, "");

async function requestJson(method, path) {
  const response = await fetch(`${baseUrl}${path}`, { method });
  if (!response.ok) {
    throw new Error(`${method} ${path} returned HTTP ${response.status}`);
  }
  return response.json();
}

const health = await requestJson("GET", "/health");
const telemetry = await requestJson("GET", "/telemetry");
const stop = await requestJson("POST", "/stop");

if (health.ok !== true) {
  throw new Error("health did not report ok=true");
}
if (!telemetry.robot || telemetry.profile !== "sim") {
  throw new Error("telemetry did not look like a sim frame");
}
if (stop.ok !== true) {
  throw new Error("stop did not report ok=true");
}

console.log(JSON.stringify({
  ok: true,
  runtime: "node",
  profile: health.profile,
  robot: telemetry.robot,
  stop,
}));
