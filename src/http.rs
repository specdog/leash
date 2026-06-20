use std::{convert::Infallible, net::SocketAddr, time::Duration};

use anyhow::Result;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Form, State, WebSocketUpgrade,
    },
    http::{header::CONTENT_TYPE, HeaderValue, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Redirect, Response,
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
    Router::new()
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
