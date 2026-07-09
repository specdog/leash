use std::{env, sync::mpsc as std_mpsc, thread, time::Duration};

use anyhow::{anyhow, Context, Result};
use axum::body::Bytes;
use tokio::{sync::mpsc, task, time};
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
    let handle = task::spawn_blocking(move || {
        let dev = configured_device(&device)?;
        let mut stream = MmapStream::with_buffers(&dev, Type::VideoCapture, BUFFER_COUNT)
            .context("create V4L2 mmap capture stream")?;
        let (frame, _) = stream.next().context("capture V4L2 MJPEG frame")?;
        let jpeg =
            jpeg_frame(frame).ok_or_else(|| anyhow!("V4L2 capture returned non-JPEG frame"))?;
        Ok::<_, anyhow::Error>(Bytes::copy_from_slice(jpeg))
    });

    time::timeout(SNAPSHOT_TIMEOUT, handle)
        .await
        .map_err(|_| anyhow!("V4L2 camera snapshot timed out"))?
        .context("join V4L2 snapshot worker")?
}

pub(crate) async fn start_mjpeg_stream(device: String) -> Result<mpsc::Receiver<Bytes>> {
    let (sender, receiver) = mpsc::channel(8);
    let (ready_sender, ready_receiver) = std_mpsc::sync_channel(1);

    thread::Builder::new()
        .name("leash-v4l2-mjpeg".to_string())
        .spawn(move || {
            if let Err(error) = run_mjpeg_stream(device, sender, ready_sender) {
                tracing::warn!(error = %error, "V4L2 MJPEG stream stopped");
            }
        })
        .context("spawn V4L2 MJPEG stream worker")?;

    let ready = time::timeout(
        STREAM_READY_TIMEOUT,
        task::spawn_blocking(move || ready_receiver.recv()),
    )
    .await
    .map_err(|_| anyhow!("V4L2 camera stream produced no frame"))?
    .context("join V4L2 stream readiness worker")?
    .map_err(|_| anyhow!("V4L2 stream worker exited before first frame"))?;

    ready.map_err(|message| anyhow!(message))?;
    Ok(receiver)
}

fn run_mjpeg_stream(
    device: String,
    sender: mpsc::Sender<Bytes>,
    ready_sender: std_mpsc::SyncSender<Result<(), String>>,
) -> Result<()> {
    let dev = match configured_device(&device) {
        Ok(dev) => dev,
        Err(error) => {
            let _ = ready_sender.send(Err(error.to_string()));
            return Err(error);
        }
    };
    let mut stream = match MmapStream::with_buffers(&dev, Type::VideoCapture, BUFFER_COUNT) {
        Ok(stream) => stream,
        Err(error) => {
            let message = format!("create V4L2 mmap capture stream: {error}");
            let _ = ready_sender.send(Err(message.clone()));
            return Err(anyhow!(message));
        }
    };

    let mut announced_ready = false;
    loop {
        let (frame, _) = match stream.next() {
            Ok(frame) => frame,
            Err(error) => {
                if !announced_ready {
                    let _ = ready_sender.send(Err(format!("capture V4L2 MJPEG frame: {error}")));
                }
                return Err(anyhow!("capture V4L2 MJPEG frame: {error}"));
            }
        };
        let Some(jpeg) = jpeg_frame(frame) else {
            if !announced_ready {
                let _ = ready_sender.send(Err("V4L2 capture returned non-JPEG frame".to_string()));
            }
            return Err(anyhow!("V4L2 capture returned non-JPEG frame"));
        };

        let chunk = multipart_jpeg_frame(jpeg);
        if sender.blocking_send(chunk).is_err() {
            return Ok(());
        }
        if !announced_ready {
            announced_ready = true;
            let _ = ready_sender.send(Ok(()));
        }
    }
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
