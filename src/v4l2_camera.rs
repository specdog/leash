use std::{
    env,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, LazyLock, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};
use axum::body::Bytes;
use tokio::{sync::watch, time};
use v4l::{
    buffer::Type,
    format::FourCC,
    io::{mmap::Stream as MmapStream, traits::CaptureStream},
    prelude::Device,
    video::{capture::Parameters, Capture},
    Format,
};

const DEFAULT_FRAME_SIZE: (u32, u32) = (1280, 720);
const DEFAULT_FRAMERATE: u32 = 30;
const STREAM_READY_TIMEOUT: Duration = Duration::from_secs(4);
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(4);
const BUFFER_COUNT: u32 = 4;
const MJPEG_BOUNDARY: &str = "leashframe";
const MAX_FRAME_AGE: Duration = Duration::from_secs(2);

static CAMERA_HUB: LazyLock<Mutex<Option<Arc<CameraHub>>>> = LazyLock::new(|| Mutex::new(None));

#[derive(Debug, Clone)]
pub(crate) struct MjpegFrame {
    jpeg: Bytes,
    sequence: u64,
    captured_at_ms: u128,
    monotonic_ns: u128,
}

impl MjpegFrame {
    pub(crate) fn jpeg(&self) -> Bytes {
        self.jpeg.clone()
    }

    pub(crate) fn multipart(&self) -> Bytes {
        multipart_jpeg_frame(
            &self.jpeg,
            self.sequence,
            self.captured_at_ms,
            self.monotonic_ns,
        )
    }

    fn fresh(&self) -> bool {
        now_ms().saturating_sub(self.captured_at_ms) <= MAX_FRAME_AGE.as_millis()
    }
}

#[derive(Debug)]
struct CameraHub {
    device: String,
    frames: watch::Sender<Option<MjpegFrame>>,
    stop: AtomicBool,
    alive: AtomicBool,
    sequence: AtomicU64,
}

impl CameraHub {
    fn start(device: String) -> Result<Arc<Self>> {
        let (frames, _) = watch::channel(None);
        let hub = Arc::new(Self {
            device,
            frames,
            stop: AtomicBool::new(false),
            alive: AtomicBool::new(true),
            sequence: AtomicU64::new(0),
        });
        let worker = Arc::clone(&hub);
        thread::Builder::new()
            .name("leash-v4l2-hub".to_string())
            .spawn(move || {
                if let Err(error) = worker.capture() {
                    tracing::warn!(error = %error, "V4L2 camera hub stopped");
                }
                worker.alive.store(false, Ordering::Release);
            })
            .context("spawn V4L2 camera hub")?;
        Ok(hub)
    }

    fn capture(&self) -> Result<()> {
        let dev = configured_device(&self.device)?;
        let mut stream = MmapStream::with_buffers(&dev, Type::VideoCapture, BUFFER_COUNT)
            .context("create V4L2 mmap capture stream")?;
        let started = Instant::now();
        while !self.stop.load(Ordering::Acquire) {
            let (frame, _) = stream.next().context("capture V4L2 MJPEG frame")?;
            let jpeg =
                jpeg_frame(frame).ok_or_else(|| anyhow!("V4L2 capture returned non-JPEG frame"))?;
            let sequence = self
                .sequence
                .fetch_add(1, Ordering::AcqRel)
                .saturating_add(1);
            self.frames.send_replace(Some(MjpegFrame {
                jpeg: Bytes::copy_from_slice(jpeg),
                sequence,
                captured_at_ms: now_ms(),
                monotonic_ns: started.elapsed().as_nanos(),
            }));
        }
        Ok(())
    }

    fn subscribe(&self) -> watch::Receiver<Option<MjpegFrame>> {
        self.frames.subscribe()
    }

    fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
}

pub(crate) fn enabled() -> bool {
    let backend = env_value("LEASH_CAMERA_BACKEND").unwrap_or_else(|| "auto".to_string());
    if backend.eq_ignore_ascii_case("ffmpeg") {
        return false;
    }
    matches!(
        env_value("LEASH_CAMERA_INPUT_FORMAT")
            .unwrap_or_else(|| "mjpeg".to_string())
            .to_ascii_lowercase()
            .as_str(),
        "auto" | "mjpeg" | "mjpg"
    )
}

pub(crate) async fn capture_mjpeg_frame(device: String) -> Result<Bytes> {
    let mut receiver = camera_hub(device)?.subscribe();
    let frame = wait_for_frame(&mut receiver, SNAPSHOT_TIMEOUT).await?;
    Ok(frame.jpeg())
}

pub(crate) async fn start_mjpeg_stream(
    device: String,
) -> Result<watch::Receiver<Option<MjpegFrame>>> {
    let mut receiver = camera_hub(device)?.subscribe();
    let _ = wait_for_frame(&mut receiver, STREAM_READY_TIMEOUT).await?;
    Ok(receiver)
}

pub(crate) fn recover() {
    if let Some(hub) = CAMERA_HUB.lock().expect("camera hub lock").take() {
        hub.stop();
    }
}

pub(crate) fn hub_active() -> bool {
    CAMERA_HUB
        .lock()
        .expect("camera hub lock")
        .as_ref()
        .is_some_and(|hub| hub.alive.load(Ordering::Acquire))
}

fn camera_hub(device: String) -> Result<Arc<CameraHub>> {
    let mut current = CAMERA_HUB.lock().expect("camera hub lock");
    if let Some(hub) = current.as_ref() {
        if hub.device == device && hub.alive.load(Ordering::Acquire) {
            return Ok(Arc::clone(hub));
        }
        hub.stop();
    }
    let hub = CameraHub::start(device)?;
    *current = Some(Arc::clone(&hub));
    Ok(hub)
}

async fn wait_for_frame(
    receiver: &mut watch::Receiver<Option<MjpegFrame>>,
    timeout: Duration,
) -> Result<MjpegFrame> {
    if let Some(frame) = receiver.borrow().clone().filter(MjpegFrame::fresh) {
        return Ok(frame);
    }
    time::timeout(timeout, async {
        loop {
            receiver
                .changed()
                .await
                .map_err(|_| anyhow!("V4L2 camera hub stopped before producing a frame"))?;
            if let Some(frame) = receiver
                .borrow_and_update()
                .clone()
                .filter(MjpegFrame::fresh)
            {
                return Ok(frame);
            }
        }
    })
    .await
    .map_err(|_| anyhow!("V4L2 camera hub produced no fresh frame"))?
}

fn configured_device(device: &str) -> Result<Device> {
    let dev = Device::with_path(device).with_context(|| format!("open V4L2 device {device}"))?;
    let (width, height) = camera_video_size().unwrap_or(DEFAULT_FRAME_SIZE);
    let requested_format = Format::new(width, height, FourCC::new(b"MJPG"));
    let actual_format = dev
        .set_format(&requested_format)
        .with_context(|| format!("set V4L2 MJPEG format {width}x{height}"))?;
    if actual_format.fourcc != FourCC::new(b"MJPG") {
        return Err(anyhow!(
            "V4L2 device selected {}, not MJPG",
            actual_format.fourcc
        ));
    }

    let requested_fps = camera_framerate().unwrap_or(DEFAULT_FRAMERATE);
    let actual_params = dev
        .set_params(&Parameters::with_fps(requested_fps))
        .with_context(|| format!("set V4L2 framerate {requested_fps}"))?;
    tracing::info!(
        device,
        width = actual_format.width,
        height = actual_format.height,
        fourcc = %actual_format.fourcc,
        interval = %actual_params.interval,
        "configured V4L2 MJPEG camera"
    );
    Ok(dev)
}

fn camera_video_size() -> Option<(u32, u32)> {
    let value = env_value("LEASH_CAMERA_VIDEO_SIZE")?;
    let (width, height) = value.split_once('x')?;
    Some((width.trim().parse().ok()?, height.trim().parse().ok()?))
}

fn camera_framerate() -> Option<u32> {
    env_value("LEASH_CAMERA_FRAMERATE")?.parse().ok()
}

fn env_value(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn jpeg_frame(frame: &[u8]) -> Option<&[u8]> {
    let start = frame.windows(2).position(|bytes| bytes == [0xff, 0xd8])?;
    let jpeg = &frame[start..];
    let end = jpeg
        .windows(2)
        .rposition(|bytes| bytes == [0xff, 0xd9])
        .map(|index| index + 2)
        .unwrap_or(jpeg.len());
    Some(&jpeg[..end])
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn multipart_jpeg_frame(
    frame: &[u8],
    sequence: u64,
    captured_at_ms: u128,
    monotonic_ns: u128,
) -> Bytes {
    let header = format!(
        "--{MJPEG_BOUNDARY}\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nX-Leash-Sequence: {sequence}\r\nX-Leash-Captured-At-Ms: {captured_at_ms}\r\nX-Leash-Monotonic-Ns: {monotonic_ns}\r\n\r\n",
        frame.len(),
    );
    let mut chunk = Vec::with_capacity(header.len() + frame.len() + 2);
    chunk.extend_from_slice(header.as_bytes());
    chunk.extend_from_slice(frame);
    chunk.extend_from_slice(b"\r\n");
    Bytes::from(chunk)
}
