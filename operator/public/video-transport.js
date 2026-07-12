function normalizeVideoTransport(value) {
  const transport = String(value || "auto").trim().toLowerCase();
  if (!["auto", "webrtc", "mjpeg"].includes(transport)) {
    throw new Error(`invalid videoTransport '${value}'`);
  }
  return transport;
}

function chooseVideoTransport({ configured, webrtcUrl, webRtcSupported, rtcFallback }) {
  const transport = normalizeVideoTransport(configured);
  if (transport === "mjpeg") return "mjpeg";

  const canUseWebRtc = Boolean(webrtcUrl) && webRtcSupported && !rtcFallback;
  if (transport === "webrtc") {
    return canUseWebRtc ? "webrtc" : "unavailable";
  }
  return canUseWebRtc ? "webrtc" : "mjpeg";
}

if (typeof module !== "undefined" && module.exports) {
  module.exports = { chooseVideoTransport, normalizeVideoTransport };
}
