use std::{
    collections::VecDeque,
    convert::Infallible,
    env,
    net::SocketAddr,
    path::Path,
    process::Stdio,
    sync::{Arc, LazyLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use axum::{
    body::{Body, Bytes},
    extract::{
        ws::{Message, WebSocket},
        Form, Path as AxumPath, State, WebSocketUpgrade,
    },
    http::{
        header::{AUTHORIZATION, CONTENT_TYPE},
        HeaderMap, HeaderValue, StatusCode,
    },
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Redirect, Response,
    },
    routing::{get, post},
    Json, Router,
};
use futures_util::{stream, SinkExt, Stream, StreamExt};
use parking_lot::Mutex as ParkingMutex;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::process::Command as TokioCommand;
use tokio::sync::{Mutex as TokioMutex, OwnedMutexGuard};
use tokio::time;
use tower_http::cors::CorsLayer;

use crate::adapter::{
    CameraAdapter, CameraInputConfig, CameraStreamCodec, FfmpegV4l2CameraAdapter,
};
use crate::capability::InvocationOrigin;
use crate::runtime::{
    CAMERA_PAN_MAX_DEG, CAMERA_PAN_MIN_DEG, CAMERA_TILT_MAX_DEG, CAMERA_TILT_MIN_DEG,
};
use crate::types::{AgentMessageAck, AgentMessageList};
#[cfg(feature = "webrtc")]
use crate::webrtc_camera::{camera_webrtc_status, camera_webrtc_ws};
use crate::{
    runtime::Harness,
    transport::StreamRecvError,
    types::{SpeedMode, VerifiedZeroEvidence, ZeroCommandReason},
    LocalizationProviderUpdate,
};

static CAMERA_PROCESS_LOCK: LazyLock<Arc<TokioMutex<()>>> =
    LazyLock::new(|| Arc::new(TokioMutex::new(())));
static CAMERA_RUNTIME_STATE: LazyLock<ParkingMutex<CameraRuntimeState>> =
    LazyLock::new(|| ParkingMutex::new(CameraRuntimeState::default()));

const CAMERA_FAILURE_HISTORY_LIMIT: usize = 16;

#[derive(Debug, Default)]
struct CameraRuntimeState {
    active_owner: Option<String>,
    active_since_ms: Option<u128>,
    active_generation: u64,
    recovery_generation: u64,
    recovery_count: u64,
    last_recovery_ms: Option<u128>,
    recent_failures: VecDeque<crate::types::CameraStreamFailure>,
}

impl CameraRuntimeState {
    fn start(&mut self, owner: &str) -> u64 {
        self.active_owner = Some(owner.to_string());
        self.active_since_ms = Some(camera_now_ms());
        self.active_generation = self.recovery_generation;
        self.active_generation
    }

    fn finish(&mut self, owner: &str, generation: u64) {
        if self.active_owner.as_deref() == Some(owner) && self.active_generation == generation {
            self.active_owner = None;
            self.active_since_ms = None;
        }
    }

    fn record_failure(&mut self, owner: &str, reason: &str) {
        if self.recent_failures.len() == CAMERA_FAILURE_HISTORY_LIMIT {
            self.recent_failures.pop_front();
        }
        self.recent_failures
            .push_back(crate::types::CameraStreamFailure {
                ts_ms: camera_now_ms(),
                owner: owner.to_string(),
                reason: reason.to_string(),
            });
    }

    fn recover(&mut self) -> crate::types::CameraRecoveryResponse {
        let previous_owner = self.active_owner.clone();
        self.recovery_generation = self.recovery_generation.saturating_add(1);
        self.recovery_count = self.recovery_count.saturating_add(1);
        self.last_recovery_ms = Some(camera_now_ms());
        crate::types::CameraRecoveryResponse {
            ok: true,
            recovery_requested: previous_owner.is_some(),
            previous_owner,
            recovery_generation: self.recovery_generation,
            recovery_count: self.recovery_count,
        }
    }

    fn health(&self, device: String) -> crate::types::CameraStreamHealth {
        let device_available = Path::new(&device).exists();
        let recovering =
            self.active_owner.is_some() && self.active_generation < self.recovery_generation;
        let status = if !device_available {
            "unavailable"
        } else if recovering {
            "recovering"
        } else if self.active_owner.is_some() {
            "active"
        } else {
            "idle"
        };
        crate::types::CameraStreamHealth {
            ok: device_available && !recovering,
            status: status.to_string(),
            device,
            device_available,
            active_owner: self.active_owner.clone(),
            active_since_ms: self.active_since_ms,
            recovery_generation: self.recovery_generation,
            recovery_count: self.recovery_count,
            last_recovery_ms: self.last_recovery_ms,
            recent_failures: self.recent_failures.iter().cloned().collect(),
        }
    }
}

pub(crate) struct CameraActivityGuard {
    _process_guard: OwnedMutexGuard<()>,
    owner: &'static str,
    generation: u64,
}

impl CameraActivityGuard {
    pub(crate) fn recovery_requested(&self) -> bool {
        CAMERA_RUNTIME_STATE.lock().recovery_generation != self.generation
    }

    pub(crate) fn record_failure(&self, reason: &str) {
        CAMERA_RUNTIME_STATE
            .lock()
            .record_failure(self.owner, reason);
    }
}

impl Drop for CameraActivityGuard {
    fn drop(&mut self) {
        CAMERA_RUNTIME_STATE
            .lock()
            .finish(self.owner, self.generation);
    }
}

#[derive(Debug, Deserialize)]
struct PilotTokenReq {
    token: String,
    ttl_secs: Option<u64>,
    speed_mode: Option<SpeedMode>,
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
    approval: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct CameraAimReq {
    token: Option<String>,
    pan_deg: f64,
    tilt_deg: f64,
    speed: Option<u32>,
    accel: Option<u32>,
    approval: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct EstopResetReq {
    token: Option<String>,
    approval: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct VerifiedStopReq {
    reason: ZeroCommandReason,
}

#[derive(Debug, Deserialize)]
struct AgentMessageReq {
    text: String,
    source: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct PatrolZoneStartReq {
    speed_mode: Option<SpeedMode>,
}

#[derive(Debug, Default, Deserialize)]
struct DashboardActionReq {
    token: Option<String>,
    ttl_secs: Option<u64>,
    speed_mode: Option<SpeedMode>,
    approval: Option<bool>,
}

pub async fn serve_http(harness: Harness, listen: SocketAddr) -> Result<()> {
    let app = router(harness);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(addr = %listener.local_addr()?, "leash http listening");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(feature = "mcp")]
pub async fn serve_mcp_http(harness: Harness, listen: SocketAddr) -> Result<()> {
    let app = mcp_router(harness);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(addr = %listener.local_addr()?, "leash mcp http listening");
    axum::serve(listener, app).await?;
    Ok(())
}

pub fn router(harness: Harness) -> Router {
    let app = Router::new()
        .route("/", get(dashboard_page))
        .route("/dashboard", get(dashboard_page))
        .route("/dashboard/authorize", post(dashboard_authorize))
        .route("/dashboard/stop", post(dashboard_stop))
        .route("/dashboard/estop", post(dashboard_estop))
        .route("/dashboard/estop-reset", post(dashboard_estop_reset))
        .route("/dashboard/capture", post(dashboard_capture))
        .route("/health", get(health))
        .route("/capabilities", get(capabilities))
        .route("/modules", get(modules))
        .route("/telemetry", get(telemetry))
        .route("/localization", get(localization_status))
        .route("/localization/update", post(localization_update))
        .route("/events/telemetry", get(sse_telemetry))
        .route("/sse/telemetry", get(sse_telemetry))
        .route("/sensors", get(sensors))
        .route("/camera/status", get(camera_status))
        .route("/camera/health", get(camera_stream_health))
        .route("/camera/stream/health", get(camera_stream_health))
        .route("/camera/stream/recover", post(camera_stream_recover))
        .route("/camera/snapshot", get(camera_snapshot))
        .route("/camera/stream.mjpg", get(camera_stream))
        .route("/camera/aim", get(camera_aim_status).post(camera_aim))
        .route("/gimbal/aim", get(camera_aim_status).post(camera_aim))
        .route("/agent", get(agent_page))
        .route("/agent/messages", get(agent_messages).post(agent_message))
        .route("/agent/send", post(agent_message))
        .route("/capture", post(capture))
        .route("/pilot/authorize", post(pilot_authorize))
        .route("/pilot/speed-mode", post(pilot_speed_mode))
        .route("/waypoints", get(waypoints))
        .route("/patrol/zones", get(patrol_zones))
        .route("/patrol/zones/:zone_id/start", post(patrol_zone_start))
        .route("/patrol/status", get(patrol_status))
        .route("/patrol/stop", post(patrol_stop))
        .route("/drive", post(drive))
        .route("/motors/drive", post(drive))
        .route("/motors/stop", post(motors_stop))
        .route("/motors/stop/verified", post(motors_stop_verified))
        .route("/stop", post(motors_stop))
        .route("/stop/verified", post(motors_stop_verified))
        .route("/estop", post(estop))
        .route("/estop/reset", post(estop_reset))
        .route("/stream", get(stream))
        .route("/ws/telemetry", get(ws_telemetry));
    #[cfg(feature = "webrtc")]
    let app = app
        .route("/camera/webrtc", get(camera_webrtc_status))
        .route("/camera/webrtc/ws", get(camera_webrtc_ws));
    app.with_state(harness).layer(CorsLayer::permissive())
}

#[cfg(feature = "mcp")]
pub fn mcp_router(harness: Harness) -> Router {
    Router::new()
        .route("/", get(mcp_status))
        .route("/status", get(mcp_status))
        .route("/tools", get(mcp_tools))
        .route("/list-tools", get(mcp_tools))
        .route("/modules", get(mcp_modules))
        .route("/call", post(mcp_call))
        .route("/mcp", post(mcp_protocol))
        .route("/mcp/status", get(mcp_status))
        .route("/mcp/tools", get(mcp_tools))
        .route("/mcp/list-tools", get(mcp_tools))
        .route("/mcp/modules", get(mcp_modules))
        .route("/mcp/call", post(mcp_call))
        .with_state(harness)
        .layer(CorsLayer::permissive())
}

async fn health(State(harness): State<Harness>) -> Json<crate::types::Health> {
    Json(harness.health())
}

async fn capabilities(State(harness): State<Harness>) -> Json<crate::types::Capabilities> {
    Json(harness.capabilities())
}

async fn waypoints(State(harness): State<Harness>) -> Json<crate::types::SavedWaypointList> {
    Json(harness.waypoints())
}

async fn patrol_zones(State(harness): State<Harness>) -> Json<crate::types::PatrolZoneList> {
    Json(harness.patrol_zones())
}

async fn patrol_status(State(harness): State<Harness>) -> Json<crate::types::PatrolStatus> {
    Json(harness.patrol_status())
}

async fn patrol_zone_start(
    AxumPath(zone_id): AxumPath<String>,
    State(harness): State<Harness>,
    Json(req): Json<PatrolZoneStartReq>,
) -> Result<Json<crate::types::PatrolStatus>, HttpError> {
    Ok(Json(harness.start_patrol_zone(
        &zone_id,
        req.speed_mode.unwrap_or(SpeedMode::Low),
    )?))
}

async fn patrol_stop(
    State(harness): State<Harness>,
) -> Result<Json<crate::types::PatrolStatus>, HttpError> {
    Ok(Json(harness.stop_patrol()?))
}

async fn modules(State(harness): State<Harness>) -> Json<crate::module::ModuleGraph> {
    Json(harness.module_graph())
}

#[cfg(feature = "mcp")]
async fn mcp_status(State(harness): State<Harness>) -> Json<crate::mcp::McpStatus> {
    Json(crate::mcp::status(&harness, "mcp-http"))
}

#[cfg(feature = "mcp")]
async fn mcp_tools() -> Json<crate::mcp::McpToolList> {
    Json(crate::mcp::tool_list())
}

#[cfg(feature = "mcp")]
async fn mcp_modules(State(harness): State<Harness>) -> Json<crate::mcp::McpModuleToolMap> {
    Json(crate::mcp::module_tool_map(&harness))
}

#[cfg(feature = "mcp")]
async fn mcp_call(
    State(harness): State<Harness>,
    Json(req): Json<McpCallReq>,
) -> Result<Json<crate::mcp::McpCallResponse>, HttpError> {
    Ok(Json(crate::mcp::call_tool(
        &harness,
        &req.tool,
        req.args.unwrap_or_else(|| json!({})),
    )?))
}

#[cfg(feature = "mcp")]
async fn mcp_protocol(
    State(harness): State<Harness>,
    headers: HeaderMap,
    Json(req): Json<McpJsonRpcReq>,
) -> Response {
    if !mcp_origin_allowed(&headers) {
        return mcp_http_error(
            StatusCode::FORBIDDEN,
            req.id,
            -32000,
            "origin is not allowed",
        );
    }
    if let Some(version) = headers
        .get("mcp-protocol-version")
        .and_then(|value| value.to_str().ok())
    {
        if !MCP_SUPPORTED_PROTOCOL_VERSIONS.contains(&version) {
            return mcp_http_error(
                StatusCode::BAD_REQUEST,
                req.id,
                -32600,
                "unsupported MCP protocol version",
            );
        }
    }
    if req.jsonrpc != "2.0" {
        return mcp_http_error(
            StatusCode::BAD_REQUEST,
            req.id,
            -32600,
            "jsonrpc must be '2.0'",
        );
    }

    let Some(id) = req.id else {
        return StatusCode::ACCEPTED.into_response();
    };
    let params = req.params.unwrap_or_else(|| json!({}));
    let result = match req.method.as_str() {
        "initialize" => {
            let requested = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(MCP_PROTOCOL_VERSION);
            let negotiated = if MCP_SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
                requested
            } else {
                MCP_PROTOCOL_VERSION
            };
            json!({
                "protocolVersion": negotiated,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": {
                    "name": "leash",
                    "title": "Leash Robotics Harness",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "instructions": "Use tools to inspect or operate Leash. Physical motion remains subject to runtime safety policy and explicit actuation gates."
            })
        }
        "ping" => json!({}),
        "tools/list" => serde_json::to_value(crate::mcp::protocol_tool_list())
            .expect("MCP tool descriptors serialize"),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return mcp_http_error(
                    StatusCode::OK,
                    Some(id),
                    -32602,
                    "tools/call requires params.name",
                );
            };
            if !crate::mcp::tool_descriptors()
                .iter()
                .any(|tool| tool.name == name)
            {
                return mcp_http_error(StatusCode::OK, Some(id), -32602, "unknown MCP tool");
            }
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            serde_json::to_value(crate::mcp::protocol_call_tool(&harness, name, arguments))
                .expect("MCP tool result serializes")
        }
        _ => return mcp_http_error(StatusCode::OK, Some(id), -32601, "method not found"),
    };

    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
}

#[cfg(feature = "mcp")]
fn mcp_http_error(status: StatusCode, id: Option<Value>, code: i64, message: &str) -> Response {
    (
        status,
        Json(json!({
            "jsonrpc": "2.0",
            "id": id.unwrap_or(Value::Null),
            "error": { "code": code, "message": message }
        })),
    )
        .into_response()
}

#[cfg(feature = "mcp")]
fn mcp_origin_allowed(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get("origin") else {
        return true;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let Some(authority) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .and_then(|value| value.split('/').next())
    else {
        return false;
    };
    authority == "localhost"
        || authority.starts_with("localhost:")
        || authority == "127.0.0.1"
        || authority.starts_with("127.0.0.1:")
        || authority == "[::1]"
        || authority.starts_with("[::1]:")
}

async fn telemetry(State(harness): State<Harness>) -> Json<crate::types::TelemetryFrame> {
    Json(harness.telemetry())
}

async fn localization_status(
    State(harness): State<Harness>,
) -> Json<crate::localization::LocalizationProviderStatus> {
    Json(harness.localization_provider_status())
}

async fn localization_update(
    State(harness): State<Harness>,
    headers: HeaderMap,
    Json(update): Json<LocalizationProviderUpdate>,
) -> Response {
    let expected = match localization_ingress_token() {
        Ok(token) => token,
        Err(error) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"ok": false, "error": error.to_string()})),
            )
                .into_response();
        }
    };
    if !localization_authorized(&headers, &expected) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"ok": false, "error": "localization ingress authorization failed"})),
        )
            .into_response();
    }
    if let Err(error) = harness.submit_localization_update(update) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"ok": false, "error": error.to_string()})),
        )
            .into_response();
    }
    (
        StatusCode::ACCEPTED,
        Json(json!({
            "ok": true,
            "localization_provider": harness.localization_provider_status()
        })),
    )
        .into_response()
}

fn localization_ingress_token() -> Result<String> {
    let path = env::var("LEASH_LOCALIZATION_INGRESS_TOKEN_FILE").map_err(|_| {
        anyhow::anyhow!(
            "localization ingress is disabled; set LEASH_LOCALIZATION_INGRESS_TOKEN_FILE"
        )
    })?;
    let token = std::fs::read_to_string(&path)
        .map_err(|error| anyhow::anyhow!("cannot read localization ingress token file: {error}"))?;
    let token = token.trim().to_string();
    if token.is_empty() {
        anyhow::bail!("localization ingress token file is empty");
    }
    Ok(token)
}

fn localization_authorized(headers: &HeaderMap, expected: &str) -> bool {
    let provided = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    provided.is_some_and(|provided| constant_time_eq(provided.as_bytes(), expected.as_bytes()))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

async fn sensors(State(harness): State<Harness>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "role": harness.config().role,
        "sensors": harness.telemetry().sensors
    }))
}

async fn camera_status(State(harness): State<Harness>) -> Json<Value> {
    let mut camera =
        serde_json::to_value(harness.telemetry().sensors.camera).unwrap_or_else(|_| {
            json!({
                "status": "available",
                "health": "healthy",
                "snapshot_url": "/camera/snapshot",
                "stream_url": null
            })
        });
    if let Some(camera) = camera.as_object_mut() {
        let device = camera_device_path();
        if Path::new(&device).exists() {
            camera.insert(
                "stream_url".to_string(),
                Value::String("/camera/stream.mjpg".to_string()),
            );
            #[cfg(feature = "webrtc")]
            if camera_webrtc_enabled() {
                camera.insert(
                    "webrtc_url".to_string(),
                    Value::String("/camera/webrtc/ws".to_string()),
                );
            }
            camera.insert("device".to_string(), Value::String(device));
        }
    }
    Json(json!({
        "ok": true,
        "camera": camera,
        "stream_health": camera_stream_health_snapshot(),
        "gimbal": camera_aim_descriptor()
    }))
}

async fn camera_stream_health() -> Json<crate::types::CameraStreamHealth> {
    Json(camera_stream_health_snapshot())
}

async fn camera_stream_recover() -> Json<crate::types::CameraRecoveryResponse> {
    Json(CAMERA_RUNTIME_STATE.lock().recover())
}

fn camera_stream_health_snapshot() -> crate::types::CameraStreamHealth {
    CAMERA_RUNTIME_STATE.lock().health(camera_device_path())
}

async fn camera_aim_status() -> Json<Value> {
    Json(json!({
        "ok": true,
        "gimbal": camera_aim_descriptor()
    }))
}

fn camera_aim_descriptor() -> Value {
    json!({
        "status": "available",
        "capability": "camera_aim",
        "endpoint": "/camera/aim",
        "aliases": ["/gimbal/aim"],
        "range": {
            "pan_deg": [CAMERA_PAN_MIN_DEG, CAMERA_PAN_MAX_DEG],
            "tilt_deg": [CAMERA_TILT_MIN_DEG, CAMERA_TILT_MAX_DEG]
        }
    })
}

#[cfg_attr(not(feature = "webrtc"), allow(dead_code))]
pub(crate) fn camera_process_lock() -> Arc<TokioMutex<()>> {
    CAMERA_PROCESS_LOCK.clone()
}

pub(crate) fn camera_activity(owner: &'static str) -> Result<CameraActivityGuard> {
    let process_guard = camera_process_lock()
        .try_lock_owned()
        .map_err(|_| anyhow::anyhow!("camera is busy; stream or capture already active"))?;
    let generation = CAMERA_RUNTIME_STATE.lock().start(owner);
    Ok(CameraActivityGuard {
        _process_guard: process_guard,
        owner,
        generation,
    })
}

pub(crate) fn camera_record_failure(owner: &str, reason: &str) {
    CAMERA_RUNTIME_STATE.lock().record_failure(owner, reason);
}

pub(crate) fn camera_device_path() -> String {
    env::var("LEASH_CAMERA_DEVICE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "/dev/video0".to_string())
}

pub(crate) fn camera_env_arg(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && value != "auto")
}

fn camera_now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(feature = "webrtc")]
pub(crate) fn camera_webrtc_enabled() -> bool {
    let value = match env::var("LEASH_WEBRTC_ENABLED") {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => return true,
        Err(env::VarError::NotUnicode(_)) => return false,
    };
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on" | "enabled"
    )
}

#[cfg(feature = "webrtc")]
pub(crate) fn camera_v4l2_input_args(device: &str) -> Vec<String> {
    FfmpegV4l2CameraAdapter.input_args(device, &camera_input_config())
}

fn camera_input_config() -> CameraInputConfig {
    CameraInputConfig {
        input_format: camera_env_arg("LEASH_CAMERA_INPUT_FORMAT"),
        video_size: camera_env_arg("LEASH_CAMERA_VIDEO_SIZE"),
        framerate: camera_env_arg("LEASH_CAMERA_FRAMERATE"),
    }
}

fn camera_stream_codec() -> CameraStreamCodec {
    match camera_env_arg("LEASH_CAMERA_STREAM_CODEC").as_deref() {
        Some("copy") => CameraStreamCodec::Copy,
        _ => CameraStreamCodec::Mjpeg {
            quality: camera_env_arg("LEASH_CAMERA_MJPEG_QUALITY")
                .unwrap_or_else(|| "5".to_string()),
        },
    }
}

async fn camera_snapshot() -> Result<Response, HttpError> {
    let device = camera_device_path();
    if !Path::new(&device).exists() {
        camera_record_failure("snapshot", "device-unavailable");
        return Err(anyhow::anyhow!("camera device {device} is not available").into());
    }
    let camera_guard = camera_activity("snapshot")?;

    #[cfg(all(feature = "v4l2-camera", target_os = "linux"))]
    if crate::v4l2_camera::enabled() {
        let frame = crate::v4l2_camera::capture_mjpeg_frame(device)
            .await
            .inspect_err(|_| {
                camera_guard.record_failure("capture-failed");
            })?;
        let mut response = frame.into_response();
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
        return Ok(response);
    }

    let plan = FfmpegV4l2CameraAdapter.capture_plan(&device, &camera_input_config());
    let mut child = TokioCommand::new(&plan.program)
        .args(&plan.args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| {
            camera_guard.record_failure("capture-start-failed");
            anyhow::anyhow!("start ffmpeg camera capture: {err}")
        })?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("ffmpeg camera capture did not expose stdout"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("ffmpeg camera capture did not expose stderr"))?;
    let stdout_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await.map(|_| bytes)
    });
    let stderr_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).await.map(|_| bytes)
    });

    let status = match time::timeout(Duration::from_secs(4), child.wait()).await {
        Ok(result) => result.map_err(|err| anyhow::anyhow!("wait ffmpeg camera capture: {err}"))?,
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            camera_guard.record_failure("capture-timeout");
            return Err(anyhow::anyhow!("ffmpeg camera capture timed out").into());
        }
    };
    let stdout = stdout_task
        .await
        .map_err(|err| anyhow::anyhow!("join ffmpeg camera stdout reader: {err}"))?
        .map_err(|err| anyhow::anyhow!("read ffmpeg camera stdout: {err}"))?;
    let stderr = stderr_task
        .await
        .map_err(|err| anyhow::anyhow!("join ffmpeg camera stderr reader: {err}"))?
        .map_err(|err| anyhow::anyhow!("read ffmpeg camera stderr: {err}"))?;

    if !status.success() || stdout.is_empty() {
        let stderr = String::from_utf8_lossy(&stderr);
        camera_guard.record_failure("capture-encoder-failed");
        return Err(anyhow::anyhow!("ffmpeg camera capture failed: {stderr}").into());
    }

    let mut response = Bytes::from(stdout).into_response();
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_str(&plan.content_type)
            .map_err(|err| anyhow::anyhow!("invalid camera adapter content type: {err}"))?,
    );
    Ok(response)
}

async fn camera_stream() -> Result<Response, HttpError> {
    let device = camera_device_path();
    if !Path::new(&device).exists() {
        camera_record_failure("mjpeg", "device-unavailable");
        return Err(anyhow::anyhow!("camera device {device} is not available").into());
    }
    let camera_guard = camera_activity("mjpeg")?;

    #[cfg(all(feature = "v4l2-camera", target_os = "linux"))]
    if crate::v4l2_camera::enabled() {
        let receiver = crate::v4l2_camera::start_mjpeg_stream(device)
            .await
            .inspect_err(|_| {
                camera_guard.record_failure("stream-start-failed");
            })?;
        let stream = stream::unfold(
            (receiver, camera_guard),
            |(mut receiver, camera_guard)| async move {
                loop {
                    if camera_guard.recovery_requested() {
                        return None;
                    }
                    match time::timeout(Duration::from_millis(100), receiver.recv()).await {
                        Err(_) => continue,
                        Ok(Some(chunk)) => {
                            return Some((
                                Ok::<Bytes, Infallible>(chunk),
                                (receiver, camera_guard),
                            ));
                        }
                        Ok(None) => {
                            camera_guard.record_failure("stream-ended");
                            return None;
                        }
                    }
                }
            },
        );
        let mut response = Body::from_stream(stream).into_response();
        response.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("multipart/x-mixed-replace; boundary=leashframe"),
        );
        response.headers_mut().insert(
            "cache-control",
            HeaderValue::from_static("no-store, no-cache, must-revalidate"),
        );
        return Ok(response);
    }

    let plan =
        FfmpegV4l2CameraAdapter.stream_plan(&device, &camera_input_config(), camera_stream_codec());
    let mut command = TokioCommand::new(&plan.program);
    command.kill_on_drop(true).args(&plan.args);

    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| {
            camera_guard.record_failure("stream-start-failed");
            anyhow::anyhow!("start ffmpeg camera stream: {err}")
        })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("ffmpeg camera stream did not expose stdout"))?;

    let (first_chunk, stdout, child) = match read_first_camera_frame(stdout, child).await {
        Ok(stream) => stream,
        Err(err) => {
            camera_guard.record_failure("stream-first-frame-failed");
            return Err(err);
        }
    };
    let stream = stream::unfold(
        (Some(first_chunk), stdout, child, camera_guard),
        |(first_chunk, mut stdout, mut child, camera_guard)| async move {
            if camera_guard.recovery_requested() {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return None;
            }
            if let Some(first_chunk) = first_chunk {
                return Some((
                    Ok::<Bytes, Infallible>(first_chunk),
                    (None, stdout, child, camera_guard),
                ));
            }

            loop {
                if camera_guard.recovery_requested() {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    return None;
                }
                let mut chunk = vec![0; 32 * 1024];
                match time::timeout(Duration::from_millis(100), stdout.read(&mut chunk)).await {
                    Err(_) => continue,
                    Ok(Ok(0)) => {
                        let _ = child.wait().await;
                        camera_guard.record_failure("stream-ended");
                        return None;
                    }
                    Ok(Ok(size)) => {
                        chunk.truncate(size);
                        return Some((
                            Ok::<Bytes, Infallible>(Bytes::from(chunk)),
                            (None, stdout, child, camera_guard),
                        ));
                    }
                    Ok(Err(_)) => {
                        let _ = child.kill().await;
                        camera_guard.record_failure("stream-read-failed");
                        return None;
                    }
                }
            }
        },
    );

    let mut response = Body::from_stream(stream).into_response();
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_str(&plan.content_type)
            .map_err(|err| anyhow::anyhow!("invalid camera adapter content type: {err}"))?,
    );
    response.headers_mut().insert(
        "cache-control",
        HeaderValue::from_static("no-store, no-cache, must-revalidate"),
    );
    Ok(response)
}

async fn read_first_camera_frame(
    mut stdout: tokio::process::ChildStdout,
    mut child: tokio::process::Child,
) -> Result<(Bytes, tokio::process::ChildStdout, tokio::process::Child), HttpError> {
    let mut first_chunk = Vec::with_capacity(64 * 1024);
    loop {
        let mut chunk = vec![0; 32 * 1024];
        match time::timeout(Duration::from_secs(2), stdout.read(&mut chunk)).await {
            Err(_) => {
                let _ = child.kill().await;
                return Err(anyhow::anyhow!("ffmpeg camera stream produced no frame").into());
            }
            Ok(Err(err)) => {
                let _ = child.kill().await;
                return Err(anyhow::anyhow!("read ffmpeg camera stream: {err}").into());
            }
            Ok(Ok(0)) => {
                let _ = child.wait().await;
                return Err(
                    anyhow::anyhow!("ffmpeg camera stream ended before first frame").into(),
                );
            }
            Ok(Ok(size)) => {
                chunk.truncate(size);
                first_chunk.extend_from_slice(&chunk);
                if first_chunk.windows(2).any(|bytes| bytes == [0xff, 0xd8]) {
                    return Ok((Bytes::from(first_chunk), stdout, child));
                }
                if first_chunk.len() > 512 * 1024 {
                    let _ = child.kill().await;
                    return Err(
                        anyhow::anyhow!("ffmpeg camera stream produced no JPEG frame").into(),
                    );
                }
            }
        }
    }
}

async fn dashboard_page(State(harness): State<Harness>) -> Response {
    let config = harness.config();
    let health = harness.health();
    let telemetry = harness.telemetry();
    let capabilities = harness.capabilities();
    let module_graph = harness.module_graph();
    let health_status = if health.ok { "ok" } else { "attention" };
    let health_dot = if health.ok { "ok" } else { "warn" };
    let health_metrics = dashboard_metrics(vec![
        ("ok", health.ok.to_string()),
        ("mode", health.mode.clone()),
        ("uptime ms", health.uptime_ms.to_string()),
        ("estop", health.estop.to_string()),
        (
            "deadman",
            if health.deadman_ok { "ok" } else { "stale" }.to_string(),
        ),
        (
            "accelerator",
            health.accelerator.active.as_str().to_string(),
        ),
    ]);
    let policy_metrics = dashboard_metrics(vec![
        ("mode", config.policy_mode.as_str().to_string()),
        ("untokened drive", config.allow_untokened_drive.to_string()),
        (
            "physical actuation",
            harness.physical_actuation_enabled().to_string(),
        ),
        (
            "stream transport",
            config.stream_transport.as_str().to_string(),
        ),
    ]);
    let telemetry_metrics = dashboard_metrics(vec![
        ("ts ms", telemetry.ts_ms.to_string()),
        ("left", telemetry.left_cmd.to_string()),
        ("right", telemetry.right_cmd.to_string()),
        ("speed", speed_mode_label(telemetry.speed_mode).to_string()),
        ("battery", optional_f64(telemetry.battery_v)),
        ("source", telemetry.source.clone()),
    ]);
    let modules = dashboard_modules(&module_graph.modules);
    let capability_items = dashboard_capabilities(&capabilities.capabilities);
    let logs_tail = dashboard_logs(&harness);
    let body = format!(
        r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta http-equiv="refresh" content="2">
  <title>Leash Command Center</title>
  <style>
    :root {{
      color-scheme: light dark;
      font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      background: #f5f7f8;
      color: #172126;
    }}
    * {{ box-sizing: border-box; }}
    body {{
      margin: 0;
      min-height: 100vh;
      background: #f5f7f8;
      color: #172126;
    }}
    header {{
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 16px;
      padding: 18px 24px;
      border-bottom: 1px solid #d7dee2;
      background: #ffffff;
    }}
    h1 {{ margin: 0; font-size: 20px; font-weight: 700; }}
    h2 {{ margin: 0 0 12px; font-size: 14px; font-weight: 700; color: #31424a; }}
    main {{
      display: grid;
      grid-template-columns: minmax(280px, 0.9fr) minmax(360px, 1.4fr);
      gap: 16px;
      padding: 16px;
    }}
    section {{
      min-width: 0;
      border: 1px solid #d7dee2;
      border-radius: 8px;
      background: #ffffff;
      padding: 14px;
    }}
    .stack {{ display: grid; gap: 16px; align-content: start; }}
    .grid {{ display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 8px; }}
    .metric {{
      min-height: 68px;
      border: 1px solid #e1e6e9;
      border-radius: 6px;
      padding: 10px;
      background: #fbfcfc;
    }}
    .label {{ display: block; margin-bottom: 5px; color: #60717a; font-size: 12px; }}
    .value {{ display: block; overflow-wrap: anywhere; font-size: 18px; font-weight: 700; }}
    .value.small {{ font-size: 13px; font-weight: 600; line-height: 1.35; }}
    .status-dot {{
      display: inline-block;
      width: 10px;
      height: 10px;
      margin-right: 8px;
      border-radius: 50%;
      background: #8a9499;
    }}
    .status-dot.ok {{ background: #0f8a5f; }}
    .status-dot.warn {{ background: #b25e09; }}
    .toolbar {{ display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }}
    button, input, select {{
      min-height: 34px;
      border: 1px solid #bac5ca;
      border-radius: 6px;
      padding: 7px 10px;
      font: inherit;
      background: #ffffff;
      color: #172126;
    }}
    button {{ cursor: pointer; font-weight: 650; }}
    button.primary {{ background: #0b6b7a; color: #ffffff; border-color: #0b6b7a; }}
    button.danger {{ background: #a32929; color: #ffffff; border-color: #a32929; }}
    button:disabled {{ cursor: not-allowed; opacity: 0.55; }}
    input {{ width: 160px; }}
    ul {{ margin: 0; padding: 0; list-style: none; display: grid; gap: 8px; }}
    li {{
      border: 1px solid #e1e6e9;
      border-radius: 6px;
      padding: 8px 10px;
      background: #fbfcfc;
      overflow-wrap: anywhere;
    }}
    pre {{
      margin: 0;
      min-height: 260px;
      max-height: 420px;
      overflow: auto;
      white-space: pre-wrap;
      border: 1px solid #e1e6e9;
      border-radius: 6px;
      padding: 10px;
      background: #101820;
      color: #d7f3e3;
      font: 12px ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    }}
    .wide {{ grid-column: 1 / -1; }}
    @media (max-width: 840px) {{
      header {{ align-items: flex-start; flex-direction: column; padding: 14px; }}
      main {{ grid-template-columns: 1fr; padding: 12px; }}
      .grid {{ grid-template-columns: 1fr; }}
      input {{ width: min(100%, 220px); }}
    }}
    @media (prefers-color-scheme: dark) {{
      :root, body {{ background: #0f1417; color: #e6ecef; }}
      header, section {{ background: #141b1f; border-color: #2c3940; }}
      h2 {{ color: #c6d1d6; }}
      .metric, li {{ background: #182126; border-color: #2c3940; }}
      .label {{ color: #9aabb3; }}
      button, input, select {{ background: #101820; border-color: #43535a; color: #e6ecef; }}
    }}
  </style>
</head>
<body>
  <header>
    <div>
      <h1>Leash Command Center</h1>
      <span id="dashboard-status"><span class="status-dot {health_dot}"></span>{health_status} / {role} / {profile}</span>
    </div>
    <form class="toolbar" method="post">
      <input name="token" value="dashboard-token" aria-label="Pilot token">
      <input type="hidden" name="ttl_secs" value="120">
      <input type="hidden" name="approval" value="true">
      <select name="speed_mode" aria-label="Speed mode">
        <option value="low">low</option>
        <option value="medium" selected>medium</option>
        <option value="high">high</option>
      </select>
      <button class="primary" type="submit" formaction="/dashboard/authorize">Authorize</button>
      <button type="submit" formaction="/dashboard/stop">Stop</button>
      <button class="danger" type="submit" formaction="/dashboard/estop">E-Stop</button>
      <button type="submit" formaction="/dashboard/estop-reset">Reset</button>
      <button type="submit" formaction="/dashboard/capture">Capture</button>
    </form>
  </header>
  <main data-telemetry-ts="{telemetry_ts}">
    <div class="stack">
      <section>
        <h2>Health</h2>
        <div id="health-grid" class="grid">{health_metrics}</div>
      </section>
      <section>
        <h2>Policy</h2>
        <div id="policy-grid" class="grid">{policy_metrics}</div>
      </section>
      <section>
        <h2>Telemetry</h2>
        <div id="telemetry-grid" class="grid">{telemetry_metrics}</div>
      </section>
    </div>
    <div class="stack">
      <section>
        <h2>Modules</h2>
        <ul id="module-list">{modules}</ul>
      </section>
      <section>
        <h2>Capabilities</h2>
        <ul id="capability-list">{capability_items}</ul>
      </section>
      <section>
        <h2>Logs Tail</h2>
        <pre id="dashboard-log">{logs_tail}</pre>
      </section>
    </div>
  </main>
</body>
</html>
"##,
        health_dot = health_dot,
        health_status = health_status,
        role = html_escape(&health.role),
        profile = html_escape(&health.profile),
        telemetry_ts = telemetry.ts_ms,
        health_metrics = health_metrics,
        policy_metrics = policy_metrics,
        telemetry_metrics = telemetry_metrics,
        modules = modules,
        capability_items = capability_items,
        logs_tail = logs_tail
    );
    html_response(body)
}

async fn dashboard_authorize(
    State(harness): State<Harness>,
    Form(req): Form<DashboardActionReq>,
) -> Redirect {
    let token = cleaned_token(req.token).unwrap_or_else(|| "dashboard-token".to_string());
    dashboard_invoke(
        &harness,
        "authorize",
        json!({
            "token": token,
            "ttl_secs": req.ttl_secs.unwrap_or(120),
            "speed_mode": req.speed_mode.unwrap_or_default(),
        }),
    );
    Redirect::to("/dashboard")
}

async fn dashboard_stop(
    State(harness): State<Harness>,
    Form(_req): Form<DashboardActionReq>,
) -> Redirect {
    dashboard_invoke(&harness, "stop", json!({}));
    Redirect::to("/dashboard")
}

async fn dashboard_estop(
    State(harness): State<Harness>,
    Form(_req): Form<DashboardActionReq>,
) -> Redirect {
    dashboard_invoke(&harness, "estop", json!({}));
    Redirect::to("/dashboard")
}

async fn dashboard_estop_reset(
    State(harness): State<Harness>,
    Form(req): Form<DashboardActionReq>,
) -> Redirect {
    dashboard_invoke(
        &harness,
        "estop_reset",
        json!({
            "token": cleaned_token(req.token),
            "approval": req.approval.or(Some(true)),
        }),
    );
    Redirect::to("/dashboard")
}

async fn dashboard_capture(
    State(harness): State<Harness>,
    Form(_req): Form<DashboardActionReq>,
) -> Redirect {
    dashboard_invoke(&harness, "capture", json!({}));
    Redirect::to("/dashboard")
}

fn dashboard_invoke(harness: &Harness, action: &str, args: Value) {
    match harness.capability_registry().invoke_value_with_origin(
        action,
        args,
        InvocationOrigin::Http,
    ) {
        Ok(payload) => harness.record_dashboard_event(format!("{action} ok {payload}")),
        Err(error) => harness.record_dashboard_event(format!("{action} error {error}")),
    }
}

fn dashboard_metrics(items: Vec<(&str, String)>) -> String {
    items
        .into_iter()
        .map(|(label, value)| {
            format!(
                r#"<div class="metric"><span class="label">{}</span><span class="value small">{}</span></div>"#,
                html_escape(label),
                html_escape(&value)
            )
        })
        .collect()
}

fn dashboard_modules(modules: &[crate::module::ModuleInfo]) -> String {
    modules
        .iter()
        .map(|module| {
            format!(
                r#"<li><strong>{}</strong><br>{} / {:?} / {}</li>"#,
                html_escape(&module.name),
                html_escape(&module.module_type),
                module.state,
                html_escape(module.health.message.as_deref().unwrap_or("-")),
            )
        })
        .collect()
}

fn dashboard_capabilities(capabilities: &[crate::capability::CapabilityDescriptor]) -> String {
    capabilities
        .iter()
        .map(|capability| {
            format!(
                r#"<li><strong>{}</strong><br>{} / {}</li>"#,
                html_escape(&capability.name),
                capability.safety.as_str(),
                html_escape(&capability.description),
            )
        })
        .collect()
}

fn dashboard_logs(harness: &Harness) -> String {
    let mut lines = harness.dashboard_events();
    lines.extend(
        harness
            .agent_messages()
            .into_iter()
            .rev()
            .take(8)
            .map(|message| {
                format!(
                    "{} agent:{} {}",
                    message.ts_ms, message.source, message.text
                )
            }),
    );
    if lines.is_empty() {
        lines.push("no dashboard actions yet".to_string());
    }
    html_escape(&lines.join("\n"))
}

fn cleaned_token(token: Option<String>) -> Option<String> {
    token
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

fn optional_f64(value: Option<f64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn speed_mode_label(speed_mode: SpeedMode) -> &'static str {
    match speed_mode {
        SpeedMode::Low => "low",
        SpeedMode::Medium => "medium",
        SpeedMode::High => "high",
    }
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

async fn agent_messages(State(harness): State<Harness>) -> Json<AgentMessageList> {
    Json(AgentMessageList {
        ok: true,
        messages: harness.agent_messages(),
    })
}

async fn agent_message(
    State(harness): State<Harness>,
    Json(req): Json<AgentMessageReq>,
) -> Result<Json<AgentMessageAck>, HttpError> {
    let source = req.source.unwrap_or_else(|| "http".to_string());
    let message = harness.submit_agent_message(source, req.text)?;
    let response = harness.agent_model_response(&message.text)?;
    Ok(Json(AgentMessageAck {
        ok: true,
        message,
        response,
    }))
}

async fn agent_page() -> Response {
    let body = r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Leash Agent Input</title>
  <style>
    :root { color-scheme: light dark; font-family: ui-sans-serif, system-ui, sans-serif; }
    body { margin: 0; min-height: 100vh; display: grid; place-items: center; background: Canvas; color: CanvasText; }
    main { width: min(560px, calc(100vw - 32px)); display: grid; gap: 12px; }
    h1 { margin: 0; font-size: 20px; font-weight: 650; }
    textarea { width: 100%; min-height: 120px; box-sizing: border-box; padding: 12px; font: inherit; }
    button { justify-self: start; padding: 8px 12px; font: inherit; }
    pre { margin: 0; min-height: 24px; white-space: pre-wrap; }
  </style>
</head>
<body>
  <main>
    <h1>Leash Agent Input</h1>
    <form id="agent-form">
      <textarea id="agent-text" name="text" autofocus required></textarea>
      <button type="submit">Send</button>
    </form>
    <pre id="agent-output"></pre>
  </main>
  <script>
    const form = document.querySelector("#agent-form");
    const text = document.querySelector("#agent-text");
    const output = document.querySelector("#agent-output");
    form.addEventListener("submit", async (event) => {
      event.preventDefault();
      const response = await fetch("/agent/messages", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ source: "web", text: text.value })
      });
      const payload = await response.json();
      output.textContent = JSON.stringify(payload, null, 2);
      if (payload.ok) text.value = "";
    });
  </script>
</body>
</html>
"##;
    html_response(body)
}

fn html_response(body: impl IntoResponse) -> Response {
    let mut response = body.into_response();
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response
}

async fn capture(State(harness): State<Harness>) -> Result<Json<Value>, HttpError> {
    Ok(Json(
        harness.capability_registry().invoke_value_with_origin(
            "capture",
            json!({}),
            InvocationOrigin::Http,
        )?,
    ))
}

async fn pilot_authorize(
    State(harness): State<Harness>,
    Json(req): Json<PilotTokenReq>,
) -> Result<Json<Value>, HttpError> {
    Ok(Json(
        harness.capability_registry().invoke_value_with_origin(
            "authorize",
            json!({
                "token": req.token,
                "ttl_secs": req.ttl_secs,
                "speed_mode": req.speed_mode,
            }),
            InvocationOrigin::Http,
        )?,
    ))
}

async fn pilot_speed_mode(
    State(harness): State<Harness>,
    Json(req): Json<SpeedModeReq>,
) -> Result<Json<Value>, HttpError> {
    Ok(Json(
        harness.capability_registry().invoke_value_with_origin(
            "speed_mode",
            json!({
                "token": req.token,
                "speed_mode": req.speed_mode,
            }),
            InvocationOrigin::Http,
        )?,
    ))
}

async fn drive(
    State(harness): State<Harness>,
    Json(req): Json<DriveReq>,
) -> Result<Json<Value>, HttpError> {
    Ok(Json(
        harness.capability_registry().invoke_value_with_origin(
            "drive",
            json!({
                "token": req.token,
                "left": req.left,
                "right": req.right,
                "speed_mode": req.speed_mode,
                "approval": req.approval,
            }),
            InvocationOrigin::Http,
        )?,
    ))
}

async fn camera_aim(
    State(harness): State<Harness>,
    Json(req): Json<CameraAimReq>,
) -> Result<Json<Value>, HttpError> {
    Ok(Json(
        harness.capability_registry().invoke_value_with_origin(
            "camera_aim",
            json!({
                "token": req.token,
                "pan_deg": req.pan_deg,
                "tilt_deg": req.tilt_deg,
                "speed": req.speed,
                "accel": req.accel,
                "approval": req.approval,
            }),
            InvocationOrigin::Http,
        )?,
    ))
}

async fn motors_stop(State(harness): State<Harness>) -> Result<Json<Value>, HttpError> {
    Ok(Json(
        harness.capability_registry().invoke_value_with_origin(
            "stop",
            json!({}),
            InvocationOrigin::Http,
        )?,
    ))
}

async fn motors_stop_verified(
    State(harness): State<Harness>,
    request: Option<Json<VerifiedStopReq>>,
) -> Result<Json<VerifiedZeroEvidence>, HttpError> {
    let reason = request
        .map(|Json(request)| request.reason)
        .unwrap_or(ZeroCommandReason::OperatorRequest);
    let evidence = tokio::task::spawn_blocking(move || harness.stop_verified(reason))
        .await
        .map_err(|error| anyhow::anyhow!("verified zero task failed: {error}"))??;
    Ok(Json(evidence))
}

async fn estop(State(harness): State<Harness>) -> Result<Json<Value>, HttpError> {
    Ok(Json(
        harness.capability_registry().invoke_value_with_origin(
            "estop",
            json!({}),
            InvocationOrigin::Http,
        )?,
    ))
}

async fn estop_reset(
    State(harness): State<Harness>,
    req: Option<Json<EstopResetReq>>,
) -> Result<Json<Value>, HttpError> {
    let req = req.map(|Json(req)| req).unwrap_or_default();
    Ok(Json(
        harness.capability_registry().invoke_value_with_origin(
            "estop_reset",
            json!({
                "token": req.token,
                "approval": req.approval,
            }),
            InvocationOrigin::Http,
        )?,
    ))
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

async fn sse_telemetry(
    State(harness): State<Harness>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, HttpError> {
    let receiver = harness.subscribe_stream("telemetry")?;
    let stream = telemetry_sse_stream(receiver);
    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(5))
            .text("keepalive"),
    ))
}

async fn handle_telemetry_socket(socket: WebSocket, harness: Harness) {
    let (mut sender, mut receiver) = socket.split();
    let Ok(mut telemetry) = harness.subscribe_stream("telemetry") else {
        let _ = sender
            .send(Message::Text(stream_error_text(
                "error",
                "telemetry stream unavailable",
            )))
            .await;
        return;
    };

    tokio::spawn(async move { while receiver.next().await.is_some() {} });

    let mut keepalive = time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = keepalive.tick() => {
                if sender.send(Message::Ping(Vec::new())).await.is_err() {
                    break;
                }
            }
            message = telemetry.recv() => {
                let text = match message {
                    Ok(message) => message.payload.to_string(),
                    Err(StreamRecvError::Lagged(skipped)) => stream_lagged_text(skipped),
                    Err(StreamRecvError::Closed) => {
                        let _ = sender
                            .send(Message::Text(stream_error_text("closed", "telemetry stream closed")))
                            .await;
                        break;
                    }
                };
                if sender.send(Message::Text(text)).await.is_err() {
                    break;
                }
            }
        }
    }
}

fn telemetry_sse_stream(
    receiver: crate::transport::StreamSubscriber,
) -> impl Stream<Item = Result<Event, Infallible>> {
    stream::unfold((receiver, false), |(mut receiver, done)| async move {
        if done {
            return None;
        }
        let (event, done) = match receiver.recv().await {
            Ok(message) => (
                Event::default()
                    .event("telemetry")
                    .data(message.payload.to_string()),
                false,
            ),
            Err(StreamRecvError::Lagged(skipped)) => (
                Event::default()
                    .event("lagged")
                    .data(json!({"kind":"lagged","skipped":skipped}).to_string()),
                false,
            ),
            Err(StreamRecvError::Closed) => (
                Event::default()
                    .event("closed")
                    .data(stream_error_text("closed", "telemetry stream closed")),
                true,
            ),
        };
        Some((Ok(event), (receiver, done)))
    })
}

fn stream_lagged_text(skipped: u64) -> String {
    json!({"kind":"lagged","skipped":skipped}).to_string()
}

fn stream_error_text(kind: &str, message: &str) -> String {
    json!({"kind":kind,"ok":false,"error":message}).to_string()
}

#[derive(Debug)]
struct HttpError(anyhow::Error);

#[cfg(feature = "mcp")]
#[derive(Debug, Deserialize)]
struct McpCallReq {
    tool: String,
    args: Option<Value>,
}

#[cfg(feature = "mcp")]
const MCP_PROTOCOL_VERSION: &str = "2025-11-25";

#[cfg(feature = "mcp")]
const MCP_SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];

#[cfg(feature = "mcp")]
#[derive(Debug, Deserialize)]
struct McpJsonRpcReq {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

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

#[cfg(test)]
mod tests {
    use std::fs;

    use axum::{
        extract::State,
        http::{header::AUTHORIZATION, HeaderMap, HeaderValue, StatusCode},
        Json,
    };

    use super::{
        camera_activity, localization_authorized, localization_update, motors_stop_verified,
        CameraRuntimeState, CAMERA_RUNTIME_STATE,
    };
    use crate::{Harness, HarnessConfig, LocalizationProviderUpdate};
    use tokio::sync::Mutex;

    static LOCALIZATION_ENV_LOCK: Mutex<()> = Mutex::const_new(());

    #[test]
    fn camera_runtime_state_tracks_owner_recovery_and_bounded_failures() {
        let mut state = CameraRuntimeState::default();
        let generation = state.start("mjpeg");
        let active = state.health("/".to_string());
        assert_eq!(active.status, "active");
        assert_eq!(active.active_owner.as_deref(), Some("mjpeg"));

        let recovery = state.recover();
        assert!(recovery.recovery_requested);
        assert_eq!(recovery.previous_owner.as_deref(), Some("mjpeg"));
        assert_eq!(state.health("/".to_string()).status, "recovering");

        state.finish("mjpeg", generation);
        assert_eq!(state.health("/".to_string()).status, "idle");

        for _ in 0..20 {
            state.record_failure("mjpeg", "stream-ended");
        }
        assert_eq!(state.health("/".to_string()).recent_failures.len(), 16);
    }

    #[test]
    fn camera_activity_serializes_snapshot_and_stream_owners() {
        *CAMERA_RUNTIME_STATE.lock() = CameraRuntimeState::default();
        let snapshot = camera_activity("snapshot").unwrap();

        let error = camera_activity("mjpeg").err().unwrap().to_string();
        assert!(error.contains("camera is busy"));
        CAMERA_RUNTIME_STATE.lock().recover();
        assert!(snapshot.recovery_requested());

        drop(snapshot);
        let mjpeg = camera_activity("mjpeg").unwrap();
        assert_eq!(
            CAMERA_RUNTIME_STATE.lock().active_owner.as_deref(),
            Some("mjpeg")
        );
        drop(mjpeg);
        *CAMERA_RUNTIME_STATE.lock() = CameraRuntimeState::default();
    }

    #[test]
    fn localization_ingress_requires_an_exact_bearer_token() {
        let mut headers = HeaderMap::new();
        assert!(!localization_authorized(&headers, "bridge-secret"));

        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer wrong"));
        assert!(!localization_authorized(&headers, "bridge-secret"));

        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer bridge-secret"),
        );
        assert!(localization_authorized(&headers, "bridge-secret"));
    }

    #[tokio::test]
    async fn verified_zero_stop_is_not_exposed_as_an_agent_capability() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();
        assert!(harness
            .capability_registry()
            .descriptors()
            .iter()
            .all(|descriptor| descriptor.name != "stop_verified"));

        let error = motors_stop_verified(State(harness), None)
            .await
            .unwrap_err()
            .0
            .to_string();
        assert!(error.contains("Waveshare UGV physical adapter"));
    }

    #[tokio::test]
    async fn localization_http_ingress_projects_into_the_generic_provider_queue() {
        let _guard = LOCALIZATION_ENV_LOCK.lock().await;
        let token_path =
            std::env::temp_dir().join(format!("leash-localization-token-{}", std::process::id()));
        fs::write(&token_path, "bridge-secret\n").unwrap();
        std::env::set_var("LEASH_LOCALIZATION_INGRESS_TOKEN_FILE", &token_path);

        let harness = Harness::new(HarnessConfig::default()).unwrap();
        let update = LocalizationProviderUpdate::from_telemetry(2, &harness.telemetry());
        let unauthorized = localization_update(
            State(harness.clone()),
            HeaderMap::new(),
            Json(update.clone()),
        )
        .await;
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer bridge-secret"),
        );
        let accepted = localization_update(State(harness), headers, Json(update)).await;
        assert_eq!(accepted.status(), StatusCode::ACCEPTED);

        std::env::remove_var("LEASH_LOCALIZATION_INGRESS_TOKEN_FILE");
        fs::remove_file(token_path).unwrap();
    }
}
