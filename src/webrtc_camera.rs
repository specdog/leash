use std::{env, path::Path, sync::Arc, time::Duration};

use anyhow::{anyhow, Result};
use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::Response,
    Json,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::{
    io::AsyncReadExt,
    process::Command as TokioCommand,
    sync::{mpsc, watch, Notify},
    time,
};
use tracing::{debug, warn};
use webrtc::{
    api::{
        interceptor_registry::register_default_interceptors,
        media_engine::{MediaEngine, MIME_TYPE_H264},
        APIBuilder,
    },
    ice_transport::{
        ice_candidate::{RTCIceCandidate, RTCIceCandidateInit},
        ice_connection_state::RTCIceConnectionState,
        ice_server::RTCIceServer,
    },
    interceptor::registry::Registry,
    media::Sample,
    peer_connection::{
        configuration::RTCConfiguration, peer_connection_state::RTCPeerConnectionState,
        sdp::session_description::RTCSessionDescription,
    },
    rtp_transceiver::rtp_codec::RTCRtpCodecCapability,
    track::track_local::{track_local_static_sample::TrackLocalStaticSample, TrackLocal},
};

use crate::http::{
    camera_device_path, camera_env_arg, camera_process_lock, camera_v4l2_input_args,
};

pub async fn camera_webrtc_status() -> Json<Value> {
    let device = camera_device_path();
    Json(json!({
        "ok": Path::new(&device).exists(),
        "status": if Path::new(&device).exists() { "available" } else { "unavailable" },
        "device": device,
        "codec": "video/H264",
        "signaling_url": "/camera/webrtc/ws",
        "transport": "webrtc"
    }))
}

pub async fn camera_webrtc_ws(ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(|socket| async move {
        if let Err(err) = handle_camera_webrtc_socket(socket).await {
            warn!(?err, "camera WebRTC socket failed");
        }
    })
}

async fn handle_camera_webrtc_socket(socket: WebSocket) -> Result<()> {
    let device = camera_device_path();
    if !Path::new(&device).exists() {
        return Err(anyhow!("camera device {device} is not available"));
    }

    let (mut ws_tx, mut ws_rx) = socket.split();
    let (signal_tx, mut signal_rx) = mpsc::channel::<Value>(32);
    let writer = tokio::spawn(async move {
        while let Some(value) = signal_rx.recv().await {
            if ws_tx.send(Message::Text(value.to_string())).await.is_err() {
                break;
            }
        }
    });

    let mut media_engine = MediaEngine::default();
    media_engine.register_default_codecs()?;
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut media_engine)?;
    let api = APIBuilder::new()
        .with_media_engine(media_engine)
        .with_interceptor_registry(registry)
        .build();

    let config = RTCConfiguration {
        ice_servers: webrtc_ice_servers(),
        ..Default::default()
    };
    let peer_connection = Arc::new(api.new_peer_connection(config).await?);
    let connected = Arc::new(Notify::new());
    let (stop_tx, stop_rx) = watch::channel(false);

    let video_track = Arc::new(TrackLocalStaticSample::new(
        RTCRtpCodecCapability {
            mime_type: MIME_TYPE_H264.to_owned(),
            ..Default::default()
        },
        "camera".to_owned(),
        "leash".to_owned(),
    ));
    let rtp_sender = peer_connection
        .add_track(Arc::clone(&video_track) as Arc<dyn TrackLocal + Send + Sync>)
        .await?;

    tokio::spawn(async move {
        let mut rtcp_buf = vec![0u8; 1500];
        while rtp_sender.read(&mut rtcp_buf).await.is_ok() {}
    });

    let candidate_tx = signal_tx.clone();
    peer_connection.on_ice_candidate(Box::new(move |candidate: Option<RTCIceCandidate>| {
        let candidate_tx = candidate_tx.clone();
        Box::pin(async move {
            let value = match candidate {
                Some(candidate) => match candidate.to_json() {
                    Ok(candidate) => json!({ "type": "candidate", "candidate": candidate }),
                    Err(err) => json!({ "type": "error", "error": err.to_string() }),
                },
                None => json!({ "type": "ice-complete" }),
            };
            let _ = candidate_tx.send(value).await;
        })
    }));

    let connected_notify = connected.clone();
    let state_stop_tx = stop_tx.clone();
    peer_connection.on_ice_connection_state_change(Box::new(
        move |state: RTCIceConnectionState| {
            let connected_notify = connected_notify.clone();
            let state_stop_tx = state_stop_tx.clone();
            Box::pin(async move {
                debug!(%state, "WebRTC ICE state changed");
                if state == RTCIceConnectionState::Connected {
                    connected_notify.notify_waiters();
                }
                if matches!(
                    state,
                    RTCIceConnectionState::Failed
                        | RTCIceConnectionState::Disconnected
                        | RTCIceConnectionState::Closed
                ) {
                    let _ = state_stop_tx.send(true);
                }
            })
        },
    ));

    let peer_stop_tx = stop_tx.clone();
    peer_connection.on_peer_connection_state_change(Box::new(
        move |state: RTCPeerConnectionState| {
            let peer_stop_tx = peer_stop_tx.clone();
            Box::pin(async move {
                debug!(%state, "WebRTC peer state changed");
                if matches!(
                    state,
                    RTCPeerConnectionState::Failed
                        | RTCPeerConnectionState::Disconnected
                        | RTCPeerConnectionState::Closed
                ) {
                    let _ = peer_stop_tx.send(true);
                }
            })
        },
    ));

    let media_signal_tx = signal_tx.clone();
    let media_peer_connection = peer_connection.clone();
    let media_task = tokio::spawn(async move {
        wait_connected_or_stopped(connected, stop_rx.clone()).await;
        if *stop_rx.borrow() {
            return;
        }
        if let Err(err) = stream_camera_h264(video_track, stop_rx).await {
            let _ = media_signal_tx
                .send(json!({ "type": "error", "error": err.to_string() }))
                .await;
        }
        let _ = media_signal_tx
            .send(json!({ "type": "ended", "reason": "camera encoder ended" }))
            .await;
        let _ = media_peer_connection.close().await;
    });

    while let Some(message) = ws_rx.next().await {
        match message? {
            Message::Text(text) => {
                if let Err(err) = handle_signal_message(&peer_connection, &signal_tx, &text).await {
                    let _ = signal_tx
                        .send(json!({ "type": "error", "error": err.to_string() }))
                        .await;
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    let _ = stop_tx.send(true);
    let _ = peer_connection.close().await;
    media_task.abort();
    writer.abort();
    Ok(())
}

async fn handle_signal_message(
    peer_connection: &Arc<webrtc::peer_connection::RTCPeerConnection>,
    signal_tx: &mpsc::Sender<Value>,
    text: &str,
) -> Result<()> {
    let value: Value = serde_json::from_str(text)?;
    match value.get("type").and_then(Value::as_str) {
        Some("offer") => {
            let offer: RTCSessionDescription = serde_json::from_value(value)?;
            peer_connection.set_remote_description(offer).await?;
            let answer = peer_connection.create_answer(None).await?;
            peer_connection.set_local_description(answer).await?;
            let Some(local_description) = peer_connection.local_description().await else {
                return Err(anyhow!("WebRTC local description was not created"));
            };
            signal_tx
                .send(json!({
                    "type": "answer",
                    "sdp": local_description.sdp
                }))
                .await?;
        }
        Some("candidate") => {
            if !value.get("candidate").is_some_and(Value::is_null) {
                let candidate: RTCIceCandidateInit =
                    serde_json::from_value(value["candidate"].clone())?;
                if !candidate.candidate.trim().is_empty() {
                    peer_connection.add_ice_candidate(candidate).await?;
                }
            }
        }
        Some("close") => {}
        Some(kind) => return Err(anyhow!("unknown WebRTC signal type '{kind}'")),
        None => return Err(anyhow!("missing WebRTC signal type")),
    }
    Ok(())
}

async fn wait_connected_or_stopped(connected: Arc<Notify>, mut stop_rx: watch::Receiver<bool>) {
    tokio::select! {
        _ = connected.notified() => {}
        _ = stop_rx.changed() => {}
    }
}

async fn stream_camera_h264(
    video_track: Arc<TrackLocalStaticSample>,
    mut stop_rx: watch::Receiver<bool>,
) -> Result<()> {
    let device = camera_device_path();
    let _camera_guard = camera_process_lock()
        .try_lock_owned()
        .map_err(|_| anyhow!("camera is busy; stream or capture already active"))?;

    let mut child = TokioCommand::new("ffmpeg")
        .args(camera_webrtc_ffmpeg_args(&device))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| anyhow!("start ffmpeg WebRTC camera encoder: {err}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("ffmpeg WebRTC encoder did not expose stdout"))?;

    let sample_duration = webrtc_sample_duration();
    let mut buffer = Vec::with_capacity(256 * 1024);

    loop {
        let mut chunk = vec![0u8; 32 * 1024];
        tokio::select! {
            _ = stop_rx.changed() => {
                break;
            }
            read = stdout.read(&mut chunk) => {
                let size = read.map_err(|err| anyhow!("read ffmpeg WebRTC H264 stream: {err}"))?;
                if size == 0 {
                    for nal in drain_h264_nals(&mut buffer, true) {
                        write_h264_sample(&video_track, nal, sample_duration).await?;
                    }
                    break;
                }
                chunk.truncate(size);
                buffer.extend_from_slice(&chunk);
                for nal in drain_h264_nals(&mut buffer, false) {
                    write_h264_sample(&video_track, nal, sample_duration).await?;
                }
                if buffer.len() > 2 * 1024 * 1024 {
                    return Err(anyhow!("ffmpeg WebRTC H264 stream did not produce NAL boundaries"));
                }
            }
        }
    }

    let _ = child.kill().await;
    let _ = time::timeout(Duration::from_secs(2), child.wait()).await;
    Ok(())
}

async fn write_h264_sample(
    video_track: &TrackLocalStaticSample,
    nal: Vec<u8>,
    duration: Duration,
) -> Result<()> {
    if nal.is_empty() {
        return Ok(());
    }
    video_track
        .write_sample(&Sample {
            data: nal.into(),
            duration,
            ..Default::default()
        })
        .await?;
    Ok(())
}

fn camera_webrtc_ffmpeg_args(device: &str) -> Vec<String> {
    let encoder = camera_env_arg("LEASH_WEBRTC_ENCODER").unwrap_or_else(|| "libx264".to_string());
    let gop = camera_env_arg("LEASH_WEBRTC_GOP")
        .unwrap_or_else(|| camera_env_arg("LEASH_CAMERA_FRAMERATE").unwrap_or_else(|| "5".into()));

    let mut args = vec![
        "-nostdin".to_string(),
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-fflags".to_string(),
        "nobuffer".to_string(),
        "-flags".to_string(),
        "low_delay".to_string(),
    ];
    args.extend(camera_v4l2_input_args(device));
    args.extend([
        "-an".to_string(),
        "-vf".to_string(),
        "format=yuv420p".to_string(),
        "-c:v".to_string(),
        encoder.clone(),
        "-pix_fmt".to_string(),
        "yuv420p".to_string(),
        "-g".to_string(),
        gop.clone(),
        "-keyint_min".to_string(),
        gop,
        "-bf".to_string(),
        "0".to_string(),
    ]);

    if encoder.contains("x264") {
        args.extend([
            "-preset".to_string(),
            camera_env_arg("LEASH_WEBRTC_X264_PRESET").unwrap_or_else(|| "ultrafast".to_string()),
            "-tune".to_string(),
            "zerolatency".to_string(),
            "-profile:v".to_string(),
            "baseline".to_string(),
            "-x264-params".to_string(),
            "repeat-headers=1:scenecut=0".to_string(),
        ]);
    }

    args.extend(["-f".to_string(), "h264".to_string(), "pipe:1".to_string()]);
    args
}

fn webrtc_ice_servers() -> Vec<RTCIceServer> {
    env::var("LEASH_WEBRTC_STUN_URL")
        .ok()
        .map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty())
        .map(|url| RTCIceServer {
            urls: vec![url],
            ..Default::default()
        })
        .into_iter()
        .collect()
}

fn webrtc_sample_duration() -> Duration {
    let fps = camera_env_arg("LEASH_CAMERA_FRAMERATE")
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| *value > 0.0)
        .unwrap_or(5.0);
    Duration::from_millis((1000.0 / fps).round().clamp(1.0, 1000.0) as u64)
}

fn drain_h264_nals(buffer: &mut Vec<u8>, eof: bool) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let Some((start, start_len)) = find_start_code(buffer, 0) else {
            if !eof && buffer.len() > 4 {
                let keep_from = buffer.len().saturating_sub(4);
                buffer.drain(..keep_from);
            }
            break;
        };

        if start > 0 {
            buffer.drain(..start);
        }

        let Some((next, _)) = find_start_code(buffer, start_len) else {
            if eof && buffer.len() > start_len {
                let nal = buffer[start_len..].to_vec();
                buffer.clear();
                if !nal.is_empty() {
                    out.push(nal);
                }
            }
            break;
        };

        let nal = buffer[start_len..next].to_vec();
        buffer.drain(..next);
        if !nal.is_empty() {
            out.push(nal);
        }
    }
    out
}

fn find_start_code(bytes: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut index = from;
    while index + 3 <= bytes.len() {
        if bytes[index..].starts_with(&[0, 0, 1]) {
            return Some((index, 3));
        }
        if index + 4 <= bytes.len() && bytes[index..].starts_with(&[0, 0, 0, 1]) {
            return Some((index, 4));
        }
        index += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::drain_h264_nals;

    #[test]
    fn drains_annex_b_nals_and_keeps_partial_tail() {
        let mut buffer = b"xx\0\0\0\x01abc\0\0\x01de".to_vec();
        let nals = drain_h264_nals(&mut buffer, false);
        assert_eq!(nals, vec![b"abc".to_vec()]);
        assert_eq!(buffer, b"\0\0\x01de".to_vec());

        let nals = drain_h264_nals(&mut buffer, true);
        assert_eq!(nals, vec![b"de".to_vec()]);
        assert!(buffer.is_empty());
    }
}
