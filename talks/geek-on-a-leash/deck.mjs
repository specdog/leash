import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import PptxGenJS from "pptxgenjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = here;
const assets = path.join(root, "assets");
const output = path.resolve(process.argv[2] || path.join(root, ".build", "geek-on-a-leash.pptx"));
const notesOutput = path.join(root, "output", "speaker-notes.md");
const qrPath = process.env.TALK_QR_PNG || path.join(root, "output", "geek-on-a-leash-qr.png");
const qrExpiry = process.env.TALK_QR_EXPIRES || "QR added after publication preflight";

fs.mkdirSync(path.dirname(output), { recursive: true });
fs.mkdirSync(path.dirname(notesOutput), { recursive: true });

const pptx = new PptxGenJS();
pptx.layout = "LAYOUT_WIDE";
pptx.author = "Eric Manganaro";
pptx.company = "specdog";
pptx.subject = "Leash architecture, Rust safety boundaries, and a live UGV demonstration";
pptx.title = "Geek on a Leash — Rust at the Boundary Between Intent and Motion";
pptx.lang = "en-US";
pptx.theme = {
  headFontFace: "Arial Black",
  bodyFontFace: "Arial",
  lang: "en-US",
};
pptx.defineLayout({ name: "WIDE", width: 13.333, height: 7.5 });
pptx.layout = "WIDE";

const C = {
  paper: "F7F2E8",
  paper2: "FFFDF7",
  ink: "0B0B0B",
  blue: "2855FF",
  lime: "B8FF32",
  pink: "FF5B9D",
  yellow: "FFD63D",
  red: "EF3340",
  grey: "6B6B66",
  paleBlue: "DDE5FF",
  palePink: "FFDCEB",
  paleYellow: "FFF0AA",
  paleLime: "E4FFAE",
  white: "FFFFFF",
};

const SH = pptx.ShapeType;
const LINE = { color: C.ink, width: 2.25, beginArrowType: "none", endArrowType: "none" };
const BASE_SHA = "566bc569b24bf5f392291b142469282fcdfac2b3";
const GH = `https://github.com/specdog/leash/blob/${BASE_SHA}`;
const REPO = "https://github.com/specdog/leash";
const WAVESHARE = "https://www.waveshare.com/product/ai/robots/ugv-rover-pt-jetson-orin-ai-kit.htm";
const RAILWAY_BUCKETS = "https://docs.railway.com/storage-buckets";

const notes = [];

function slide(bg = C.paper, section = "LEASH", number = null) {
  const s = pptx.addSlide();
  s.background = { color: bg };
  s.addShape(SH.line, { x: 0.3, y: 0.28, w: 12.73, h: 0, line: { color: C.ink, width: 1.25 } });
  s.addText(section, { x: 0.42, y: 0.1, w: 3.2, h: 0.24, fontFace: "Arial Black", fontSize: 9.5, bold: true, color: C.ink, charSpacing: 1.4, margin: 0 });
  if (number !== null) {
    s.addText(String(number).padStart(2, "0"), { x: 12.42, y: 0.1, w: 0.45, h: 0.24, fontFace: "Courier New", fontSize: 9.5, bold: true, color: C.ink, align: "right", margin: 0 });
  }
  return s;
}

function title(s, text, opts = {}) {
  const x = opts.x ?? 0.55;
  const y = opts.y ?? 0.62;
  const w = opts.w ?? 12.15;
  const h = opts.h ?? 1.0;
  s.addText(text, {
    x, y, w, h,
    fontFace: "Arial Black",
    fontSize: opts.size ?? 28,
    bold: true,
    color: opts.color ?? C.ink,
    margin: 0,
    breakLine: false,
    valign: "mid",
    fit: "shrink",
  });
}

function body(s, text, x, y, w, h, opts = {}) {
  s.addText(text, {
    x, y, w, h,
    fontFace: opts.mono ? "Courier New" : "Arial",
    fontSize: opts.size ?? 18,
    bold: opts.bold ?? false,
    color: opts.color ?? C.ink,
    margin: opts.margin ?? 0,
    valign: opts.valign ?? "top",
    align: opts.align ?? "left",
    breakLine: false,
    fit: "shrink",
    lineSpacingMultiple: 1.0,
  });
}

function pill(s, text, x, y, w, fill = C.lime, color = C.ink, size = 12) {
  s.addShape(SH.roundRect, { x, y, w, h: 0.42, rectRadius: 0.08, fill: { color: fill }, line: { color: C.ink, width: 1.5 } });
  body(s, text, x + 0.12, y + 0.08, w - 0.24, 0.24, { size, bold: true, color, valign: "mid", align: "center" });
}

function box(s, text, x, y, w, h, fill = C.paper2, opts = {}) {
  s.addShape(opts.shape || SH.rect, { x, y, w, h, fill: { color: fill }, line: { color: opts.lineColor || C.ink, width: opts.lineWidth || 2.2 } });
  body(s, text, x + (opts.pad ?? 0.18), y + (opts.pad ?? 0.18), w - 2 * (opts.pad ?? 0.18), h - 2 * (opts.pad ?? 0.18), {
    size: opts.size ?? 17,
    bold: opts.bold ?? true,
    align: opts.align ?? "center",
    valign: opts.valign ?? "mid",
    mono: opts.mono ?? false,
    color: opts.color ?? C.ink,
  });
}

function arrow(s, x, y, w, h = 0, color = C.ink, width = 2.5) {
  s.addShape(SH.line, { x, y, w, h, line: { color, width, endArrowType: "triangle" } });
}

function dot(s, x, y, r, fill, line = C.ink) {
  s.addShape(SH.ellipse, { x, y, w: r, h: r, fill: { color: fill }, line: { color: line, width: 2 } });
}

function addImage(s, file, x, y, w, h, contain = true) {
  const p = path.join(assets, file);
  if (!fs.existsSync(p)) return;
  if (!contain) {
    s.addImage({ path: p, x, y, w, h });
    return;
  }
  const isWide = file.includes("qualia") || file.includes("terminal");
  const ratio = isWide ? (file.includes("qualia") ? 2824 / 1384 : 912 / 488) : file.includes("camera") ? 640 / 480 : 1;
  const target = w / h;
  let iw = w, ih = h, ix = x, iy = y;
  if (ratio > target) {
    ih = w / ratio;
    iy = y + (h - ih) / 2;
  } else {
    iw = h * ratio;
    ix = x + (w - iw) / 2;
  }
  s.addImage({ path: p, x: ix, y: iy, w: iw, h: ih });
}

function photoFrame(s, file, x, y, w, h, accent = C.ink, contain = true) {
  s.addShape(SH.rect, { x: x - 0.08, y: y - 0.08, w: w + 0.16, h: h + 0.16, fill: { color: C.paper2 }, line: { color: accent, width: 3 } });
  addImage(s, file, x, y, w, h, contain);
}

function note(s, number, talk, sources = []) {
  const src = sources.length ? `\n\n[Sources]\n${sources.map((u) => `- ${u}`).join("\n")}` : "";
  const all = `${talk}${src}`;
  s.addNotes(all);
  notes.push(`## ${String(number).padStart(2, "0")}\n\n${all}\n`);
}

function quoteMark(s, x, y, size = 80, color = C.blue) {
  body(s, "“", x, y, 1.0, 1.0, { size, bold: true, color, mono: false });
}

function footer(s, text = "github.com/specdog/leash") {
  body(s, text, 0.55, 7.13, 5.2, 0.18, { size: 8.5, bold: true, color: C.grey });
}

// 01 — title
{
  const s = slide(C.paper, "GEEK ON A LEASH", 1);
  s.addShape(SH.rect, { x: 0.45, y: 0.58, w: 7.55, h: 5.95, fill: { color: C.blue }, line: { color: C.ink, width: 3 } });
  body(s, "GEEK ON\nA LEASH", 0.78, 0.9, 6.7, 2.35, { size: 43, bold: true, color: C.white, mono: false });
  body(s, "Rust at the boundary between\nintent and motion", 0.82, 3.55, 6.35, 1.05, { size: 23, bold: true, color: C.white });
  pill(s, "OPEN SOURCE • MIT", 0.82, 5.55, 2.55, C.lime, C.ink, 11);
  photoFrame(s, "waveshare-ugv-rover-front.jpg", 8.55, 0.76, 4.15, 4.15, C.ink, true);
  body(s, "ERIC MANGANARO", 8.62, 5.26, 3.9, 0.34, { size: 18, bold: true });
  body(s, "Leash + Pinkie • 60 minutes", 8.62, 5.68, 3.9, 0.3, { size: 13, color: C.grey });
  if (fs.existsSync(qrPath)) {
    s.addImage({ path: qrPath, x: 11.2, y: 5.22, w: 1.2, h: 1.2 });
  }
  footer(s);
  note(s, 1, "Welcome. This is a talk about a small piece of software with an intentionally narrow job: stand between an intelligent requester and physical motors. We will spend most of the hour inside Leash, then use Pinkie for a bounded live proof.", [REPO, WAVESHARE]);
}

// 02
{
  const s = slide(C.yellow, "THE PROBLEM", 2);
  title(s, "Who gets to write the motors?", { size: 36, y: 0.72 });
  box(s, "HUMAN", 0.65, 2.1, 2.45, 1.25, C.paper2, { size: 22 });
  box(s, "AGENT", 0.65, 4.4, 2.45, 1.25, C.paleBlue, { size: 22 });
  arrow(s, 3.25, 2.72, 2.2, 1.5, C.ink, 3);
  arrow(s, 3.25, 5.02, 2.2, -1.5, C.ink, 3);
  box(s, "?", 5.45, 2.52, 2.05, 2.05, C.pink, { size: 48 });
  arrow(s, 7.65, 3.55, 2.1, 0, C.ink, 3);
  box(s, "MOTOR\nDRIVER", 9.85, 2.5, 2.75, 2.1, C.ink, { size: 24, color: C.white, lineColor: C.ink });
  body(s, "The dangerous bug is not bad planning.\nIt is ambiguous authority.", 3.9, 5.6, 5.6, 0.82, { size: 19, bold: true, align: "center" });
  footer(s);
  note(s, 2, "Start at the boundary. In an agentic robot, the planner, human, autonomy stack, and recovery code may all want motion. If authority is implicit, every integration becomes a safety argument. Leash makes the question explicit.");
}

// 03
{
  const s = slide(C.paper, "THE METAPHOR", 3);
  title(s, "A leash does not choose the walk.", { size: 34 });
  body(s, "It limits how intent reaches the body.", 0.62, 1.48, 7.3, 0.55, { size: 25, bold: true, color: C.blue });
  dot(s, 1.05, 3.15, 0.62, C.pink);
  body(s, "intent", 0.72, 3.92, 1.3, 0.35, { size: 15, bold: true, align: "center" });
  s.addShape(SH.arc, { x: 1.65, y: 2.38, w: 6.55, h: 2.15, adjustPoint: 0.3, rotate: 2, fill: { color: C.paper, transparency: 100 }, line: { color: C.blue, width: 7 } });
  dot(s, 8.0, 3.15, 0.62, C.lime);
  body(s, "motion", 7.7, 3.92, 1.3, 0.35, { size: 15, bold: true, align: "center" });
  box(s, "LEASH", 4.05, 2.78, 1.85, 1.1, C.yellow, { size: 22 });
  body(s, "The planner proposes.\nThe boundary decides.", 9.25, 2.68, 3.25, 1.4, { size: 25, bold: true });
  pill(s, "SMALL • FAST • AUDITABLE", 9.27, 4.55, 3.1, C.lime, C.ink, 10.5);
  footer(s);
  note(s, 3, "The metaphor is literal enough to be useful. Leash does not invent missions, recognize rooms, or decide what is interesting. It constrains the path from requested action to the physical body.", [REPO]);
}

// 04
{
  const s = slide(C.ink, "THE CANONICAL RULE", 4);
  quoteMark(s, 0.6, 0.75, 80, C.lime);
  body(s, "AI may request motion.", 1.32, 1.17, 10.8, 0.76, { size: 36, bold: true, color: C.white });
  body(s, "LEASH DECIDES WHETHER\nIT IS ALLOWED.", 1.32, 2.26, 10.7, 2.08, { size: 43, bold: true, color: C.lime });
  s.addShape(SH.line, { x: 1.34, y: 5.0, w: 10.8, h: 0, line: { color: C.white, width: 2 } });
  body(s, "This sentence is the architecture test.", 1.35, 5.33, 9.6, 0.5, { size: 21, bold: true, color: C.white });
  body(s, "If a feature violates it, the feature belongs somewhere else.", 1.35, 5.98, 10.8, 0.42, { size: 16, color: C.paper });
  footer(s, REPO);
  note(s, 4, "This is the canonical rule from the project README. I use it as a design filter: language models and planners produce candidates; only Leash can authorize a candidate for hardware execution.", [`${GH}/README.md`]);
}

// 05
{
  const s = slide(C.paper, "THE MACHINE", 5);
  title(s, "Pinkie is where the abstractions meet friction.", { size: 31 });
  photoFrame(s, "pinkie-live-camera-2026-09-01.jpg", 0.7, 1.65, 5.35, 4.0, C.pink, false);
  photoFrame(s, "waveshare-ugv-rover-angle.jpg", 7.25, 1.42, 5.0, 5.0, C.ink, true);
  pill(s, "LIVE CAMERA • 2026-09-01", 0.84, 5.9, 3.0, C.lime, C.ink, 10);
  body(s, "Waveshare UGV\nJetson Orin NX\nLiDAR + camera + IMU + odometry", 7.3, 5.72, 5.0, 0.82, { size: 14, bold: true });
  footer(s);
  note(s, 5, "Pinkie is the current robot. The left image is a live camera snapshot from today; the right is the official Waveshare product image. The important point is not the brand. It is that real serial buses, stale sensors, batteries, and motor controllers turn vague intent into concrete failure modes.", [WAVESHARE, `${GH}/implementations/waveshare-ugv/README.md`]);
}

// 06
{
  const s = slide(C.paleBlue, "THE HARDWARE CHAIN", 6);
  title(s, "Every layer narrows what the next layer may do.", { size: 30 });
  const ys = [1.65, 2.64, 3.63, 4.62, 5.61];
  const labels = [
    ["MISSION / HUMAN", C.paper2],
    ["HTTP • CLI • MCP", C.yellow],
    ["LEASH RUNTIME", C.lime],
    ["WAVESHARE ADAPTER", C.pink],
    ["UART → CONTROLLER → MOTORS", C.ink],
  ];
  labels.forEach(([label, fill], i) => {
    box(s, label, 1.15 + i * 0.68, ys[i], 8.55 - i * 1.36, 0.65, fill, { size: 16, color: fill === C.ink ? C.white : C.ink });
    if (i < labels.length - 1) arrow(s, 5.43, ys[i] + 0.68, 0, 0.27, C.ink, 2.2);
  });
  body(s, "REQUEST", 10.35, 1.75, 1.5, 0.34, { size: 13, bold: true, color: C.blue, align: "center" });
  arrow(s, 11.1, 2.2, 0, 2.85, C.blue, 4);
  body(s, "EFFECT", 10.35, 5.34, 1.5, 0.34, { size: 13, bold: true, color: C.blue, align: "center" });
  footer(s);
  note(s, 6, "Follow a motion request downward. Protocol parsing cannot write motors. The registry resolves a capability. Policy validates and authorizes. The runtime selects one adapter. Only that adapter owns the hardware path.", [`${GH}/README.md`, `${GH}/crates/leash-runtime/src/lib.rs`, `${GH}/implementations/waveshare-ugv/adapter.rs`]);
}

// 07
{
  const s = slide(C.yellow, "THE DEVICE OWNER", 7);
  title(s, "One serial owner. Zero mystery writers.", { size: 35 });
  const writers = [["CLI", 0.8], ["HTTP", 2.55], ["MCP", 4.3], ["NAV", 6.05]];
  writers.forEach(([t, x], i) => {
    box(s, t, x, 2.0 + (i % 2) * 1.0, 1.35, 0.78, i === 3 ? C.palePink : C.paper2, { size: 18 });
    arrow(s, x + 1.42, 2.4 + (i % 2), 2.4 - i * 0.3, 1.4 - (i % 2), C.ink, 2);
  });
  box(s, "LEASH\nDEVICE OWNER", 8.7, 2.18, 2.15, 2.05, C.lime, { size: 20 });
  arrow(s, 10.96, 3.2, 0.75, 0, C.ink, 3);
  box(s, "UART", 11.78, 2.72, 0.85, 0.95, C.ink, { size: 16, color: C.white });
  body(s, "All roads converge before bytes reach the controller.", 2.05, 5.25, 8.9, 0.62, { size: 24, bold: true, align: "center" });
  footer(s);
  note(s, 7, "The single-owner rule is operational, not philosophical. The runtime status reports the Waveshare controller owner. If another process owns the serial port, Leash cannot make a meaningful authority claim.", [`${GH}/crates/leash-runtime/src/lib.rs`, `${GH}/implementations/waveshare-ugv/adapter.rs`]);
}

// 08
{
  const s = slide(C.ink, "THE SURFACE", 8);
  title(s, "Three protocols. One capability contract.", { size: 32, color: C.white });
  box(s, "$ leash drive --linear 0.08 --for 400ms", 0.7, 1.7, 5.95, 0.9, C.paper2, { mono: true, size: 16, align: "left", pad: 0.25 });
  box(s, "POST /v1/capabilities/drive/propose", 6.85, 1.7, 5.75, 0.9, C.paleBlue, { mono: true, size: 15, align: "left", pad: 0.25 });
  box(s, "tools.call(\"drive.propose\", candidate)", 0.7, 3.0, 7.2, 0.9, C.palePink, { mono: true, size: 15, align: "left", pad: 0.25 });
  arrow(s, 3.7, 4.4, 3.0, 0, C.lime, 4);
  box(s, "Capability< Candidate → Authorized >", 6.85, 4.0, 5.75, 1.0, C.lime, { mono: true, size: 16 });
  body(s, "Syntax changes. Authority does not.", 0.72, 5.55, 8.0, 0.52, { size: 25, bold: true, color: C.white });
  footer(s, REPO);
  note(s, 8, "Leash exposes CLI, HTTP, and MCP-shaped access, but those are transports. The capability boundary is the stable idea. A caller submits a typed candidate and receives an authorization or a refusal—not direct hardware access.", [`${GH}/src/http.rs`, `${GH}/src/capability.rs`, `${GH}/src/runtime.rs`]);
}

// 09
{
  const s = slide(C.paper, "THE STACK", 9);
  title(s, "Policy above mechanism. Evidence beside both.", { size: 29 });
  const rows = [
    ["INTENT", "mission • operator • autonomy", C.paleBlue],
    ["CAPABILITY", "resolve • validate • authorize", C.yellow],
    ["SAFETY KERNEL", "deadline • E-stop • verified zero", C.lime],
    ["ADAPTER", "encode • transmit • read back", C.pink],
    ["HARDWARE", "controller • motors • sensors", C.ink],
  ];
  rows.forEach(([a, b, fill], i) => {
    const y = 1.45 + i * 0.93;
    box(s, a, 0.7, y, 2.45, 0.67, fill, { size: 15, color: fill === C.ink ? C.white : C.ink });
    body(s, b, 3.55, y + 0.16, 4.6, 0.33, { size: 16, bold: true, color: C.grey });
  });
  s.addShape(SH.rect, { x: 9.0, y: 1.45, w: 3.2, h: 4.38, fill: { color: C.blue }, line: { color: C.ink, width: 2.5 } });
  body(s, "EVIDENCE", 9.42, 1.86, 2.4, 0.45, { size: 23, bold: true, color: C.white, align: "center" });
  body(s, "proposal\ndecision\ntransition\ntelemetry\nresult", 9.5, 2.62, 2.2, 2.2, { size: 20, bold: true, color: C.white, align: "center" });
  body(s, "Append-only, not an afterthought.", 8.75, 6.18, 3.7, 0.36, { size: 14, bold: true, align: "center" });
  footer(s);
  note(s, 9, "This is the architectural split. Intent is rich and nondeterministic. Authorization and the safety kernel are small and deterministic. The adapter translates to hardware. Evidence records the whole crossing so a later replay can explain what happened.", [`${GH}/crates/leash-core/src`, `${GH}/crates/leash-runtime/src`, `${GH}/crates/leash-evidence/src`]);
}

// 10
{
  const s = slide(C.paleLime, "CAPABILITIES", 10);
  title(s, "A registry turns verbs into explicit authority.", { size: 31 });
  const caps = [["drive", C.pink], ["stop", C.red], ["observe", C.paleBlue], ["map", C.yellow], ["camera", C.paper2]];
  caps.forEach(([t, fill], i) => {
    const y = 1.45 + i * 0.95;
    box(s, t, 0.75, y, 2.0, 0.64, fill, { size: 17, color: fill === C.red ? C.white : C.ink });
    arrow(s, 2.88, y + 0.32, 1.15, 0, C.ink, 2.2);
  });
  s.addShape(SH.rect, { x: 4.2, y: 1.25, w: 3.45, h: 4.95, fill: { color: C.blue }, line: { color: C.ink, width: 3 } });
  body(s, "CAPABILITY\nREGISTRY", 4.55, 2.25, 2.75, 1.15, { size: 25, bold: true, color: C.white, align: "center" });
  body(s, "name → schema\npolicy → handler", 4.65, 3.72, 2.55, 0.95, { size: 17, mono: true, color: C.white, align: "center" });
  const outcomes = [["AUTHORIZED", C.lime, 8.45, 1.55], ["REJECTED", C.pink, 9.15, 3.2], ["UNAVAILABLE", C.yellow, 8.45, 4.85]];
  outcomes.forEach(([t, fill, x, y]) => {
    arrow(s, 7.78, 3.65, x - 7.88, y - 3.3, C.ink, 2.2);
    box(s, t, x, y, 3.1, 0.84, fill, { size: 16 });
  });
  footer(s);
  note(s, 10, "Capabilities name the effect and its contract. Unknown verbs do not fall through to magic. Unavailable hardware is not silently simulated. Each call ends as authorized, rejected, or unavailable, and that outcome can be recorded.", [`${GH}/src/capability.rs`, `${GH}/src/types.rs`]);
}

// 11
{
  const s = slide(C.paper, "AUTHORITY", 11);
  title(s, "Supertraits describe the hardware boundary.", { size: 34 });
  box(s, "pub trait ControllerIo:\n    Read + Write + Send {}\n\nimpl<T> ControllerIo for T\nwhere\n    T: Read + Write + Send\n{}", 0.65, 1.45, 7.1, 4.65, C.ink, { mono: true, size: 20, align: "left", color: C.white, pad: 0.36 });
  body(s, "Read + Write", 8.38, 1.75, 3.85, 0.42, { size: 24, bold: true, color: C.blue });
  body(s, "the byte-level contract", 8.4, 2.25, 3.7, 0.34, { size: 15, bold: true, color: C.grey });
  body(s, "Send", 8.38, 3.15, 3.85, 0.42, { size: 24, bold: true, color: C.pink });
  body(s, "safe to move into owner thread", 8.4, 3.65, 3.7, 0.42, { size: 15, bold: true, color: C.grey });
  body(s, "blanket impl", 8.38, 4.62, 3.85, 0.42, { size: 24, bold: true, color: C.ink });
  body(s, "any conforming I/O becomes usable", 8.4, 5.12, 3.7, 0.42, { size: 15, bold: true, color: C.grey });
  pill(s, "ZERO METHODS • STRONG BOUNDS", 8.4, 5.75, 3.65, C.lime, C.ink, 10);
  footer(s);
  note(s, 11, "This is real Leash syntax. ControllerIo is a marker trait with three supertraits: Read, Write, and Send. The blanket implementation means any type satisfying those bounds automatically implements ControllerIo. There are no methods to fake and no inheritance tree. The Send bound is architectural: ownership of the I/O object can move into the controller-owner thread.", [`${GH}/crates/leash-waveshare/src/lib.rs`]);
}

// 12
{
  const s = slide(C.ink, "THE KERNEL", 12);
  title(s, "The safety kernel is a deterministic loop—not a conversation.", { size: 29, color: C.white });
  const loop = [["READ", 1.0, 2.5, C.paleBlue], ["DECIDE", 4.25, 1.45, C.yellow], ["WRITE", 7.5, 2.5, C.pink], ["PROVE", 4.25, 4.45, C.lime]];
  loop.forEach(([t, x, y, fill]) => {
    box(s, t, x, y, 2.1, 0.9, fill, { size: 20 });
  });
  arrow(s, 3.12, 2.72, 1.05, -0.55, C.white, 3);
  arrow(s, 6.38, 2.17, 1.05, 0.55, C.white, 3);
  arrow(s, 8.55, 3.45, -2.1, 1.25, C.white, 3);
  arrow(s, 4.2, 4.72, -2.0, -1.2, C.white, 3);
  body(s, "10 ms tick", 10.2, 2.32, 2.1, 0.45, { size: 24, bold: true, color: C.lime, align: "center" });
  body(s, "bounded mailbox\npriority E-stop\nexplicit states", 10.2, 3.05, 2.1, 1.4, { size: 17, color: C.white, align: "center" });
  body(s, "A model may miss a token deadline.\nThe motor boundary may not.", 1.0, 5.85, 8.5, 0.48, { size: 22, bold: true, color: C.white });
  footer(s, REPO);
  note(s, 12, "Leash’s CPU supervisor runs with a default ten-millisecond tick. It reads bounded inputs, makes a deterministic transition, writes an effect, and records evidence. The language-model loop is outside this deadline.", [`${GH}/crates/leash-runtime/src/safety_supervisor.rs`]);
}

// 13
{
  const s = slide(C.paper, "RUST TYPES", 13);
  title(s, "Smart constructors close numeric ranges.", { size: 32 });
  box(s, "pub struct NormalizedDrive(f64);\n\nimpl NormalizedDrive {\n  pub fn new(value: f64)\n    -> Result<Self, DomainError>\n  {\n    if !value.is_finite() { … }\n    if !(-1.0..=1.0).contains(&value) { … }\n    Ok(Self(value))\n  }\n}", 0.65, 1.4, 7.15, 4.95, C.ink, { mono: true, size: 18.5, align: "left", color: C.white, pad: 0.34 });
  box(s, "f64", 8.55, 1.62, 1.35, 0.75, C.palePink, { mono: true, size: 21 });
  arrow(s, 10.02, 2.0, 0.65, 0, C.ink, 2.5);
  box(s, "Result", 10.8, 1.62, 1.55, 0.75, C.yellow, { mono: true, size: 19 });
  body(s, "private field", 8.38, 2.82, 3.85, 0.38, { size: 22, bold: true, color: C.blue });
  body(s, "outside modules cannot bypass new()", 8.4, 3.32, 3.7, 0.55, { size: 15, bold: true, color: C.grey });
  body(s, "finite + bounded", 8.38, 4.25, 3.85, 0.38, { size: 22, bold: true, color: C.pink });
  body(s, "NaN and ±∞ never reach encoding", 8.4, 4.75, 3.7, 0.55, { size: 15, bold: true, color: C.grey });
  pill(s, "INVALID AT CONSTRUCTION", 8.4, 5.78, 3.7, C.lime, C.ink, 10);
  footer(s);
  note(s, 13, "NormalizedDrive is a one-field tuple struct whose field is private. Callers cannot construct NormalizedDrive(2.0); they must use new, which rejects non-finite and out-of-range values and returns Result. After construction, the rest of the program can rely on the invariant instead of rechecking every float.", [`${GH}/crates/leash-core/src/drive.rs`]);
}

// 14
{
  const s = slide(C.pink, "BOUNDED EFFECTS", 14);
  title(s, "Generics encode the authority transition.", { size: 32 });
  box(s, "pub struct Candidate<C> {\n  pub deadline: MonotonicNanos,\n  pub command: C,\n}", 0.65, 1.55, 4.0, 2.2, C.paper2, { mono: true, size: 19, align: "left", pad: 0.32 });
  arrow(s, 4.88, 2.65, 1.05, 0, C.ink, 4);
  box(s, "SafetyGate\n.authorize(candidate, now)", 6.05, 1.76, 2.35, 1.8, C.yellow, { mono: true, size: 17 });
  arrow(s, 8.55, 2.65, 1.0, 0, C.ink, 4);
  box(s, "pub struct Authorized<C> {\n  command_id: CommandId,\n  evidence_id: EvidenceId,\n  authorized_at: MonotonicNanos,\n  command: C,\n}", 9.65, 1.35, 3.05, 2.65, C.lime, { mono: true, size: 14.5, align: "left", pad: 0.25 });
  box(s, "Err(SafetyDenial::Expired)", 4.88, 4.72, 3.95, 0.8, C.ink, { mono: true, size: 16, color: C.white });
  arrow(s, 7.22, 3.72, -0.45, 0.85, C.ink, 2.5);
  body(s, "C is generic: the transition works for any command payload.\nAuthorized fields are private: gateways cannot forge the post-policy state.", 0.82, 5.82, 11.6, 0.63, { size: 18, bold: true, align: "center" });
  footer(s);
  note(s, 14, "Candidate<C> and Authorized<C> carry the same generic command type C, but they are different states. authorize consumes the candidate, checks the gate and deadline, allocates evidence identity, and returns Authorized<C>. Authorized’s fields are private, so a transport cannot assemble one with a struct literal. The compile-fail doctest proves that forgery does not compile.", [`${GH}/crates/leash-core/src/drive.rs`]);
}

// 15
{
  const s = slide(C.paper, "STOP PATH", 15);
  title(s, "E-stop skips the queue. CPU keeps authority.", { size: 32 });
  const y = 2.0;
  box(s, "NORMAL\nPROPOSALS", 0.7, y, 2.3, 1.3, C.paleBlue, { size: 18 });
  box(s, "BOUNDED\nMAILBOX 32", 4.0, y, 2.3, 1.3, C.yellow, { size: 18 });
  box(s, "100 Hz CPU\nSUPERVISOR", 7.3, y, 2.3, 1.3, C.lime, { size: 18 });
  box(s, "MOTORS", 10.6, y, 2.0, 1.3, C.ink, { size: 20, color: C.white });
  arrow(s, 3.08, 2.65, 0.82, 0, C.ink, 2.5);
  arrow(s, 6.38, 2.65, 0.82, 0, C.ink, 2.5);
  arrow(s, 9.68, 2.65, 0.82, 0, C.ink, 2.5);
  box(s, "E-STOP", 3.95, 4.55, 2.35, 0.86, C.red, { size: 22, color: C.white });
  arrow(s, 6.42, 4.85, 1.98, -1.25, C.red, 5);
  body(s, "priority safety mailbox", 6.75, 4.75, 2.75, 0.32, { size: 13, bold: true, color: C.red, align: "center" });
  body(s, "Safety traffic is not ordinary traffic.", 3.2, 5.95, 7.0, 0.5, { size: 24, bold: true, align: "center" });
  footer(s);
  note(s, 15, "The supervisor has a bounded proposal channel and a separate priority safety path. Emergency stop is not allowed to wait behind ordinary motion requests. The CPU loop remains the final authority even when CUDA shadow computation is active.", [`${GH}/crates/leash-runtime/src/safety_supervisor.rs`, `${GH}/crates/leash-cuda/README.md`]);
}

// 16
{
  const s = slide(C.paleYellow, "OWNERSHIP", 16);
  title(s, "Associated types keep adapters concrete.", { size: 33 });
  box(s, "pub trait ActuationPort: Send + 'static {\n  type Acknowledgement:\n    ActuationAcknowledgement;\n  type Error:\n    Display + Send + 'static;\n\n  fn submit_drive(\n    &mut self,\n    command: Authorized<DifferentialDrive>,\n  ) -> Result<(), Self::Error>;\n}", 0.62, 1.35, 7.3, 5.28, C.ink, { mono: true, size: 17.5, align: "left", color: C.white, pad: 0.32 });
  body(s, "&mut self", 8.58, 1.68, 3.8, 0.4, { size: 25, bold: true, color: C.blue });
  body(s, "exclusive mutable access", 8.6, 2.2, 3.65, 0.36, { size: 15, bold: true, color: C.grey });
  body(s, "Self::Error", 8.58, 3.0, 3.8, 0.4, { size: 25, bold: true, color: C.pink });
  body(s, "adapter-specific, statically known", 8.6, 3.52, 3.65, 0.5, { size: 15, bold: true, color: C.grey });
  body(s, "Authorized<…>", 8.58, 4.45, 3.8, 0.4, { size: 25, bold: true, color: C.ink });
  body(s, "raw candidates cannot cross", 8.6, 4.98, 3.65, 0.4, { size: 15, bold: true, color: C.grey });
  pill(s, "DYNAMIC HARDWARE • STATIC CONTRACT", 8.48, 5.88, 3.95, C.lime, C.ink, 9.2);
  footer(s);
  note(s, 16, "ActuationPort uses associated types rather than trait generics. Each implementation chooses one acknowledgement and error type, so the supervisor can be generic over the port without erasing its concrete contract. The Send plus static bounds allow the port to live in the supervisor thread. submit_drive takes mutable self and only Authorized<DifferentialDrive>, enforcing exclusive access and post-policy input at the signature.", [`${GH}/crates/leash-runtime/src/supervisor.rs`, `${GH}/crates/leash-waveshare/src/lib.rs`]);
}

// 17
{
  const s = slide(C.paper, "SENSORS", 17);
  title(s, "Freshness is a first-class input to motion.", { size: 34 });
  const sensors = [
    ["LIDAR", "range + clearance", C.lime],
    ["CAMERA", "context + evidence", C.paleBlue],
    ["IMU", "orientation + dynamics", C.yellow],
    ["ODOMETRY", "motion + displacement", C.pink],
    ["BATTERY", "83.3% live", C.paper2],
  ];
  sensors.forEach(([a, b, fill], i) => {
    const x = 0.72 + (i % 3) * 4.15;
    const y = 1.62 + Math.floor(i / 3) * 2.05;
    s.addShape(SH.ellipse, { x, y, w: 0.78, h: 0.78, fill: { color: fill }, line: { color: C.ink, width: 2.3 } });
    body(s, String(i + 1), x, y + 0.2, 0.78, 0.3, { size: 18, bold: true, align: "center" });
    body(s, a, x + 1.02, y + 0.02, 2.7, 0.35, { size: 19, bold: true });
    body(s, b, x + 1.02, y + 0.48, 2.7, 0.42, { size: 14, color: C.grey });
  });
  s.addShape(SH.line, { x: 0.75, y: 5.75, w: 11.85, h: 0, line: { color: C.ink, width: 2 } });
  body(s, "STALE ≠ SAFE", 0.78, 6.0, 2.6, 0.42, { size: 22, bold: true, color: C.red });
  body(s, "A value without an age is not a control input.", 3.5, 6.03, 7.4, 0.4, { size: 19, bold: true });
  footer(s);
  note(s, 17, "Today’s read-only sensor endpoint reported fresh LiDAR, camera, IMU, and odometry plus 83.3 percent battery. The design lesson is that a sensor value must carry freshness and health; stale but plausible values are dangerous.", [`${GH}/src/types.rs`, `${GH}/src/http.rs`]);
}

// 18
{
  const s = slide(C.blue, "EVIDENCE", 18);
  title(s, "Replay should explain the crossing—not invent a story.", { size: 29, color: C.white });
  const events = [["PROPOSAL", C.paper2], ["POLICY", C.yellow], ["TRANSITION", C.lime], ["WRITE", C.pink], ["TELEMETRY", C.paleBlue], ["RESULT", C.paper2]];
  events.forEach(([t, fill], i) => {
    const x = 0.62 + i * 2.06;
    box(s, t, x, 2.45, 1.65, 0.78, fill, { size: 12.5 });
    if (i < events.length - 1) arrow(s, x + 1.7, 2.84, 0.28, 0, C.white, 2.3);
    body(s, `${i + 1}`, x + 0.58, 1.82, 0.5, 0.34, { size: 16, bold: true, color: C.white, align: "center" });
  });
  s.addShape(SH.line, { x: 0.72, y: 4.35, w: 11.75, h: 0, line: { color: C.white, width: 2 } });
  body(s, "4,807,319", 0.75, 4.72, 3.2, 0.75, { size: 36, bold: true, color: C.lime });
  body(s, "durable records on live Pinkie", 0.78, 5.52, 4.1, 0.35, { size: 15, bold: true, color: C.white });
  body(s, "Evidence is useful only if it survives\nthe process that made the decision.", 6.1, 4.72, 5.85, 1.05, { size: 23, bold: true, color: C.white, align: "right" });
  footer(s, REPO);
  note(s, 18, "The live runtime reported more than 4.8 million durable records. The point is not the number; it is the sequence. Replay should reconstruct the request, authorization, state transition, hardware write, telemetry, and result without asking an agent to narrate what it thinks happened.", [`${GH}/crates/leash-evidence/src`, `${GH}/crates/leash-runtime/src`]);
}

// 19
{
  const s = slide(C.paper, "STATE", 19);
  title(s, "Safety is a state machine with visible exits.", { size: 34 });
  const states = [
    ["IDLE", 0.75, 2.25, C.paper2],
    ["ARMED", 3.35, 1.3, C.yellow],
    ["MOVING", 6.15, 2.25, C.lime],
    ["STOPPING", 8.95, 1.3, C.pink],
    ["VERIFIED\nZERO", 10.55, 3.7, C.paleBlue],
    ["FAULT", 4.25, 4.7, C.red],
  ];
  states.forEach(([t, x, y, fill]) => box(s, t, x, y, 2.0, 0.95, fill, { size: 16, color: fill === C.red ? C.white : C.ink }));
  arrow(s, 2.83, 2.55, 0.42, -0.62);
  arrow(s, 5.45, 1.82, 0.62, 0.65);
  arrow(s, 8.25, 2.48, 0.62, -0.65);
  arrow(s, 10.35, 2.22, 0.82, 1.35);
  arrow(s, 10.45, 4.22, -4.1, 0.9, C.red, 2.5);
  arrow(s, 4.35, 4.72, -2.3, -1.35, C.red, 2.5);
  body(s, "timeout • E-stop • stale sensor • ownership loss", 3.2, 6.1, 7.1, 0.35, { size: 16, bold: true, color: C.red, align: "center" });
  footer(s);
  note(s, 19, "The safety lifecycle is explicit. Motion is not a boolean. Arm, move, stop, verify zero, and fault are different states with defined exits. Timeouts, E-stop, stale sensors, and ownership loss all have an explicit route away from motion.", [`${GH}/crates/leash-runtime/src/safety_supervisor.rs`, `${GH}/crates/leash-core/src/drive.rs`]);
}

// 20
{
  const s = slide(C.lime, "VERIFIED STOP", 20);
  title(s, "A stop command is a request. Zero is the proof.", { size: 32 });
  body(s, "37.622 ms", 0.65, 1.65, 6.3, 1.0, { size: 46, bold: true });
  body(s, "physical E-stop acknowledgement", 0.7, 2.66, 5.8, 0.4, { size: 18, bold: true });
  s.addShape(SH.line, { x: 0.75, y: 3.55, w: 11.65, h: 0, line: { color: C.ink, width: 3 } });
  const xs = [1.0, 3.6, 6.2, 8.8, 11.4];
  const labs = ["REQUEST", "ACK", "WRITE 0", "READBACK", "PROOF"];
  xs.forEach((x, i) => {
    dot(s, x, 4.33, 0.52, i === 4 ? C.blue : C.paper2);
    body(s, labs[i], x - 0.4, 5.08, 1.32, 0.3, { size: 11, bold: true, align: "center", color: i === 4 ? C.blue : C.ink });
    if (i < xs.length - 1) arrow(s, x + 0.55, 4.59, xs[i + 1] - x - 0.72, 0, C.ink, 2.2);
  });
  body(s, "The evidence bundle also verified final zero.", 5.95, 1.88, 6.05, 0.55, { size: 23, bold: true, align: "right" });
  pill(s, "MEASURED ON JETSON ORIN NX", 4.85, 6.05, 3.55, C.paper2, C.ink, 10.5);
  footer(s);
  note(s, 20, "In the physical rollout evidence, E-stop acknowledgement was measured at 37.622 milliseconds and the final state was verified zero. This is the distinction: sending a stop is not the success condition. Observing and recording zero is.", [`${GH}/crates/leash-runtime/evidence/jetson-orin-nx-rv2-16-physical-rollout-20260829.json`]);
}

// 21
{
  const s = slide(C.ink, "CUDA", 21);
  title(s, "CUDA accelerates. CPU owns motion.", { size: 34, color: C.white });
  box(s, "CPU\nAUTHORITATIVE", 0.8, 2.05, 3.2, 2.05, C.lime, { size: 24 });
  box(s, "CUDA\nSHADOW", 5.05, 2.05, 3.2, 2.05, C.blue, { size: 24, color: C.white });
  box(s, "MOTOR\nWRITE", 9.3, 2.05, 3.2, 2.05, C.paper2, { size: 24 });
  arrow(s, 4.12, 3.08, 0.8, 0, C.white, 3);
  arrow(s, 8.38, 3.08, 0.8, 0, C.white, 3);
  s.addShape(SH.line, { x: 5.34, y: 4.65, w: 2.6, h: 0, line: { color: C.red, width: 5 } });
  body(s, "NO DIRECT AUTHORITY", 5.18, 4.9, 2.95, 0.34, { size: 12, bold: true, color: C.red, align: "center" });
  body(s, "promote only after parity + performance evidence", 2.45, 5.85, 8.45, 0.38, { size: 18, bold: true, color: C.white, align: "center" });
  footer(s, REPO);
  note(s, 21, "CUDA is active on Pinkie, but the live runtime reports CPU final authority. The CUDA design is shadow-first: compare results, measure performance, and fall back. Acceleration is not permission, and GPU availability is not a reason to widen the authority surface.", [`${GH}/crates/leash-cuda/README.md`, `${GH}/crates/leash-cuda/evidence/jetson-orin-nx-rv2-13-20260829.json`]);
}

// 22
{
  const s = slide(C.paleBlue, "INTEGRATION", 22);
  title(s, "Phantom types stop coordinate frames from drifting.", { size: 30 });
  box(s, "pub enum Map {}\npub enum Odom {}\n\npub struct Frame<Tag> {\n  name: FrameName,\n  marker: PhantomData<fn() -> Tag>,\n}\n\npub struct Pose2<Tag> {\n  frame: Frame<Tag>,\n  x: Meters, y: Meters, yaw: Radians,\n}", 0.62, 1.3, 7.15, 5.42, C.ink, { mono: true, size: 17.2, align: "left", color: C.white, pad: 0.32 });
  box(s, "Pose2<Map>", 8.42, 1.62, 3.6, 0.82, C.lime, { mono: true, size: 21 });
  box(s, "Pose2<Odom>", 8.42, 3.0, 3.6, 0.82, C.yellow, { mono: true, size: 21 });
  s.addShape(SH.line, { x: 8.62, y: 4.3, w: 3.18, h: 0, line: { color: C.red, width: 5, beginArrowType: "triangle", endArrowType: "triangle" } });
  body(s, "won’t unify", 9.2, 4.52, 2.0, 0.34, { size: 16, bold: true, color: C.red, align: "center" });
  box(s, "Nav2 path → typed proposal → Leash", 8.15, 5.42, 4.2, 0.78, C.pink, { mono: true, size: 14.5 });
  footer(s);
  note(s, 22, "Map and Odom are zero-variant marker enums. Frame<Tag> carries PhantomData, so the tag exists at compile time without runtime storage. Pose2<Map> and Pose2<Odom> are different types; the compile-fail doctest proves they cannot be exchanged silently. The ROS2 bridge converts Nav2 output into typed proposals, but Leash still authorizes the effect.", [`${GH}/crates/leash-core/src/frame.rs`, `${GH}/crates/leash-ros2/src/lib.rs`]);
}

// 23
{
  const s = slide(C.paper, "MEASURED", 23);
  title(s, "The boundary is small enough to measure.", { size: 35 });
  const metrics = [
    ["58,306 ns", "p99 CPU transition", C.lime],
    ["0", "deadline misses at 100 Hz", C.paleBlue],
    ["110,293/s", "durable evidence records", C.yellow],
    ["37.622 ms", "physical E-stop acknowledgement", C.pink],
  ];
  metrics.forEach(([v, l, fill], i) => {
    const x = 0.68 + (i % 2) * 6.18;
    const y = 1.52 + Math.floor(i / 2) * 2.4;
    s.addShape(SH.rect, { x, y, w: 5.75, h: 1.75, fill: { color: fill }, line: { color: C.ink, width: 2.5 } });
    body(s, v, x + 0.3, y + 0.24, 5.15, 0.58, { size: 31, bold: true });
    body(s, l, x + 0.32, y + 1.06, 5.1, 0.32, { size: 15, bold: true });
  });
  body(s, "JETSON ORIN NX • RECORDED EVIDENCE • NOT A SIMULATION", 2.6, 6.38, 8.1, 0.34, { size: 13, bold: true, color: C.blue, align: "center" });
  footer(s);
  note(s, 23, "These are measured artifacts committed with Leash: 58,306 nanoseconds p99 transition latency, zero deadline misses at 100 hertz, 110,293 durable records per second in the evidence run, and 37.622 milliseconds physical E-stop acknowledgement. They are not promises for every machine; they are reproducible evidence from this Jetson.", [`${GH}/crates/leash-runtime/evidence/jetson-orin-nx-rv2-16-nomotion-20260829.json`, `${GH}/crates/leash-runtime/evidence/jetson-orin-nx-evidence-20260829.json`, `${GH}/crates/leash-runtime/evidence/jetson-orin-nx-rv2-16-physical-rollout-20260829.json`]);
}

// 24
{
  const s = slide(C.yellow, "OPEN SOURCE", 24);
  title(s, "Leash is a boundary you can replace piece by piece.", { size: 30 });
  const pieces = [
    ["CORE", "types + contracts", C.paper2, 0.8, 1.7],
    ["RUNTIME", "policy + supervisor", C.lime, 4.75, 1.7],
    ["ADAPTER", "hardware encoding", C.pink, 8.7, 1.7],
    ["EVIDENCE", "append + replay", C.paleBlue, 2.78, 4.0],
    ["CUDA", "shadow acceleration", C.blue, 6.73, 4.0],
  ];
  pieces.forEach(([a, b, fill, x, y]) => {
    box(s, a, x, y, 3.05, 1.15, fill, { size: 21, color: fill === C.blue ? C.white : C.ink });
    body(s, b, x + 0.18, y + 1.32, 2.7, 0.32, { size: 13, bold: true, align: "center" });
  });
  arrow(s, 3.95, 2.28, 0.7, 0);
  arrow(s, 7.9, 2.28, 0.7, 0);
  arrow(s, 5.25, 3.0, -0.65, 0.85);
  arrow(s, 7.55, 3.0, 0.65, 0.85);
  pill(s, "MIT LICENSE", 5.15, 6.05, 3.0, C.ink, C.white, 13);
  footer(s);
  note(s, 24, "Leash is MIT licensed and modular. Core types, runtime policy, hardware adapters, evidence, and CUDA can evolve independently so long as they preserve the authority contract. The goal is not a universal robotics framework; it is a sharp boundary that can fit inside different stacks.", [`${GH}/LICENSE`, `${GH}/Cargo.toml`, REPO]);
}

// 25
{
  const s = slide(C.paper, "HONEST STATUS", 25);
  title(s, "Live today is not the same as claimed tomorrow.", { size: 31 });
  body(s, "LIVE + VERIFIED", 0.78, 1.48, 5.55, 0.42, { size: 22, bold: true, color: C.blue });
  body(s, "NOT CLAIMED", 7.0, 1.48, 5.55, 0.42, { size: 22, bold: true, color: C.red });
  const live = ["controller ownership", "CPU final authority", "fresh LiDAR / IMU / odometry", "camera snapshot", "CUDA available in shadow", "durable evidence"];
  const not = ["whole-house autonomy", "active mapping at this moment", "visual odometry lock", "GPU motor authority", "RL policy in the control loop", "semantic room understanding"];
  live.forEach((t, i) => {
    dot(s, 0.82, 2.18 + i * 0.68, 0.3, C.lime);
    body(s, t, 1.3, 2.12 + i * 0.68, 5.15, 0.35, { size: 16, bold: true });
  });
  not.forEach((t, i) => {
    dot(s, 7.02, 2.18 + i * 0.68, 0.3, C.pink);
    body(s, t, 7.5, 2.12 + i * 0.68, 5.05, 0.35, { size: 16, bold: true });
  });
  s.addShape(SH.line, { x: 6.68, y: 1.38, w: 0, h: 4.9, line: { color: C.ink, width: 2 } });
  footer(s);
  note(s, 25, "For the demo, I am separating live proof from future direction. Today we verified ownership, CPU authority, fresh sensors, camera, CUDA availability, and evidence. Mapping is currently initializing and visual odometry is unavailable, so I will not claim autonomous mapping or SLAM lock.", [`${GH}/README.md`]);
}

// 26
{
  const s = slide(C.ink, "QUALIA • 2 MINUTES", 26);
  title(s, "Qualia thinks on a slower clock.", { size: 35, color: C.white });
  photoFrame(s, "qualia-world-current.png", 0.65, 1.45, 8.4, 4.12, C.lime, true);
  box(s, "LEASH\n10 ms", 9.65, 1.55, 2.6, 1.2, C.lime, { size: 22 });
  arrow(s, 10.95, 2.88, 0, 0.85, C.white, 3);
  box(s, "QUALIA\nseconds → minutes", 9.65, 3.85, 2.6, 1.3, C.blue, { size: 19, color: C.white });
  body(s, "scene • mission • ontology • learning", 1.2, 5.96, 7.6, 0.4, { size: 18, bold: true, color: C.white, align: "center" });
  footer(s, REPO);
  note(s, 26, "Qualia is intentionally outside Leash. It works on scene understanding, missions, ontologies, and longer-lived learning. Those asynchronous updates may improve future proposals, but they do not enter the ten-millisecond safety loop or gain motor authority.", [REPO]);
}

// 27
{
  const s = slide(C.paper, "THE HANDOFF", 27);
  title(s, "Intelligence crosses as a typed proposal.", { size: 33 });
  photoFrame(s, "hermes-terminal-current.png", 0.65, 1.55, 5.0, 2.68, C.blue, true);
  box(s, "MISSION", 0.85, 5.0, 2.0, 0.78, C.paleBlue, { size: 17 });
  arrow(s, 2.98, 5.38, 0.8, 0);
  box(s, "CANDIDATE", 3.9, 5.0, 2.0, 0.78, C.yellow, { size: 17 });
  arrow(s, 6.03, 5.38, 0.8, 0);
  box(s, "LEASH", 6.95, 5.0, 2.0, 0.78, C.lime, { size: 17 });
  arrow(s, 9.08, 5.38, 0.8, 0);
  box(s, "EFFECT", 10.0, 5.0, 2.0, 0.78, C.pink, { size: 17 });
  s.addShape(SH.rect, { x: 6.4, y: 1.5, w: 5.7, h: 2.68, fill: { color: C.ink }, line: { color: C.ink, width: 2.5 } });
  body(s, "{\n  \"linear\": 0.08,\n  \"angular\": 0.0,\n  \"duration_ms\": 400,\n  \"reason\": \"bounded demo\"\n}", 6.78, 1.83, 4.95, 2.08, { size: 17, mono: true, color: C.white });
  footer(s);
  note(s, 27, "This is the entire relationship in one slide. Hermes or Qualia can form a mission and candidate. The candidate includes bounded magnitude, duration, and context. Leash validates and either authorizes an effect or records a refusal. The mission system remains replaceable.", [`${GH}/src/types.rs`, `${GH}/src/capability.rs`, REPO]);
}

// 28
{
  const s = slide(C.yellow, "LIVE DEMO", 28);
  title(s, "Observe. Approve. Pulse. Prove zero.", { size: 36 });
  const steps = [
    ["1", "READ-ONLY\nPREFLIGHT", C.paper2],
    ["2", "HUMAN\nAPPROVAL", C.paleBlue],
    ["3", "≤ 0.10\n≤ 500 ms", C.pink],
    ["4", "VERIFY\nSTOP", C.lime],
    ["5", "SHOW\nEVIDENCE", C.blue],
  ];
  steps.forEach(([n, t, fill], i) => {
    const x = 0.48 + i * 2.56;
    dot(s, x + 0.72, 1.65, 0.65, fill === C.blue ? C.lime : C.blue);
    body(s, n, x + 0.72, 1.82, 0.65, 0.3, { size: 17, bold: true, color: fill === C.blue ? C.ink : C.white, align: "center" });
    box(s, t, x, 2.65, 2.1, 1.5, fill, { size: 17, color: fill === C.blue ? C.white : C.ink });
    if (i < steps.length - 1) arrow(s, x + 2.16, 3.4, 0.32, 0, C.ink, 2.5);
  });
  s.addShape(SH.line, { x: 0.68, y: 4.85, w: 12.0, h: 0, line: { color: C.ink, width: 2.5 } });
  body(s, "ANY RED GATE → RECORDED FALLBACK", 0.75, 5.25, 5.6, 0.42, { size: 20, bold: true, color: C.red });
  body(s, "No improvising around the boundary.", 6.6, 5.25, 5.7, 0.42, { size: 20, bold: true, align: "right" });
  pill(s, "NO AUTONOMY CLAIM", 4.97, 6.18, 3.25, C.ink, C.white, 11);
  footer(s);
  note(s, 28, "The demo sequence is intentionally boring. First a read-only preflight. Then an explicit human approval and operator token. One low-speed pulse, no more than 0.10 normalized drive and no more than 500 milliseconds. Then verified stop and evidence. If any gate is red, I use the recorded fallback instead of improvising.", [`${GH}/crates/leash-runtime/evidence/jetson-orin-nx-rv2-16-physical-rollout-20260829.json`]);
}

// 29
{
  const s = slide(C.blue, "TAKE IT WITH YOU", 29);
  body(s, "THE PLANNER\nPROPOSES.", 0.7, 0.95, 6.3, 1.55, { size: 38, bold: true, color: C.white });
  body(s, "THE BOUNDARY\nDECIDES.", 0.7, 2.75, 6.3, 1.55, { size: 38, bold: true, color: C.lime });
  body(s, "Questions?", 0.75, 5.2, 4.5, 0.62, { size: 31, bold: true, color: C.white });
  body(s, "github.com/specdog/leash", 0.78, 6.0, 5.1, 0.35, { size: 16, bold: true, color: C.white });
  s.addShape(SH.rect, { x: 8.3, y: 0.92, w: 4.2, h: 4.2, fill: { color: C.white }, line: { color: C.ink, width: 3 } });
  if (fs.existsSync(qrPath)) {
    s.addImage({ path: qrPath, x: 8.52, y: 1.14, w: 3.76, h: 3.76 });
  } else {
    body(s, "QR\nADDED AFTER\nPUBLICATION", 8.78, 2.1, 3.2, 1.4, { size: 24, bold: true, align: "center" });
  }
  pill(s, "DECK + NOTES + EVIDENCE", 8.55, 5.45, 3.7, C.yellow, C.ink, 10.5);
  body(s, qrExpiry, 8.45, 6.08, 3.9, 0.34, { size: 10.5, bold: true, color: C.white, align: "center" });
  footer(s, REPO);
  note(s, 29, "The takeaway is the rule: the planner proposes and the boundary decides. The QR points directly to this deck’s PDF and expires on the date shown. The repository is public and MIT licensed. Thank you—questions are welcome.", [REPO, RAILWAY_BUCKETS]);
}

fs.writeFileSync(notesOutput, `# Geek on a Leash — Speaker Notes\n\n${notes.join("\n")}`, "utf8");
await pptx.writeFile({ fileName: output, compression: true });
console.log(JSON.stringify({ output, slides: pptx._slides.length, notes: notes.length, qr: fs.existsSync(qrPath) ? qrPath : null }));
