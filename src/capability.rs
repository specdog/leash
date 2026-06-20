use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::{
    config::PolicyMode,
    runtime::Harness,
    types::{PatrolStrategy, PlannerGoal, SpeedMode},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum SafetyClass {
    ObserveOnly,
    SimControl,
    PhysicalStop,
    PhysicalMotion,
    PhysicalHighRisk,
}

impl SafetyClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ObserveOnly => "observe-only",
            Self::SimControl => "sim-control",
            Self::PhysicalStop => "physical-stop",
            Self::PhysicalMotion => "physical-motion",
            Self::PhysicalHighRisk => "physical-high-risk",
        }
    }

    fn is_physical_action(self) -> bool {
        matches!(
            self,
            Self::PhysicalStop | Self::PhysicalMotion | Self::PhysicalHighRisk
        )
    }

    fn is_policy_gated(self) -> bool {
        matches!(self, Self::PhysicalMotion | Self::PhysicalHighRisk)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum InvocationOrigin {
    Runtime,
    Cli,
    Http,
    Mcp,
    Agent,
}

impl InvocationOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Cli => "cli",
            Self::Http => "http",
            Self::Mcp => "mcp",
            Self::Agent => "agent",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvocationContext {
    pub origin: InvocationOrigin,
}

impl InvocationContext {
    pub fn new(origin: InvocationOrigin) -> Self {
        Self { origin }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PolicyDecision {
    Execute,
    DryRun,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct CapabilityDescriptor {
    pub name: String,
    pub description: String,
    pub module: String,
    pub safety: SafetyClass,
    pub input_schema: Value,
    pub output_schema: Value,
}

#[derive(Clone)]
pub struct CapabilityRegistry {
    harness: Harness,
    descriptors: Vec<CapabilityDescriptor>,
}

impl CapabilityRegistry {
    pub fn new(harness: Harness) -> Self {
        Self {
            harness,
            descriptors: default_capability_descriptors(),
        }
    }

    pub fn descriptors(&self) -> &[CapabilityDescriptor] {
        &self.descriptors
    }

    pub fn names(&self) -> Vec<String> {
        self.descriptors
            .iter()
            .map(|descriptor| descriptor.name.clone())
            .collect()
    }

    pub fn invoke_value(&self, name: &str, args: Value) -> Result<Value> {
        self.invoke_value_with_context(
            name,
            args,
            InvocationContext::new(InvocationOrigin::Runtime),
        )
    }

    pub fn invoke_value_with_origin(
        &self,
        name: &str,
        args: Value,
        origin: InvocationOrigin,
    ) -> Result<Value> {
        self.invoke_value_with_context(name, args, InvocationContext::new(origin))
    }

    pub fn invoke_value_with_context(
        &self,
        name: &str,
        args: Value,
        context: InvocationContext,
    ) -> Result<Value> {
        let name = canonical_name(name);
        let descriptor = self
            .descriptors
            .iter()
            .find(|descriptor| descriptor.name == name)
            .ok_or_else(|| anyhow!("unknown capability '{name}'"))?;
        let safety = descriptor.safety;
        let mut args = args_object(args)?;
        let approval = optional_bool_removed(&mut args, "approval")?.unwrap_or(false);
        let decision = self.policy_decision(name, safety, &args, approval, context)?;

        if decision == PolicyDecision::DryRun {
            self.log_policy_approved(name, safety, context, true);
            return Ok(json!({
                "ok": true,
                "dry_run": true,
                "capability": name,
                "safety": safety.as_str(),
                "origin": context.origin.as_str(),
                "policy_mode": self.harness.config().policy_mode.as_str(),
            }));
        }

        let result = self.invoke_unchecked(name, args);
        if safety.is_physical_action() {
            match &result {
                Ok(_) => self.log_policy_approved(name, safety, context, false),
                Err(err) => self.log_policy_denied(name, safety, context, &err.to_string()),
            }
        }
        result
    }

    fn invoke_unchecked(&self, name: &str, args: Map<String, Value>) -> Result<Value> {
        match name {
            "health" => {
                ensure_fields(&args, &[])?;
                serde_json::to_value(self.harness.health()).map_err(Into::into)
            }
            "capabilities" => {
                ensure_fields(&args, &[])?;
                serde_json::to_value(self.harness.capabilities()).map_err(Into::into)
            }
            "observe" => {
                ensure_fields(&args, &[])?;
                serde_json::to_value(self.harness.telemetry()).map_err(Into::into)
            }
            "capture" => {
                ensure_fields(&args, &[])?;
                serde_json::to_value(self.harness.capture()).map_err(Into::into)
            }
            "authorize" => {
                ensure_fields(&args, &["token", "ttl_secs", "speed_mode"])?;
                let token = required_string(&args, "token")?;
                let ttl_secs = optional_u64(&args, "ttl_secs")?.unwrap_or(120);
                let speed_mode = optional_speed_mode(&args, "speed_mode")?.unwrap_or_default();
                self.harness.authorize(token, ttl_secs, speed_mode)?;
                Ok(json!({ "ok": true, "ttl_secs": ttl_secs, "speed_mode": speed_mode }))
            }
            "drive" => {
                ensure_fields(&args, &["token", "left", "right", "speed_mode"])?;
                let left = required_f64(&args, "left")?;
                let right = required_f64(&args, "right")?;
                let token = optional_string(&args, "token")?;
                let speed_mode = optional_speed_mode(&args, "speed_mode")?;
                serde_json::to_value(self.harness.drive(
                    token.as_deref(),
                    left,
                    right,
                    speed_mode,
                )?)
                .map_err(Into::into)
            }
            "speed_mode" => {
                ensure_fields(&args, &["token", "speed_mode"])?;
                let token = optional_string(&args, "token")?;
                let speed_mode = required_speed_mode(&args, "speed_mode")?;
                self.harness.set_speed_mode(token.as_deref(), speed_mode)?;
                Ok(json!({ "ok": true, "speed_mode": speed_mode }))
            }
            "stop" => {
                ensure_fields(&args, &[])?;
                serde_json::to_value(self.harness.stop()?).map_err(Into::into)
            }
            "estop" => {
                ensure_fields(&args, &[])?;
                self.harness.estop()?;
                Ok(json!({ "ok": true, "estop": true }))
            }
            "estop_reset" => {
                ensure_fields(&args, &["token"])?;
                let token = optional_string(&args, "token")?;
                self.harness.reset_estop(token.as_deref())?;
                Ok(json!({ "ok": true, "estop": false }))
            }
            "planner_set_goal" => {
                ensure_fields(
                    &args,
                    &["frame_id", "x_m", "y_m", "tolerance_m", "speed_mode"],
                )?;
                let goal = PlannerGoal {
                    frame_id: optional_string(&args, "frame_id")?
                        .unwrap_or_else(|| "map".to_string()),
                    x_m: required_f64(&args, "x_m")?,
                    y_m: required_f64(&args, "y_m")?,
                    tolerance_m: optional_f64(&args, "tolerance_m")?.unwrap_or(0.1),
                    speed_mode: optional_speed_mode(&args, "speed_mode")?.unwrap_or(SpeedMode::Low),
                };
                serde_json::to_value(self.harness.set_planner_goal(goal)?).map_err(Into::into)
            }
            "planner_cancel" => {
                ensure_fields(&args, &[])?;
                serde_json::to_value(self.harness.cancel_planner_goal()?).map_err(Into::into)
            }
            "planner_status" => {
                ensure_fields(&args, &[])?;
                serde_json::to_value(self.harness.planner_status()).map_err(Into::into)
            }
            "start_patrol" => {
                ensure_fields(&args, &["strategy", "speed_mode"])?;
                let strategy = optional_patrol_strategy(&args, "strategy")?.unwrap_or_default();
                let speed_mode =
                    optional_speed_mode(&args, "speed_mode")?.unwrap_or(SpeedMode::Low);
                serde_json::to_value(self.harness.start_patrol(strategy, speed_mode)?)
                    .map_err(Into::into)
            }
            "stop_patrol" => {
                ensure_fields(&args, &[])?;
                serde_json::to_value(self.harness.stop_patrol()?).map_err(Into::into)
            }
            "patrol_status" => {
                ensure_fields(&args, &[])?;
                serde_json::to_value(self.harness.patrol_status()).map_err(Into::into)
            }
            other => Err(anyhow!("unknown capability '{other}'")),
        }
    }

    fn policy_decision(
        &self,
        name: &str,
        safety: SafetyClass,
        args: &Map<String, Value>,
        approval: bool,
        context: InvocationContext,
    ) -> Result<PolicyDecision> {
        if !safety.is_policy_gated() {
            return Ok(PolicyDecision::Execute);
        }

        if self.harness.config().profile.is_physical() && !self.harness.physical_actuation_enabled()
        {
            self.log_policy_denied(
                name,
                safety,
                context,
                "physical action requires LEASH_ALLOW_PHYSICAL_ACTUATION=1 or --allow-physical-actuation",
            );
            bail!("physical action requires explicit physical actuation gate");
        }

        if context.origin == InvocationOrigin::Agent && !approval {
            self.log_policy_denied(
                name,
                safety,
                context,
                "agent physical action requires approval=true",
            );
            bail!("agent physical action requires approval=true");
        }

        match self.harness.config().policy_mode {
            PolicyMode::DryRun => Ok(PolicyDecision::DryRun),
            PolicyMode::RequireToken => {
                if token_satisfied(args) {
                    Ok(PolicyDecision::Execute)
                } else {
                    self.log_policy_denied(
                        name,
                        safety,
                        context,
                        "policy require-token requires token for physical action",
                    );
                    bail!("policy require-token requires token for physical action");
                }
            }
            PolicyMode::RequireApproval => {
                if approval {
                    Ok(PolicyDecision::Execute)
                } else {
                    self.log_policy_denied(
                        name,
                        safety,
                        context,
                        "policy require-approval requires approval=true for physical action",
                    );
                    bail!("policy require-approval requires approval=true for physical action");
                }
            }
            PolicyMode::Deny => {
                self.log_policy_denied(name, safety, context, "policy deny blocks physical action");
                bail!("policy deny blocks physical action");
            }
        }
    }

    fn log_policy_approved(
        &self,
        name: &str,
        safety: SafetyClass,
        context: InvocationContext,
        dry_run: bool,
    ) {
        if !safety.is_physical_action() {
            return;
        }
        tracing::info!(
            capability = name,
            safety = safety.as_str(),
            origin = context.origin.as_str(),
            policy_mode = self.harness.config().policy_mode.as_str(),
            dry_run,
            "capability policy approved"
        );
    }

    fn log_policy_denied(
        &self,
        name: &str,
        safety: SafetyClass,
        context: InvocationContext,
        reason: &str,
    ) {
        if !safety.is_physical_action() {
            return;
        }
        tracing::warn!(
            capability = name,
            safety = safety.as_str(),
            origin = context.origin.as_str(),
            policy_mode = self.harness.config().policy_mode.as_str(),
            reason,
            "capability policy denied"
        );
    }
}

pub fn default_capability_descriptors() -> Vec<CapabilityDescriptor> {
    vec![
        descriptor(
            "health",
            "Read harness health and safety state",
            SafetyClass::ObserveOnly,
            object_schema(&[]),
            "Health",
        ),
        descriptor(
            "capabilities",
            "List endpoints, modules, tools, and capability metadata",
            SafetyClass::ObserveOnly,
            object_schema(&[]),
            "Capabilities",
        ),
        descriptor(
            "observe",
            "Read the latest telemetry and sensor state",
            SafetyClass::ObserveOnly,
            object_schema(&[]),
            "TelemetryFrame",
        ),
        descriptor(
            "capture",
            "Capture deterministic frame metadata",
            SafetyClass::ObserveOnly,
            object_schema(&[]),
            "CaptureResult",
        ),
        descriptor(
            "authorize",
            "Create or refresh a pilot session token",
            SafetyClass::SimControl,
            object_schema(&[
                ("token", "string", true),
                ("ttl_secs", "integer", false),
                ("speed_mode", "SpeedMode", false),
            ]),
            "PilotTokenResp",
        ),
        descriptor(
            "drive",
            "Apply left and right drive commands through safety gates",
            SafetyClass::PhysicalMotion,
            object_schema(&[
                ("token", "string", false),
                ("left", "number", true),
                ("right", "number", true),
                ("speed_mode", "SpeedMode", false),
                ("approval", "boolean", false),
            ]),
            "DriveOutcome",
        ),
        descriptor(
            "speed_mode",
            "Change the active speed cap",
            SafetyClass::SimControl,
            object_schema(&[
                ("token", "string", false),
                ("speed_mode", "SpeedMode", true),
            ]),
            "SpeedModeResp",
        ),
        descriptor(
            "stop",
            "Send a non-latching zero-speed motor stop",
            SafetyClass::PhysicalStop,
            object_schema(&[]),
            "DriveOutcome",
        ),
        descriptor(
            "estop",
            "Latch emergency stop until reset",
            SafetyClass::PhysicalStop,
            object_schema(&[]),
            "EstopResp",
        ),
        descriptor(
            "estop_reset",
            "Reset a latched emergency stop",
            SafetyClass::PhysicalHighRisk,
            object_schema(&[("token", "string", false), ("approval", "boolean", false)]),
            "EstopResp",
        ),
        descriptor(
            "planner_set_goal",
            "Set a sim-only waypoint goal and begin local planner drive commands",
            SafetyClass::SimControl,
            object_schema(&[
                ("frame_id", "string", false),
                ("x_m", "number", true),
                ("y_m", "number", true),
                ("tolerance_m", "number", false),
                ("speed_mode", "SpeedMode", false),
            ]),
            "PlannerStatus",
        ),
        descriptor(
            "planner_cancel",
            "Cancel the active sim planner goal and stop movement",
            SafetyClass::SimControl,
            object_schema(&[]),
            "PlannerStatus",
        ),
        descriptor(
            "planner_status",
            "Report the current sim planner goal, path, and last drive command",
            SafetyClass::ObserveOnly,
            object_schema(&[]),
            "PlannerStatus",
        ),
        descriptor(
            "start_patrol",
            "Start a sim-only patrol using coverage, frontier, or random goal selection",
            SafetyClass::SimControl,
            object_schema(&[
                ("strategy", "PatrolStrategy", false),
                ("speed_mode", "SpeedMode", false),
            ]),
            "PatrolStatus",
        ),
        descriptor(
            "stop_patrol",
            "Stop the active sim patrol and cancel planner movement",
            SafetyClass::SimControl,
            object_schema(&[]),
            "PatrolStatus",
        ),
        descriptor(
            "patrol_status",
            "Report active sim patrol strategy, goal, path, and visited cells",
            SafetyClass::ObserveOnly,
            object_schema(&[]),
            "PatrolStatus",
        ),
    ]
}

fn descriptor(
    name: &str,
    description: &str,
    safety: SafetyClass,
    input_schema: Value,
    output_type: &str,
) -> CapabilityDescriptor {
    CapabilityDescriptor {
        name: name.to_string(),
        description: description.to_string(),
        module: "harness-runtime".to_string(),
        safety,
        input_schema,
        output_schema: json!({ "type": output_type }),
    }
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

fn canonical_name(name: &str) -> &str {
    match name {
        "motors.stop" | "motors/stop" => "stop",
        "estop/reset" | "estop.reset" => "estop_reset",
        "planner.set_goal" | "planner/set_goal" => "planner_set_goal",
        "planner.cancel" | "planner/cancel" => "planner_cancel",
        "planner.status" | "planner/status" => "planner_status",
        "patrol.start" | "patrol/start" | "patrol_start" => "start_patrol",
        "patrol.stop" | "patrol/stop" | "patrol_stop" => "stop_patrol",
        "patrol.status" | "patrol/status" => "patrol_status",
        other => other,
    }
}

fn args_object(args: Value) -> Result<Map<String, Value>> {
    match args {
        Value::Null => Ok(Map::new()),
        Value::Object(map) => Ok(map),
        _ => bail!("capability args must be a JSON object"),
    }
}

fn ensure_fields(args: &Map<String, Value>, allowed: &[&str]) -> Result<()> {
    for key in args.keys() {
        if !allowed.contains(&key.as_str()) {
            bail!("unexpected argument '{key}'");
        }
    }
    Ok(())
}

fn required_string(args: &Map<String, Value>, key: &str) -> Result<String> {
    optional_string(args, key)?.ok_or_else(|| anyhow!("{key} is required"))
}

fn optional_string(args: &Map<String, Value>, key: &str) -> Result<Option<String>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => bail!("{key} must be a string"),
    }
}

fn optional_bool_removed(args: &mut Map<String, Value>, key: &str) -> Result<Option<bool>> {
    match args.remove(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(value)),
        Some(_) => bail!("{key} must be a boolean"),
    }
}

fn token_satisfied(args: &Map<String, Value>) -> bool {
    args.get("token")
        .and_then(Value::as_str)
        .is_some_and(|token| !token.trim().is_empty())
}

fn optional_u64(args: &Map<String, Value>, key: &str) -> Result<Option<u64>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(value)) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| anyhow!("{key} must be an unsigned integer")),
        Some(_) => bail!("{key} must be an unsigned integer"),
    }
}

fn required_f64(args: &Map<String, Value>, key: &str) -> Result<f64> {
    match args.get(key) {
        Some(Value::Number(value)) => value
            .as_f64()
            .ok_or_else(|| anyhow!("{key} must be a finite number")),
        Some(_) => bail!("{key} must be a number"),
        None => bail!("{key} is required"),
    }
}

fn optional_f64(args: &Map<String, Value>, key: &str) -> Result<Option<f64>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(value)) => value
            .as_f64()
            .map(Some)
            .ok_or_else(|| anyhow!("{key} must be a finite number")),
        Some(_) => bail!("{key} must be a number"),
    }
}

fn required_speed_mode(args: &Map<String, Value>, key: &str) -> Result<SpeedMode> {
    optional_speed_mode(args, key)?.ok_or_else(|| anyhow!("{key} is required"))
}

fn optional_speed_mode(args: &Map<String, Value>, key: &str) -> Result<Option<SpeedMode>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|err| anyhow!("{key} must be a valid speed mode: {err}")),
    }
}

fn optional_patrol_strategy(
    args: &Map<String, Value>,
    key: &str,
) -> Result<Option<PatrolStrategy>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|err| anyhow!("{key} must be a valid patrol strategy: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::PolicyMode, types::TelemetryFrame, HarnessConfig};

    #[tokio::test]
    async fn descriptors_are_unique() {
        let registry = CapabilityRegistry::new(Harness::new(HarnessConfig::default()).unwrap());
        let names = registry.names();
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(names.len(), unique.len());
    }

    #[tokio::test]
    async fn descriptors_cover_all_safety_classes() {
        let registry = CapabilityRegistry::new(Harness::new(HarnessConfig::default()).unwrap());
        let classes = registry
            .descriptors()
            .iter()
            .map(|descriptor| descriptor.safety)
            .collect::<std::collections::HashSet<_>>();

        for safety in [
            SafetyClass::ObserveOnly,
            SafetyClass::SimControl,
            SafetyClass::PhysicalStop,
            SafetyClass::PhysicalMotion,
            SafetyClass::PhysicalHighRisk,
        ] {
            assert!(classes.contains(&safety), "missing {safety:?}");
        }
    }

    #[tokio::test]
    async fn rejects_invalid_drive_args_before_command_changes() {
        let harness = Harness::new(HarnessConfig {
            policy_mode: PolicyMode::RequireApproval,
            ..HarnessConfig::default()
        })
        .unwrap();
        let registry = CapabilityRegistry::new(harness.clone());
        let err = registry
            .invoke_value("drive", json!({ "left": 0.2, "approval": true }))
            .unwrap_err()
            .to_string();
        assert!(err.contains("right is required"));

        let telemetry = harness.telemetry();
        assert_eq!(telemetry.left_cmd, 0.0);
        assert_eq!(telemetry.right_cmd, 0.0);
    }

    #[tokio::test]
    async fn observe_returns_telemetry_value() {
        let registry = CapabilityRegistry::new(Harness::new(HarnessConfig::default()).unwrap());
        let value = registry.invoke_value("observe", json!({})).unwrap();
        let telemetry: TelemetryFrame = serde_json::from_value(value).unwrap();
        assert_eq!(telemetry.robot, "robot");
    }

    #[tokio::test]
    async fn patrol_capabilities_start_stop_and_report_status() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();
        let registry = CapabilityRegistry::new(harness.clone());

        let started = registry
            .invoke_value_with_origin(
                "start_patrol",
                json!({ "strategy": "frontier", "speed_mode": "low" }),
                InvocationOrigin::Agent,
            )
            .unwrap();

        assert_eq!(started["ok"], true);
        assert_eq!(started["active"], true);
        assert_eq!(started["strategy"], "frontier");
        assert!(harness.planner_status().active);

        let status = registry.invoke_value("patrol_status", json!({})).unwrap();
        assert_eq!(status["active"], true);
        assert_eq!(status["visited_cells"][0], "2,2");

        let stopped = registry.invoke_value("stop_patrol", json!({})).unwrap();
        assert_eq!(stopped["active"], false);
        assert_eq!(stopped["status"], "stopped");
        assert_eq!(harness.telemetry().left_cmd, 0.0);
    }

    #[tokio::test]
    async fn require_token_policy_blocks_physical_motion_without_token() {
        let harness = Harness::new(HarnessConfig {
            allow_untokened_drive: false,
            ..HarnessConfig::default()
        })
        .unwrap();
        let registry = CapabilityRegistry::new(harness.clone());

        let err = registry
            .invoke_value_with_origin(
                "drive",
                json!({ "left": 0.2, "right": 0.2 }),
                InvocationOrigin::Http,
            )
            .unwrap_err()
            .to_string();

        assert!(err.contains("require-token"));
        assert_eq!(harness.telemetry().left_cmd, 0.0);
    }

    #[tokio::test]
    async fn require_approval_policy_blocks_physical_motion_without_approval() {
        let harness = Harness::new(HarnessConfig {
            policy_mode: PolicyMode::RequireApproval,
            ..HarnessConfig::default()
        })
        .unwrap();
        let registry = CapabilityRegistry::new(harness.clone());

        let err = registry
            .invoke_value_with_origin(
                "drive",
                json!({ "left": 0.2, "right": 0.2 }),
                InvocationOrigin::Cli,
            )
            .unwrap_err()
            .to_string();

        assert!(err.contains("approval=true"));
        assert_eq!(harness.telemetry().left_cmd, 0.0);
    }

    #[tokio::test]
    async fn dry_run_policy_approves_without_command_change() {
        let harness = Harness::new(HarnessConfig {
            policy_mode: PolicyMode::DryRun,
            ..HarnessConfig::default()
        })
        .unwrap();
        let registry = CapabilityRegistry::new(harness.clone());

        let value = registry
            .invoke_value_with_origin(
                "drive",
                json!({ "left": 0.2, "right": 0.2 }),
                InvocationOrigin::Cli,
            )
            .unwrap();

        assert_eq!(value["dry_run"], true);
        assert_eq!(value["safety"], "physical-motion");
        assert_eq!(harness.telemetry().left_cmd, 0.0);
    }

    #[tokio::test]
    async fn deny_policy_still_allows_stop_and_estop() {
        let harness = Harness::new(HarnessConfig {
            policy_mode: PolicyMode::Deny,
            ..HarnessConfig::default()
        })
        .unwrap();
        let registry = CapabilityRegistry::new(harness);

        let drive_err = registry
            .invoke_value_with_origin(
                "drive",
                json!({ "left": 0.2, "right": 0.2 }),
                InvocationOrigin::Mcp,
            )
            .unwrap_err()
            .to_string();
        assert!(drive_err.contains("policy deny"));

        let stop = registry
            .invoke_value_with_origin("stop", json!({}), InvocationOrigin::Mcp)
            .unwrap();
        assert_eq!(stop["ok"], true);

        let estop = registry
            .invoke_value_with_origin("estop", json!({}), InvocationOrigin::Mcp)
            .unwrap();
        assert_eq!(estop["estop"], true);
    }

    #[tokio::test]
    async fn agent_physical_motion_requires_approval() {
        let harness = Harness::new(HarnessConfig {
            policy_mode: PolicyMode::RequireApproval,
            ..HarnessConfig::default()
        })
        .unwrap();
        let registry = CapabilityRegistry::new(harness.clone());

        let err = registry
            .invoke_value_with_origin(
                "drive",
                json!({ "left": 0.2, "right": 0.2 }),
                InvocationOrigin::Agent,
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("agent physical action"));

        registry
            .invoke_value_with_origin(
                "drive",
                json!({ "left": 0.2, "right": 0.2, "approval": true }),
                InvocationOrigin::Agent,
            )
            .unwrap();
        assert!(harness.telemetry().left_cmd > 0.0);
    }
}
