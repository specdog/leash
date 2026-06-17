use anyhow::Result;
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    tool, tool_handler, tool_router,
    transport::stdio,
    Json, ServerHandler, ServiceExt,
};
use serde::{Deserialize, Serialize};

use crate::{runtime::Harness, types::SpeedMode};

#[derive(Clone)]
pub struct LeashMcp {
    harness: Harness,
    tool_router: ToolRouter<Self>,
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for LeashMcp {}

#[tool_router(router = tool_router)]
impl LeashMcp {
    pub fn new(harness: Harness) -> Self {
        Self {
            harness,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(name = "health", description = "Read harness health and safety state")]
    pub async fn health(&self) -> Json<crate::types::Health> {
        Json(self.harness.health())
    }

    #[tool(
        name = "capabilities",
        description = "List harness endpoints, MCP tools, and speed modes"
    )]
    pub async fn capabilities(&self) -> Json<crate::types::Capabilities> {
        Json(self.harness.capabilities())
    }

    #[tool(
        name = "observe",
        description = "Read the latest telemetry and sensor state"
    )]
    pub async fn observe(&self) -> Json<crate::types::TelemetryFrame> {
        Json(self.harness.telemetry())
    }

    #[tool(
        name = "invoke_capability",
        description = "Invoke a named harness capability such as authorize, drive, stop, estop, estop_reset, or speed_mode"
    )]
    pub async fn invoke_capability(
        &self,
        params: Parameters<InvokeCapabilityParams>,
    ) -> Result<String, String> {
        let params = params.0;
        let value = match params.capability.as_str() {
            "authorize" => {
                let token = params.token.ok_or("authorize requires token")?;
                let ttl_secs = params.ttl_secs.unwrap_or(120);
                let speed_mode = params.speed_mode.unwrap_or_default();
                self.harness
                    .authorize(token, ttl_secs, speed_mode)
                    .map_err(|err| err.to_string())?;
                serde_json::json!({ "ok": true, "ttl_secs": ttl_secs, "speed_mode": speed_mode })
            }
            "drive" => {
                let left = params.left.ok_or("drive requires left")?;
                let right = params.right.ok_or("drive requires right")?;
                serde_json::to_value(
                    self.harness
                        .drive(params.token.as_deref(), left, right, params.speed_mode)
                        .map_err(|err| err.to_string())?,
                )
                .map_err(|err| err.to_string())?
            }
            "stop" | "motors.stop" => {
                serde_json::to_value(self.harness.stop().map_err(|err| err.to_string())?)
                    .map_err(|err| err.to_string())?
            }
            "estop" => {
                self.harness.estop().map_err(|err| err.to_string())?;
                serde_json::json!({ "ok": true, "estop": true })
            }
            "estop_reset" | "estop/reset" => {
                self.harness.reset_estop();
                serde_json::json!({ "ok": true, "estop": false })
            }
            "speed_mode" => {
                let speed_mode = params.speed_mode.ok_or("speed_mode requires speed_mode")?;
                self.harness
                    .set_speed_mode(params.token.as_deref(), speed_mode)
                    .map_err(|err| err.to_string())?;
                serde_json::json!({ "ok": true, "speed_mode": speed_mode })
            }
            other => return Err(format!("unknown capability '{other}'")),
        };
        serde_json::to_string_pretty(&value).map_err(|err| err.to_string())
    }

    #[tool(
        name = "stop",
        description = "Send a non-latching zero-speed motor stop"
    )]
    pub async fn stop(&self) -> Result<Json<crate::types::DriveOutcome>, String> {
        self.harness.stop().map(Json).map_err(|err| err.to_string())
    }

    #[tool(
        name = "estop",
        description = "Latch emergency stop until estop_reset is invoked"
    )]
    pub async fn estop(&self) -> Result<String, String> {
        self.harness.estop().map_err(|err| err.to_string())?;
        Ok("estop latched".to_string())
    }

    #[tool(
        name = "capture",
        description = "Capture a deterministic frame or physical adapter capture metadata"
    )]
    pub async fn capture(&self) -> Json<crate::types::CaptureResult> {
        Json(self.harness.capture())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct InvokeCapabilityParams {
    pub capability: String,
    pub token: Option<String>,
    pub ttl_secs: Option<u64>,
    pub left: Option<f64>,
    pub right: Option<f64>,
    pub speed_mode: Option<SpeedMode>,
}

pub async fn serve_stdio(harness: Harness) -> Result<()> {
    let service = LeashMcp::new(harness).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
