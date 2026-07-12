const test = require("node:test");
const assert = require("node:assert/strict");

const {
  chooseVideoTransport,
  normalizeVideoTransport,
} = require("../public/video-transport.js");

test("normalizes configured transport values", () => {
  assert.equal(normalizeVideoTransport(" WebRTC "), "webrtc");
  assert.equal(normalizeVideoTransport(undefined), "auto");
});

test("rejects unknown configured transport values", () => {
  assert.throws(() => normalizeVideoTransport("webrt"), /videoTransport/);
});

test("strict WebRTC does not fall back when browser support is unavailable", () => {
  assert.equal(
    chooseVideoTransport({
      configured: "webrtc",
      webrtcUrl: "/camera/webrtc/ws",
      webRtcSupported: false,
      rtcFallback: false,
    }),
    "unavailable",
  );
});

test("strict WebRTC requires a signaling URL", () => {
  assert.equal(
    chooseVideoTransport({
      configured: "webrtc",
      webrtcUrl: null,
      webRtcSupported: true,
      rtcFallback: false,
    }),
    "unavailable",
  );
});

test("strict WebRTC uses signaling when available", () => {
  assert.equal(
    chooseVideoTransport({
      configured: "webrtc",
      webrtcUrl: "/camera/webrtc/ws",
      webRtcSupported: true,
      rtcFallback: false,
    }),
    "webrtc",
  );
});

test("automatic transport falls back to MJPEG after WebRTC failure", () => {
  assert.equal(
    chooseVideoTransport({
      configured: "auto",
      webrtcUrl: "/camera/webrtc/ws",
      webRtcSupported: true,
      rtcFallback: true,
    }),
    "mjpeg",
  );
});

test("explicit MJPEG never attempts WebRTC", () => {
  assert.equal(
    chooseVideoTransport({
      configured: "mjpeg",
      webrtcUrl: "/camera/webrtc/ws",
      webRtcSupported: true,
      rtcFallback: false,
    }),
    "mjpeg",
  );
});
