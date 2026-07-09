const grid = document.querySelector("#fleet-grid");
const template = document.querySelector("#robot-template");
const fleetName = document.querySelector("#fleet-name");
const fleetStatus = document.querySelector("#fleet-status");
const tokenInput = document.querySelector("#operator-token");
const speedMode = document.querySelector("#speed-mode");
const refreshAll = document.querySelector("#refresh-all");
const stopAll = document.querySelector("#stop-all");
const selectorOptions = document.querySelector("#selector-options");
const selectorStatus = document.querySelector("#selector-status");

const state = new Map();
let fleet = { robots: [], pollMs: 2500, snapshotMs: 3000 };
let selectedRobotId = localStorage.getItem("leash.operator.selection") || "fleet";

const OPERATOR_TOKEN_KEY = "leash.operator.token";
const DRIVE_LOOP_MS = 33;
const AIM_LOOP_MS = 33;
const AIM_SEND_MS = 33;
const JOYSTICK_RADIUS = 46;
const JOYSTICK_DEADZONE = 0.08;
const DRIVE_MAX = 0.24;
const DRIVE_SMOOTHING = 0.42;
const AIM_SMOOTHING = 0.52;
const AIM_RESPONSE_EXPONENT = 1.45;
const AIM_PAN_DEG_PER_SEC = 145;
const AIM_TILT_DEG_PER_SEC = 105;
const AIM_SERVO_SPEED = 150;
const AIM_SERVO_ACCEL = 16;
const AUTH_REFRESH_MS = 60_000;
const AUTH_TTL_SECS = 180;

function newOperatorToken() {
  if (globalThis.crypto?.randomUUID) {
    return `op-${globalThis.crypto.randomUUID().slice(0, 8)}`;
  }
  return `op-${Math.random().toString(36).slice(2, 10)}`;
}

function loadOperatorToken() {
  const stored = localStorage.getItem(OPERATOR_TOKEN_KEY);
  if (stored && stored.trim()) return stored.trim();
  const created = newOperatorToken();
  localStorage.setItem(OPERATOR_TOKEN_KEY, created);
  return created;
}

tokenInput.value = loadOperatorToken();
tokenInput.addEventListener("change", () => {
  const value = tokenInput.value.trim() || newOperatorToken();
  tokenInput.value = value;
  localStorage.setItem(OPERATOR_TOKEN_KEY, value);
});

function api(robot, route, options = {}) {
  return fetch(`/api/robots/${encodeURIComponent(robot.id)}/${route}`, {
    ...options,
    headers: {
      "content-type": "application/json",
      ...(options.headers || {}),
    },
  });
}

async function jsonApi(robot, route, options = {}) {
  const response = await api(robot, route, options);
  const payload = await response.json();
  if (!response.ok || payload.ok === false) {
    throw new Error(payload.error || `${route} failed`);
  }
  return payload;
}

function robotState(robot) {
  if (!state.has(robot.id)) {
    state.set(robot.id, {
      tokenReady: false,
      pan: 0,
      tilt: 0,
      snapshotBusy: false,
      streaming: false,
      streamCapable: false,
      streamStatus: "snapshot",
      streamNonce: 0,
      streamReconnectTimer: null,
      streamReconnectAttempts: 0,
      streamLastLogAt: 0,
      lastCamera: null,
      cameraStatus: "unknown",
      rtc: null,
      rtcStarting: false,
      rtcFallback: false,
      driveTarget: { x: 0, y: 0 },
      driveSmoothed: { x: 0, y: 0 },
      cameraTarget: { x: 0, y: 0 },
      cameraSmoothed: { x: 0, y: 0 },
      driveLast: { left: 0, right: 0 },
      driveTimer: null,
      driveInFlight: false,
      aimTimer: null,
      aimInFlight: false,
      aimLastMs: 0,
      aimLastSendMs: 0,
      aimLocalRev: 0,
      authInFlight: false,
      authPromise: null,
      authorizedAt: 0,
      health: null,
      telemetry: null,
      lastLog: [],
    });
  }
  return state.get(robot.id);
}

function log(robot, message) {
  const current = robotState(robot);
  const time = new Date().toLocaleTimeString();
  current.lastLog.unshift(`${time} ${message}`);
  current.lastLog = current.lastLog.slice(0, 5);
  const card = document.querySelector(`[data-robot-id="${robot.id}"]`);
  if (card) {
    card.querySelector(".log").textContent = current.lastLog.join("\n");
  }
}

function token() {
  return tokenInput.value.trim() || "operator";
}

function setRobotClass(card, className) {
  card.classList.remove("ok", "warn", "down");
  card.classList.add(className);
}

function formatNumber(value, digits = 2) {
  return Number.isFinite(value) ? value.toFixed(digits) : "-";
}

function batteryLabel(telemetry) {
  const voltage = telemetry?.battery_v ?? telemetry?.sensors?.battery?.voltage_v;
  const percent = telemetry?.battery_pct ?? telemetry?.sensors?.battery?.level_pct;
  if (Number.isFinite(percent) && Number.isFinite(voltage)) {
    return `battery ${percent.toFixed(0)}% ${voltage.toFixed(2)} V`;
  }
  if (Number.isFinite(percent)) return `battery ${percent.toFixed(0)}%`;
  if (Number.isFinite(voltage)) return `battery ${voltage.toFixed(2)} V`;
  return "battery -";
}

function updateHud(robot) {
  const card = document.querySelector(`[data-robot-id="${robot.id}"]`);
  if (!card) return;
  const current = robotState(robot);
  card.querySelector(".hud-name").textContent = robot.name;
  card.querySelector(".hud-link").textContent = current.streamStatus;
  card.querySelector(".hud-battery").textContent = batteryLabel(current.telemetry);
  card.querySelector(".hud-drive").textContent =
    `L ${formatNumber(current.driveLast.left)} / R ${formatNumber(current.driveLast.right)}`;
  card.querySelector(".hud-speed").textContent =
    `speed ${current.telemetry?.speed_mode || speedMode.value}`;
  card.querySelector(".hud-estop").textContent =
    `estop ${current.health?.estop == null ? "-" : current.health.estop}`;
  card.querySelector(".hud-aim").textContent =
    `pan ${formatNumber(current.pan, 0)} / tilt ${formatNumber(current.tilt, 0)}`;
}

function renderSelector() {
  selectorOptions.textContent = "";
  selectorOptions.appendChild(selectorButton("fleet", "Fleet", `${fleet.robots.length} bots`));
  for (const robot of fleet.robots) {
    const current = robotState(robot);
    const detail = [current.health?.ok ? "online" : "checking", current.cameraStatus]
      .filter(Boolean)
      .join(" / ");
    selectorOptions.appendChild(selectorButton(robot.id, robot.name, detail));
  }
  applySelection();
}

function selectorButton(id, label, detail) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "selector-option";
  button.dataset.selection = id;
  const title = document.createElement("span");
  title.textContent = label;
  const meta = document.createElement("small");
  meta.textContent = detail || "-";
  button.append(title, meta);
  button.addEventListener("click", () => {
    selectedRobotId = id;
    localStorage.setItem("leash.operator.selection", selectedRobotId);
    applySelection();
  });
  return button;
}

function updateSelectorStatus() {
  const online = fleet.robots.filter((robot) => robotState(robot).health?.ok).length;
  selectorStatus.textContent = `${online}/${fleet.robots.length} connected`;
  for (const button of selectorOptions.querySelectorAll(".selector-option")) {
    const id = button.dataset.selection;
    button.classList.toggle("active", id === selectedRobotId);
    if (id === "fleet") {
      button.querySelector("small").textContent = `${online}/${fleet.robots.length} connected`;
      continue;
    }
    const robot = fleet.robots.find((item) => item.id === id);
    if (!robot) continue;
    const current = robotState(robot);
    const connected = current.health?.ok ? "online" : "offline";
    button.querySelector("small").textContent = `${connected} / camera ${current.cameraStatus}`;
    button.classList.toggle("down", !current.health?.ok);
  }
}

function applySelection() {
  if (selectedRobotId !== "fleet" && !fleet.robots.some((robot) => robot.id === selectedRobotId)) {
    selectedRobotId = "fleet";
  }
  const single = selectedRobotId !== "fleet";
  document.body.classList.toggle("single-operator", single);
  grid.classList.toggle("single-view", single);
  for (const card of grid.querySelectorAll(".robot")) {
    card.hidden = single && card.dataset.robotId !== selectedRobotId;
  }
  updateSelectorStatus();
}

function renderFleet() {
  grid.textContent = "";
  for (const robot of fleet.robots) {
    const node = template.content.firstElementChild.cloneNode(true);
    node.dataset.robotId = robot.id;
    node.querySelector(".robot-name").textContent = robot.name;
    node.querySelector(".robot-meta").textContent = [robot.role, robot.location]
      .filter(Boolean)
      .join(" / ");
    node.querySelector(".snapshot").alt = `${robot.name} camera`;
    node.querySelector(".hud-name").textContent = robot.name;

    node.querySelector(".authorize").addEventListener("click", () => authorize(robot));
    node.querySelector(".camera-refresh").addEventListener("click", () => refreshCamera(robot));
    node.querySelector(".stop").addEventListener("click", () => stopRobot(robot));
    node.querySelector(".estop").addEventListener("click", () => estopRobot(robot));
    node.querySelector(".aim-center").addEventListener("click", () => aimRobot(robot, 0, 0, true));

    for (const button of node.querySelectorAll(".aim-btn")) {
      button.addEventListener("click", () => {
        aimRobot(robot, Number(button.dataset.pan), Number(button.dataset.tilt), false);
      });
    }

    for (const button of node.querySelectorAll(".drive")) {
      button.addEventListener("click", () => {
        driveRobot(robot, Number(button.dataset.left), Number(button.dataset.right));
      });
    }

    for (const joystick of node.querySelectorAll(".joystick")) {
      bindJoystick(robot, joystick, joystick.dataset.stick);
    }

    grid.appendChild(node);
    updateHud(robot);
  }
}

async function refreshRobot(robot) {
  const card = document.querySelector(`[data-robot-id="${robot.id}"]`);
  if (!card) return;
  try {
    const { health, telemetry, camera } = await jsonApi(robot, "summary");
    const current = robotState(robot);
    current.health = health;
    current.telemetry = telemetry;
    setRobotClass(card, health.ok ? "ok" : "warn");
    card.querySelector(".state-text").textContent = health.ok ? "online" : "attention";
    card.querySelector(".metric-health").textContent = health.ok ? "ok" : "attention";
    card.querySelector(".metric-battery").textContent = batteryLabel(telemetry).replace("battery ", "");
    card.querySelector(".metric-estop").textContent = String(health.estop);
    card.querySelector(".metric-profile").textContent = health.profile || "-";
    if (camera.gimbal?.range) {
      card.dataset.panMin = camera.gimbal.range.pan_deg[0];
      card.dataset.panMax = camera.gimbal.range.pan_deg[1];
      card.dataset.tiltMin = camera.gimbal.range.tilt_deg[0];
      card.dataset.tiltMax = camera.gimbal.range.tilt_deg[1];
    }
    current.lastCamera = camera.camera;
    current.cameraStatus = camera.camera?.status || "unknown";
    maybeStartStream(robot, camera.camera);
    updateHud(robot);
    updateSelectorStatus();
  } catch (error) {
    const current = robotState(robot);
    current.health = null;
    current.cameraStatus = "unknown";
    setRobotClass(card, "down");
    card.querySelector(".state-text").textContent = "offline";
    card.querySelector(".metric-health").textContent = "down";
    log(robot, error.message);
    updateHud(robot);
    updateSelectorStatus();
  }
}

function maybeStartStream(robot, camera) {
  const current = robotState(robot);
  current.streamCapable = camera?.status === "available" && Boolean(camera?.stream_url);
  if (!current.streamCapable) {
    if (current.streamReconnectTimer) {
      clearTimeout(current.streamReconnectTimer);
      current.streamReconnectTimer = null;
    }
    current.streamReconnectAttempts = 0;
    if (camera?.status && camera.status !== "available") {
      current.streamStatus = camera.status;
      updateHud(robot);
    }
    return;
  }
  if (current.streaming || current.streamReconnectTimer) return;
  if (robot.videoTransport === "webrtc" && !camera?.webrtc_url) {
    current.streamStatus = "webrtc unavailable";
    current.cameraStatus = "fault";
    updateHud(robot);
    updateSelectorStatus();
    refreshSnapshot(robot, { force: true, cacheOnly: true });
    return;
  }
  if (robot.videoTransport !== "mjpeg" && camera?.webrtc_url && !current.rtcFallback) {
    startWebRtcStream(robot, camera);
    return;
  }

  const image = document.querySelector(`[data-robot-id="${robot.id}"] .snapshot`);
  if (!image) return;
  current.streaming = true;
  image.onload = () => {
    current.streamReconnectAttempts = 0;
    current.cameraStatus = "live";
    updateSelectorStatus();
  };
  image.onerror = () => {
    current.streaming = false;
    current.streamStatus = "reconnecting";
    current.cameraStatus = "reconnecting";
    current.streamReconnectAttempts += 1;
    image.onerror = null;
    image.removeAttribute("src");
    const delayMs = reconnectDelayMs(current);
    logStreamReconnect(robot, delayMs);
    updateHud(robot);
    updateSelectorStatus();
    refreshSnapshot(robot, { force: true, cacheOnly: true });
    scheduleStreamReconnect(robot, delayMs);
  };
  image.src =
    `/api/robots/${encodeURIComponent(robot.id)}/stream.mjpg?stream=${current.streamNonce}&t=${Date.now()}`;
  current.streamStatus = "stream live";
  updateHud(robot);
  log(robot, "camera stream connected");
}

async function startWebRtcStream(robot, camera) {
  const current = robotState(robot);
  if (current.rtcStarting || current.rtc) return;
  const card = document.querySelector(`[data-robot-id="${robot.id}"]`);
  const video = card?.querySelector(".webrtc-video");
  const image = card?.querySelector(".snapshot");
  if (!video || !image || !window.RTCPeerConnection) {
    current.rtcFallback = true;
    maybeStartStream(robot, camera);
    return;
  }

  current.rtcStarting = true;
  current.streamStatus = "webrtc connecting";
  current.cameraStatus = "connecting";
  updateHud(robot);
  updateSelectorStatus();

  const pc = new RTCPeerConnection({ iceServers: [] });
  const ws = new WebSocket(webrtcSignalUrl(robot, camera));
  current.rtc = { pc, ws };

  const fail = (message) => {
    closeWebRtc(robot);
    if (robot.videoTransport === "webrtc") {
      current.streaming = false;
      current.streamStatus = "webrtc unavailable";
      current.cameraStatus = "fault";
      log(robot, message || "webrtc unavailable");
      updateHud(robot);
      updateSelectorStatus();
      refreshSnapshot(robot, { force: true, cacheOnly: true });
      return;
    }
    current.rtcFallback = true;
    current.streaming = false;
    current.streamStatus = "reconnecting";
    current.cameraStatus = "reconnecting";
    log(robot, message || "webrtc unavailable; falling back");
    updateHud(robot);
    updateSelectorStatus();
    maybeStartStream(robot, camera);
  };

  pc.addTransceiver("video", { direction: "recvonly" });
  pc.ontrack = (event) => {
    const stream = event.streams?.[0] || new MediaStream([event.track]);
    video.srcObject = stream;
    video.classList.add("active");
    image.removeAttribute("src");
    current.streaming = true;
    current.streamReconnectAttempts = 0;
    current.streamStatus = "webrtc live";
    current.cameraStatus = "live";
    updateHud(robot);
    updateSelectorStatus();
  };
  pc.onicecandidate = (event) => {
    if (ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({ type: "candidate", candidate: event.candidate }));
    }
  };
  pc.onconnectionstatechange = () => {
    if (["failed", "disconnected", "closed"].includes(pc.connectionState)) {
      fail(`webrtc ${pc.connectionState}; falling back`);
    }
  };

  ws.onopen = async () => {
    try {
      const offer = await pc.createOffer();
      await pc.setLocalDescription(offer);
      ws.send(JSON.stringify(pc.localDescription));
    } catch (error) {
      fail(error.message);
    }
  };
  ws.onmessage = async (event) => {
    try {
      const message = JSON.parse(event.data);
      if (message.type === "answer") {
        await pc.setRemoteDescription(message);
      } else if (message.type === "candidate" && message.candidate) {
        await pc.addIceCandidate(message.candidate);
      } else if (message.type === "ended") {
        fail(message.reason || "webrtc ended; falling back");
      } else if (message.type === "error") {
        fail(message.error);
      }
    } catch (error) {
      fail(error.message);
    }
  };
  ws.onerror = () => fail("webrtc signal failed; falling back");
  ws.onclose = () => {
    if (current.rtc?.ws === ws && !current.streaming) {
      fail("webrtc signal closed; falling back");
    }
  };
  current.rtcStarting = false;
}

function webrtcSignalUrl(robot, camera) {
  const base = new URL(robot.baseUrl || window.location.origin);
  const url = new URL(camera.webrtc_url, base);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  return url.toString();
}

function closeWebRtc(robot) {
  const current = robotState(robot);
  const rtc = current.rtc;
  current.rtc = null;
  current.rtcStarting = false;
  if (rtc?.ws && rtc.ws.readyState <= WebSocket.OPEN) {
    try {
      rtc.ws.send(JSON.stringify({ type: "close" }));
    } catch (_) {}
    rtc.ws.close();
  }
  if (rtc?.pc) {
    rtc.pc.close();
  }
  const card = document.querySelector(`[data-robot-id="${robot.id}"]`);
  const video = card?.querySelector(".webrtc-video");
  if (video) {
    video.pause();
    video.srcObject = null;
    video.classList.remove("active");
  }
}

function reconnectDelayMs(current) {
  const exponent = Math.min(5, Math.max(0, current.streamReconnectAttempts - 1));
  return Math.min(5_000, 500 * 2 ** exponent);
}

function logStreamReconnect(robot, delayMs) {
  const current = robotState(robot);
  const now = Date.now();
  if (now - current.streamLastLogAt < 5_000) return;
  current.streamLastLogAt = now;
  log(robot, `stream unavailable; retrying in ${Math.round(delayMs / 1000)}s`);
}

function scheduleStreamReconnect(robot, delayMs = 750) {
  const current = robotState(robot);
  if (current.streamReconnectTimer) return;
  current.streamReconnectTimer = setTimeout(() => {
    current.streamReconnectTimer = null;
    if (!current.streaming) {
      maybeStartStream(robot, current.lastCamera);
    }
  }, delayMs);
}

function resetCameraStream(robot, reason = "camera refresh") {
  const current = robotState(robot);
  if (current.streamReconnectTimer) {
    clearTimeout(current.streamReconnectTimer);
    current.streamReconnectTimer = null;
  }
  current.streamNonce += 1;
  current.streaming = false;
  current.streamReconnectAttempts = 0;
  current.rtcFallback = false;
  current.streamStatus = reason;
  current.cameraStatus = "refreshing";
  closeWebRtc(robot);
  const image = document.querySelector(`[data-robot-id="${robot.id}"] .snapshot`);
  if (image) {
    image.onload = null;
    image.onerror = null;
    image.removeAttribute("src");
  }
  updateHud(robot);
  updateSelectorStatus();
}

async function refreshCamera(robot) {
  resetCameraStream(robot);
  try {
    await jsonApi(robot, "camera-refresh", { method: "POST", body: "{}" });
  } catch (error) {
    log(robot, error.message);
  }
  await refreshRobot(robot);
  scheduleStreamReconnect(robot, 100);
  log(robot, "camera refreshed");
}

function refreshSnapshot(robot, options = {}) {
  const image = document.querySelector(`[data-robot-id="${robot.id}"] .snapshot`);
  const current = robotState(robot);
  if (
    !image ||
    current.snapshotBusy ||
    (!options.force && (current.streaming || current.streamCapable))
  ) {
    return;
  }
  current.snapshotBusy = true;
  const done = () => {
    current.snapshotBusy = false;
    image.onload = null;
    image.onerror = null;
    updateSelectorStatus();
  };
  image.onload = () => {
    current.cameraStatus = "snapshot";
    done();
  };
  image.onerror = () => {
    current.cameraStatus = "fault";
    done();
  };
  const params = new URLSearchParams({ t: String(Date.now()) });
  if (options.cacheOnly) params.set("cache", "1");
  image.src = `/api/robots/${encodeURIComponent(robot.id)}/snapshot?${params.toString()}`;
}

async function authorize(robot, options = {}) {
  const current = robotState(robot);
  if (current.authInFlight) return current.authPromise;
  current.authInFlight = true;
  current.authPromise = (async () => {
    try {
      await jsonApi(robot, "authorize", {
        method: "POST",
        body: JSON.stringify({
          token: token(),
          ttl_secs: AUTH_TTL_SECS,
          speed_mode: speedMode.value,
        }),
      });
      current.tokenReady = true;
      current.authorizedAt = Date.now();
      if (!options.silent) {
        log(robot, `authorized ${speedMode.value}`);
      }
    } catch (error) {
      current.tokenReady = false;
      if (!options.silent) {
        log(robot, error.message);
      }
    } finally {
      current.authInFlight = false;
      current.authPromise = null;
    }
  })();
  return current.authPromise;
}

async function ensureAuthorized(robot) {
  const current = robotState(robot);
  if (current.tokenReady && Date.now() - current.authorizedAt < AUTH_REFRESH_MS) {
    return;
  }
  await authorize(robot, { silent: true });
}

function clamp(value, min, max) {
  return Math.max(min, Math.min(max, value));
}

function deadzone(value) {
  return Math.abs(value) < JOYSTICK_DEADZONE ? 0 : value;
}

function responseCurve(value, exponent) {
  if (value === 0) return 0;
  return Math.sign(value) * Math.pow(Math.abs(value), exponent);
}

function smoothVector(current, targetKey, smoothedKey, alpha) {
  const target = current[targetKey];
  const smoothed = current[smoothedKey];
  smoothed.x += (target.x - smoothed.x) * alpha;
  smoothed.y += (target.y - smoothed.y) * alpha;
  if (Math.abs(target.x) === 0 && Math.abs(smoothed.x) < 0.018) smoothed.x = 0;
  if (Math.abs(target.y) === 0 && Math.abs(smoothed.y) < 0.018) smoothed.y = 0;
  return smoothed;
}

function bindJoystick(robot, node, kind) {
  const thumb = node.querySelector(".joystick-thumb");
  let pointerId = null;

  function setThumb(dx, dy) {
    thumb.style.transform = `translate(calc(-50% + ${dx}px), calc(-50% + ${dy}px))`;
  }

  function vectorFromEvent(event) {
    const rect = node.querySelector(".joystick-base").getBoundingClientRect();
    const centerX = rect.left + rect.width / 2;
    const centerY = rect.top + rect.height / 2;
    const rawX = event.clientX - centerX;
    const rawY = event.clientY - centerY;
    const length = Math.hypot(rawX, rawY);
    const scale = length > JOYSTICK_RADIUS ? JOYSTICK_RADIUS / length : 1;
    const dx = rawX * scale;
    const dy = rawY * scale;
    setThumb(dx, dy);
    return {
      x: deadzone(dx / JOYSTICK_RADIUS),
      y: deadzone(-dy / JOYSTICK_RADIUS),
    };
  }

  function apply(event) {
    const vector = vectorFromEvent(event);
    if (kind === "drive") {
      startDriveJoystick(robot, vector);
    } else {
      startCameraJoystick(robot, vector);
    }
  }

  function release() {
    if (pointerId == null) return;
    pointerId = null;
    node.classList.remove("active");
    setThumb(0, 0);
    if (kind === "drive") {
      stopDriveJoystick(robot);
    } else {
      stopCameraJoystick(robot);
    }
  }

  node.addEventListener("pointerdown", (event) => {
    event.preventDefault();
    pointerId = event.pointerId;
    node.setPointerCapture(pointerId);
    node.classList.add("active");
    apply(event);
  });
  node.addEventListener("pointermove", (event) => {
    if (event.pointerId !== pointerId) return;
    event.preventDefault();
    apply(event);
  });
  node.addEventListener("pointerup", release);
  node.addEventListener("pointercancel", release);
  node.addEventListener("lostpointercapture", release);
}

function drivePairFromVector(vector) {
  const forward = deadzone(vector.y);
  const turn = deadzone(vector.x);
  return {
    left: clamp((forward + turn) * DRIVE_MAX, -DRIVE_MAX, DRIVE_MAX),
    right: clamp((forward - turn) * DRIVE_MAX, -DRIVE_MAX, DRIVE_MAX),
  };
}

function startDriveJoystick(robot, vector) {
  const current = robotState(robot);
  current.driveTarget = vector;
  updateHud(robot);
  if (!current.driveTimer) {
    current.driveSmoothed = { x: 0, y: 0 };
    current.driveTimer = setInterval(() => sendJoystickDrive(robot), DRIVE_LOOP_MS);
    sendJoystickDrive(robot);
  }
}

async function sendJoystickDrive(robot) {
  const current = robotState(robot);
  if (current.driveInFlight) return;
  const vector = smoothVector(current, "driveTarget", "driveSmoothed", DRIVE_SMOOTHING);
  const command = drivePairFromVector(vector);
  current.driveLast = command;
  updateHud(robot);
  current.driveInFlight = true;
  try {
    await ensureAuthorized(robot);
    await jsonApi(robot, "drive", {
      method: "POST",
      body: JSON.stringify({
        token: token(),
        left: Number(command.left.toFixed(3)),
        right: Number(command.right.toFixed(3)),
        speed_mode: speedMode.value,
      }),
    });
  } catch (error) {
    log(robot, error.message);
  } finally {
    current.driveInFlight = false;
  }
}

function stopDriveJoystick(robot) {
  const current = robotState(robot);
  clearInterval(current.driveTimer);
  current.driveTimer = null;
  current.driveTarget = { x: 0, y: 0 };
  current.driveSmoothed = { x: 0, y: 0 };
  current.driveLast = { left: 0, right: 0 };
  updateHud(robot);
  stopRobot(robot);
}

function startCameraJoystick(robot, vector) {
  const current = robotState(robot);
  current.cameraTarget = vector;
  current.aimLastMs = current.aimLastMs || Date.now();
  if (!current.aimTimer) {
    current.cameraSmoothed = { x: 0, y: 0 };
    current.aimTimer = setInterval(() => sendJoystickAim(robot), AIM_LOOP_MS);
    sendJoystickAim(robot);
  }
}

async function sendJoystickAim(robot) {
  const current = robotState(robot);
  const vector = smoothVector(current, "cameraTarget", "cameraSmoothed", AIM_SMOOTHING);
  const panInput = responseCurve(deadzone(vector.x), AIM_RESPONSE_EXPONENT);
  const tiltInput = responseCurve(deadzone(vector.y), AIM_RESPONSE_EXPONENT);
  if (panInput === 0 && tiltInput === 0) return;

  const now = Date.now();
  const dt = Math.min(0.24, Math.max(0.02, (now - current.aimLastMs) / 1000));
  current.aimLastMs = now;
  const card = document.querySelector(`[data-robot-id="${robot.id}"]`);
  const panMin = Number(card?.dataset.panMin ?? -180);
  const panMax = Number(card?.dataset.panMax ?? 180);
  const tiltMin = Number(card?.dataset.tiltMin ?? -30);
  const tiltMax = Number(card?.dataset.tiltMax ?? 90);
  current.pan = clamp(current.pan + panInput * AIM_PAN_DEG_PER_SEC * dt, panMin, panMax);
  current.tilt = clamp(current.tilt + tiltInput * AIM_TILT_DEG_PER_SEC * dt, tiltMin, tiltMax);
  current.aimLocalRev += 1;
  updateHud(robot);

  if (current.aimInFlight || now - current.aimLastSendMs < AIM_SEND_MS) return;
  current.aimInFlight = true;
  current.aimLastSendMs = now;
  const localRev = current.aimLocalRev;
  try {
    const result = await jsonApi(robot, "aim", {
      method: "POST",
      body: JSON.stringify({
        token: token(),
        pan_deg: Number(current.pan.toFixed(1)),
        tilt_deg: Number(current.tilt.toFixed(1)),
        speed: AIM_SERVO_SPEED,
        accel: AIM_SERVO_ACCEL,
      }),
    });
    if (current.aimLocalRev === localRev) {
      current.pan = result.pan_deg ?? current.pan;
      current.tilt = result.tilt_deg ?? current.tilt;
      updateHud(robot);
    }
  } catch (error) {
    log(robot, error.message);
  } finally {
    current.aimInFlight = false;
  }
}

function stopCameraJoystick(robot) {
  const current = robotState(robot);
  clearInterval(current.aimTimer);
  current.aimTimer = null;
  current.cameraTarget = { x: 0, y: 0 };
  current.cameraSmoothed = { x: 0, y: 0 };
  current.aimLastMs = 0;
  current.aimLastSendMs = 0;
}

async function aimRobot(robot, panDelta, tiltDelta, absolute) {
  try {
    await ensureAuthorized(robot);
    const current = robotState(robot);
    const card = document.querySelector(`[data-robot-id="${robot.id}"]`);
    const panMin = Number(card?.dataset.panMin ?? -180);
    const panMax = Number(card?.dataset.panMax ?? 180);
    const tiltMin = Number(card?.dataset.tiltMin ?? -30);
    const tiltMax = Number(card?.dataset.tiltMax ?? 90);
    current.pan = absolute ? panDelta : clamp(current.pan + panDelta, panMin, panMax);
    current.tilt = absolute ? tiltDelta : clamp(current.tilt + tiltDelta, tiltMin, tiltMax);
    const result = await jsonApi(robot, "aim", {
      method: "POST",
      body: JSON.stringify({
        token: token(),
        pan_deg: current.pan,
        tilt_deg: current.tilt,
        speed: AIM_SERVO_SPEED,
        accel: AIM_SERVO_ACCEL,
      }),
    });
    log(robot, `aim pan=${result.pan_deg} tilt=${result.tilt_deg}`);
    updateHud(robot);
  } catch (error) {
    log(robot, error.message);
  }
}

async function driveRobot(robot, left, right) {
  try {
    await ensureAuthorized(robot);
    const current = robotState(robot);
    current.driveLast = { left, right };
    updateHud(robot);
    await jsonApi(robot, "drive", {
      method: "POST",
      body: JSON.stringify({
        token: token(),
        left,
        right,
        speed_mode: speedMode.value,
      }),
    });
    log(robot, `drive ${left}, ${right}`);
  } catch (error) {
    log(robot, error.message);
  }
}

async function stopRobot(robot) {
  try {
    await jsonApi(robot, "stop", { method: "POST", body: "{}" });
    const current = robotState(robot);
    current.driveLast = { left: 0, right: 0 };
    updateHud(robot);
    log(robot, "stop");
  } catch (error) {
    log(robot, error.message);
  }
}

async function estopRobot(robot) {
  try {
    await jsonApi(robot, "estop", { method: "POST", body: "{}" });
    log(robot, "estop");
  } catch (error) {
    log(robot, error.message);
  }
}

async function refreshEverything(options = {}) {
  if (options.resetStreams) {
    await Promise.allSettled(
      fleet.robots.map((robot) =>
        jsonApi(robot, "camera-refresh", { method: "POST", body: "{}" }),
      ),
    );
    for (const robot of fleet.robots) {
      resetCameraStream(robot);
    }
  }
  await Promise.all(fleet.robots.map(refreshRobot));
  for (const robot of fleet.robots) refreshSnapshot(robot);
}

async function boot() {
  const response = await fetch("/api/fleet");
  const payload = await response.json();
  if (!payload.ok) throw new Error(payload.error || "fleet load failed");
  fleet = payload;
  fleetName.textContent = fleet.fleetName;
  fleetStatus.textContent = `${fleet.robots.length} robots`;
  renderFleet();
  renderSelector();
  await refreshEverything();
  await Promise.allSettled(fleet.robots.map((robot) => authorize(robot, { silent: true })));
  setInterval(() => fleet.robots.forEach(refreshRobot), fleet.pollMs);
  setInterval(() => fleet.robots.forEach(refreshSnapshot), Math.max(100, fleet.snapshotMs));
  setInterval(() => {
    fleet.robots.forEach((robot) => authorize(robot, { silent: true }));
  }, AUTH_REFRESH_MS);
}

refreshAll.addEventListener("click", () => refreshEverything({ resetStreams: true }));
stopAll.addEventListener("click", () => fleet.robots.forEach(stopRobot));

boot().catch((error) => {
  fleetStatus.textContent = error.message;
});
