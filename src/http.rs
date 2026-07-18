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
        Form, Path as AxumPath, Query, State, WebSocketUpgrade,
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
use crate::agent_runtime::{
    AgentRunOutput, AgentRuntime, AgentRuntimeSnapshot, AgentTaskStopOutput, AgentTaskStore,
    CapabilityPermissions,
};
use crate::capability::{InvocationOrigin, SafetyClass};
use crate::runtime::{
    CAMERA_PAN_MAX_DEG, CAMERA_PAN_MIN_DEG, CAMERA_TILT_MAX_DEG, CAMERA_TILT_MIN_DEG,
};
use crate::types::{AgentMessageAck, AgentMessageList};
#[cfg(feature = "webrtc")]
use crate::webrtc_camera::{camera_webrtc_status, camera_webrtc_ws};
use crate::{
    runtime::Harness, transport::StreamRecvError, types::SpeedMode, LocalizationProviderUpdate,
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
struct AgentMessageReq {
    text: String,
    source: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct AgentConsoleQuery {
    session: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AgentRunReq {
    prompt: String,
    session: Option<String>,
    #[serde(default)]
    continue_last: bool,
}

#[derive(Debug, Deserialize)]
struct AgentCapabilityReq {
    capability: String,
    args: Option<Value>,
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
        .route("/telemetry/compact", get(compact_telemetry))
        .route("/cognition/status", get(cognition_status))
        .route("/cognition/snapshot", get(cognition_snapshot))
        .route("/cognition/checkpoint", post(cognition_checkpoint))
        .route("/cognition/boundary", post(cognition_boundary_update))
        .route("/events/cognition", get(sse_cognition))
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
        .route("/agent/state", get(agent_console_state))
        .route("/agent/run", post(agent_console_run))
        .route("/agent/capability", post(agent_console_capability))
        .route("/agent/tasks/:name/stop", post(agent_console_task_stop))
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
        .route("/stop", post(motors_stop))
        .route("/estop", post(estop))
        .route("/estop/reset", post(estop_reset))
        .route("/stream", get(stream))
        .route("/ws/telemetry", get(ws_telemetry));
    #[cfg(feature = "webrtc")]
    let app = app
        .route("/camera/webrtc", get(camera_webrtc_status))
        .route("/camera/webrtc/ws", get(camera_webrtc_ws));
    #[cfg(feature = "mcp")]
    let app = app
        .route("/mcp", post(mcp_protocol))
        .route("/mcp/status", get(mcp_status))
        .route("/mcp/tools", get(mcp_tools))
        .route("/mcp/list-tools", get(mcp_tools))
        .route("/mcp/modules", get(mcp_modules))
        .route("/mcp/call", post(mcp_call));
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

async fn compact_telemetry(State(harness): State<Harness>) -> Json<Value> {
    Json(compact_telemetry_value(&harness))
}

fn compact_telemetry_value(harness: &Harness) -> Value {
    let telemetry = serde_json::to_value(harness.telemetry()).unwrap_or_else(|_| json!({}));
    let mut voxel_grid = telemetry
        .get("voxel_grid")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let Some(grid) = voxel_grid.as_object_mut() {
        let voxels = grid
            .get("voxels")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|voxel| voxel.get("occupancy").and_then(Value::as_u64).unwrap_or(0) >= 50)
            .take(5_000)
            .map(|voxel| {
                Value::Array(vec![
                    voxel.get("x").cloned().unwrap_or(Value::Null),
                    voxel.get("y").cloned().unwrap_or(Value::Null),
                    voxel.get("z").cloned().unwrap_or(Value::Null),
                    voxel.get("occupancy").cloned().unwrap_or(Value::Null),
                ])
            })
            .collect::<Vec<_>>();
        grid.insert("voxels".to_string(), Value::Array(voxels));
    }

    json!({
        "schema_version": "leash.telemetry.compact.v1",
        "ts_ms": telemetry.get("ts_ms").cloned().unwrap_or(Value::Null),
        "sensors": telemetry.get("sensors").cloned().unwrap_or_else(|| json!({})),
        "localization": telemetry.get("localization").cloned().unwrap_or_else(|| json!({})),
        "localization_provider": telemetry.get("localization_provider").cloned().unwrap_or_else(|| json!({})),
        "voxel_grid": voxel_grid,
        "path": telemetry.get("path").cloned().unwrap_or_else(|| json!({})),
        "cognition": harness.cognition_boundary()
    })
}

async fn cognition_status(
    State(harness): State<Harness>,
) -> Json<crate::cognition::CognitionStatusV1> {
    Json(harness.cognition_status())
}

async fn cognition_snapshot(
    State(harness): State<Harness>,
) -> Json<Vec<crate::cognition::CognitionLayerSnapshotV1>> {
    Json(harness.cognition_snapshots())
}

async fn cognition_checkpoint(
    State(harness): State<Harness>,
) -> Result<Json<crate::cognition::CognitionCheckpointV1>, HttpError> {
    Ok(Json(harness.cognition_checkpoint()?))
}

async fn cognition_boundary_update(
    State(harness): State<Harness>,
    headers: HeaderMap,
    Json(frame): Json<crate::cognition::CognitionBoundaryFrameV1>,
) -> Response {
    let expected = match cognition_ingress_token() {
        Ok(token) => token,
        Err(error) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"ok": false, "error": error.to_string()})),
            )
                .into_response();
        }
    };
    if !bearer_authorized(&headers, &expected) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"ok": false, "error": "cognition ingress authorization failed"})),
        )
            .into_response();
    }
    match harness.submit_cognition_boundary(frame) {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(json!({"ok": true, "cognition": harness.cognition_status()})),
        )
            .into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"ok": false, "error": error.to_string()})),
        )
            .into_response(),
    }
}

fn cognition_ingress_token() -> Result<String> {
    let path = env::var("LEASH_COGNITION_INGRESS_TOKEN_FILE").map_err(|_| {
        anyhow::anyhow!("cognition ingress is disabled; set LEASH_COGNITION_INGRESS_TOKEN_FILE")
    })?;
    read_ingress_token(&path, "cognition")
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
    read_ingress_token(&path, "localization")
}

fn read_ingress_token(path: &str, name: &str) -> Result<String> {
    let token = std::fs::read_to_string(path)
        .map_err(|error| anyhow::anyhow!("cannot read {name} ingress token file: {error}"))?;
    let token = token.trim().to_string();
    if token.is_empty() {
        anyhow::bail!("{name} ingress token file is empty");
    }
    Ok(token)
}

fn localization_authorized(headers: &HeaderMap, expected: &str) -> bool {
    bearer_authorized(headers, expected)
}

fn bearer_authorized(headers: &HeaderMap, expected: &str) -> bool {
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
        "gimbal": camera_aim_descriptor(harness.camera_aim_state())
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

async fn camera_aim_status(State(harness): State<Harness>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "gimbal": camera_aim_descriptor(harness.camera_aim_state())
    }))
}

fn camera_aim_descriptor(pose: Option<crate::types::CameraAimState>) -> Value {
    json!({
        "status": "available",
        "capability": "camera_aim",
        "endpoint": "/camera/aim",
        "aliases": ["/gimbal/aim"],
        "known": pose.is_some(),
        "source": pose.as_ref().map(|state| state.source.as_str()).unwrap_or("unavailable"),
        "pose": pose,
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

async fn agent_console_state(
    State(harness): State<Harness>,
    Query(query): Query<AgentConsoleQuery>,
) -> Result<Json<AgentRuntimeSnapshot>, HttpError> {
    let runtime = AgentRuntime::from_env(harness, CapabilityPermissions::from_env()?)?;
    Ok(Json(runtime.snapshot(query.session.as_deref(), 6)?))
}

async fn agent_console_run(
    State(harness): State<Harness>,
    Json(req): Json<AgentRunReq>,
) -> Result<Json<AgentRunOutput>, HttpError> {
    let runtime = AgentRuntime::from_env(harness, CapabilityPermissions::from_env()?)?;
    Ok(Json(runtime.run_prompt(
        &req.prompt,
        req.session.as_deref(),
        req.continue_last,
    )?))
}

async fn agent_console_capability(
    State(harness): State<Harness>,
    Json(req): Json<AgentCapabilityReq>,
) -> Result<Json<Value>, HttpError> {
    let capability = req.capability.trim();
    let registry = harness.capability_registry();
    let descriptor = registry
        .descriptors()
        .iter()
        .find(|descriptor| descriptor.name == capability)
        .ok_or_else(|| anyhow::anyhow!("unknown capability '{capability}'"))?;
    if descriptor.safety != SafetyClass::ObserveOnly {
        return Err(anyhow::anyhow!(
            "the headful console only probes observe-only capabilities; '{capability}' is {}",
            descriptor.safety.as_str()
        )
        .into());
    }
    let args = req.args.unwrap_or_else(|| json!({}));
    if !args.is_object() {
        return Err(anyhow::anyhow!("capability args must be a JSON object").into());
    }

    let runtime = AgentRuntime::from_env(harness, CapabilityPermissions::from_env()?)?;
    let result = runtime.invoke_capability(capability, args)?;
    Ok(Json(json!({
        "ok": true,
        "capability": capability,
        "result": result,
    })))
}

async fn agent_console_task_stop(
    AxumPath(name): AxumPath<String>,
) -> Result<Json<AgentTaskStopOutput>, HttpError> {
    Ok(Json(
        AgentTaskStore::from_env()?.stop(&name, Duration::from_secs(2))?,
    ))
}

async fn agent_page() -> Response {
    let body = r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="color-scheme" content="dark">
  <title>Leash Agent Console</title>
  <style>
    :root {
      color-scheme: dark;
      font-family: Inter, ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      font-synthesis: none;
      --bg: #080c10;
      --panel: #0e141a;
      --panel-raised: #141b22;
      --panel-hover: #19222b;
      --border: #26323d;
      --border-strong: #3a4754;
      --text: #f4f7fa;
      --muted: #94a3b2;
      --quiet: #667585;
      --accent: #f97316;
      --accent-strong: #fb923c;
      --green: #45d483;
      --red: #ff6868;
      --amber: #f8bf4f;
      --blue: #67b6ff;
      --radius: 8px;
    }

    * { box-sizing: border-box; }
    html { background: var(--bg); }
    body { margin: 0; min-width: 320px; min-height: 100vh; background: var(--bg); color: var(--text); }
    button, input, select, textarea { font: inherit; }
    button, select, input { min-height: 44px; }
    button { cursor: pointer; }
    button:disabled { cursor: not-allowed; opacity: .5; }
    a { color: inherit; }
    :focus-visible { outline: 2px solid var(--accent-strong); outline-offset: 2px; }

    .app { min-height: 100vh; display: grid; grid-template-rows: auto 1fr; }
    .topbar {
      min-height: 64px;
      padding: 10px 16px;
      display: flex;
      align-items: center;
      gap: 18px;
      border-bottom: 1px solid var(--border);
      background: rgba(8, 12, 16, .96);
      position: sticky;
      top: 0;
      z-index: 20;
    }
    .brand { display: flex; align-items: center; gap: 10px; min-width: max-content; }
    .brand-mark {
      width: 10px;
      height: 28px;
      border-radius: 2px;
      background: var(--accent);
      box-shadow: 0 0 24px rgba(249, 115, 22, .28);
    }
    .brand strong { display: block; font-size: 14px; letter-spacing: .14em; }
    .brand small { display: block; margin-top: 2px; color: var(--muted); font-size: 11px; letter-spacing: .08em; }
    .topbar-status { min-width: 0; flex: 1; display: flex; align-items: center; gap: 8px; overflow-x: auto; scrollbar-width: none; }
    .topbar-status::-webkit-scrollbar { display: none; }
    .chip {
      min-height: 28px;
      padding: 5px 9px;
      display: inline-flex;
      align-items: center;
      gap: 7px;
      border: 1px solid var(--border);
      border-radius: 999px;
      color: var(--muted);
      background: var(--panel);
      font: 600 11px/1.2 ui-monospace, SFMono-Regular, Menlo, monospace;
      white-space: nowrap;
    }
    .live-dot { width: 7px; height: 7px; border-radius: 50%; background: var(--quiet); }
    .live-dot.ok { background: var(--green); box-shadow: 0 0 0 3px rgba(69, 212, 131, .12); }
    .live-dot.bad { background: var(--red); }
    .dashboard-link {
      min-height: 44px;
      padding: 0 13px;
      display: inline-flex;
      align-items: center;
      border: 1px solid var(--border);
      border-radius: var(--radius);
      text-decoration: none;
      color: var(--muted);
      font-size: 13px;
      font-weight: 650;
      white-space: nowrap;
    }
    .dashboard-link:hover { color: var(--text); border-color: var(--border-strong); background: var(--panel); }

    .workspace {
      min-height: 0;
      display: grid;
      grid-template-columns: minmax(210px, 260px) minmax(420px, 1fr) minmax(290px, 350px);
    }
    .rail, .main-console { min-width: 0; min-height: 0; }
    .rail { background: var(--panel); }
    .rail-left { border-right: 1px solid var(--border); }
    .rail-right { border-left: 1px solid var(--border); }
    .rail-scroll { height: calc(100vh - 65px); overflow-y: auto; padding: 14px; }
    .section-heading { margin-bottom: 12px; display: flex; align-items: center; justify-content: space-between; gap: 10px; }
    .eyebrow { margin: 0; color: var(--quiet); font: 700 10px/1.2 ui-monospace, SFMono-Regular, Menlo, monospace; letter-spacing: .14em; text-transform: uppercase; }
    h1, h2, h3, p { margin-top: 0; }
    h2 { margin-bottom: 0; font-size: 14px; font-weight: 700; }
    h3 { margin-bottom: 0; font-size: 13px; }
    .count { color: var(--quiet); font: 600 11px/1 ui-monospace, SFMono-Regular, Menlo, monospace; }

    .button {
      min-height: 44px;
      padding: 0 13px;
      border: 1px solid var(--border-strong);
      border-radius: var(--radius);
      color: var(--text);
      background: var(--panel-raised);
      font-size: 13px;
      font-weight: 680;
    }
    .button:hover:not(:disabled) { background: var(--panel-hover); border-color: #52606d; }
    .button-primary { border-color: #c9570c; background: var(--accent); color: #160a02; }
    .button-primary:hover:not(:disabled) { background: var(--accent-strong); border-color: var(--accent-strong); }
    .button-danger { border-color: rgba(255, 104, 104, .45); color: #ffaaaa; background: rgba(255, 104, 104, .06); }
    .button-small { min-height: 36px; padding: 0 10px; font-size: 12px; }
    .button-wide { width: 100%; }

    .session-list { display: grid; gap: 6px; margin-top: 10px; }
    .session-item {
      width: 100%;
      min-height: 64px;
      padding: 10px;
      display: grid;
      gap: 5px;
      text-align: left;
      border: 1px solid transparent;
      border-radius: var(--radius);
      color: var(--muted);
      background: transparent;
    }
    .session-item:hover { border-color: var(--border); background: var(--panel-hover); color: var(--text); }
    .session-item.active { border-color: rgba(249, 115, 22, .52); background: rgba(249, 115, 22, .08); color: var(--text); }
    .session-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font: 650 12px/1.3 ui-monospace, SFMono-Regular, Menlo, monospace; }
    .session-meta { display: flex; justify-content: space-between; gap: 8px; color: var(--quiet); font-size: 11px; }
    .empty { padding: 18px 12px; border: 1px dashed var(--border); border-radius: var(--radius); color: var(--quiet); font-size: 12px; line-height: 1.55; }
    .storage-note { margin-top: 18px; padding-top: 14px; border-top: 1px solid var(--border); color: var(--quiet); font-size: 10px; line-height: 1.5; overflow-wrap: anywhere; }

    .main-console { height: calc(100vh - 65px); display: grid; grid-template-rows: auto minmax(0, 1fr) auto; background: #0a0f14; }
    .console-header { min-height: 67px; padding: 12px 18px; display: flex; align-items: center; justify-content: space-between; gap: 14px; border-bottom: 1px solid var(--border); }
    .console-title { min-width: 0; }
    .console-title h1 { margin: 3px 0 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 17px; }
    .model-meta { flex: 0 0 auto; color: var(--quiet); font: 600 11px/1.4 ui-monospace, SFMono-Regular, Menlo, monospace; text-align: right; white-space: pre-line; }
    .transcript { min-height: 0; padding: 22px clamp(16px, 4vw, 54px); overflow-y: auto; scroll-behavior: smooth; }
    .transcript-inner { width: min(820px, 100%); margin: 0 auto; display: grid; gap: 24px; }
    .turn { display: grid; gap: 12px; }
    .message-label { margin-bottom: 6px; color: var(--quiet); font: 700 10px/1.2 ui-monospace, SFMono-Regular, Menlo, monospace; letter-spacing: .1em; text-transform: uppercase; }
    .message {
      max-width: 92%;
      padding: 12px 14px;
      border: 1px solid var(--border);
      border-radius: var(--radius);
      white-space: pre-wrap;
      overflow-wrap: anywhere;
      line-height: 1.55;
      font-size: 14px;
    }
    .message-operator { justify-self: end; border-color: rgba(249, 115, 22, .35); background: rgba(249, 115, 22, .08); }
    .operator-wrap { display: grid; justify-items: end; }
    .message-agent { background: var(--panel); }
    .turn-time { color: var(--quiet); font: 500 10px/1 ui-monospace, SFMono-Regular, Menlo, monospace; }
    .transcript-empty { min-height: 100%; display: grid; place-items: center; }
    .transcript-empty-card { width: min(480px, 100%); padding: 22px; border: 1px dashed var(--border-strong); border-radius: var(--radius); color: var(--muted); text-align: center; }
    .transcript-empty-card strong { display: block; margin-bottom: 7px; color: var(--text); font-size: 15px; }
    .transcript-empty-card span { font-size: 13px; line-height: 1.5; }

    .composer { padding: 12px 18px 16px; border-top: 1px solid var(--border); background: var(--panel); }
    .composer-inner { width: min(900px, 100%); margin: 0 auto; display: grid; gap: 8px; }
    .composer-row { display: grid; grid-template-columns: minmax(0, 1fr) auto; align-items: end; gap: 9px; }
    label { display: grid; gap: 6px; color: var(--muted); font-size: 11px; font-weight: 650; }
    input, select, textarea {
      width: 100%;
      border: 1px solid var(--border-strong);
      border-radius: var(--radius);
      color: var(--text);
      background: #0a0f14;
    }
    input, select { padding: 0 11px; }
    textarea { padding: 11px; resize: vertical; line-height: 1.45; }
    input::placeholder, textarea::placeholder { color: #596878; }
    .session-field { max-width: 320px; }
    .prompt-field { min-height: 78px; max-height: 220px; }
    .composer-help { display: flex; justify-content: space-between; gap: 12px; color: var(--quiet); font-size: 10px; }

    .ops-section + .ops-section { margin-top: 22px; padding-top: 20px; border-top: 1px solid var(--border); }
    .health-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 7px; }
    .metric { min-height: 57px; padding: 9px; border: 1px solid var(--border); border-radius: var(--radius); background: #0a0f14; }
    .metric span { display: block; color: var(--quiet); font: 650 9px/1.2 ui-monospace, SFMono-Regular, Menlo, monospace; letter-spacing: .08em; text-transform: uppercase; }
    .metric strong { display: block; margin-top: 8px; overflow: hidden; text-overflow: ellipsis; color: var(--text); font-size: 12px; white-space: nowrap; }
    .metric strong.ok { color: var(--green); }
    .metric strong.warn { color: var(--amber); }
    .metric strong.bad { color: var(--red); }
    .permission-note { margin: 10px 0 0; color: var(--quiet); font-size: 10px; line-height: 1.5; overflow-wrap: anywhere; }

    .task-list { display: grid; gap: 8px; }
    .task-card { padding: 11px; border: 1px solid var(--border); border-radius: var(--radius); background: #0a0f14; }
    .task-head { display: flex; align-items: start; justify-content: space-between; gap: 10px; }
    .task-name { font: 700 12px/1.3 ui-monospace, SFMono-Regular, Menlo, monospace; overflow-wrap: anywhere; }
    .state-badge { padding: 4px 6px; border-radius: 4px; background: var(--panel-hover); color: var(--muted); font: 700 9px/1 ui-monospace, SFMono-Regular, Menlo, monospace; text-transform: uppercase; }
    .state-badge.running { color: var(--green); background: rgba(69, 212, 131, .09); }
    .state-badge.failed { color: var(--red); background: rgba(255, 104, 104, .09); }
    .task-stats { margin: 10px 0; display: grid; grid-template-columns: 1fr 1fr; gap: 5px 12px; color: var(--quiet); font-size: 10px; }
    .task-stats strong { color: var(--muted); font-weight: 650; }
    .task-log { max-height: 100px; margin: 0 0 9px; padding: 8px; overflow: auto; border-left: 2px solid var(--border-strong); color: #a9b6c2; background: var(--panel); white-space: pre-wrap; overflow-wrap: anywhere; font: 500 9px/1.45 ui-monospace, SFMono-Regular, Menlo, monospace; }
    .task-error { color: var(--red); }

    .probe-form { display: grid; gap: 9px; }
    .args-field { min-height: 82px; font: 500 11px/1.45 ui-monospace, SFMono-Regular, Menlo, monospace; }
    .probe-output { max-height: 220px; margin: 0; padding: 10px; overflow: auto; border: 1px solid var(--border); border-radius: var(--radius); color: #b9d7ef; background: #080c10; white-space: pre-wrap; overflow-wrap: anywhere; font: 500 10px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace; }
    .probe-note { margin: 0; color: var(--quiet); font-size: 10px; line-height: 1.5; }
    .status-line { min-height: 18px; margin: 8px 0 0; color: var(--muted); font-size: 11px; }
    .status-line.error { color: var(--red); }

    @media (max-width: 1120px) {
      .workspace { grid-template-columns: 220px minmax(400px, 1fr) 300px; }
      .rail-scroll { padding: 11px; }
    }
    @media (max-width: 940px) {
      .workspace { grid-template-columns: 210px minmax(0, 1fr); }
      .rail-right { grid-column: 1 / -1; border-left: 0; border-top: 1px solid var(--border); }
      .rail-right .rail-scroll { height: auto; display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 18px; }
      .ops-section + .ops-section { margin-top: 0; padding-top: 0; border-top: 0; }
    }
    @media (max-width: 700px) {
      .topbar { align-items: flex-start; flex-wrap: wrap; gap: 8px 12px; }
      .brand { flex: 1; }
      .topbar-status { order: 3; flex-basis: 100%; }
      .dashboard-link { min-height: 40px; }
      .workspace { display: block; }
      .rail-left { border-right: 0; border-bottom: 1px solid var(--border); }
      .rail-scroll { height: auto; }
      .session-list { grid-template-columns: repeat(auto-fit, minmax(170px, 1fr)); }
      .storage-note { display: none; }
      .main-console { height: auto; min-height: 72vh; }
      .transcript { min-height: 44vh; max-height: 62vh; padding: 18px 14px; }
      .console-header { padding: 10px 14px; }
      .composer { padding: 11px 14px 14px; }
      .composer-row { grid-template-columns: 1fr; }
      .composer-row .button { width: 100%; }
      .session-field { max-width: none; }
      .rail-right .rail-scroll { display: grid; grid-template-columns: 1fr; }
    }
    @media (prefers-reduced-motion: reduce) {
      *, *::before, *::after { scroll-behavior: auto !important; transition: none !important; animation: none !important; }
    }
  </style>
</head>
<body>
  <div class="app">
    <header class="topbar">
      <div class="brand" aria-label="Leash agent console">
        <span class="brand-mark" aria-hidden="true"></span>
        <span><strong>LEASH</strong><small>AGENT CONSOLE</small></span>
      </div>
      <div class="topbar-status" aria-label="Runtime status">
        <span class="chip"><span id="live-dot" class="live-dot"></span><span id="connection-label">CONNECTING</span></span>
        <span id="profile-chip" class="chip">PROFILE —</span>
        <span id="mode-chip" class="chip">MODE —</span>
        <span id="motion-chip" class="chip">MOTION —</span>
      </div>
      <a class="dashboard-link" href="/dashboard">Robot dashboard</a>
    </header>

    <div class="workspace">
      <aside class="rail rail-left" aria-label="Agent sessions">
        <div class="rail-scroll">
          <div class="section-heading">
            <div><p class="eyebrow">Persistent state</p><h2>Sessions</h2></div>
            <span id="session-count" class="count">0</span>
          </div>
          <button id="new-session" class="button button-wide" type="button">New session</button>
          <div id="session-list" class="session-list"></div>
          <p id="state-path" class="storage-note">Waiting for state directory…</p>
        </div>
      </aside>

      <main class="main-console">
        <header class="console-header">
          <div class="console-title">
            <p class="eyebrow">Conversation</p>
            <h1 id="session-title">New session</h1>
          </div>
          <div id="model-meta" class="model-meta">provider —<br>model —</div>
        </header>

        <section id="transcript" class="transcript" aria-label="Session transcript" aria-live="polite">
          <div id="transcript-inner" class="transcript-inner"></div>
        </section>

        <form id="prompt-form" class="composer">
          <div class="composer-inner">
            <label class="session-field" for="session-id">Session ID
              <input id="session-id" name="session" autocomplete="off" maxlength="80" pattern="[A-Za-z0-9_-]+" placeholder="generated when blank">
            </label>
            <div class="composer-row">
              <label for="agent-prompt">Instruction
                <textarea id="agent-prompt" class="prompt-field" name="prompt" required autofocus placeholder="Ask the configured agent to inspect, reason, or summarize…"></textarea>
              </label>
              <button id="run-button" class="button button-primary" type="submit">Run agent</button>
            </div>
            <div class="composer-help"><span>Shift + Enter for a new line</span><span id="run-state" role="status" aria-live="polite">Ready</span></div>
          </div>
        </form>
      </main>

      <aside class="rail rail-right" aria-label="Runtime operations">
        <div class="rail-scroll">
          <section class="ops-section">
            <div class="section-heading">
              <div><p class="eyebrow">Safety boundary</p><h2>Runtime</h2></div>
            </div>
            <div class="health-grid">
              <div class="metric"><span>Harness</span><strong id="health-value">—</strong></div>
              <div class="metric"><span>Deadman</span><strong id="deadman-value">—</strong></div>
              <div class="metric"><span>E-stop</span><strong id="estop-value">—</strong></div>
              <div class="metric"><span>Navigation</span><strong id="navigation-value">—</strong></div>
            </div>
            <p id="permissions" class="permission-note">Loading agent permissions…</p>
          </section>

          <section class="ops-section">
            <div class="section-heading">
              <div><p class="eyebrow">Supervised work</p><h2>Tasks</h2></div>
              <span id="task-count" class="count">0</span>
            </div>
            <div id="task-list" class="task-list"></div>
          </section>

          <section class="ops-section">
            <div class="section-heading">
              <div><p class="eyebrow">Read-only tool call</p><h2>Capability probe</h2></div>
            </div>
            <form id="probe-form" class="probe-form">
              <label for="capability-select">Capability
                <select id="capability-select" required></select>
              </label>
              <label for="capability-args">JSON arguments
                <textarea id="capability-args" class="args-field" spellcheck="false">{}</textarea>
              </label>
              <button id="probe-button" class="button" type="submit">Invoke probe</button>
              <p class="probe-note">This panel accepts observe-only capabilities. Motion is never exposed here.</p>
              <pre id="probe-output" class="probe-output" aria-live="polite">No probe run yet.</pre>
            </form>
          </section>
          <p id="console-status" class="status-line" role="status" aria-live="polite"></p>
        </div>
      </aside>
    </div>
  </div>
  <script>
    const elements = {
      liveDot: document.querySelector("#live-dot"),
      connectionLabel: document.querySelector("#connection-label"),
      profileChip: document.querySelector("#profile-chip"),
      modeChip: document.querySelector("#mode-chip"),
      motionChip: document.querySelector("#motion-chip"),
      sessionCount: document.querySelector("#session-count"),
      sessionList: document.querySelector("#session-list"),
      newSession: document.querySelector("#new-session"),
      statePath: document.querySelector("#state-path"),
      sessionTitle: document.querySelector("#session-title"),
      modelMeta: document.querySelector("#model-meta"),
      transcript: document.querySelector("#transcript"),
      transcriptInner: document.querySelector("#transcript-inner"),
      promptForm: document.querySelector("#prompt-form"),
      sessionId: document.querySelector("#session-id"),
      prompt: document.querySelector("#agent-prompt"),
      runButton: document.querySelector("#run-button"),
      runState: document.querySelector("#run-state"),
      health: document.querySelector("#health-value"),
      deadman: document.querySelector("#deadman-value"),
      estop: document.querySelector("#estop-value"),
      navigation: document.querySelector("#navigation-value"),
      permissions: document.querySelector("#permissions"),
      taskCount: document.querySelector("#task-count"),
      taskList: document.querySelector("#task-list"),
      probeForm: document.querySelector("#probe-form"),
      capabilitySelect: document.querySelector("#capability-select"),
      capabilityArgs: document.querySelector("#capability-args"),
      probeButton: document.querySelector("#probe-button"),
      probeOutput: document.querySelector("#probe-output"),
      consoleStatus: document.querySelector("#console-status")
    };

    let selectedSessionId = new URLSearchParams(window.location.search).get("session");
    let latestState = null;
    let refreshing = false;
    let promptRunning = false;
    let newSessionDraft = false;

    function textElement(tag, className, text) {
      const element = document.createElement(tag);
      if (className) element.className = className;
      element.textContent = text;
      return element;
    }

    function setMetric(element, text, tone) {
      element.textContent = text;
      element.className = tone || "";
    }

    function formatAge(timestamp) {
      const age = Math.max(0, Date.now() - Number(timestamp));
      if (age < 1000) return "now";
      if (age < 60000) return Math.floor(age / 1000) + "s ago";
      if (age < 3600000) return Math.floor(age / 60000) + "m ago";
      return Math.floor(age / 3600000) + "h ago";
    }

    function formatDuration(milliseconds) {
      const value = Number(milliseconds);
      if (value < 1000) return value + "ms";
      if (value < 60000) return (value / 1000).toFixed(value % 1000 === 0 ? 0 : 1) + "s";
      return (value / 60000).toFixed(1) + "m";
    }

    async function fetchJson(url, options) {
      const response = await fetch(url, options);
      let payload;
      try {
        payload = await response.json();
      } catch (_) {
        throw new Error("Leash returned an unreadable response (HTTP " + response.status + ")");
      }
      if (!response.ok || payload.ok === false) {
        throw new Error(payload.error || "Request failed (HTTP " + response.status + ")");
      }
      return payload;
    }

    function setConnection(ok, message) {
      elements.liveDot.className = "live-dot " + (ok ? "ok" : "bad");
      elements.connectionLabel.textContent = ok ? "LIVE" : "OFFLINE";
      elements.consoleStatus.textContent = message || (ok ? "State synchronized" : "Connection lost");
      elements.consoleStatus.className = "status-line" + (ok ? "" : " error");
    }

    function renderSessions(state) {
      elements.sessionList.replaceChildren();
      elements.sessionCount.textContent = String(state.sessions.length);
      if (state.sessions.length === 0) {
        elements.sessionList.append(textElement("div", "empty", "No saved sessions yet. Run an instruction to create one."));
        return;
      }
      state.sessions.forEach((session) => {
        const button = document.createElement("button");
        button.type = "button";
        button.className = "session-item" + (session.id === selectedSessionId ? " active" : "");
        button.setAttribute("aria-pressed", session.id === selectedSessionId ? "true" : "false");
        button.append(textElement("span", "session-name", session.id));
        const meta = document.createElement("span");
        meta.className = "session-meta";
        meta.append(textElement("span", "", session.turns + (session.turns === 1 ? " turn" : " turns")));
        meta.append(textElement("span", "", formatAge(session.updated_at_ms)));
        button.append(meta);
        button.addEventListener("click", () => selectSession(session.id));
        elements.sessionList.append(button);
      });
    }

    function renderTranscript(session) {
      elements.transcriptInner.replaceChildren();
      if (!session || session.turns.length === 0) {
        const empty = document.createElement("div");
        empty.className = "transcript-empty";
        const card = document.createElement("div");
        card.className = "transcript-empty-card";
        card.append(textElement("strong", "", session ? "Session ready" : "Start a visible agent run"));
        card.append(textElement("span", "", "Instructions and model responses will appear here while the persisted runtime state updates around them."));
        empty.append(card);
        elements.transcriptInner.append(empty);
        return;
      }
      session.turns.forEach((turn) => {
        const article = document.createElement("article");
        article.className = "turn";

        const operator = document.createElement("div");
        operator.className = "operator-wrap";
        operator.append(textElement("div", "message-label", "Operator · turn " + turn.sequence));
        operator.append(textElement("div", "message message-operator", turn.prompt));
        article.append(operator);

        const agent = document.createElement("div");
        agent.append(textElement("div", "message-label", "Agent"));
        agent.append(textElement("div", "message message-agent", turn.response.text));
        agent.append(textElement("div", "turn-time", new Date(Number(turn.started_at_ms)).toLocaleString()));
        article.append(agent);
        elements.transcriptInner.append(article);
      });
    }

    function taskEventText(task) {
      if (task.last_error) return task.last_error;
      if (!task.recent_events || task.recent_events.length === 0) return "No log events yet.";
      const event = task.recent_events[task.recent_events.length - 1];
      return JSON.stringify(event, null, 2);
    }

    function renderTasks(state) {
      elements.taskList.replaceChildren();
      elements.taskCount.textContent = String(state.tasks.length);
      if (state.tasks.length === 0) {
        elements.taskList.append(textElement("div", "empty", "No supervised tasks in this state directory."));
        return;
      }
      state.tasks.forEach((task) => {
        const card = document.createElement("article");
        card.className = "task-card";
        const head = document.createElement("div");
        head.className = "task-head";
        head.append(textElement("span", "task-name", task.name));
        const badge = textElement("span", "state-badge " + (task.running ? "running" : String(task.state).toLowerCase()), task.state);
        head.append(badge);
        card.append(head);

        const stats = document.createElement("div");
        stats.className = "task-stats";
        stats.append(textElement("span", "", "CAP  "));
        stats.lastChild.append(textElement("strong", "", task.capability));
        stats.append(textElement("span", "", "RUNS  "));
        stats.lastChild.append(textElement("strong", "", String(task.runs) + (task.max_runs ? " / " + task.max_runs : "")));
        stats.append(textElement("span", "", "EVERY  "));
        stats.lastChild.append(textElement("strong", "", formatDuration(task.interval_ms)));
        stats.append(textElement("span", "", "PID  "));
        stats.lastChild.append(textElement("strong", "", String(task.pid)));
        card.append(stats);

        const log = textElement("pre", "task-log" + (task.last_error ? " task-error" : ""), taskEventText(task));
        card.append(log);
        if (task.running) {
          const stop = textElement("button", "button button-danger button-small", "Stop task");
          stop.type = "button";
          stop.addEventListener("click", () => stopTask(task.name, stop));
          card.append(stop);
        }
        elements.taskList.append(card);
      });
    }

    function renderCapabilities(state) {
      const previous = elements.capabilitySelect.value;
      const capabilities = state.capabilities.filter((item) => item.safety === "observe-only");
      elements.capabilitySelect.replaceChildren();
      capabilities.forEach((capability) => {
        const option = document.createElement("option");
        option.value = capability.name;
        option.textContent = capability.name + " · " + capability.module;
        elements.capabilitySelect.append(option);
      });
      if (capabilities.some((item) => item.name === previous)) {
        elements.capabilitySelect.value = previous;
      } else if (capabilities.some((item) => item.name === "health")) {
        elements.capabilitySelect.value = "health";
      }
      elements.probeButton.disabled = capabilities.length === 0;
    }

    function renderRuntime(state) {
      const health = state.health;
      elements.profileChip.textContent = "PROFILE " + health.profile.toUpperCase();
      elements.modeChip.textContent = "MODE " + health.mode.toUpperCase();
      elements.motionChip.textContent = health.physical_actuation_enabled ? "MOTION ARMED" : "MOTION LOCKED";
      setMetric(elements.health, health.ok ? "healthy" : "degraded", health.ok ? "ok" : "bad");
      setMetric(elements.deadman, health.deadman_ok ? "ready" : "not ready", health.deadman_ok ? "ok" : "warn");
      setMetric(elements.estop, health.estop ? "engaged" : "clear", health.estop ? "bad" : "ok");
      setMetric(elements.navigation, health.physical_navigation_enabled ? "enabled" : "locked", health.physical_navigation_enabled ? "warn" : "ok");
      const allow = state.permissions.allow.length ? state.permissions.allow.join(", ") : "all registered capabilities";
      const deny = state.permissions.deny.length ? state.permissions.deny.join(", ") : "none";
      elements.permissions.textContent = "Agent scope — allow: " + allow + "; deny: " + deny + ". Probe remains observe-only.";
      elements.statePath.textContent = "State: " + state.state_dir;
    }

    function render(state) {
      latestState = state;
      if (!selectedSessionId && !newSessionDraft && state.selected_session) selectedSessionId = state.selected_session.id;
      const selected = state.selected_session && state.selected_session.id === selectedSessionId
        ? state.selected_session
        : null;
      renderRuntime(state);
      renderSessions(state);
      renderTranscript(selected);
      renderTasks(state);
      renderCapabilities(state);
      elements.sessionTitle.textContent = selected ? selected.id : "New session";
      elements.modelMeta.textContent = selected
        ? "provider " + selected.provider + "\nmodel " + selected.model
        : "provider —\nmodel —";
      if (document.activeElement !== elements.sessionId) {
        elements.sessionId.value = selected ? selected.id : "";
      }
    }

    async function loadState(options) {
      if (refreshing) return;
      refreshing = true;
      try {
        const params = new URLSearchParams();
        if (selectedSessionId) params.set("session", selectedSessionId);
        const suffix = params.toString() ? "?" + params.toString() : "";
        const state = await fetchJson("/agent/state" + suffix);
        render(state);
        setConnection(true, "Updated " + new Date().toLocaleTimeString());
        if (options && options.scroll) elements.transcript.scrollTop = elements.transcript.scrollHeight;
      } catch (error) {
        setConnection(false, error.message);
      } finally {
        refreshing = false;
      }
    }

    async function selectSession(id) {
      selectedSessionId = id;
      newSessionDraft = false;
      const url = new URL(window.location.href);
      url.searchParams.set("session", id);
      window.history.replaceState({}, "", url);
      await loadState({ scroll: true });
    }

    elements.newSession.addEventListener("click", () => {
      selectedSessionId = null;
      newSessionDraft = true;
      const url = new URL(window.location.href);
      url.searchParams.delete("session");
      window.history.replaceState({}, "", url);
      elements.sessionId.value = "";
      elements.sessionTitle.textContent = "New session";
      elements.modelMeta.textContent = "provider —\nmodel —";
      renderTranscript(null);
      if (latestState) renderSessions(latestState);
      elements.prompt.focus();
    });

    elements.prompt.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && !event.shiftKey) {
        event.preventDefault();
        elements.promptForm.requestSubmit();
      }
    });

    elements.promptForm.addEventListener("submit", async (event) => {
      event.preventDefault();
      if (promptRunning) return;
      promptRunning = true;
      elements.runButton.disabled = true;
      elements.runState.textContent = "Agent running…";
      try {
        const session = elements.sessionId.value.trim() || null;
        const payload = await fetchJson("/agent/run", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ prompt: elements.prompt.value, session, continue_last: false })
        });
        selectedSessionId = payload.session.id;
        newSessionDraft = false;
        const url = new URL(window.location.href);
        url.searchParams.set("session", selectedSessionId);
        window.history.replaceState({}, "", url);
        elements.prompt.value = "";
        elements.runState.textContent = "Turn " + payload.turn.sequence + " complete";
        await loadState({ scroll: true });
      } catch (error) {
        elements.runState.textContent = error.message;
        elements.consoleStatus.textContent = error.message;
        elements.consoleStatus.className = "status-line error";
      } finally {
        promptRunning = false;
        elements.runButton.disabled = false;
      }
    });

    async function stopTask(name, button) {
      button.disabled = true;
      button.textContent = "Stopping…";
      try {
        await fetchJson("/agent/tasks/" + encodeURIComponent(name) + "/stop", { method: "POST" });
        await loadState();
      } catch (error) {
        elements.consoleStatus.textContent = error.message;
        elements.consoleStatus.className = "status-line error";
        button.disabled = false;
        button.textContent = "Stop task";
      }
    }

    elements.probeForm.addEventListener("submit", async (event) => {
      event.preventDefault();
      elements.probeButton.disabled = true;
      elements.probeOutput.textContent = "Invoking…";
      try {
        let args;
        try {
          args = JSON.parse(elements.capabilityArgs.value);
        } catch (_) {
          throw new Error("Arguments are not valid JSON");
        }
        if (!args || Array.isArray(args) || typeof args !== "object") {
          throw new Error("Arguments must be a JSON object");
        }
        const payload = await fetchJson("/agent/capability", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ capability: elements.capabilitySelect.value, args })
        });
        elements.probeOutput.textContent = JSON.stringify(payload, null, 2);
      } catch (error) {
        elements.probeOutput.textContent = error.message;
      } finally {
        elements.probeButton.disabled = false;
      }
    });

    document.addEventListener("visibilitychange", () => {
      if (!document.hidden) loadState();
    });
    loadState({ scroll: true });
    window.setInterval(() => {
      if (!document.hidden && !promptRunning) loadState();
    }, 1500);
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

async fn sse_cognition(
    State(harness): State<Harness>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let receiver = harness.subscribe_cognition();
    let stream = stream::unfold(receiver, |mut receiver| async move {
        loop {
            match receiver.recv().await {
                Ok(frame) => {
                    let data = serde_json::to_string(&frame).unwrap_or_else(|_| "{}".to_string());
                    return Some((Ok(Event::default().event("cognition").data(data)), receiver));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(5))
            .text("keepalive"),
    )
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
        extract::{Query, State},
        http::{header::AUTHORIZATION, HeaderMap, HeaderValue, StatusCode},
        Json,
    };
    use serde_json::json;

    use super::{
        agent_console_capability, agent_console_run, agent_console_state, camera_activity,
        compact_telemetry_value, localization_authorized, localization_update, AgentCapabilityReq,
        AgentConsoleQuery, AgentRunReq, CameraRuntimeState, CAMERA_RUNTIME_STATE,
    };
    use crate::{Harness, HarnessConfig, LocalizationProviderUpdate};
    use tokio::sync::Mutex;

    static LOCALIZATION_ENV_LOCK: Mutex<()> = Mutex::const_new(());
    static AGENT_ENV_LOCK: Mutex<()> = Mutex::const_new(());

    #[tokio::test]
    async fn compact_telemetry_keeps_qualia_inputs_and_drops_dense_surfaces() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();
        let value = compact_telemetry_value(&harness);
        assert_eq!(
            value.get("schema_version").and_then(|value| value.as_str()),
            Some("leash.telemetry.compact.v1")
        );
        assert!(value.get("sensors").is_some());
        assert!(value.get("localization").is_some());
        assert!(value.get("voxel_grid").is_some());
        assert!(value.get("path").is_some());
        assert!(value.get("costmap").is_none());
        assert!(value
            .pointer("/voxel_grid/voxels")
            .and_then(|value| value.as_array())
            .is_some_and(|voxels| voxels.len() <= 5_000));
    }

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

    #[tokio::test]
    async fn agent_console_runs_persisted_turns_and_only_probes_observe_capabilities() {
        let _guard = AGENT_ENV_LOCK.lock().await;
        let state_root =
            std::env::temp_dir().join(format!("leash-http-agent-console-{}", std::process::id()));
        let previous_state_dir = std::env::var_os("LEASH_STATE_DIR");
        std::env::set_var("LEASH_STATE_DIR", &state_root);

        let harness = Harness::new(HarnessConfig::default()).unwrap();
        let Json(run) = agent_console_run(
            State(harness.clone()),
            Json(AgentRunReq {
                prompt: "show current state".to_string(),
                session: Some("headful-test".to_string()),
                continue_last: false,
            }),
        )
        .await
        .unwrap();
        assert_eq!(run.session.id, "headful-test");

        let Json(snapshot) = agent_console_state(
            State(harness.clone()),
            Query(AgentConsoleQuery {
                session: Some("headful-test".to_string()),
            }),
        )
        .await
        .unwrap();
        assert_eq!(snapshot.selected_session.unwrap().turns.len(), 1);

        let Json(probe) = agent_console_capability(
            State(harness.clone()),
            Json(AgentCapabilityReq {
                capability: "health".to_string(),
                args: Some(json!({})),
            }),
        )
        .await
        .unwrap();
        assert_eq!(probe["ok"], true);

        let denied = agent_console_capability(
            State(harness),
            Json(AgentCapabilityReq {
                capability: "drive".to_string(),
                args: Some(json!({ "left": 0.0, "right": 0.0 })),
            }),
        )
        .await
        .unwrap_err();
        assert!(denied.0.to_string().contains("observe-only"));

        if let Some(value) = previous_state_dir {
            std::env::set_var("LEASH_STATE_DIR", value);
        } else {
            std::env::remove_var("LEASH_STATE_DIR");
        }
        let _ = fs::remove_dir_all(state_root);
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
