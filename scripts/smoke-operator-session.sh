#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

node --check operator/public/session.js
node --check operator/public/app.js

REPO_ROOT="$repo_root" node <<'EOF'
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const session = require(path.join(process.env.REPO_ROOT, "operator/public/session.js"));

const root = process.env.REPO_ROOT;
const fixtureText = fs.readFileSync(path.join(root, "examples/replay/operator-session.json"), "utf8");
const fixture = session.parse(fixtureText);
assert.equal(fixture.format, session.FORMAT);
assert.equal(fixture.robots[0].id, "example-rover");

const beforeRecovery = session.snapshotAt(fixture, 800).robots["example-rover"];
assert.equal(beforeRecovery.camera_failures.length, 1);
assert.equal(beforeRecovery.joystick_drive.left, 0.12);
assert.equal(beforeRecovery.camera_recovery, undefined);

const afterRecovery = session.snapshotAt(fixture, 1500).robots["example-rover"];
assert.equal(afterRecovery.summary.telemetry.battery_pct, 90);
assert.equal(afterRecovery.camera_recovery.recovery_count, 1);
assert.equal(afterRecovery.frame_health.ok, true);

const recorder = session.createRecorder({
  fleetName: "Local Test Fleet",
  robots: [{
    id: "test-rover",
    name: "Test Rover",
    role: "sim",
    location: "test fixture",
    videoTransport: "auto",
    baseUrl: "http://192.0.2.42:8000",
  }],
}, 1000);
recorder.record("operator-ownership", "test-rover", {
  active: true,
  owner_id: "operator-a1b2c3d4e5f6",
  expires_in_ms: 1000,
}, 1010);
recorder.record("joystick-drive", "test-rover", { left: 0.1, right: 0.1 }, 1020);
recorder.record("camera-failure", "test-rover", { owner: "mjpeg", reason: "ended" }, 1030);
recorder.record("camera-recovery", "test-rover", { recovery_count: 1 }, 1040);
const recorded = recorder.finish(1100);
const serialized = JSON.stringify(recorded);
assert.equal(recorded.robots[0].baseUrl, undefined);
assert.equal(recorded.robots[0].base_url, undefined);
assert.doesNotMatch(serialized, /192\.0\.2\.42/);
assert.throws(
  () => recorder.record("telemetry", "test-rover", { nested: { token: "raw-secret" } }, 1050),
  /sensitive field/,
);

const html = fs.readFileSync(path.join(root, "operator/public/index.html"), "utf8");
const app = fs.readFileSync(path.join(root, "operator/public/app.js"), "utf8");
assert.match(html, /id="debug-session"[^>]*hidden/);
assert.match(html, /id="session-timeline"/);
assert.match(html, /src="\/session\.js"/);
assert.match(app, /debugReplayActive/);
assert.match(app, /offline replay disables live robot requests/);
assert.match(app, /LeashSession\.snapshotAt/);
assert.match(app, /offline replay/);

console.log(JSON.stringify({
  ok: true,
  fixtureEvents: fixture.events.length,
  recordedKinds: [...new Set(recorded.events.map((event) => event.kind))],
  privateBaseUrlExcluded: true,
  debugReplay: "camera, telemetry, ownership, and joystick state",
}));
EOF
