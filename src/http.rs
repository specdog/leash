use std::{convert::Infallible, net::SocketAddr, time::Duration};

use anyhow::Result;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    http::{header::CONTENT_TYPE, HeaderValue, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use futures_util::{stream, SinkExt, Stream, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::time;
use tower_http::cors::CorsLayer;

use crate::capability::InvocationOrigin;
use crate::types::{AgentMessageAck, AgentMessageList};
use crate::{runtime::Harness, transport::StreamRecvError, types::SpeedMode};

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
    Router::new()
        .route("/health", get(health))
        .route("/capabilities", get(capabilities))
        .route("/modules", get(modules))
        .route("/telemetry", get(telemetry))
        .route("/events/telemetry", get(sse_telemetry))
        .route("/sse/telemetry", get(sse_telemetry))
        .route("/sensors", get(sensors))
        .route("/camera/status", get(camera_status))
        .route("/agent", get(agent_page))
        .route("/agent/messages", get(agent_messages).post(agent_message))
        .route("/agent/send", post(agent_message))
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

#[cfg(feature = "mcp")]
pub fn mcp_router(harness: Harness) -> Router {
    Router::new()
        .route("/", get(mcp_status))
        .route("/status", get(mcp_status))
        .route("/tools", get(mcp_tools))
        .route("/list-tools", get(mcp_tools))
        .route("/modules", get(mcp_modules))
        .route("/call", post(mcp_call))
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
