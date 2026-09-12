use std::{
    env,
    sync::LazyLock,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};
use axum::body::Bytes;
use parking_lot::Mutex;
use tokio::{sync::watch, time};
use v4l::{
    buffer::{Flags, Type},
    format::FourCC,
    io::{mmap::Stream as MmapStream, traits::CaptureStream},
    prelude::Device,
    video::{capture::Parameters, Capture},
    Format,
};

const DEFAULT_FRAME_SIZE: (u32, u32) = (1280, 720);
const DEFAULT_FRAMERATE: u32 = 30;
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(4);
const MAX_FRAME_AGE: Duration = Duration::from_millis(500);
const BUFFER_COUNT: u32 = 4;
const MJPEG_BOUNDARY: &str = "leashframe";

#[derive(Clone)]
pub(crate) struct CameraFrame {
    pub jpeg: Bytes,
    pub sequence: u32,
    pub acquisition_started_ms: u64,
    pub dequeued_ms: u64,
    pub driver_timestamp_us: Option<u64>,
    pub driver_timestamp_flags: u32,
    dequeued_at: Instant,
    driver_age: Duration,
}

impl CameraFrame {
    pub(crate) fn fresh(&self) -> bool {
        self.driver_age.saturating_add(self.dequeued_at.elapsed()) <= MAX_FRAME_AGE
    }

    pub(crate) fn multipart(&self) -> Bytes {
        multipart_jpeg_frame(&self.jpeg)
    }
}

type FrameState = Option<Result<CameraFrame, String>>;
struct SharedCamera {
    device: String,
    receiver: watch::Receiver<FrameState>,
}
static SHARED_CAMERA: LazyLock<Mutex<Option<SharedCamera>>> = LazyLock::new(|| Mutex::new(None));

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

pub(crate) fn subscribe(
    device: String,
    harness: crate::Harness,
) -> Result<watch::Receiver<FrameState>> {
    let mut shared = SHARED_CAMERA.lock();
    if let Some(camera) = shared.as_ref() {
        if camera.receiver.has_changed().is_ok() {
            if camera.device != device {
                return Err(anyhow!("shared V4L2 camera already owns another device"));
            }
            return Ok(camera.receiver.clone());
        }
    }
    // The guard belongs to the native owner, never to a snapshot or stream client.
    // It continues to exclude ffmpeg/WebRTC device owners and supports recovery.
    let guard = crate::http::camera_activity("v4l2-shared")?;
    let (sender, receiver) = watch::channel(None);
    let owner_device = device.clone();
    thread::Builder::new()
        .name("leash-v4l2-owner".to_string())
        .spawn(move || {
            let result = run_shared_camera(owner_device, &sender, &guard);
            if let Err(error) = &result {
                guard.record_failure("native-capture-failed");
                tracing::warn!(error = %error, "shared V4L2 camera stopped");
            }
            // run_shared_camera has closed the stream/device before the owner
            // guard is released and the watch sender becomes closed.
            drop(guard);
            sender.send_replace(Some(Err(result
                .err()
                .map(|e| e.to_string())
                .unwrap_or_else(|| "shared V4L2 camera recovery requested".into()))));
        })
        .context("spawn shared V4L2 capture owner")?;
    let mut lighting = receiver.clone();
    tokio::spawn(async move {
        while lighting.changed().await.is_ok() {
            let frame = lighting.borrow_and_update().clone();
            if let Some(Ok(frame)) =
                frame.filter(|state| state.as_ref().is_ok_and(CameraFrame::fresh))
            {
                let harness = harness.clone();
                // No serial write or JPEG decoding can block device capture.
                let _ = tokio::task::spawn_blocking(move || {
                    harness.observe_camera_lighting(
                        &frame.jpeg,
                        frame.acquisition_started_ms,
                        frame.dequeued_ms,
                    )
                })
                .await;
            }
        }
    });
    *shared = Some(SharedCamera {
        device,
        receiver: receiver.clone(),
    });
    Ok(receiver)
}

pub(crate) async fn latest_frame(
    receiver: &mut watch::Receiver<FrameState>,
) -> Result<CameraFrame> {
    time::timeout(SNAPSHOT_TIMEOUT, async {
        loop {
            let state = receiver.borrow_and_update().clone();
            match state {
                Some(Ok(frame)) if frame.fresh() => return Ok(frame),
                Some(Err(error)) => return Err(anyhow!(error)),
                _ => {}
            }
            receiver
                .changed()
                .await
                .map_err(|_| anyhow!("shared V4L2 camera stopped"))?;
        }
    })
    .await
    .map_err(|_| anyhow!("shared V4L2 camera has no frame newer than 500 ms"))?
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn monotonic_us() -> Result<u64> {
    let mut stamp = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // CLOCK_MONOTONIC is the clock named by V4L2's timestamp flag.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut stamp) } != 0 {
        return Err(std::io::Error::last_os_error()).context("read V4L2 monotonic clock");
    }
    Ok(stamp.tv_sec as u64 * 1_000_000 + stamp.tv_nsec as u64 / 1000)
}

fn run_shared_camera(
    device: String,
    sender: &watch::Sender<FrameState>,
    guard: &crate::http::CameraActivityGuard,
) -> Result<()> {
    let dev = configured_device(&device)?;
    let mut stream = MmapStream::with_buffers(&dev, Type::VideoCapture, BUFFER_COUNT)
        .context("create shared V4L2 mmap capture stream")?;
    stream.set_timeout(Duration::from_secs(2));
    while !guard.recovery_requested() {
        let started_ms = unix_ms();
        let (bytes, metadata) = stream.next().context("capture shared V4L2 MJPEG frame")?;
        let dequeued_at = Instant::now();
        let dequeued_ms = unix_ms();
        if metadata.flags.contains(Flags::ERROR) {
            continue;
        }
        let driver_timestamp_us = (metadata.timestamp.sec >= 0 && metadata.timestamp.usec >= 0)
            .then(|| metadata.timestamp.sec as u64 * 1_000_000 + metadata.timestamp.usec as u64);
        let driver_age = if metadata.flags.contains(Flags::TIMESTAMP_MONOTONIC) {
            let stamp = driver_timestamp_us
                .filter(|stamp| *stamp > 0)
                .ok_or_else(|| anyhow!("V4L2 frame omitted its monotonic timestamp"))?;
            let now = monotonic_us()?;
            if stamp > now.saturating_add(100_000) {
                continue;
            }
            Duration::from_micros(now.saturating_sub(stamp))
        } else {
            Duration::ZERO
        };
        if driver_age > MAX_FRAME_AGE {
            continue;
        }
        let bytes = bytes
            .get(..metadata.bytesused as usize)
            .ok_or_else(|| anyhow!("V4L2 frame size exceeds its buffer"))?;
        let jpeg =
            jpeg_frame(bytes).ok_or_else(|| anyhow!("V4L2 capture returned incomplete JPEG"))?;
        if jpeg.len() > 4 * 1024 * 1024 {
            return Err(anyhow!("V4L2 JPEG exceeds 4 MiB"));
        }
        sender.send_replace(Some(Ok(CameraFrame {
            jpeg: Bytes::copy_from_slice(jpeg),
            sequence: metadata.sequence,
            acquisition_started_ms: started_ms
                .min(dequeued_ms.saturating_sub(driver_age.as_millis() as u64)),
            dequeued_ms,
            driver_timestamp_us,
            driver_timestamp_flags: metadata.flags.bits(),
            dequeued_at,
            driver_age,
        })));
    }
    Ok(())
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
        .map(|index| index + 2)?;
    Some(&jpeg[..end])
}

fn multipart_jpeg_frame(frame: &[u8]) -> Bytes {
    let header = format!(
        "--{MJPEG_BOUNDARY}\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
        frame.len()
    );
    let mut chunk = Vec::with_capacity(header.len() + frame.len() + 2);
    chunk.extend_from_slice(header.as_bytes());
    chunk.extend_from_slice(frame);
    chunk.extend_from_slice(b"\r\n");
    Bytes::from(chunk)
}
