(function attachLeashSession(globalObject) {
  "use strict";

  const FORMAT = "leash-operator-session-v1";
  const EVENT_KINDS = new Set([
    "summary",
    "operator-ownership",
    "joystick-drive",
    "joystick-camera",
    "camera-failure",
    "camera-recovery",
    "frame-health",
    "telemetry",
  ]);
  const SENSITIVE_KEYS = new Set(["token", "password", "secret", "credential", "base_url"]);

  function clone(value) {
    return JSON.parse(JSON.stringify(value));
  }

  function rejectSensitive(value, path = "data") {
    if (Array.isArray(value)) {
      value.forEach((item, index) => rejectSensitive(item, `${path}[${index}]`));
      return;
    }
    if (!value || typeof value !== "object") return;
    for (const [key, nested] of Object.entries(value)) {
      const normalized = key.toLowerCase().replaceAll("-", "_");
      if (SENSITIVE_KEYS.has(normalized)) {
        throw new Error(`operator session contains sensitive field ${path}.${key}`);
      }
      rejectSensitive(nested, `${path}.${key}`);
    }
  }

  function safeRobots(fleet) {
    return (fleet.robots || []).map((robot) => ({
      id: String(robot.id || "").trim(),
      name: String(robot.name || robot.id || "").trim(),
      role: String(robot.role || "robot").trim(),
      location: String(robot.location || "").trim(),
      video_transport: String(robot.videoTransport || robot.video_transport || "auto"),
    }));
  }

  function validateRecording(input) {
    const recording = clone(input);
    if (recording.format !== FORMAT) throw new Error(`unsupported recording format: ${recording.format}`);
    if (!String(recording.fleet_name || "").trim()) throw new Error("fleet_name is required");
    if (!Number.isFinite(recording.started_at_ms) || !Number.isFinite(recording.ended_at_ms)) {
      throw new Error("recording timestamps must be finite numbers");
    }
    if (recording.ended_at_ms < recording.started_at_ms) {
      throw new Error("recording ended before it started");
    }
    if (!Array.isArray(recording.robots) || !Array.isArray(recording.events)) {
      throw new Error("recording robots and events must be arrays");
    }
    const ids = new Set();
    for (const robot of recording.robots) {
      if (!String(robot.id || "").trim() || !String(robot.name || "").trim()) {
        throw new Error("recorded robot id and name are required");
      }
      if (ids.has(robot.id)) throw new Error(`duplicate recorded robot: ${robot.id}`);
      ids.add(robot.id);
      if (Object.hasOwn(robot, "baseUrl") || Object.hasOwn(robot, "base_url")) {
        throw new Error("recordings must not include robot base URLs");
      }
    }
    const duration = recording.ended_at_ms - recording.started_at_ms;
    let previousOffset = 0;
    recording.events.forEach((event, index) => {
      if (!EVENT_KINDS.has(event.kind)) throw new Error(`unsupported event kind: ${event.kind}`);
      if (!ids.has(event.robot_id)) throw new Error(`event ${index} references unknown robot`);
      if (!Number.isFinite(event.offset_ms) || event.offset_ms < previousOffset) {
        throw new Error("recording events must have sorted finite offsets");
      }
      if (event.offset_ms > duration) throw new Error(`event ${index} exceeds recording duration`);
      if (!Number.isFinite(event.ts_ms) || event.ts_ms !== recording.started_at_ms + event.offset_ms) {
        throw new Error(`event ${index} timestamp does not match its offset`);
      }
      rejectSensitive(event.data);
      previousOffset = event.offset_ms;
    });
    return recording;
  }

  function createRecorder(fleet, now = Date.now()) {
    const startedAt = Number(now);
    const recording = {
      format: FORMAT,
      fleet_name: String(fleet.fleetName || fleet.fleet_name || "Leash Fleet"),
      started_at_ms: startedAt,
      ended_at_ms: startedAt,
      robots: safeRobots(fleet),
      events: [],
    };
    const robotIds = new Set(recording.robots.map((robot) => robot.id));

    return {
      record(kind, robotId, data, timestamp = Date.now()) {
        if (!EVENT_KINDS.has(kind)) throw new Error(`unsupported event kind: ${kind}`);
        if (!robotIds.has(robotId)) throw new Error(`unknown recording robot: ${robotId}`);
        rejectSensitive(data);
        const previous = recording.events.at(-1)?.offset_ms || 0;
        const offset = Math.max(previous, Math.max(0, Number(timestamp) - startedAt));
        recording.events.push({
          offset_ms: offset,
          ts_ms: startedAt + offset,
          robot_id: robotId,
          kind,
          data: clone(data),
        });
        recording.ended_at_ms = startedAt + offset;
      },
      finish(timestamp = Date.now()) {
        recording.ended_at_ms = Math.max(
          recording.ended_at_ms,
          startedAt + Math.max(0, Number(timestamp) - startedAt),
        );
        return validateRecording(recording);
      },
      snapshot() {
        return clone(recording);
      },
    };
  }

  function snapshotAt(input, requestedOffset) {
    const recording = validateRecording(input);
    const duration = recording.ended_at_ms - recording.started_at_ms;
    const offset = Math.max(0, Math.min(duration, Number(requestedOffset) || 0));
    const robots = Object.fromEntries(
      recording.robots.map((robot) => [robot.id, { robot: clone(robot), events: [] }]),
    );
    for (const event of recording.events) {
      if (event.offset_ms > offset) break;
      const state = robots[event.robot_id];
      state.events.push(clone(event));
      if (event.kind === "summary") state.summary = clone(event.data);
      if (event.kind === "telemetry") state.telemetry = clone(event.data);
      if (event.kind === "operator-ownership") state.operator_token = clone(event.data);
      if (event.kind === "camera-failure") {
        state.camera_failures ||= [];
        state.camera_failures.push({ ts_ms: event.ts_ms, ...clone(event.data) });
      }
      if (event.kind === "camera-recovery") state.camera_recovery = clone(event.data);
      if (event.kind === "frame-health") state.frame_health = clone(event.data);
      if (event.kind === "joystick-drive") state.joystick_drive = clone(event.data);
      if (event.kind === "joystick-camera") state.joystick_camera = clone(event.data);
    }
    return { offset_ms: offset, duration_ms: duration, robots };
  }

  function parse(text) {
    return validateRecording(JSON.parse(text));
  }

  const api = { FORMAT, createRecorder, parse, snapshotAt, validateRecording };
  globalObject.LeashSession = api;
  if (typeof module !== "undefined" && module.exports) module.exports = api;
})(globalThis);
