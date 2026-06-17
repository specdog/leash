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
        name = "modules",
        description = "List harness modules and stream metadata"
    )]
    pub async fn modules(&self) -> Json<crate::module::ModuleGraph> {
        Json(self.harness.module_graph())
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
        let mut args = serde_json::to_value(&params.0).map_err(|err| err.to_string())?;
        let capability = args
            .get("capability")
            .and_then(|value| value.as_str())
            .ok_or("capability is required")?
            .to_string();
        if let Some(object) = args.as_object_mut() {
            object.remove("capability");
        }
        let value = self
            .harness
            .capability_registry()
            .invoke_value(&capability, args)
            .map_err(|err| err.to_string())?;
        serde_json::to_string_pretty(&value).map_err(|err| err.to_string())
    }

    #[tool(
        name = "stop",
        description = "Send a non-latching zero-speed motor stop"
    )]
    pub async fn stop(&self) -> Result<Json<crate::types::DriveOutcome>, String> {
        let value = self
            .harness
            .capability_registry()
            .invoke_value("stop", serde_json::json!({}))
            .map_err(|err| err.to_string())?;
        serde_json::from_value(value)
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(
        name = "estop",
        description = "Latch emergency stop until estop_reset is invoked"
    )]
    pub async fn estop(&self) -> Result<String, String> {
        self.harness
            .capability_registry()
            .invoke_value("estop", serde_json::json!({}))
            .map_err(|err| err.to_string())?;
        Ok("estop latched".to_string())
    }

    #[tool(
        name = "capture",
        description = "Capture a deterministic frame or physical adapter capture metadata"
    )]
    pub async fn capture(&self) -> Result<Json<crate::types::CaptureResult>, String> {
        let value = self
            .harness
            .capability_registry()
            .invoke_value("capture", serde_json::json!({}))
            .map_err(|err| err.to_string())?;
        serde_json::from_value(value)
            .map(Json)
            .map_err(|err| err.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct InvokeCapabilityParams {
    pub capability: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_mode: Option<SpeedMode>,
}

pub async fn serve_stdio(harness: Harness) -> Result<()> {
    let service = LeashMcp::new(harness).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
