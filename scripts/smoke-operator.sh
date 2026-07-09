#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

node --check operator/server.js
node --check operator/public/app.js
node --check operator/public/session.js
node operator/server.js --check-config operator/fleet.example.json >/dev/null

REPO_ROOT="$repo_root" node <<'EOF'
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { loadFleet, validateFleetConfig } = require(path.join(
  process.env.REPO_ROOT,
  "operator/server.js",
));

const root = process.env.REPO_ROOT;
const examplePath = path.join(root, "operator/fleet.example.json");
const fleet = loadFleet(examplePath);
assert.equal(fleet.robots.length, 2);

const example = fs.readFileSync(examplePath, "utf8");
assert.doesNotMatch(example, /\b10\.\d+\.\d+\.\d+\b/);
assert.doesNotMatch(example, /\b192\.168\.\d+\.\d+\b/);
assert.doesNotMatch(example, /\b172\.(?:1[6-9]|2\d|3[01])\.\d+\.\d+\b/);
assert.doesNotMatch(example, /guard-dog|pinkie/i);

const errors = validateFleetConfig({
  surprise: true,
  robots: [
    { id: "duplicate", baseUrl: "http://192.0.2.20:8000" },
    {
      id: "duplicate",
      baseUrl: "ftp://user:pass@example.test/private?secret=yes",
      privateKey: "not-allowed",
      videoTransport: "magic",
    },
  ],
});
for (const expected of [
  "root.surprise: unknown field",
  "robots[1].id: duplicate robot id 'duplicate'",
  "robots[1].privateKey: unknown field",
  "robots[1].videoTransport: expected one of auto, mjpeg, webrtc",
  "robots[1].baseUrl: expected an http or https URL",
  "robots[1].baseUrl: credentials are not allowed",
  "robots[1].baseUrl: expected an origin URL without a path, query, or fragment",
]) {
  assert(errors.includes(expected), `missing validation error: ${expected}`);
}

const schema = JSON.parse(fs.readFileSync(path.join(root, "operator/fleet.schema.json"), "utf8"));
assert.equal(schema.additionalProperties, false);
assert.deepEqual(schema.required, ["robots"]);
assert.deepEqual(schema.$defs.robot.required, ["id", "baseUrl"]);

const html = fs.readFileSync(path.join(root, "operator/public/index.html"), "utf8");
const css = fs.readFileSync(path.join(root, "operator/public/styles.css"), "utf8");
assert.match(html, /class="mobile-estop estop danger"/);
assert.match(html, /metric-health-history/);
assert.match(html, /metric-camera-history/);
assert.match(html, /metric-token/);
assert.match(html, /class="patrol-zone"/);
assert.match(html, /class="patrol-start"/);
assert.match(html, /class="patrol-stop"/);
assert.match(html, /metric-motion-events/);
assert.match(html, /id="debug-session"[^>]*hidden/);
assert.match(html, /src="\/session\.js"/);
assert.match(css, /orientation: landscape/);
assert.match(css, /body\.single-operator button\.mobile-estop/);
assert.match(css, /position: fixed/);

console.log(JSON.stringify({
  ok: true,
  robots: fleet.robots.length,
  validationErrorsProved: errors.length,
  mobileEstop: "fixed in single-bot mobile view",
  patrolControls: "configured zone selector and start/stop controls present",
}));
EOF
