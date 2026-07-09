use std::time::Duration;

use anyhow::{bail, Result};
use reqwest::Client;
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    tool, tool_handler, tool_router,
    transport::stdio,
    ServerHandler, ServiceExt,
};
use serde_json::{json, Value};

use crate::mcp::InvokeCapabilityParams;

#[derive(Clone)]
pub struct LeashMcpBridge {
    base_url: String,
    client: Client,
    tool_router: ToolRouter<Self>,
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for LeashMcpBridge {}

#[tool_router(router = tool_router)]
impl LeashMcpBridge {
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let base_url = normalize_base_url(base_url.into())?;
        let client = Client::builder().timeout(Duration::from_secs(10)).build()?;
        Ok(Self {
            base_url,
            client,
            tool_router: Self::tool_router(),
        })
    }

    #[tool(name = "health", description = "Read harness health and safety state")]
    pub async fn health(&self) -> Result<String, String> {
        self.call_remote("health", json!({})).await
    }

    #[tool(
        name = "capabilities",
        description = "List harness endpoints, MCP tools, and speed modes"
    )]
    pub async fn capabilities(&self) -> Result<String, String> {
        self.call_remote("capabilities", json!({})).await
    }

    #[tool(
        name = "modules",
        description = "List harness modules and stream metadata"
    )]
    pub async fn modules(&self) -> Result<String, String> {
        self.call_remote("modules", json!({})).await
    }

    #[tool(
        name = "observe",
        description = "Read the latest telemetry and sensor state"
    )]
    pub async fn observe(&self) -> Result<String, String> {
        self.call_remote("observe", json!({})).await
    }

    #[tool(
        name = "invoke_capability",
        description = "Invoke a named harness capability such as authorize, drive, camera_aim, stop, estop, estop_reset, speed_mode, planner_set_goal, planner_cancel, planner_status, start_patrol, stop_patrol, patrol_status, memory_tag_location, memory_list, memory_query, or memory_clear"
    )]
    pub async fn invoke_capability(
        &self,
        params: Parameters<InvokeCapabilityParams>,
    ) -> Result<String, String> {
        let args = serde_json::to_value(&params.0).map_err(|err| err.to_string())?;
        self.call_remote("invoke_capability", args).await
    }

    #[tool(
        name = "stop",
        description = "Send a non-latching zero-speed motor stop"
    )]
    pub async fn stop(&self) -> Result<String, String> {
        self.call_remote("stop", json!({})).await
    }

    #[tool(
        name = "estop",
        description = "Latch emergency stop until estop_reset is invoked"
    )]
    pub async fn estop(&self) -> Result<String, String> {
        self.call_remote("estop", json!({})).await
    }

    #[tool(
        name = "capture",
        description = "Capture a deterministic frame or physical adapter capture metadata"
    )]
    pub async fn capture(&self) -> Result<String, String> {
        self.call_remote("capture", json!({})).await
    }
}

impl LeashMcpBridge {
    async fn call_remote(&self, tool: &str, args: Value) -> Result<String, String> {
        let response = self
            .client
            .post(mcp_endpoint(&self.base_url, "call"))
            .json(&json!({ "tool": tool, "args": args }))
            .send()
            .await
            .map_err(|err| format!("failed to call remote MCP tool '{tool}': {err}"))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|err| format!("failed to read remote MCP response for '{tool}': {err}"))?;
        if !status.is_success() {
            return Err(format!(
                "remote MCP tool '{tool}' returned HTTP {status}: {}",
                truncate_error(&text)
            ));
        }
        Ok(text)
    }
}

pub async fn serve_stdio(base_url: impl Into<String>) -> Result<()> {
    let service = LeashMcpBridge::new(base_url)?.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

pub fn mcp_endpoint(base_url: &str, path: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    if base.ends_with("/mcp") {
        format!("{base}/{path}")
    } else {
        format!("{base}/mcp/{path}")
    }
}

fn normalize_base_url(base_url: String) -> Result<String> {
    let base_url = base_url.trim().trim_end_matches('/').to_string();
    if base_url.is_empty() {
        bail!("bridge URL cannot be empty");
    }
    Ok(base_url)
}

fn truncate_error(text: &str) -> String {
    let mut truncated: String = text.chars().take(400).collect();
    if text.chars().count() > 400 {
        truncated.push_str("...");
    }
    truncated
}

#[cfg(test)]
mod tests {
    use super::{mcp_endpoint, normalize_base_url};

    #[test]
    fn endpoint_adds_mcp_prefix() {
        assert_eq!(
            mcp_endpoint("http://127.0.0.1:9990", "call"),
            "http://127.0.0.1:9990/mcp/call"
        );
    }

    #[test]
    fn endpoint_uses_existing_mcp_base() {
        assert_eq!(
            mcp_endpoint("http://127.0.0.1:9990/mcp/", "/call"),
            "http://127.0.0.1:9990/mcp/call"
        );
    }

    #[test]
    fn base_url_must_not_be_empty() {
        assert!(normalize_base_url("   ".to_string()).is_err());
    }
}
