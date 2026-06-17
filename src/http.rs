use std::{net::SocketAddr, time::Duration};

use anyhow::Result;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    http::{header::CONTENT_TYPE, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::time;
use tower_http::cors::CorsLayer;

use crate::{runtime::Harness, types::SpeedMode};

#[derive(Debug, Deserialize)]
struct PilotTokenReq {
    token: String,
    ttl_secs: Option<u64>,
    speed_mode: Option<SpeedMode>,
}

#[derive(Debug, Serialize)]
struct PilotTokenResp {
    ok: bool,
    ttl_secs: u64,
    speed_mode: SpeedMode,
}

#[derive(Debug, Deserialize)]
struct SpeedModeReq {
    token: Option<String>,
    speed_mode: SpeedMode,
}

#[derive(Debug, Deserialize)]
struct DriveReq {
    token: Option<String>,
    left: f64,
    right: f64,
    speed_mode: Option<SpeedMode>,
}

pub async fn serve_http(harness: Harness, listen: SocketAddr) -> Result<()> {
    let app = router(harness);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(addr = %listener.local_addr()?, "leash http listening");
    axum::serve(listener, app).await?;
    Ok(())
}

pub fn router(harness: Harness) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/capabilities", get(capabilities))
        .route("/telemetry", get(telemetry))
        .route("/sensors", get(sensors))
        .route("/camera/status", get(camera_status))
        .route("/capture", post(capture))
        .route("/pilot/authorize", post(pilot_authorize))
        .route("/pilot/speed-mode", post(pilot_speed_mode))
        .route("/drive", post(drive))
        .route("/motors/drive", post(drive))
        .route("/motors/stop", post(motors_stop))
        .route("/stop", post(motors_stop))
        .route("/estop", post(estop))
        .route("/estop/reset", post(estop_reset))
        .route("/stream", get(stream))
        .route("/ws/telemetry", get(ws_telemetry))
        .with_state(harness)
        .layer(CorsLayer::permissive())
}

async fn health(State(harness): State<Harness>) -> Json<crate::types::Health> {
    Json(harness.health())
}

async fn capabilities(State(harness): State<Harness>) -> Json<crate::types::Capabilities> {
    Json(harness.capabilities())
}

async fn telemetry(State(harness): State<Harness>) -> Json<crate::types::TelemetryFrame> {
    Json(harness.telemetry())
}

async fn sensors(State(harness): State<Harness>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "role": harness.config().role,
        "sensors": harness.telemetry().sensors
    }))
}

async fn camera_status(State(harness): State<Harness>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "camera": harness.telemetry().sensors.camera
    }))
}

async fn capture(State(harness): State<Harness>) -> Json<crate::types::CaptureResult> {
    Json(harness.capture())
}

async fn pilot_authorize(
    State(harness): State<Harness>,
    Json(req): Json<PilotTokenReq>,
) -> Result<Json<PilotTokenResp>, HttpError> {
    let ttl_secs = req.ttl_secs.unwrap_or(120);
    let speed_mode = req.speed_mode.unwrap_or_default();
    harness.authorize(req.token, ttl_secs, speed_mode)?;
    Ok(Json(PilotTokenResp {
        ok: true,
        ttl_secs,
        speed_mode,
    }))
}

async fn pilot_speed_mode(
    State(harness): State<Harness>,
    Json(req): Json<SpeedModeReq>,
) -> Result<Json<Value>, HttpError> {
    harness.set_speed_mode(req.token.as_deref(), req.speed_mode)?;
    Ok(Json(json!({ "ok": true, "speed_mode": req.speed_mode })))
}

async fn drive(
    State(harness): State<Harness>,
    Json(req): Json<DriveReq>,
) -> Result<Json<crate::types::DriveOutcome>, HttpError> {
    Ok(Json(harness.drive(
        req.token.as_deref(),
        req.left,
        req.right,
        req.speed_mode,
    )?))
}

async fn motors_stop(
    State(harness): State<Harness>,
) -> Result<Json<crate::types::DriveOutcome>, HttpError> {
    Ok(Json(harness.stop()?))
}

async fn estop(State(harness): State<Harness>) -> Result<Json<Value>, HttpError> {
    harness.estop()?;
    Ok(Json(json!({ "ok": true, "estop": true })))
}

async fn estop_reset(State(harness): State<Harness>) -> Json<Value> {
    harness.reset_estop();
    Json(json!({ "ok": true, "estop": false }))
}

async fn stream(State(harness): State<Harness>) -> Response {
    let body = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="320" height="240"><rect width="320" height="240" fill="#101820"/><text x="20" y="120" fill="#f6f1d1" font-family="monospace" font-size="18">leash {}</text></svg>"##,
        harness.config().role
    );
    let mut response = body.into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("image/svg+xml"));
    response
}

async fn ws_telemetry(ws: WebSocketUpgrade, State(harness): State<Harness>) -> Response {
    ws.on_upgrade(move |socket| handle_telemetry_socket(socket, harness))
}

async fn handle_telemetry_socket(socket: WebSocket, harness: Harness) {
    let (mut sender, mut receiver) = socket.split();
    let mut telemetry = harness.subscribe_telemetry();

    tokio::spawn(async move { while receiver.next().await.is_some() {} });

    let mut keepalive = time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = keepalive.tick() => {
                if sender.send(Message::Ping(Vec::new())).await.is_err() {
                    break;
                }
            }
            frame = telemetry.recv() => {
                let Ok(frame) = frame else { break };
                let Ok(text) = serde_json::to_string(&frame) else { break };
                if sender.send(Message::Text(text)).await.is_err() {
                    break;
                }
            }
        }
    }
}

#[derive(Debug)]
struct HttpError(anyhow::Error);

impl<E> From<E> for HttpError
where
    E: Into<anyhow::Error>,
{
    fn from(error: E) -> Self {
        Self(error.into())
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": self.0.to_string() })),
        )
            .into_response()
    }
}
