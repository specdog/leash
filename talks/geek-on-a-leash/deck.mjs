import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import PptxGenJS from "pptxgenjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const assets = path.join(here, "assets");
const output = path.resolve(process.argv[2] || path.join(here, ".build", "geek-on-a-leash.pptx"));
const notesOutput = path.join(here, "output", "speaker-notes.md");
const qrPath = process.env.TALK_QR_PNG || path.join(here, "output", "geek-on-a-leash-qr.png");
const qrExpiry = process.env.TALK_QR_EXPIRES || "QR added after publication preflight";

fs.mkdirSync(path.dirname(output), { recursive: true });
fs.mkdirSync(path.dirname(notesOutput), { recursive: true });

const pptx = new PptxGenJS();
pptx.defineLayout({ name: "WIDE", width: 13.333, height: 7.5 });
pptx.layout = "WIDE";
pptx.author = "Eric Manganaro";
pptx.company = "specdog";
pptx.subject = "A source-level Rust Tuesdays talk about Leash and physical authority";
pptx.title = "Geek on a Leash — Rust Tuesdays";
pptx.lang = "en-US";
pptx.theme = { headFontFace: "Arial Black", bodyFontFace: "Arial", lang: "en-US" };

const C = {
  paper: "F7F2E8",
  paper2: "FFFDF7",
  ink: "0B0B0B",
  code: "101419",
  codeLine: "28313A",
  blue: "2855FF",
  cyan: "5FD7FF",
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
const BASE_SHA = "566bc569b24bf5f392291b142469282fcdfac2b3";
const GH = `https://github.com/specdog/leash/blob/${BASE_SHA}`;
const REPO = "https://github.com/specdog/leash";
const WAVESHARE = "https://www.waveshare.com/product/ai/robots/ugv-rover-pt-jetson-orin-ai-kit.htm";
const notes = [];

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
    fit: opts.fit ?? "shrink",
    lineSpacingMultiple: 1,
  });
}

function slide(bg = C.paper, section = "RUST TUESDAYS", number = null) {
  const s = pptx.addSlide();
  s.background = { color: bg };
  s.addShape(SH.line, { x: 0.32, y: 0.29, w: 12.69, h: 0, line: { color: C.ink, width: 1.25 } });
  body(s, section, 0.43, 0.08, 4.4, 0.19, { size: 9.5, bold: true });
  body(s, "RUST TUESDAYS", 5.2, 0.08, 3.0, 0.19, { size: 9.5, bold: true, align: "center", color: C.blue });
  if (number !== null) body(s, String(number).padStart(2, "0"), 12.45, 0.08, 0.4, 0.19, { size: 9.5, bold: true, mono: true, align: "right" });
  return s;
}

function title(s, text, opts = {}) {
  body(s, text, opts.x ?? 0.55, opts.y ?? 0.58, opts.w ?? 12.2, opts.h ?? 0.75, {
    size: opts.size ?? 28,
    bold: true,
    color: opts.color ?? C.ink,
    valign: "mid",
  });
}

function footer(s, text = `specdog/leash • ${BASE_SHA.slice(0, 8)}`) {
  body(s, text, 0.56, 7.16, 6.4, 0.14, { size: 8.5, bold: true, color: C.grey });
}

function pill(s, text, x, y, w, fill = C.lime, color = C.ink, size = 11) {
  s.addShape(SH.roundRect, { x, y, w, h: 0.4, rectRadius: 0.06, fill: { color: fill }, line: { color: C.ink, width: 1.4 } });
  body(s, text, x + 0.08, y + 0.07, w - 0.16, 0.23, { size, bold: true, color, align: "center", valign: "mid" });
}

function box(s, text, x, y, w, h, fill = C.paper2, opts = {}) {
  s.addShape(SH.rect, { x, y, w, h, fill: { color: fill }, line: { color: opts.lineColor ?? C.ink, width: opts.lineWidth ?? 2.1 } });
  body(s, text, x + (opts.pad ?? 0.17), y + (opts.pad ?? 0.15), w - 2 * (opts.pad ?? 0.17), h - 2 * (opts.pad ?? 0.15), {
    size: opts.size ?? 18,
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

function photo(s, file, x, y, w, h, accent = C.ink) {
  const image = path.join(assets, file);
  if (!fs.existsSync(image)) return;
  s.addShape(SH.rect, { x: x - 0.07, y: y - 0.07, w: w + 0.14, h: h + 0.14, fill: { color: C.paper2 }, line: { color: accent, width: 2.7 } });
  s.addImage({ path: image, x, y, w, h });
}

function note(s, number, talk, sources = []) {
  const sourceText = sources.length ? `\n\n[Sources]\n${sources.map((url) => `- ${url}`).join("\n")}` : "";
  const all = `${talk}${sourceText}`;
  s.addNotes(all);
  notes.push(`## ${String(number).padStart(2, "0")}\n\n${all}\n`);
}

function codeBlock(s, label, code, x, y, w, h, opts = {}) {
  s.addShape(SH.rect, { x, y, w, h, fill: { color: C.code }, line: { color: opts.accent ?? C.ink, width: 2.1 } });
  s.addShape(SH.rect, { x, y, w, h: 0.39, fill: { color: opts.accent ?? C.lime }, line: { color: opts.accent ?? C.ink, width: 0 } });
  body(s, label, x + 0.13, y + 0.09, w - 0.26, 0.2, { size: 10.5, bold: true, mono: true, color: C.ink });
  body(s, code.trim(), x + 0.18, y + 0.56, w - 0.36, h - 0.72, {
    size: opts.size ?? 16.5,
    mono: true,
    color: opts.color ?? C.white,
    fit: "shrink",
  });
}

function takeaways(s, items, x = 9.08, y = 1.58, w = 3.72, h = 4.98, accent = C.lime) {
  const gap = 0.13;
  const each = (h - gap * (items.length - 1)) / items.length;
  items.forEach((item, index) => {
    const iy = y + index * (each + gap);
    s.addShape(SH.rect, { x, y: iy, w, h: each, fill: { color: index === 0 ? accent : C.paper2 }, line: { color: C.ink, width: 1.8 } });
    body(s, item, x + 0.18, iy + 0.13, w - 0.36, each - 0.26, { size: index === 0 ? 18 : 16, bold: true, valign: "mid" });
  });
}

function codeSlide(number, section, heading, file, code, items, talk, sources, opts = {}) {
  const s = slide(opts.bg ?? C.paper, section, number);
  title(s, heading, { size: opts.titleSize ?? 27 });
  codeBlock(s, file, code, 0.55, 1.52, opts.codeW ?? 8.25, 5.35, { accent: opts.accent ?? C.lime, size: opts.codeSize ?? 16.5 });
  takeaways(s, items, 9.05, 1.52, 3.73, 5.35, opts.accent ?? C.lime);
  footer(s);
  note(s, number, talk, sources);
  return s;
}

// 01 — title
{
  const s = slide(C.paper, "GEEK ON A LEASH", 1);
  s.addShape(SH.rect, { x: 0.46, y: 0.58, w: 7.5, h: 5.92, fill: { color: C.blue }, line: { color: C.ink, width: 3 } });
  body(s, "GEEK ON\nA LEASH", 0.78, 0.88, 6.6, 2.1, { size: 43, bold: true, color: C.white });
  body(s, "A source-level Rust talk about\nphysical authority", 0.82, 3.36, 6.45, 1.15, { size: 24, bold: true, color: C.white });
  pill(s, "RUST TUESDAYS • 60 MIN", 0.82, 5.5, 2.95, C.lime, C.ink, 11);
  photo(s, "waveshare-ugv-rover-front.jpg", 8.52, 0.77, 4.12, 4.12);
  body(s, "ERIC MANGANARO", 8.58, 5.18, 3.6, 0.34, { size: 18, bold: true });
  body(s, "Leash + Pinkie", 8.58, 5.62, 2.8, 0.28, { size: 14, color: C.grey });
  if (fs.existsSync(qrPath)) s.addImage({ path: qrPath, x: 11.2, y: 5.17, w: 1.22, h: 1.22 });
  footer(s, "github.com/specdog/leash");
  note(s, 1, "This is the Rust Tuesdays version: substantially more source code, fewer broad architecture slides, and a line-by-line tour of the language mechanisms that make Leash safe enough to sit near motors.", [REPO, WAVESHARE]);
}

// 02 — authority question
{
  const s = slide(C.yellow, "THE BOUNDARY", 2);
  title(s, "Who is allowed to write the motors?", { size: 34 });
  box(s, "HUMAN", 0.65, 2.0, 2.35, 1.15, C.paper2, { size: 22 });
  box(s, "AGENT", 0.65, 4.4, 2.35, 1.15, C.paleBlue, { size: 22 });
  arrow(s, 3.15, 2.58, 2.0, 1.42, C.ink, 3);
  arrow(s, 3.15, 4.97, 2.0, -1.42, C.ink, 3);
  box(s, "TYPE\nBOUNDARY", 5.25, 2.52, 2.4, 2.2, C.pink, { size: 23 });
  arrow(s, 7.82, 3.62, 1.85, 0, C.ink, 3);
  box(s, "MOTOR\nDRIVER", 9.82, 2.52, 2.82, 2.2, C.ink, { size: 24, color: C.white });
  body(s, "The dangerous bug is ambiguous authority.", 3.45, 5.75, 6.25, 0.48, { size: 22, bold: true, align: "center" });
  footer(s);
  note(s, 2, "The core problem is authority, not intelligence. Several producers may propose motion, but only one narrow boundary may authorize it. Rust lets us represent that authority in types rather than conventions.", [`${GH}/README.md`]);
}

// 03 — canonical rule
{
  const s = slide(C.paper, "THE RULE", 3);
  title(s, "The canonical rule is intentionally boring.", { size: 32 });
  box(s, "request", 0.7, 2.45, 2.0, 1.15, C.paleBlue, { size: 22, mono: true });
  arrow(s, 2.86, 3.02, 1.15, 0, C.ink, 3);
  box(s, "propose", 4.08, 2.45, 2.0, 1.15, C.paleYellow, { size: 22, mono: true });
  arrow(s, 6.24, 3.02, 1.15, 0, C.ink, 3);
  box(s, "authorize", 7.46, 2.45, 2.15, 1.15, C.palePink, { size: 22, mono: true });
  arrow(s, 9.78, 3.02, 1.15, 0, C.ink, 3);
  box(s, "actuate", 11.0, 2.45, 1.7, 1.15, C.lime, { size: 20, mono: true });
  body(s, "No adapter, planner, or GPU kernel may skip a verb.", 1.35, 4.55, 10.6, 0.7, { size: 27, bold: true, align: "center", color: C.blue });
  footer(s);
  note(s, 3, "Four verbs define the boundary. The rest of this talk asks how Rust preserves that order across generics, traits, threads, drivers, serialization, ROS2, and CUDA.", [`${GH}/README.md`]);
}

// 04 — hardware
{
  const s = slide(C.paper, "PINKIE", 4);
  title(s, "The types terminate in real hardware.", { size: 31 });
  photo(s, "waveshare-ugv-rover-angle.jpg", 0.72, 1.52, 5.1, 4.75, C.blue);
  photo(s, "pinkie-live-camera-2026-09-01.jpg", 6.2, 1.52, 3.35, 2.55, C.pink);
  box(s, "Jetson Orin NX", 9.9, 1.52, 2.62, 0.82, C.lime, { size: 18 });
  box(s, "serial motor MCU", 9.9, 2.58, 2.62, 0.82, C.paleYellow, { size: 17, mono: true });
  box(s, "LiDAR + camera", 9.9, 3.64, 2.62, 0.82, C.paleBlue, { size: 18 });
  body(s, "A bad abstraction becomes\na moving machine.", 6.28, 4.62, 5.9, 1.1, { size: 27, bold: true });
  footer(s);
  note(s, 4, "Pinkie is the concrete forcing function: Jetson compute, motor controller, LiDAR, camera, and an actual chassis. Every language decision we discuss has a physical consequence at this boundary.", [WAVESHARE]);
}

// 05 — crate graph
{
  const s = slide(C.paper, "WORKSPACE", 5);
  title(s, "The crate graph is an authority graph.", { size: 31 });
  box(s, "leash-core\nno dependencies", 4.9, 1.43, 3.05, 1.0, C.lime, { size: 20, mono: true });
  const crates = [
    ["runtime", 0.62, 3.0, C.palePink],
    ["gateway", 3.08, 3.0, C.paleBlue],
    ["ros2", 5.54, 3.0, C.paleYellow],
    ["waveshare", 8.0, 3.0, C.paleLime],
    ["cuda", 10.46, 3.0, C.palePink],
  ];
  crates.forEach(([name, x, y, fill]) => {
    box(s, `leash-${name}`, x, y, 2.18, 0.92, fill, { size: 16.5, mono: true });
    arrow(s, x + 1.09, y - 0.08, 5.43 - (x + 1.09), -0.48, C.ink, 1.8);
  });
  box(s, "replay", 5.54, 4.75, 2.18, 0.92, C.paper2, { size: 17, mono: true });
  arrow(s, 6.63, 4.66, 0, -0.6, C.ink, 1.8);
  body(s, "Dependencies point inward. Physical authority stays narrow.", 1.55, 6.02, 10.2, 0.5, { size: 23, bold: true, align: "center" });
  footer(s);
  note(s, 5, "The workspace structure is not packaging trivia. Core defines the legal vocabulary. Runtime owns authority. Adapters and accelerators depend inward, so they cannot redefine the rules without creating a visible dependency violation.", [`${GH}/Cargo.toml`, `${GH}/crates/leash-core/Cargo.toml`, `${GH}/crates/leash-runtime/Cargo.toml`]);
}

// 06 — why Rust
{
  const s = slide(C.blue, "WHY RUST", 6);
  title(s, "Use the compiler to make illegal motion awkward.", { size: 32, color: C.white });
  const rows = [
    ["NEWTYPE", "validate once; keep raw floats out"],
    ["TYPESTATE", "authorization cannot be forged"],
    ["TRAITS", "hardware varies; contract does not"],
    ["OWNERSHIP", "one thread owns physical I/O"],
    ["DROP", "shutdown is part of the type lifecycle"],
  ];
  rows.forEach(([left, right], index) => {
    const y = 1.48 + index * 0.96;
    box(s, left, 0.7, y, 2.35, 0.72, index % 2 ? C.pink : C.lime, { size: 16, mono: true });
    body(s, right, 3.38, y + 0.13, 8.7, 0.4, { size: 22, bold: true, color: C.white });
  });
  footer(s, "Rust Tuesdays • types are part of the control plane");
  note(s, 6, "Rust is not valuable here because it is fashionable or merely fast. It is valuable because validation, ownership, and lifecycle rules can be made structural and checked before the robot is powered.", [`${GH}/crates/leash-core/src/drive.rs`, `${GH}/crates/leash-runtime/src/supervisor.rs`]);
}

// 07 — unsafe policy
codeSlide(7, "UNSAFE POLICY", "Unsafe is denied by default, then isolated.", "crate roots", String.raw`
// leash-runtime/src/lib.rs
#![forbid(unsafe_code)]

// leash-waveshare/src/lib.rs
#![forbid(unsafe_code)]

// leash-cuda/src/lib.rs
#![deny(unsafe_code)]

#[cfg(feature = "cuda")]
#[allow(unsafe_code)]
mod device;`, [
  "The default is a compiler error.",
  "The exception is named and feature-gated.",
  "Review the island, not the ocean.",
], "Start with unsafe policy. Runtime and hardware adapters forbid it. CUDA denies it at the crate root, then opens one explicitly named module. That makes the unsafe surface searchable and reviewable.", [`${GH}/crates/leash-runtime/src/lib.rs`, `${GH}/crates/leash-waveshare/src/lib.rs`, `${GH}/crates/leash-cuda/src/lib.rs`], { accent: C.pink, codeSize: 17 });

// 08 — newtype validation
codeSlide(8, "NEWTYPES", "A private field turns validation into a constructor.", "leash-core/src/drive.rs", String.raw`
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd)]
pub struct NormalizedDrive(f64);

impl NormalizedDrive {
    pub const ZERO: Self = Self(0.0);

    pub fn new(value: f64) -> Result<Self, DomainError> {
        if !value.is_finite() {
            return Err(DomainError::NonFinite("normalized drive"));
        }
        if !(-1.0..=1.0).contains(&value) {
            return Err(DomainError::OutOfRange("normalized drive"));
        }
        Ok(Self(value))
    }
}`, [
  "Tuple field is private.",
  "All construction crosses one gate.",
  "Downstream code receives a proof.",
], "The newtype is the first important move. NormalizedDrive is as cheap as f64 at runtime, but its private tuple field means external code must call the validating constructor. After construction, the rest of the system can treat range and finiteness as already proven.", [`${GH}/crates/leash-core/src/drive.rs`], { accent: C.lime, codeSize: 16.2 });

// 09 — derive semantics
codeSlide(9, "DATA SEMANTICS", "Derives are an API decision, not decoration.", "leash-core/src/drive.rs", String.raw`
#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd)]
pub struct NormalizedDrive(f64);

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DifferentialDrive {
    pub left: NormalizedDrive,
    pub right: NormalizedDrive,
}

impl DifferentialDrive {
    pub const STOP: Self = Self {
        left: NormalizedDrive::ZERO,
        right: NormalizedDrive::ZERO,
    };
}`, [
  "Copy is safe because the value is tiny and immutable.",
  "PartialEq supports exact protocol tests.",
  "STOP is a canonical, allocation-free value.",
], "Derives encode semantics. Copy says this value has no unique resource identity. Debug makes evidence legible. PartialEq makes transition tests precise. The module may construct STOP directly because it owns the private invariant.", [`${GH}/crates/leash-core/src/drive.rs`], { accent: C.cyan, codeSize: 16.2 });

// 10 — unit macro
codeSlide(10, "MACROS", "One macro stamps the same invariant onto every unit.", "leash-core/src/units.rs", String.raw`
macro_rules! finite_unit {
    ($name:ident, $label:literal) => {
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct $name(f64);

        impl $name {
            pub fn new(value: f64) -> Result<Self, DomainError> {
                if !value.is_finite() {
                    return Err(DomainError::NonFinite($label));
                }
                Ok(Self(value))
            }
        }
    };
}`, [
  "macro_rules! removes invariant drift.",
  "Each expansion is still a distinct type.",
  "Meters cannot silently become radians.",
], "This is a good use of macro_rules: generate repetitive, inspectable domain types while preserving nominal type separation. The macro removes boilerplate without erasing the unit distinction.", [`${GH}/crates/leash-core/src/units.rs`], { accent: C.yellow, codeSize: 15.7 });

// 11 — NonZeroU64 sequence
codeSlide(11, "VALID STATES", "NonZeroU64 removes an invalid state from memory.", "leash-core/src/time.rs", String.raw`
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sequence(NonZeroU64);

impl Sequence {
    pub fn new(value: u64) -> Result<Self, DomainError> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(DomainError::Zero("sequence"))
    }

    pub fn next(self) -> Result<Self, DomainError> {
        self.get()
            .checked_add(1)
            .ok_or(DomainError::Overflow("sequence"))
            .and_then(Self::new)
    }
}`, [
  "Zero cannot be represented.",
  "checked_add makes overflow explicit.",
  "Option says absence is ordinary, not exceptional.",
], "Sequence uses a standard-library niche type rather than a comment saying zero is reserved. new converts Option from NonZeroU64 into a domain Result. next composes checked arithmetic with and_then so zero and overflow stay explicit.", [`${GH}/crates/leash-core/src/time.rs`], { accent: C.lime, codeSize: 15.9 });

// 12 — monotonic time
codeSlide(12, "TIME", "Time arithmetic is checked and domain-specific.", "leash-core/src/time.rs", String.raw`
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonotonicNanos(u64);

impl MonotonicNanos {
    pub fn checked_add(self, duration: DurationNanos)
        -> Result<Self, DomainError> {
        self.0
            .checked_add(duration.0)
            .map(Self)
            .ok_or(DomainError::Overflow("monotonic timestamp"))
    }

    pub fn duration_since(self, earlier: Self)
        -> Result<DurationNanos, DomainError> {
        self.0
            .checked_sub(earlier.0)
            .map(DurationNanos)
            .ok_or(DomainError::TimeReversed)
    }
}`, [
  "Monotonic is not wall-clock time.",
  "Subtraction cannot go negative unnoticed.",
  "Overflow is visible in the return type.",
], "A u64 timestamp is easy to misuse. The newtype names the clock domain, and every operation is checked. A caller must handle overflow or reversed time explicitly instead of receiving a wrapped value.", [`${GH}/crates/leash-core/src/time.rs`], { accent: C.cyan, codeSize: 14.7 });

// 13 — generic map
codeSlide(13, "GENERICS", "Stamped<T>::map moves metadata without cloning it.", "leash-core/src/time.rs", String.raw`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamped<T> {
    pub at: MonotonicNanos,
    pub sequence: Sequence,
    pub value: T,
}

impl<T> Stamped<T> {
    pub fn map<U>(self, transform: impl FnOnce(T) -> U) -> Stamped<U> {
        Stamped {
            at: self.at,
            sequence: self.sequence,
            value: transform(self.value),
        }
    }
}`, [
  "self is consumed: no partial alias survives.",
  "FnOnce permits transforms that consume captures.",
  "Only the payload type changes.",
], "This small generic method shows ownership doing useful work. Consuming self allows the envelope fields to move directly. FnOnce is the least restrictive correct callback bound because the transform is invoked exactly once.", [`${GH}/crates/leash-core/src/time.rs`], { accent: C.pink, codeSize: 15.2 });

// 14 — candidate
codeSlide(14, "PROPOSALS", "Candidate<C> carries intent, identity, and a deadline.", "leash-core/src/drive.rs", String.raw`
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate<C> {
    pub id: CommandId,
    pub issued_at: MonotonicNanos,
    pub deadline: MonotonicNanos,
    pub command: C,
}

impl<C> Candidate<C> {
    pub const fn new(
        id: CommandId,
        issued_at: MonotonicNanos,
        deadline: MonotonicNanos,
        command: C,
    ) -> Self {
        Self { id, issued_at, deadline, command }
    }
}`, [
  "C keeps the envelope command-agnostic.",
  "CommandId binds producer epoch and sequence.",
  "A proposal is data, never authority.",
], "Candidate is generic over the command payload, but identity and timing are shared. The constructor remains a plain data constructor: possession of Candidate never implies permission to actuate.", [`${GH}/crates/leash-core/src/drive.rs`], { accent: C.paleBlue, codeSize: 15.7 });

// 15 — Authorized typestate
codeSlide(15, "TYPESTATE", "Authorization is a value you cannot construct outside core.", "leash-core/src/drive.rs", String.raw`
#[derive(Debug, Clone, PartialEq)]
pub struct Authorized<C> {
    command_id: CommandId,
    evidence_id: EvidenceId,
    authorized_at: MonotonicNanos,
    command: C,
}

impl<C> Authorized<C> {
    pub const fn command_id(&self) -> CommandId {
        self.command_id
    }

    pub const fn evidence_id(&self) -> EvidenceId {
        self.evidence_id
    }

    pub fn command(&self) -> &C {
        &self.command
    }
}
`, [
  "Private fields block struct literals downstream.",
  "Accessors expose facts, not construction power.",
  "The type is a capability token.",
], "Authorized is a typestate boundary. The runtime can require this type at the motor port. Adapters can inspect it, but only the defining module can create it. That changes authorization from a boolean convention into possession of an unforgeable value.", [`${GH}/crates/leash-core/src/drive.rs`], { accent: C.lime, codeSize: 15.7 });

// 16 — authorize
codeSlide(16, "CONTROL FLOW", "The gate consumes a proposal and returns a capability.", "leash-core/src/drive.rs", String.raw`
pub fn authorize<C>(
    &mut self,
    candidate: Candidate<C>,
    now: MonotonicNanos,
) -> Result<Authorized<C>, SafetyDenial> {
    match self.state {
        SafetyState::Disarmed => return Err(SafetyDenial::Disarmed),
        SafetyState::EStopped => return Err(SafetyDenial::EStopped),
        SafetyState::Faulted => return Err(SafetyDenial::Faulted),
        SafetyState::Ready | SafetyState::Moving => {}
    }

    if candidate.issued_at > now {
        return Err(SafetyDenial::IssuedInFuture);
    }
    if candidate.deadline < now {
        return Err(SafetyDenial::Expired);
    }

    let evidence_id = self.next_evidence_id()?;
    Ok(Authorized {
        command_id: candidate.id,
        evidence_id,
        authorized_at: now,
        command: candidate.command,
    })
}`, [
  "candidate is moved: it cannot be reused accidentally.",
  "match is exhaustive over safety state.",
  "? exits on the first failed proof.",
], "Look at the function signature first. It consumes Candidate<C> and either returns Authorized<C> or a typed denial. The body is an audit trail: safety state, temporal direction, deadline, evidence sequence, then construction of the capability.", [`${GH}/crates/leash-core/src/drive.rs`], { accent: C.pink, codeSize: 13.9 });

// 17 — compile fail
codeSlide(17, "COMPILE-FAIL TESTS", "The public API is tested by code that must not compile.", "leash-core/src/drive.rs doctest", String.raw`
/// \`\`\`compile_fail
/// use leash_core::DifferentialDrive;
/// let _command = DifferentialDrive::new(0.5, 0.5);
/// \`\`\`

/// \`\`\`compile_fail
/// use leash_core::{Authorized, DifferentialDrive};
/// let _forged = Authorized::<DifferentialDrive> {
///     command: DifferentialDrive::STOP,
/// };
/// \`\`\``, [
  "Documentation becomes a negative contract.",
  "Refactors fail if the forbidden path opens.",
  "The compiler is part of the test harness.",
], "Compile-fail doctests protect absence: raw floats must not enter the drive command, and downstream code must not forge authorization. Runtime tests cannot prove those APIs are impossible; compiler tests can.", [`${GH}/crates/leash-core/src/drive.rs`], { accent: C.red, codeSize: 15.6 });

// 18 — PhantomData
codeSlide(18, "PHANTOM TYPES", "Frame<Tag> stores no tag at runtime—and still separates frames.", "leash-core/src/frame.rs", String.raw`
pub enum Map {}
pub enum Odom {}
pub enum Base {}
pub enum Sensor {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame<Tag> {
    name: FrameName,
    marker: PhantomData<fn() -> Tag>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pose2<Tag> {
    pub frame: Frame<Tag>,
    pub x: Meters,
    pub y: Meters,
    pub yaw: Radians,
}`, [
  "Empty enums are type-level labels.",
  "PhantomData ties Tag to ownership/type rules.",
  "No tag bytes are stored in Pose2.",
], "Map, Odom, Base, and Sensor are uninhabited marker types. PhantomData tells the compiler that Frame logically depends on Tag even though no Tag value exists at runtime. The result is zero-cost coordinate-frame separation.", [`${GH}/crates/leash-core/src/frame.rs`], { accent: C.cyan, codeSize: 15.0 });

// 19 — frame compile fail
codeSlide(19, "PHANTOM TYPES", "A pose in Odom is not a pose in Map.", "leash-core/src/frame.rs doctest", String.raw`
/// \`\`\`compile_fail
/// use leash_core::{Frame, FrameName, Map, Meters, Odom, Pose2, Radians};
/// fn consume_map(_: Pose2<Map>) {}
/// let odom = Frame::<Odom>::new(FrameName::new("odom").unwrap());
/// let pose = Pose2::new(
///     odom,
///     Meters::new(0.0).unwrap(),
///     Meters::new(0.0).unwrap(),
///     Radians::new(0.0).unwrap(),
/// );
/// consume_map(pose);
/// \`\`\`
`, [
  "Same fields do not mean same semantics.",
  "The missing transform becomes a compiler error.",
  "Zero runtime checks; strong compile-time separation.",
], "Robotics code is full of structurally identical coordinates with incompatible meaning. A phantom frame tag makes the missing transform visible at the call site. The explicit conversion becomes a reviewable operation instead of an assumption.", [`${GH}/crates/leash-core/src/frame.rs`], { accent: C.yellow, codeSize: 15.3 });

// 20 — supertrait blanket impl
codeSlide(20, "TRAITS", "Supertraits define the minimum hardware capability.", "leash-waveshare/src/lib.rs", String.raw`
pub trait ControllerIo: Read + Write + Send {}

impl<T> ControllerIo for T
where
    T: Read + Write + Send,
{}

pub trait ControllerIoFactory: Send {
    fn open(&mut self) -> io::Result<Box<dyn ControllerIo>>;
}`, [
  "Read + Write + Send is the complete capability set.",
  "The blanket impl admits every compatible transport.",
  "Box<dyn Trait> erases the concrete serial type.",
], "ControllerIo is a trait alias pattern on stable Rust. The blanket implementation means serial ports, test doubles, and future transports participate automatically if they satisfy Read, Write, and Send.", [`${GH}/crates/leash-waveshare/src/lib.rs`], { accent: C.lime, codeSize: 15.5 });

// 21 — factory closure
codeSlide(21, "TRAIT OBJECTS", "A closure can be the reconnection factory.", "leash-waveshare/src/lib.rs", String.raw`
pub trait ControllerIoFactory: Send {
    fn open(&mut self) -> io::Result<Box<dyn ControllerIo>>;
}

impl<F> ControllerIoFactory for F
where
    F: FnMut() -> io::Result<Box<dyn ControllerIo>> + Send,
{
    fn open(&mut self) -> io::Result<Box<dyn ControllerIo>> {
        self()
    }
}
`, [
  "FnMut permits reconnect state to evolve.",
  "Send permits ownership transfer to the I/O thread.",
  "The trait object keeps Supervisor non-generic at runtime.",
], "The factory itself is object-safe and the closure blanket implementation keeps call sites light. FnMut is deliberate: opening a connection may update retry counters or consume mutable configuration.", [`${GH}/crates/leash-waveshare/src/lib.rs`], { accent: C.pink, codeSize: 15.0 });

// 22 — associated types
codeSlide(22, "ASSOCIATED TYPES", "The actuation port chooses its acknowledgement protocol.", "leash-runtime/src/supervisor.rs", String.raw`
pub trait ActuationAcknowledgement: Send + 'static {
    fn applied(&self) -> bool;
    fn verified_zero(&self) -> bool;

    fn command_id(&self) -> Option<CommandId> { None }
    fn evidence_id(&self) -> Option<EvidenceId> { None }
    fn applied_sequence(&self) -> Option<u64> { None }
}

pub trait ActuationPort: Send + 'static {
    type Acknowledgement: ActuationAcknowledgement;
    type Error: fmt::Display + Send + 'static;

    fn submit_drive(
        &mut self,
        command: Authorized<DifferentialDrive>,
    ) -> Result<(), Self::Error>;

    fn try_acknowledgement(
        &mut self,
    ) -> Result<Option<Self::Acknowledgement>, Self::Error>;
}`, [
  "Associated types bind one Ack and Error to each port.",
  "The port accepts only Authorized<Drive>.",
  "'static permits the whole port to live on a thread.",
], "Associated types are better than extra generic parameters here because each concrete port has one canonical acknowledgement and error type. The signature also makes the typestate boundary unavoidable: raw DifferentialDrive is not accepted.", [`${GH}/crates/leash-runtime/src/supervisor.rs`], { accent: C.cyan, codeSize: 14.5 });

// 23 — generic supervisor
codeSlide(23, "GENERIC BOUNDS", "An equality bound connects the supervisor to the port.", "leash-runtime/src/supervisor.rs", String.raw`
pub struct CpuSafetySupervisor<A>
{
    handle: SupervisorHandle,
    events: Option<LatestReader<SupervisorEvent<A>>>,
    worker: Option<JoinHandle<()>>,
}

impl<A> CpuSafetySupervisor<A>
where
    A: ActuationAcknowledgement,
{
    pub fn spawn<P>(
        kernel: ControlKernel,
        port: P,
        clock: Box<dyn Clock + Send>,
        config: SupervisorConfig,
    ) -> Result<Self, SupervisorStartError>
    where
        P: ActuationPort<Acknowledgement = A>,
    {
        Self::spawn_inner(kernel, port, clock, config, None)
    }
}`, [
  "A is named once on the supervisor state.",
  "P stays generic only at construction.",
  "Acknowledgement = A is an associated-type equality.",
], "The supervisor stores acknowledgement state, so A belongs on the struct. The concrete port P is only needed to spawn the owner thread. The equality bound proves that whatever P emits is exactly the A the shared state can hold.", [`${GH}/crates/leash-runtime/src/supervisor.rs`], { accent: C.lime, codeSize: 15.0 });

// 24 — dispatch boundary
{
  const s = slide(C.paper, "DISPATCH", 24);
  title(s, "Static dispatch inside; dynamic dispatch at hardware seams.", { size: 30 });
  codeBlock(s, "MONOMORPHIZED", String.raw`
fn run_supervisor<P>(
    mut port: P,
    // ...
) where
    P: ActuationPort,
{
    // specialized for P
}`, 0.62, 1.55, 5.8, 3.7, { accent: C.lime, size: 17.5 });
  codeBlock(s, "ERASED", String.raw`
fn controller_loop(
    io: Box<dyn ControllerIo>,
) {
    // one runtime-selected I/O object
}`, 6.9, 1.55, 5.8, 3.7, { accent: C.pink, size: 17.5 });
  pill(s, "FAST CONTROL PATH", 1.65, 5.62, 3.65, C.lime, C.ink, 12);
  pill(s, "REPLACEABLE ADAPTER", 7.95, 5.62, 3.65, C.pink, C.ink, 12);
  body(s, "Choose dispatch based on ownership and change boundaries—not dogma.", 1.1, 6.32, 11.1, 0.4, { size: 21, bold: true, align: "center" });
  footer(s);
  note(s, 24, "Leash uses both dispatch models. The supervisor control loop remains generic and monomorphized. The transport is selected at runtime behind Box dyn ControllerIo. The useful question is where substitution happens and who owns the value.", [`${GH}/crates/leash-runtime/src/supervisor.rs`, `${GH}/crates/leash-waveshare/src/lib.rs`]);
}

// 25 — ownership into thread
codeSlide(25, "THREAD OWNERSHIP", "move transfers the physical port into one owner thread.", "leash-runtime/src/supervisor.rs [abridged]", String.raw`
let panic_shared = Arc::clone(&shared);
let thread_shared = Arc::clone(&shared);

let worker = thread::Builder::new()
    .name("leash-cpu-safety".to_string())
    .spawn(move || {
        let _ = thread_shared.worker_thread.set(thread::current());
        let result = std::panic::catch_unwind(
            std::panic::AssertUnwindSafe(|| {
                run_supervisor(
                    kernel, port, clock, config,
                    proposal_receiver, safety_receiver,
                    event_publisher, evidence, shared,
                );
            }),
        );
        if result.is_err() {
            panic_shared.faulted.store(true, Ordering::Release);
            panic_shared.closed.store(true, Ordering::Release);
        }
    })
    .map_err(|error| SupervisorStartError::Thread(error.to_string()))?;`, [
  "move gives the thread exclusive port ownership.",
  "Arc shares state, not mutable hardware access.",
  "catch_unwind converts panic into a fail-closed outcome.",
], "The owner thread receives port by move, so the caller cannot keep using it. Shared observation crosses the boundary through Arc. The closure is wrapped in catch_unwind so a panic becomes explicit supervisor state rather than silent loss of the actuator owner.", [`${GH}/crates/leash-runtime/src/supervisor.rs`], { accent: C.pink, codeSize: 13.9 });

// 26 — Drop lifecycle
codeSlide(26, "RAII", "Drop makes thread shutdown part of ownership.", "leash-runtime/src/supervisor.rs", String.raw`
impl<A> Drop for CpuSafetySupervisor<A>
{
    fn drop(&mut self) {
        self.handle.shared.shutdown.store(true, Ordering::Release);
        self.handle.shared.wake();

        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}`, [
  "Dropping sets the shutdown flag.",
  "Wake prevents a sleeping loop from hanging.",
  "Option::take guarantees one join attempt.",
], "RAII matters beyond memory. When the supervisor owner is dropped, it signals shutdown, wakes the worker, takes the JoinHandle out of its Option, and joins exactly once. Lifecycle behavior is co-located with lifecycle ownership.", [`${GH}/crates/leash-runtime/src/supervisor.rs`], { accent: C.lime, codeSize: 16.2 });

// 27 — bounded lane types
codeSlide(27, "BOUNDED CONCURRENCY", "Backpressure errors return ownership to the sender.", "leash-runtime/src/lane.rs", String.raw`
pub enum OverflowPolicy {
    RejectNewest,
    DropOldest,
}

pub enum SendOutcome<T> {
    Enqueued,
    ReplacedOldest(T),
}

pub enum SendError<T> {
    Closed(T),
    Full(T),
}

struct LaneState<T> {
    queue: VecDeque<T>,
    high_watermark: usize,
    sent: u64,
    received: u64,
    rejected: u64,
    dropped: u64,
}

struct LaneShared<T> {
    capacity: usize,
    policy: OverflowPolicy,
    closed: AtomicBool,
    state: Mutex<LaneState<T>>,
}`, [
  "The queue has a hard capacity.",
  "Errors preserve the rejected T.",
  "Replacement reports the evicted T.",
], "Boundedness is a correctness property for a real-time-ish control loop. Notice the ownership-aware result types: a failed send returns the value, and drop-oldest reports what was replaced. Nothing disappears implicitly.", [`${GH}/crates/leash-runtime/src/lane.rs`], { accent: C.cyan, codeSize: 16.0 });

// 28 — overflow match
codeSlide(28, "BOUNDED CONCURRENCY", "Overflow policy is an exhaustive match, not a comment.", "leash-runtime/src/lane.rs [abridged]", String.raw`
pub fn try_send(&self, value: T)
    -> Result<SendOutcome<T>, SendError<T>> {
    if self.shared.closed.load(Ordering::Acquire) {
        return Err(SendError::Closed(value));
    }

    let mut state = lock(&self.shared.state);
    if self.shared.closed.load(Ordering::Acquire) {
        return Err(SendError::Closed(value));
    }

    let outcome = if state.queue.len() == self.shared.capacity {
        match self.shared.policy {
        OverflowPolicy::RejectNewest => {
            state.rejected = state.rejected.saturating_add(1);
            return Err(SendError::Full(value));
        }
        OverflowPolicy::DropOldest => {
            let replaced = state.queue.pop_front()
                .expect("a full bounded queue contains an item");
            state.dropped = state.dropped.saturating_add(1);
            state.queue.push_back(value);
            SendOutcome::ReplacedOldest(replaced)
        }
        }
    } else {
        state.queue.push_back(value);
        SendOutcome::Enqueued
    };
    Ok(outcome)
}`, [
  "Check closed before taking the mutex.",
  "The match documents both overload semantics.",
  "No unbounded queue can consume the device.",
], "The control path chooses an overload policy explicitly. RejectNewest preserves older work. DropOldest preserves freshness. The caller sees which happened through the result type, so overload can enter evidence instead of becoming hidden latency.", [`${GH}/crates/leash-runtime/src/lane.rs`], { accent: C.yellow, codeSize: 13.8 });

// 29 — atomics mailbox
codeSlide(29, "ATOMICS", "Safety requests bypass the ordinary proposal lane.", "leash-runtime/src/safety.rs", String.raw`
struct SafetyShared {
    stop: AtomicU64,
    estop: AtomicU64,
    closed: AtomicBool,
}

impl SafetySender {
    pub fn request(&self, kind: SafetyKind)
        -> Result<u64, SafetyRequestError> {
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(SafetyRequestError::Closed);
        }
        let sequence = match kind {
            SafetyKind::Stop => increment(&self.shared.stop),
            SafetyKind::EStop => increment(&self.shared.estop),
        }?;
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(SafetyRequestError::Closed);
        }
        Ok(sequence)
    }
}

fn increment(counter: &AtomicU64) -> Result<u64, SafetyRequestError> {
    counter.fetch_update(Ordering::AcqRel, Ordering::Acquire,
        |current| current.checked_add(1))
        .map(|previous| previous + 1)
        .map_err(|_| SafetyRequestError::SequenceExhausted)
}`, [
  "Safety has a dedicated mailbox.",
  "AcqRel makes increment a read-modify-write boundary.",
  "fetch_update prevents sequence wraparound.",
], "Stop and E-stop do not compete with normal proposals for bounded-lane space. Each kind has an atomic request count. fetch_update publishes a monotonic sequence and rejects exhaustion instead of wrapping.", [`${GH}/crates/leash-runtime/src/safety.rs`], { accent: C.red, codeSize: 13.8 });

// 30 — priority
codeSlide(30, "PRIORITY", "The receiver checks E-stop before stop.", "leash-runtime/src/safety.rs", String.raw`
pub fn try_recv(&mut self)
    -> Result<Option<SafetySignal>, SafetyReceiveError> {
    let estop = self.shared.estop.load(Ordering::Acquire);
    if estop > self.seen_estop {
        let signal = signal(SafetyKind::EStop, self.seen_estop, estop);
        self.seen_estop = estop;
        return Ok(Some(signal));
    }

    let stop = self.shared.stop.load(Ordering::Acquire);
    if stop > self.seen_stop {
        let signal = signal(SafetyKind::Stop, self.seen_stop, stop);
        self.seen_stop = stop;
        return Ok(Some(signal));
    }

    if self.shared.closed.load(Ordering::Acquire) {
        return Err(SafetyReceiveError::Closed);
    }
    Ok(None)
}`, [
  "E-stop wins if both flags are present.",
  "Per-kind counters preserve coalesced requests.",
  "Closed is distinct from no pending signal.",
], "Priority is visible in control flow. E-stop is loaded before stop and returns immediately. Each counter is compared with its own seen watermark, preserving request counts without allocating a queue.", [`${GH}/crates/leash-runtime/src/safety.rs`], { accent: C.red, codeSize: 14.2 });

// 31 — latest slot
codeSlide(31, "FRESHNESS", "Option::replace makes latest-only semantics explicit.", "leash-runtime/src/latest.rs", String.raw`
pub fn publish(&self, value: Stamped<T>)
    -> Result<Option<Stamped<T>>, PublishError<T>> {
    let mut state = lock(&self.state);

    if state.last_sequence
        .is_some_and(|sequence| value.sequence <= sequence) {
        state.rejected_out_of_order =
            state.rejected_out_of_order.saturating_add(1);
        return Err(PublishError::SequenceNotIncreasing(value));
    }

    state.last_sequence = Some(value.sequence);
    state.published = state.published.saturating_add(1);
    let replaced = state.value.replace(value);
    if replaced.is_some() {
        state.replaced = state.replaced.saturating_add(1);
    }
    Ok(replaced)
}

pub fn take(&mut self) -> Option<Stamped<T>> {
    let mut state = lock(&self.state);
    let value = state.value.take();
    if value.is_some() {
        state.taken = state.taken.saturating_add(1);
    }
    value
}`, [
  "Stale sequence numbers are rejected.",
  "replace returns the displaced observation.",
  "take moves the newest value out exactly once.",
], "Some data should queue; high-rate observations often should not. The latest slot models that directly. Option replace and take express displacement and consumption without sentinel values or cloning.", [`${GH}/crates/leash-runtime/src/latest.rs`], { accent: C.cyan, codeSize: 14.7 });

// 32 — serde command enum
codeSlide(32, "WIRE CONTRACTS", "Serde turns the network shape into an enum.", "leash-gateway/src/lib.rs", String.raw`
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(
    tag = "command",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CommandRequest {
    Authorize {
        operator: String,
        expires_at_ns: u64,
    },
    Drive {
        left: f64,
        right: f64,
        deadline_ns: u64,
    },
    Stop {},
    EStop {},
    ResetEStop { approved: bool },
    SetPlannerActive { active: bool },
    CancelPlanner {},
}

`, [
  "Tagged enum gives one explicit command discriminator.",
  "deny_unknown_fields rejects schema drift.",
  "Decode errors enter the typed error path.",
], "The gateway does not pass loosely typed JSON inward. A tagged enum defines the allowed command set. Unknown fields fail closed, and serde errors are mapped into a domain error before any command reaches runtime.", [`${GH}/crates/leash-gateway/src/lib.rs`], { accent: C.pink, codeSize: 14.9 });

// 33 — error pipeline
codeSlide(33, "ERRORS", "map_err and ? preserve context without nesting.", "leash-gateway/src/lib.rs", String.raw`
pub enum GatewayError {
    InvalidJson(Box<str>),
    InvalidDomain(Box<str>),
    Proposal(SupervisorSubmitError),
    Safety(Box<str>),
    Timeout,
    Supervisor(Box<str>),
    Encode(Box<str>),
}

pub fn decode_and_execute(&self, json: &[u8])
    -> Result<Vec<u8>, GatewayError> {
    let request = serde_json::from_slice(json)
        .map_err(|error| GatewayError::InvalidJson(
            error.to_string().into_boxed_str()
        ))?;
    let response = self.execute(request)?;
    serde_json::to_vec(&response)
        .map_err(|error| GatewayError::Encode(
            error.to_string().into_boxed_str()
        ))
}`, [
  "The enum retains the failure category.",
  "? keeps the happy path visually flat.",
  "Boundary details become owned Box<str> values.",
], "This is idiomatic error plumbing with a safety benefit: failures stay categorized. map_err adapts errors at subsystem boundaries, while the question-mark operator exits immediately and keeps the success path easy to audit.", [`${GH}/crates/leash-gateway/src/lib.rs`], { accent: C.yellow, codeSize: 14.7 });

// 34 — let else
codeSlide(34, "LET-ELSE", "A missing acknowledgement ticket exits immediately.", "leash-waveshare/src/lib.rs", String.raw`
fn try_acknowledgement(&mut self)
    -> Result<Option<Self::Acknowledgement>, Self::Error> {
    let Some(ticket) = self.pending.front() else {
        return Ok(None);
    };

    match ticket.try_take().map_err(PortError::Submit)? {
        Some(ack) => {
            self.pending.pop_front();
            Ok(Some(ack))
        }
        None => Ok(None),
    }
}`, [
  "let-else handles the precondition at the top.",
  "Empty is normal; disconnected is an error.",
  "front borrows; pop_front happens only after an acknowledgement.",
], "let-else is ideal when the interesting path requires a value. front borrows the oldest ticket without removing it. Only an available acknowledgement causes pop_front, so a pending ticket retains its exact queue position.", [`${GH}/crates/leash-waveshare/src/lib.rs`], { accent: C.cyan, codeSize: 15.0 });

// 35 — adapter implementation
codeSlide(35, "ADAPTERS", "The hardware adapter maps concrete protocol into the trait.", "leash-waveshare/src/lib.rs", String.raw`
impl ActuationAcknowledgement for CommandAck {
    fn applied(&self) -> bool {
        self.outcome == AckOutcome::Applied
    }
    fn verified_zero(&self) -> bool {
        self.verified_zero
    }
    fn command_id(&self) -> Option<CommandId> {
        Some(self.command_id)
    }
}

impl ActuationPort for WaveshareActuationPort {
    type Acknowledgement = CommandAck;
    type Error = PortError;

    fn submit_drive(
        &mut self,
        command: Authorized<DifferentialDrive>,
    ) -> Result<(), Self::Error> {
        let ticket = self.handle.submit_drive(command)
            .map_err(PortError::Submit)?;
        self.pending.push_back(ticket);
        Ok(())
    }
}`, [
  "Associated types become concrete here.",
  "Authorization remains present at the last adapter.",
  "Hardware errors are not erased prematurely.",
], "The adapter is where abstract runtime contracts become controller messages. It chooses CommandAck and PortError, accepts only an Authorized drive, and preserves the typed acknowledgement contract back into runtime.", [`${GH}/crates/leash-waveshare/src/lib.rs`], { accent: C.lime, codeSize: 13.8 });

// 36 — ROS typed path
codeSlide(36, "ROS2 / NAV2", "Navigation proposals carry their frame in the type.", "leash-ros2/src/lib.rs", String.raw`
#[derive(Debug, Clone, PartialEq)]
pub struct PathProposal {
    pub frame: Frame<Map>,
    pub at: MonotonicNanos,
    pub poses: Box<[Pose2<Map>]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanarTransform<Parent, Child> {
    pub parent: Frame<Parent>,
    pub child: Frame<Child>,
    pub at: MonotonicNanos,
    pub x: Meters,
    pub y: Meters,
    pub yaw: Radians,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NavigationGoal {
    pub activity_id: ActivityId,
    pub pose: Pose2<Map>,
    pub received_at: MonotonicNanos,
}`, [
  "Map is encoded in every navigation pose.",
  "Transforms name both parent and child types.",
  "ROS2 is an adapter—not an authority bypass.",
], "The ROS2 crate carries the phantom-frame discipline to path proposals and navigation goals. A transform names both endpoints in its type. The adapter can translate Nav2 intent, but it still cannot produce motor authority directly.", [`${GH}/crates/leash-ros2/src/lib.rs`], { accent: C.paleBlue, codeSize: 14.7 });

// 37 — CUDA unsafe island
codeSlide(37, "CUDA", "The unsafe launch is narrow, validated, and non-authoritative.", "leash-cuda/src/device.rs [abridged]", String.raw`
let output_count = cells.len()
    .checked_mul(depth as usize)
    .ok_or(ComputeInputError::LengthOverflow)?;
let cell_count = u32::try_from(cells.len())
    .map_err(|_| WorkError::InvalidInput(
        ComputeInputError::LengthOverflow))?;
let launch_count = u32::try_from(output_count)
    .map_err(|_| WorkError::InvalidInput(
        ComputeInputError::LengthOverflow))?;

ensure_capacity(&self.stream,
    &mut self.occupancy_output, output_count)?;

let cells_view = self.occupancy_cells.slice(..cells.len());
let mut output_view =
    self.occupancy_output.slice_mut(..output_count);

{
    unsafe {
        self.stream.launch_builder(&self.project_occupancy)
            .arg(&cells_view)
            .arg(&mut output_view)
            .arg(&cell_count)
            .arg(&depth)
            .launch(LaunchConfig::for_num_elems(launch_count))
    }
    .map_err(|error| backend_error("launch project_occupancy", error))?;
}

let output_view = self.occupancy_output.slice(..output_count);
self.stream.clone_dtoh(&output_view)
    .map_err(|error| backend_error("download output", error))
}`, [
  "Validation sits immediately before unsafe.",
  "Typed CudaSlice views bound the launch arguments.",
  "CUDA computes shadow evidence; CPU authorizes motion.",
], "This is the unsafe island opened on slide seven. Checked multiplication, integer conversion, capacity checks, and typed device slices precede the cudarc kernel launch. CUDA accelerates compute and shadow evaluation; the CPU control kernel remains final motion authority.", [`${GH}/crates/leash-cuda/src/lib.rs`, `${GH}/crates/leash-cuda/src/device.rs`, `${GH}/crates/leash-cuda/README.md`], { accent: C.pink, codeSize: 15.0 });

// 38 — replay and evidence
{
  const s = slide(C.paper, "PROOF", 38);
  title(s, "Determinism and measurements close the loop.", { size: 30 });
  codeBlock(s, "leash-replay/src/lib.rs", String.raw`
#[derive(Debug, Clone, Copy)]
struct StableDigest(u64);

impl StableDigest {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x00000100000001b3;

    fn u64(&mut self, value: u64) {
        for byte in value.to_le_bytes() {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }
}

let first = scenario.verify().unwrap();
let second = scenario.verify().unwrap();
assert_eq!(first, second);`, 0.55, 1.5, 7.2, 4.88, { accent: C.lime, size: 15.2 });
  box(s, "58.306 µs\np99 transition", 8.08, 1.5, 2.18, 1.28, C.lime, { size: 18 });
  box(s, "0\ndeadline misses", 10.55, 1.5, 2.18, 1.28, C.paleBlue, { size: 18 });
  box(s, "110,293\ndurable records/s", 8.08, 3.08, 2.18, 1.28, C.paleYellow, { size: 18 });
  box(s, "37.622 ms\nphysical E-stop ack", 10.55, 3.08, 2.18, 1.28, C.palePink, { size: 17 });
  body(s, "Same input → same transitions → same digest", 8.15, 5.03, 4.5, 0.75, { size: 21, bold: true, align: "center" });
  footer(s);
  note(s, 38, "Types prevent classes of misuse, but the runtime still needs empirical proof. Replay runs the same scenario twice and checks exact transitions and a stable digest. Recorded Jetson evidence supplies latency, deadline, durability, and physical E-stop measurements.", [`${GH}/crates/leash-replay/src/lib.rs`, `${GH}/crates/leash-runtime/evidence/jetson-orin-nx-rv2-16-nomotion-20260829.json`, `${GH}/crates/leash-runtime/evidence/jetson-orin-nx-evidence-20260829.json`, `${GH}/crates/leash-runtime/evidence/jetson-orin-nx-rv2-16-physical-rollout-20260829.json`]);
}

// 39 — Qualia boundary
{
  const s = slide(C.paper, "THE SLOWER CLOCK", 39);
  title(s, "Qualia reasons. Leash retains physical authority.", { size: 30 });
  photo(s, "qualia-world-current.png", 0.64, 1.48, 7.55, 4.66, C.blue);
  box(s, "MISSION\nONTOLOGY\nSEMANTICS", 8.65, 1.5, 3.65, 1.4, C.paleBlue, { size: 22 });
  arrow(s, 10.47, 3.16, 0, 0.72, C.ink, 3);
  box(s, "PROPOSALS +\nEVIDENCE", 8.65, 4.02, 3.65, 1.05, C.paleYellow, { size: 21 });
  arrow(s, 10.47, 5.3, 0, 0.56, C.ink, 3);
  pill(s, "LEASH AUTHORIZES", 8.82, 6.02, 3.3, C.lime, C.ink, 12);
  footer(s);
  note(s, 39, "Qualia is intentionally outside the open-source Leash boundary. It may build missions, ontologies, and semantic evidence asynchronously. It proposes. Leash remains the small, fast, local authority path that decides whether a physical command may proceed.", [REPO]);
}

// 40 — live demo
{
  const s = slide(C.yellow, "LIVE PROOF", 40);
  title(s, "Demo the boundary, not a heroic autonomy claim.", { size: 30 });
  const steps = [
    ["1", "read-only preflight", C.paleBlue],
    ["2", "show proposal + evidence", C.paleYellow],
    ["3", "approve one ≤ 0.10 pulse", C.palePink],
    ["4", "verify zero + acknowledgement", C.lime],
  ];
  steps.forEach(([n, label, fill], index) => {
    const y = 1.48 + index * 1.15;
    box(s, n, 0.72, y, 0.72, 0.72, C.ink, { size: 22, color: C.white, mono: true });
    box(s, label, 1.68, y, 5.15, 0.72, fill, { size: 19, align: "left", pad: 0.2 });
  });
  codeBlock(s, "operator contract", String.raw`
if preflight.is_red() {
    play_recorded_fallback();
    return;
}

assert!(operator_token.is_active());
assert!(pulse.drive <= 0.10);
assert!(pulse.duration <= 500_ms);

let evidence = run_once(pulse)?;
assert_eq!(evidence.final_drive, STOP);`, 7.3, 1.48, 5.35, 4.92, { accent: C.red, size: 16.2 });
  body(s, "Never improvise movement on stage.", 0.82, 6.38, 6.0, 0.4, { size: 21, bold: true, color: C.red });
  footer(s);
  note(s, 40, "The live demo is deliberately bounded. First run the observation-only preflight. Motion requires an active operator token and explicit approval. Perform one low-drive, short-duration pulse, then show the verified-zero acknowledgement. If any gate is red, play the recorded fallback. Do not improvise motion.", [REPO]);
}

// 41 — close and QR
{
  const s = slide(C.blue, "TAKE IT HOME", 41);
  body(s, "WRITE THE\nAUTHORITY RULE\nIN TYPES.", 0.68, 0.82, 7.45, 2.55, { size: 40, bold: true, color: C.white });
  const lines = [
    "newtypes validate",
    "typestate authorizes",
    "ownership isolates",
    "Drop closes",
    "evidence proves",
  ];
  lines.forEach((line, index) => pill(s, line.toUpperCase(), 0.78 + (index % 2) * 3.15, 4.05 + Math.floor(index / 2) * 0.63, 2.8, index % 2 ? C.pink : C.lime, C.ink, 10.5));
  s.addShape(SH.rect, { x: 9.05, y: 0.85, w: 3.25, h: 4.55, fill: { color: C.paper2 }, line: { color: C.ink, width: 3 } });
  if (fs.existsSync(qrPath)) s.addImage({ path: qrPath, x: 9.42, y: 1.18, w: 2.5, h: 2.5 });
  body(s, "SLIDES + CODE", 9.35, 3.95, 2.65, 0.35, { size: 17, bold: true, align: "center" });
  body(s, qrExpiry, 9.3, 4.48, 2.75, 0.5, { size: 10.5, bold: true, align: "center", color: C.grey });
  body(s, "github.com/specdog/leash", 8.42, 5.9, 4.55, 0.4, { size: 17, bold: true, color: C.white, align: "center" });
  footer(s, "Rust Tuesdays • thank you");
  note(s, 41, "The closing idea is simple: when software crosses into physical authority, make the rule visible in the type system. The QR opens the editable deck package and the source is at specdog/leash.", [REPO]);
}

fs.writeFileSync(notesOutput, `# Geek on a Leash — Rust Tuesdays speaker notes\n\n${notes.join("\n")}`);
await pptx.writeFile({ fileName: output, compression: true });
console.log(`Wrote ${output}`);
console.log(`Wrote ${notesOutput}`);
