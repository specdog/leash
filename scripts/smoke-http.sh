#!/usr/bin/env bash
set -euo pipefail

port="${LEASH_SMOKE_PORT:-18080}"
base="http://127.0.0.1:$port"
log_file="$(mktemp -t leash-http-smoke.XXXXXX.log)"
policy_response="$(mktemp -t leash-http-policy.XXXXXX.json)"
timeout_secs="${LEASH_SMOKE_TIMEOUT_SECS:-60}"

cleanup() {
  if [[ -n "${server_pid:-}" ]] && kill -0 "$server_pid" 2>/dev/null; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -f "$log_file" "$policy_response"
}
trap cleanup EXIT

cargo run --quiet -- serve http --profile sim --no-untokened-drive --listen "127.0.0.1:$port" >"$log_file" 2>&1 &
server_pid=$!

ready=false
for _ in $(seq 1 $((timeout_secs * 10))); do
  if curl -fsS "$base/health" >/dev/null 2>&1; then
    ready=true
    break
  fi
  if ! kill -0 "$server_pid" 2>/dev/null; then
    echo "http smoke server exited before readiness" >&2
    cat "$log_file" >&2
    exit 1
  fi
  sleep 0.1
done

if [[ "$ready" != true ]]; then
  echo "http smoke server was not ready after ${timeout_secs}s" >&2
  cat "$log_file" >&2
  exit 1
fi

parse_json() {
  node -e 'JSON.parse(require("node:fs").readFileSync(0, "utf8"))' >/dev/null
}

assert_capabilities_streams() {
  node -e 'const payload = JSON.parse(require("node:fs").readFileSync(0, "utf8"));
for (const endpoint of ["WS /ws/telemetry", "GET /events/telemetry", "GET /sse/telemetry"]) {
  if (!payload.endpoints.includes(endpoint)) throw new Error(`missing endpoint: ${endpoint}`);
}
for (const endpoint of ["GET /", "GET /dashboard"]) {
  if (!payload.endpoints.includes(endpoint)) throw new Error(`missing dashboard endpoint: ${endpoint}`);
}
for (const endpoint of ["POST /dashboard/authorize", "POST /dashboard/stop", "POST /dashboard/estop", "POST /dashboard/estop-reset", "POST /dashboard/capture"]) {
  if (!payload.endpoints.includes(endpoint)) throw new Error(`missing dashboard action endpoint: ${endpoint}`);
}
for (const endpoint of ["GET /agent", "GET /agent/messages", "POST /agent/messages"]) {
  if (!payload.endpoints.includes(endpoint)) throw new Error(`missing agent endpoint: ${endpoint}`);
}'
}

assert_health_modules() {
  node -e 'const payload = JSON.parse(require("node:fs").readFileSync(0, "utf8"));
if (payload.ok !== true) throw new Error("health ok was not true");
if (!Array.isArray(payload.modules) || payload.modules.length < 3) throw new Error("health modules were missing");
if (!payload.modules.every((module) => module.state === "running")) throw new Error("not all modules were running");'
}

assert_policy_denial() {
  node -e 'const payload = JSON.parse(require("node:fs").readFileSync(0, "utf8"));
if (payload.ok !== false) throw new Error("policy denial did not return ok=false");
  if (!String(payload.error || "").includes("require-token")) throw new Error(`unexpected policy error: ${payload.error}`);'
}

assert_drive_outcome() {
  node -e 'const payload = JSON.parse(require("node:fs").readFileSync(0, "utf8"));
if (payload.ok !== true) throw new Error("drive outcome ok was not true");
if (payload.left <= 0 || payload.right <= 0) throw new Error("drive outcome did not move in simulation");
if (payload.speed_mode !== "low") throw new Error(`unexpected speed mode: ${payload.speed_mode}`);'
}

assert_stream_frame() {
  node -e 'const payload = JSON.parse(require("node:fs").readFileSync(0, "utf8"));
if (payload.kind !== "telemetry") throw new Error(`unexpected stream kind: ${payload.kind}`);
if (!payload.telemetry || payload.telemetry.profile !== "sim") throw new Error("stream telemetry payload was missing");
if (!payload.health || !Array.isArray(payload.health.modules)) throw new Error("stream health modules were missing");
if (!payload.command || typeof payload.command.left_cmd !== "number") throw new Error("stream command state was missing");
if (!payload.safety || payload.safety.deadman_ok !== true) throw new Error("stream safety state was missing");
if (!payload.visualization || payload.visualization.version !== "leash-visualization-v1") {
  throw new Error("stream visualization frame was missing");
}
if (!payload.visualization.pose || payload.visualization.pose.frame_id !== "map") {
  throw new Error("stream visualization pose was missing");
}
if (typeof payload.visualization.pose.ts_ms !== "number") {
  throw new Error("stream visualization pose timestamp was missing");
}
if (!payload.visualization.twist || payload.visualization.twist.frame_id !== "base_link") {
  throw new Error("stream visualization twist was missing");
}
if (!payload.visualization.path || !Array.isArray(payload.visualization.path.poses)) {
  throw new Error("stream visualization path was missing");
}
if (typeof payload.visualization.path.ts_ms !== "number") {
  throw new Error("stream visualization path timestamp was missing");
}
if (!payload.visualization.map || payload.visualization.map.frame_id !== "map") {
  throw new Error("stream visualization map metadata was missing");
}
if (payload.visualization.map.cell_order !== "row-major") {
  throw new Error(`unexpected visualization map cell order: ${payload.visualization.map.cell_order}`);
}
if (!payload.visualization.occupancy_grid || !Array.isArray(payload.visualization.occupancy_grid.cells)) {
  throw new Error("stream visualization occupancy grid was missing");
}
if (!payload.visualization.occupancy_grid.metadata || payload.visualization.occupancy_grid.metadata.frame_id !== "map") {
  throw new Error("stream visualization occupancy metadata was missing");
}
if (!payload.visualization.costmap || !Array.isArray(payload.visualization.costmap.costs)) {
  throw new Error("stream visualization costmap was missing");
}
if (!payload.visualization.costmap.metadata || payload.visualization.costmap.metadata.frame_id !== "map") {
  throw new Error("stream visualization costmap metadata was missing");
}
if (!payload.visualization.command || typeof payload.visualization.command.left_cmd !== "number") {
  throw new Error("stream visualization command overlay was missing");
}'
}

assert_agent_message() {
  local expected_source="$1"
  local expected_text="$2"
  EXPECT_SOURCE="$expected_source" EXPECT_TEXT="$expected_text" node -e 'const payload = JSON.parse(require("node:fs").readFileSync(0, "utf8"));
if (payload.ok !== true) throw new Error("agent message ok was not true");
if (!payload.message || payload.message.source !== process.env.EXPECT_SOURCE) {
  throw new Error(`unexpected agent source: ${payload.message && payload.message.source}`);
}
if (payload.message.text !== process.env.EXPECT_TEXT) {
  throw new Error(`unexpected agent text: ${payload.message.text}`);
}
if (!Number.isInteger(payload.message.id) || payload.message.id < 1) {
  throw new Error("agent message id was missing");
}
if (!payload.response || payload.response.provider !== "deterministic-test") {
  throw new Error("deterministic agent response was missing");
}
if (!String(payload.response.text || "").includes(process.env.EXPECT_TEXT)) {
  throw new Error(`unexpected deterministic agent response: ${payload.response.text}`);
}'
}

assert_agent_messages_include() {
  local expected_text="$1"
  EXPECT_TEXT="$expected_text" node -e 'const payload = JSON.parse(require("node:fs").readFileSync(0, "utf8"));
if (payload.ok !== true) throw new Error("agent message list ok was not true");
if (!Array.isArray(payload.messages)) throw new Error("agent messages was not an array");
if (!payload.messages.some((message) => message.text === process.env.EXPECT_TEXT)) {
  throw new Error(`agent message list did not include: ${process.env.EXPECT_TEXT}`);
}'
}

assert_dashboard_page() {
  local html
  html="$(cat)"
  for text in "Leash Command Center" "Health" "Telemetry" "Modules" "Capabilities" "Logs Tail"; do
    if [[ "$html" != *"$text"* ]]; then
      echo "dashboard missing $text" >&2
      return 1
    fi
  done
  for path in "/dashboard/authorize" "/dashboard/stop" "/dashboard/estop" "/dashboard/estop-reset" "/dashboard/capture"; do
    if [[ "$html" != *"$path"* ]]; then
      echo "dashboard missing form action $path" >&2
      return 1
    fi
  done
  if [[ "$html" == *"<script"* ]]; then
    echo "dashboard should not include script tags" >&2
    return 1
  fi
}

dashboard_ts() {
  sed -n 's/.*data-telemetry-ts="\([0-9][0-9]*\)".*/\1/p' | head -n 1
}

assert_dashboard_updates() {
  local first_ts second_ts
  first_ts="$(curl -fsS "$base/dashboard" | dashboard_ts)"
  sleep 1
  second_ts="$(curl -fsS "$base/dashboard" | dashboard_ts)"
  if [[ -z "$first_ts" || -z "$second_ts" || "$first_ts" == "$second_ts" ]]; then
    echo "dashboard telemetry timestamp did not update: '$first_ts' -> '$second_ts'" >&2
    return 1
  fi
}

post_dashboard_action() {
  local action="$1"
  curl -fsS -L "$base/dashboard/$action" \
    -H "content-type: application/x-www-form-urlencoded" \
    --data 'token=smoke-token&ttl_secs=30&speed_mode=low&approval=true' | assert_dashboard_page
}

assert_ws_telemetry() {
  LEASH_WS_URL="ws://127.0.0.1:$port/ws/telemetry" node <<'NODE' | assert_stream_frame
const url = process.env.LEASH_WS_URL;
const timeout = setTimeout(() => {
  console.error("timed out waiting for websocket telemetry");
  process.exit(1);
}, 5000);

const ws = new WebSocket(url);
ws.onmessage = (event) => {
  clearTimeout(timeout);
  console.log(event.data);
  ws.close();
};
ws.onerror = (event) => {
  clearTimeout(timeout);
  console.error("websocket telemetry failed", event.message || event.type || event);
  process.exit(1);
};
NODE
}

assert_sse_telemetry() {
  LEASH_SSE_URL="$base/events/telemetry" node <<'NODE' | assert_stream_frame
(async () => {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 5000);
  const response = await fetch(process.env.LEASH_SSE_URL, {
    signal: controller.signal,
    headers: { accept: "text/event-stream" },
  });
  if (!response.ok) throw new Error(`SSE HTTP ${response.status}`);
  if (!String(response.headers.get("content-type") || "").includes("text/event-stream")) {
    throw new Error("SSE content-type was not text/event-stream");
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let text = "";
  while (!text.includes("\n\n")) {
    const { done, value } = await reader.read();
    if (done) throw new Error("SSE stream closed before first event");
    text += decoder.decode(value, { stream: true });
  }
  clearTimeout(timeout);
  await reader.cancel();

  const event = text.slice(0, text.indexOf("\n\n"));
  const data = event
    .split(/\r?\n/)
    .filter((line) => line.startsWith("data:"))
    .map((line) => line.slice(5).trimStart())
    .join("\n");
  if (!data) throw new Error(`SSE event did not include data: ${event}`);
  console.log(data);
})().catch((error) => {
  console.error(error.message || error);
  process.exit(1);
});
NODE
}

curl -fsS "$base/health" | assert_health_modules
curl -fsS "$base/capabilities" | assert_capabilities_streams
curl -fsS "$base/modules" | parse_json
curl -fsS "$base/telemetry" | parse_json
curl -fsS "$base/sensors" | parse_json
curl -fsS -X POST "$base/capture" | parse_json
curl -fsS "$base/" | assert_dashboard_page
curl -fsS "$base/dashboard" | assert_dashboard_page
assert_dashboard_updates
post_dashboard_action authorize
post_dashboard_action stop
post_dashboard_action estop
post_dashboard_action estop-reset
post_dashboard_action capture
curl -fsS "$base/agent" | grep -q "Leash Agent Input"
curl -fsS -X POST "$base/agent/messages" \
  -H "content-type: application/json" \
  --data '{"source":"web","text":"web smoke message"}' | assert_agent_message web "web smoke message"
cargo run --quiet -- agent-send "one shot smoke message" --url "$base" | assert_agent_message cli "one shot smoke message"
printf 'interactive smoke message\n/quit\n' | cargo run --quiet -- agent-interactive --url "$base" >/dev/null
curl -fsS "$base/agent/messages" | assert_agent_messages_include "interactive smoke message"
assert_ws_telemetry
assert_sse_telemetry

policy_status="$(curl -sS -o "$policy_response" -w "%{http_code}" \
  -X POST "$base/drive" \
  -H "content-type: application/json" \
  --data '{"left":0.2,"right":0.2}')"
if [[ "$policy_status" != "400" ]]; then
  echo "expected drive without a pilot token to return HTTP 400, got $policy_status" >&2
  cat "$policy_response" >&2
  exit 1
fi
assert_policy_denial <"$policy_response"

curl -fsS -X POST "$base/pilot/authorize" \
  -H "content-type: application/json" \
  --data '{"token":"smoke-token","ttl_secs":30,"speed_mode":"low"}' | parse_json
curl -fsS -X POST "$base/estop" | parse_json
curl -fsS -X POST "$base/estop/reset" \
  -H "content-type: application/json" \
  --data '{"token":"smoke-token","approval":true}' | parse_json
curl -fsS -X POST "$base/drive" \
  -H "content-type: application/json" \
  --data '{"token":"smoke-token","left":0.2,"right":0.2}' | assert_drive_outcome
curl -fsS -X POST "$base/motors/stop" | parse_json

echo "http smoke ok: $base"
