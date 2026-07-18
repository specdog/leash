use anyhow::{anyhow, bail, Result};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    tool, tool_handler, tool_router,
    transport::stdio,
    Json, ServerHandler, ServiceExt,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::{
    capability::{InvocationOrigin, SafetyClass},
    module::{ModuleState, StackBlueprintMetadata},
    runtime::Harness,
    types::{PatrolStrategy, SpatialMemoryKind, SpeedMode},
};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct McpToolDescriptor {
    pub name: String,
    pub description: String,
    pub module: String,
    pub safety: SafetyClass,
    pub input_schema: Value,
    pub output_schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct McpToolList {
    pub ok: bool,
    pub tools: Vec<McpToolDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct McpCallResponse {
    pub ok: bool,
    pub tool: String,
    pub result: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct McpStatus {
    pub ok: bool,
    pub transport: String,
    pub role: String,
    pub profile: String,
    pub replay: bool,
    pub physical: bool,
    pub modules_healthy: bool,
    pub module_count: usize,
    pub tool_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct McpModuleToolMap {
    pub ok: bool,
    pub blueprint: StackBlueprintMetadata,
    pub modules: Vec<McpModuleTools>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct McpModuleTools {
    pub module: String,
    pub module_type: String,
    pub state: ModuleState,
    pub physical: bool,
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct McpProtocolTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct McpProtocolToolList {
    pub tools: Vec<McpProtocolTool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct McpTextContent {
    pub r#type: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct McpProtocolCallResult {
    pub content: Vec<McpTextContent>,
    pub structured_content: Value,
    pub is_error: bool,
}

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
        description = "Invoke a named harness capability such as authorize, drive, camera_aim, stop, estop, planner and patrol operations, saved waypoint and zone CRUD, or spatial-memory operations"
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
            .invoke_value_with_origin(&capability, args, InvocationOrigin::Mcp)
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
            .invoke_value_with_origin("stop", serde_json::json!({}), InvocationOrigin::Mcp)
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
            .invoke_value_with_origin("estop", serde_json::json!({}), InvocationOrigin::Mcp)
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
            .invoke_value_with_origin("capture", serde_json::json!({}), InvocationOrigin::Mcp)
            .map_err(|err| err.to_string())?;
        serde_json::from_value(value)
            .map(Json)
            .map_err(|err| err.to_string())
    }

    #[tool(
        name = "cognition_status",
        description = "Read Leash L0-L2 cognition health, cadence, precision, and zero-motion state"
    )]
    pub async fn cognition_status(&self) -> Json<crate::cognition::CognitionStatusV1> {
        Json(self.harness.cognition_status())
    }

    #[tool(
        name = "cognition_snapshot",
        description = "Read compact Leash L0-L2 predictive-coding layer snapshots"
    )]
    pub async fn cognition_snapshot(
        &self,
    ) -> Json<Vec<crate::cognition::CognitionLayerSnapshotV1>> {
        Json(self.harness.cognition_snapshots())
    }

    #[tool(
        name = "cognition_checkpoint",
        description = "Persist a cognition checkpoint without copying weights into telemetry"
    )]
    pub async fn cognition_checkpoint(
        &self,
    ) -> Result<Json<crate::cognition::CognitionCheckpointV1>, String> {
        self.harness
            .cognition_checkpoint()
            .map(Json)
            .map_err(|error| error.to_string())
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
    pub pan_deg: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tilt_deg: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accel: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x_m: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y_m: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tolerance_m: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<SpatialMemoryKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_confidence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_stale: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strategy: Option<PatrolStrategy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_mode: Option<SpeedMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval: Option<bool>,
}

pub async fn serve_stdio(harness: Harness) -> Result<()> {
    let service = LeashMcp::new(harness).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

pub fn tool_list() -> McpToolList {
    McpToolList {
        ok: true,
        tools: tool_descriptors(),
    }
}

pub fn tool_descriptors() -> Vec<McpToolDescriptor> {
    vec![
        tool_descriptor(
            "health",
            "Read harness health and safety state",
            "harness-runtime",
            SafetyClass::ObserveOnly,
            empty_object_schema(),
            "Health",
        ),
        tool_descriptor(
            "capabilities",
            "List harness endpoints, MCP tools, and speed modes",
            "harness-runtime",
            SafetyClass::ObserveOnly,
            empty_object_schema(),
            "Capabilities",
        ),
        tool_descriptor(
            "modules",
            "List harness modules and stream metadata",
            "harness-runtime",
            SafetyClass::ObserveOnly,
            empty_object_schema(),
            "ModuleGraph",
        ),
        tool_descriptor(
            "observe",
            "Read the latest telemetry and sensor state",
            "telemetry",
            SafetyClass::ObserveOnly,
            empty_object_schema(),
            "TelemetryFrame",
        ),
        tool_descriptor(
            "invoke_capability",
            "Invoke a named harness capability such as authorize, drive, camera_aim, stop, estop, planner and patrol operations, saved waypoint and zone CRUD, or spatial-memory operations",
            "harness-runtime",
            SafetyClass::PhysicalMotion,
            object_schema(&[
                ("capability", "string", true),
                ("token", "string", false),
                ("ttl_secs", "integer", false),
                ("left", "number", false),
                ("right", "number", false),
                ("pan_deg", "number", false),
                ("tilt_deg", "number", false),
                ("speed", "integer", false),
                ("accel", "integer", false),
                ("frame_id", "string", false),
                ("x_m", "number", false),
                ("y_m", "number", false),
                ("tolerance_m", "number", false),
                ("name", "string", false),
                ("kind", "SpatialMemoryKind", false),
                ("query", "string", false),
                ("confidence", "number", false),
                ("min_confidence", "number", false),
                ("include_stale", "boolean", false),
                ("strategy", "PatrolStrategy", false),
                ("speed_mode", "SpeedMode", false),
                ("approval", "boolean", false),
            ]),
            "json",
        ),
        tool_descriptor(
            "stop",
            "Send a non-latching zero-speed motor stop",
            "driver",
            SafetyClass::PhysicalStop,
            empty_object_schema(),
            "DriveOutcome",
        ),
        tool_descriptor(
            "estop",
            "Latch emergency stop until estop_reset is invoked",
            "driver",
            SafetyClass::PhysicalStop,
            empty_object_schema(),
            "EstopResp",
        ),
        tool_descriptor(
            "capture",
            "Capture a deterministic frame or physical adapter capture metadata",
            "telemetry",
            SafetyClass::ObserveOnly,
            empty_object_schema(),
            "CaptureResult",
        ),
        tool_descriptor(
            "cognition_status",
            "Read Leash L0-L2 cognition health, cadence, precision, and zero-motion state",
            "telemetry",
            SafetyClass::ObserveOnly,
            empty_object_schema(),
            "CognitionStatusV1",
        ),
        tool_descriptor(
            "cognition_snapshot",
            "Read compact Leash L0-L2 predictive-coding layer snapshots",
            "telemetry",
            SafetyClass::ObserveOnly,
            empty_object_schema(),
            "CognitionLayerSnapshotV1[]",
        ),
        tool_descriptor(
            "cognition_checkpoint",
            "Persist a cognition checkpoint without copying weights into telemetry",
            "telemetry",
            SafetyClass::ObserveOnly,
            empty_object_schema(),
            "CognitionCheckpointV1",
        ),
    ]
}

pub fn protocol_tool_list() -> McpProtocolToolList {
    McpProtocolToolList {
        tools: tool_descriptors()
            .into_iter()
            .map(|tool| McpProtocolTool {
                name: tool.name,
                description: tool.description,
                input_schema: tool.input_schema,
            })
            .collect(),
    }
}

pub fn status(harness: &Harness, transport: &str) -> McpStatus {
    let health = harness.health();
    let modules_healthy = health.ok;
    let module_count = health.modules.len();
    McpStatus {
        ok: true,
        transport: transport.to_string(),
        role: health.role,
        profile: health.profile,
        replay: health.replay,
        physical: harness.config().profile.is_physical(),
        modules_healthy,
        module_count,
        tool_count: tool_descriptors().len(),
    }
}

pub fn module_tool_map(harness: &Harness) -> McpModuleToolMap {
    let tools = tool_descriptors();
    let graph = harness.module_graph();
    let modules = graph
        .modules
        .into_iter()
        .map(|module| {
            let tool_names = tools
                .iter()
                .filter(|tool| module_matches_tool(&module.name, &module.module_type, &tool.module))
                .map(|tool| tool.name.clone())
                .collect();
            McpModuleTools {
                module: module.name,
                module_type: module.module_type,
                state: module.state,
                physical: module.physical,
                tools: tool_names,
            }
        })
        .collect();
    McpModuleToolMap {
        ok: true,
        blueprint: graph.blueprint,
        modules,
    }
}

pub fn protocol_call_tool(harness: &Harness, name: &str, args: Value) -> McpProtocolCallResult {
    match call_tool_value(harness, name, args) {
        Ok(result) => McpProtocolCallResult {
            content: vec![McpTextContent {
                r#type: "text".to_string(),
                text: serde_json::to_string(&result).unwrap_or_else(|_| "null".to_string()),
            }],
            structured_content: result,
            is_error: false,
        },
        Err(err) => McpProtocolCallResult {
            content: vec![McpTextContent {
                r#type: "text".to_string(),
                text: err.to_string(),
            }],
            structured_content: json!({ "error": err.to_string() }),
            is_error: true,
        },
    }
}

pub fn call_tool(harness: &Harness, name: &str, args: Value) -> Result<McpCallResponse> {
    call_tool_with_origin(harness, name, args, InvocationOrigin::Mcp)
}

pub fn call_tool_with_origin(
    harness: &Harness,
    name: &str,
    args: Value,
    origin: InvocationOrigin,
) -> Result<McpCallResponse> {
    Ok(McpCallResponse {
        ok: true,
        tool: canonical_tool_name(name).to_string(),
        result: call_tool_value_with_origin(harness, name, args, origin)?,
    })
}

pub fn call_tool_value(harness: &Harness, name: &str, args: Value) -> Result<Value> {
    call_tool_value_with_origin(harness, name, args, InvocationOrigin::Mcp)
}

pub fn call_tool_value_with_origin(
    harness: &Harness,
    name: &str,
    args: Value,
    origin: InvocationOrigin,
) -> Result<Value> {
    match canonical_tool_name(name) {
        "health" => {
            ensure_no_args(args)?;
            serde_json::to_value(harness.health()).map_err(Into::into)
        }
        "capabilities" => {
            ensure_no_args(args)?;
            serde_json::to_value(harness.capabilities()).map_err(Into::into)
        }
        "modules" => {
            ensure_no_args(args)?;
            serde_json::to_value(harness.module_graph()).map_err(Into::into)
        }
        "observe" => {
            ensure_no_args(args)?;
            serde_json::to_value(harness.telemetry()).map_err(Into::into)
        }
        "invoke_capability" => invoke_capability_value_with_origin(harness, args, origin),
        "stop" => {
            ensure_no_args(args)?;
            harness
                .capability_registry()
                .invoke_value_with_origin("stop", json!({}), origin)
        }
        "estop" => {
            ensure_no_args(args)?;
            harness
                .capability_registry()
                .invoke_value_with_origin("estop", json!({}), origin)
        }
        "capture" => {
            ensure_no_args(args)?;
            harness
                .capability_registry()
                .invoke_value_with_origin("capture", json!({}), origin)
        }
        "cognition_status" => {
            ensure_no_args(args)?;
            serde_json::to_value(harness.cognition_status()).map_err(Into::into)
        }
        "cognition_snapshot" => {
            ensure_no_args(args)?;
            serde_json::to_value(harness.cognition_snapshots()).map_err(Into::into)
        }
        "cognition_checkpoint" => {
            ensure_no_args(args)?;
            serde_json::to_value(harness.cognition_checkpoint()?).map_err(Into::into)
        }
        other => Err(anyhow!("unknown MCP tool '{other}'")),
    }
}

fn invoke_capability_value_with_origin(
    harness: &Harness,
    args: Value,
    origin: InvocationOrigin,
) -> Result<Value> {
    let mut args = args_object(args)?;
    let capability = args
        .remove("capability")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .ok_or_else(|| anyhow!("capability is required"))?;
    harness
        .capability_registry()
        .invoke_value_with_origin(&capability, Value::Object(args), origin)
}

fn ensure_no_args(args: Value) -> Result<()> {
    let args = args_object(args)?;
    if args.is_empty() {
        return Ok(());
    }
    let unexpected = args.keys().next().expect("non-empty args map");
    bail!("unexpected argument '{unexpected}'")
}

fn args_object(args: Value) -> Result<Map<String, Value>> {
    match args {
        Value::Null => Ok(Map::new()),
        Value::Object(map) => Ok(map),
        _ => bail!("MCP tool args must be a JSON object"),
    }
}

fn tool_descriptor(
    name: &str,
    description: &str,
    module: &str,
    safety: SafetyClass,
    input_schema: Value,
    output_type: &str,
) -> McpToolDescriptor {
    McpToolDescriptor {
        name: name.to_string(),
        description: description.to_string(),
        module: module.to_string(),
        safety,
        input_schema,
        output_schema: json!({ "type": output_type }),
    }
}

fn empty_object_schema() -> Value {
    object_schema(&[])
}

fn object_schema(fields: &[(&str, &str, bool)]) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for (name, field_type, is_required) in fields {
        properties.insert((*name).to_string(), json!({ "type": field_type }));
        if *is_required {
            required.push((*name).to_string());
        }
    }
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": properties,
        "required": required,
    })
}

fn canonical_tool_name(name: &str) -> &str {
    name
}

fn module_matches_tool(module_name: &str, module_type: &str, tool_module: &str) -> bool {
    module_name == tool_module || module_type == tool_module
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        types::{CaptureResult, DriveOutcome, Health, TelemetryFrame},
        HarnessConfig,
    };

    #[test]
    fn tool_descriptors_are_unique() {
        let tools = tool_descriptors();
        let names = tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(tools.len(), names.len());
        assert!(names.contains("health"));
        assert!(names.contains("modules"));
        assert!(names.contains("stop"));
    }

    #[test]
    fn protocol_tool_list_uses_standard_input_schema_shape() {
        let value = serde_json::to_value(protocol_tool_list()).unwrap();
        let tools = value["tools"].as_array().unwrap();
        let health = tools.iter().find(|tool| tool["name"] == "health").unwrap();
        assert_eq!(health["inputSchema"]["type"], "object");
        assert!(health.get("input_schema").is_none());
    }

    #[tokio::test]
    async fn module_tool_map_does_not_leak_session_tokens() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();
        harness
            .authorize("secret-session-token".to_string(), 30, SpeedMode::Low)
            .unwrap();

        let value = serde_json::to_string(&module_tool_map(&harness)).unwrap();
        assert!(!value.contains("secret-session-token"));
        assert!(value.contains("harness-runtime"));
        assert!(value.contains("stop"));
    }

    #[tokio::test]
    async fn call_tool_value_invokes_health_and_stop() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();
        let health: Health =
            serde_json::from_value(call_tool_value(&harness, "health", json!({})).unwrap())
                .unwrap();
        assert!(health.ok);

        let stop: DriveOutcome =
            serde_json::from_value(call_tool_value(&harness, "stop", json!({})).unwrap()).unwrap();
        assert!(stop.ok);
    }

    #[tokio::test]
    async fn protocol_tool_call_returns_text_and_structured_content() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();
        let result = protocol_call_tool(&harness, "health", json!({}));

        assert!(!result.is_error);
        assert_eq!(result.content[0].r#type, "text");
        assert_eq!(result.structured_content["ok"], true);
    }

    #[tokio::test]
    async fn invoke_capability_rejects_missing_capability() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();
        let err = call_tool_value(&harness, "invoke_capability", json!({}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("capability is required"));
    }

    #[tokio::test]
    async fn invoke_capability_policy_uses_call_origin() {
        let harness = Harness::new(HarnessConfig {
            allow_untokened_drive: false,
            ..HarnessConfig::default()
        })
        .unwrap();

        let err = call_tool_value_with_origin(
            &harness,
            "invoke_capability",
            json!({ "capability": "drive", "left": 0.2, "right": 0.2 }),
            InvocationOrigin::Cli,
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("require-token"));
    }

    #[tokio::test]
    async fn typed_outputs_stay_deserializable() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();
        let telemetry: TelemetryFrame =
            serde_json::from_value(call_tool_value(&harness, "observe", json!({})).unwrap())
                .unwrap();
        assert_eq!(telemetry.vision.status, "ok");
        assert_eq!(telemetry.vision.detections[0].label, "sim-fixture");
        let _: CaptureResult =
            serde_json::from_value(call_tool_value(&harness, "capture", json!({})).unwrap())
                .unwrap();
    }
}
