use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::{
    config::PolicyMode,
    memory::{SpatialMemoryQuery, SpatialMemoryTag},
    navigation::{PatrolZoneSpec, WaypointSpec},
    runtime::Harness,
    types::{PatrolStrategy, PlannerGoal, SpatialMemoryKind, SpeedMode, ZoneBoundaryPoint},
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
                Ok(json!({
                    "ok": true,
                    "ttl_secs": ttl_secs,
                    "speed_mode": speed_mode,
                    "operator_token": self.harness.operator_token_status(),
                }))
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
            "camera_aim" => {
                ensure_fields(&args, &["token", "pan_deg", "tilt_deg", "speed", "accel"])?;
                let token = optional_string(&args, "token")?;
                let pan_deg = required_f64(&args, "pan_deg")?;
                let tilt_deg = required_f64(&args, "tilt_deg")?;
                let speed = optional_u32(&args, "speed")?;
                let accel = optional_u32(&args, "accel")?;
                serde_json::to_value(self.harness.camera_aim(
                    token.as_deref(),
                    pan_deg,
                    tilt_deg,
                    speed,
                    accel,
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
            "waypoint_create" | "waypoint_update" => {
                ensure_fields(
                    &args,
                    &["id", "name", "frame_id", "x_m", "y_m", "tolerance_m"],
                )?;
                let spec = waypoint_spec(&args)?;
                let status = if name == "waypoint_create" {
                    self.harness.create_waypoint(spec)?
                } else {
                    self.harness.update_waypoint(spec)?
                };
                serde_json::to_value(status).map_err(Into::into)
            }
            "waypoint_list" => {
                ensure_fields(&args, &[])?;
                serde_json::to_value(self.harness.waypoints()).map_err(Into::into)
            }
            "waypoint_delete" => {
                ensure_fields(&args, &["id"])?;
                let id = required_string(&args, "id")?;
                serde_json::to_value(self.harness.delete_waypoint(&id)?).map_err(Into::into)
            }
            "patrol_zone_create" | "patrol_zone_update" => {
                ensure_fields(
                    &args,
                    &["id", "name", "frame_id", "waypoint_ids", "boundary"],
                )?;
                let spec = patrol_zone_spec(&args)?;
                let status = if name == "patrol_zone_create" {
                    self.harness.create_patrol_zone(spec)?
                } else {
                    self.harness.update_patrol_zone(spec)?
                };
                serde_json::to_value(status).map_err(Into::into)
            }
            "patrol_zone_list" => {
                ensure_fields(&args, &[])?;
                serde_json::to_value(self.harness.patrol_zones()).map_err(Into::into)
            }
            "patrol_zone_delete" => {
                ensure_fields(&args, &["id"])?;
                let id = required_string(&args, "id")?;
                serde_json::to_value(self.harness.delete_patrol_zone(&id)?).map_err(Into::into)
            }
            "start_patrol_zone" => {
                ensure_fields(&args, &["zone_id", "speed_mode"])?;
                let zone_id = required_string(&args, "zone_id")?;
                let speed_mode =
                    optional_speed_mode(&args, "speed_mode")?.unwrap_or(SpeedMode::Low);
                serde_json::to_value(self.harness.start_patrol_zone(&zone_id, speed_mode)?)
                    .map_err(Into::into)
            }
            "memory_tag_location" => {
                ensure_fields(
                    &args,
                    &["name", "kind", "frame_id", "x_m", "y_m", "confidence"],
                )?;
                let tag = SpatialMemoryTag {
                    name: required_string(&args, "name")?,
                    kind: optional_spatial_memory_kind(&args, "kind")?.unwrap_or_default(),
                    frame_id: optional_string(&args, "frame_id")?
                        .unwrap_or_else(|| "map".to_string()),
                    x_m: required_f64(&args, "x_m")?,
                    y_m: required_f64(&args, "y_m")?,
                    confidence: optional_f64(&args, "confidence")?.unwrap_or(1.0),
                };
                serde_json::to_value(self.harness.tag_spatial_memory(tag)?).map_err(Into::into)
            }
            "memory_list" => {
                ensure_fields(&args, &["include_stale"])?;
                let include_stale = optional_bool(&args, "include_stale")?.unwrap_or(true);
                serde_json::to_value(self.harness.query_spatial_memory(SpatialMemoryQuery {
                    include_stale,
                    ..SpatialMemoryQuery::default()
                })?)
                .map_err(Into::into)
            }
            "memory_query" => {
                ensure_fields(&args, &["query", "kind", "min_confidence", "include_stale"])?;
                let query = optional_string(&args, "query")?;
                let kind = optional_spatial_memory_kind(&args, "kind")?;
                let min_confidence = optional_f64(&args, "min_confidence")?;
                let include_stale = optional_bool(&args, "include_stale")?.unwrap_or(true);
                serde_json::to_value(self.harness.query_spatial_memory(SpatialMemoryQuery {
                    query,
                    kind,
                    min_confidence,
                    include_stale,
                })?)
                .map_err(Into::into)
            }
            "memory_clear" => {
                ensure_fields(&args, &[])?;
                serde_json::to_value(self.harness.clear_spatial_memory()?).map_err(Into::into)
            }
            #[cfg(feature = "mavlink-drone")]
            "drone_arm" => {
                ensure_fields(&args, &["token"])?;
                let token = optional_string(&args, "token")?;
                serde_json::to_value(self.harness.drone_command(
                    "arm",
                    token.as_deref(),
                    json!({}),
                )?)
                .map_err(Into::into)
            }
            #[cfg(feature = "mavlink-drone")]
            "drone_disarm" => {
                ensure_fields(&args, &["token"])?;
                let token = optional_string(&args, "token")?;
                serde_json::to_value(self.harness.drone_command(
                    "disarm",
                    token.as_deref(),
                    json!({}),
                )?)
                .map_err(Into::into)
            }
            #[cfg(feature = "mavlink-drone")]
            "drone_takeoff" => {
                ensure_fields(&args, &["token", "altitude_m"])?;
                let token = optional_string(&args, "token")?;
                let altitude_m = optional_f64(&args, "altitude_m")?.unwrap_or(2.0);
                serde_json::to_value(self.harness.drone_command(
                    "takeoff",
                    token.as_deref(),
                    json!({ "altitude_m": altitude_m }),
                )?)
                .map_err(Into::into)
            }
            #[cfg(feature = "mavlink-drone")]
            "drone_land" => {
                ensure_fields(&args, &["token"])?;
                let token = optional_string(&args, "token")?;
                serde_json::to_value(self.harness.drone_command(
                    "land",
                    token.as_deref(),
                    json!({}),
                )?)
                .map_err(Into::into)
            }
            #[cfg(feature = "mavlink-drone")]
            "drone_move_velocity" => {
                ensure_fields(
                    &args,
                    &["token", "vx_mps", "vy_mps", "vz_mps", "yaw_rate_radps"],
                )?;
                let token = optional_string(&args, "token")?;
                let vx_mps = optional_f64(&args, "vx_mps")?.unwrap_or(0.0);
                let vy_mps = optional_f64(&args, "vy_mps")?.unwrap_or(0.0);
                let vz_mps = optional_f64(&args, "vz_mps")?.unwrap_or(0.0);
                let yaw_rate_radps = optional_f64(&args, "yaw_rate_radps")?.unwrap_or(0.0);
                serde_json::to_value(self.harness.drone_command(
                    "move_velocity",
                    token.as_deref(),
                    json!({
                        "vx_mps": vx_mps,
                        "vy_mps": vy_mps,
                        "vz_mps": vz_mps,
                        "yaw_rate_radps": yaw_rate_radps
                    }),
                )?)
                .map_err(Into::into)
            }
            #[cfg(feature = "mavlink-drone")]
            "drone_fly_to" => {
                ensure_fields(&args, &["token", "lat_deg", "lon_deg", "altitude_m"])?;
                let token = optional_string(&args, "token")?;
                let lat_deg = required_f64(&args, "lat_deg")?;
                let lon_deg = required_f64(&args, "lon_deg")?;
                let altitude_m = required_f64(&args, "altitude_m")?;
                serde_json::to_value(self.harness.drone_command(
                    "fly_to",
                    token.as_deref(),
                    json!({
                        "lat_deg": lat_deg,
                        "lon_deg": lon_deg,
                        "altitude_m": altitude_m
                    }),
                )?)
                .map_err(Into::into)
            }
            #[cfg(feature = "manipulator")]
            "manipulator_joint_state" => {
                ensure_fields(&args, &[])?;
                serde_json::to_value(self.harness.manipulator_joint_state()).map_err(Into::into)
            }
            #[cfg(feature = "manipulator")]
            "manipulator_joint_command" => {
                ensure_fields(&args, &["token", "joints"])?;
                let token = optional_string(&args, "token")?;
                let joints = args
                    .get("joints")
                    .cloned()
                    .ok_or_else(|| anyhow!("joints is required"))?;
                serde_json::to_value(self.harness.manipulator_command(
                    "joint_command",
                    token.as_deref(),
                    json!({ "joints": joints }),
                )?)
                .map_err(Into::into)
            }
            #[cfg(feature = "manipulator")]
            "manipulator_pose_command" => {
                ensure_fields(&args, &["token", "pose"])?;
                let token = optional_string(&args, "token")?;
                let pose = args
                    .get("pose")
                    .cloned()
                    .ok_or_else(|| anyhow!("pose is required"))?;
                serde_json::to_value(self.harness.manipulator_command(
                    "pose_command",
                    token.as_deref(),
                    json!({ "pose": pose }),
                )?)
                .map_err(Into::into)
            }
            #[cfg(feature = "manipulator")]
            "manipulator_home" => {
                ensure_fields(&args, &["token"])?;
                let token = optional_string(&args, "token")?;
                serde_json::to_value(self.harness.manipulator_command(
                    "home",
                    token.as_deref(),
                    json!({}),
                )?)
                .map_err(Into::into)
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
    let descriptors = vec![
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
            "camera_aim",
            "Aim the camera gimbal in pan and tilt degrees through policy gates",
            SafetyClass::PhysicalMotion,
            object_schema(&[
                ("token", "string", false),
                ("pan_deg", "number", true),
                ("tilt_deg", "number", true),
                ("speed", "integer", false),
                ("accel", "integer", false),
                ("approval", "boolean", false),
            ]),
            "CameraAimOutcome",
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
        descriptor(
            "waypoint_create",
            "Create a persistent saved waypoint",
            SafetyClass::SimControl,
            waypoint_schema(),
            "SavedWaypointList",
        ),
        descriptor(
            "waypoint_list",
            "List persistent saved waypoints",
            SafetyClass::ObserveOnly,
            object_schema(&[]),
            "SavedWaypointList",
        ),
        descriptor(
            "waypoint_update",
            "Update a persistent saved waypoint",
            SafetyClass::SimControl,
            waypoint_schema(),
            "SavedWaypointList",
        ),
        descriptor(
            "waypoint_delete",
            "Delete a saved waypoint that is not referenced by a patrol zone",
            SafetyClass::SimControl,
            object_schema(&[("id", "string", true)]),
            "SavedWaypointList",
        ),
        descriptor(
            "patrol_zone_create",
            "Create a persistent patrol zone from saved waypoints and an optional boundary",
            SafetyClass::SimControl,
            patrol_zone_schema(),
            "PatrolZoneList",
        ),
        descriptor(
            "patrol_zone_list",
            "List persistent patrol zones",
            SafetyClass::ObserveOnly,
            object_schema(&[]),
            "PatrolZoneList",
        ),
        descriptor(
            "patrol_zone_update",
            "Update a persistent patrol zone",
            SafetyClass::SimControl,
            patrol_zone_schema(),
            "PatrolZoneList",
        ),
        descriptor(
            "patrol_zone_delete",
            "Delete a persistent patrol zone",
            SafetyClass::SimControl,
            object_schema(&[("id", "string", true)]),
            "PatrolZoneList",
        ),
        descriptor(
            "start_patrol_zone",
            "Start a saved patrol zone in sim or replay without enabling physical actuation",
            SafetyClass::SimControl,
            object_schema(&[
                ("zone_id", "string", true),
                ("speed_mode", "SpeedMode", false),
            ]),
            "PatrolStatus",
        ),
        descriptor(
            "memory_tag_location",
            "Tag or update a named map-frame location or observed object in the local spatial memory store",
            SafetyClass::SimControl,
            object_schema(&[
                ("name", "string", true),
                ("kind", "SpatialMemoryKind", false),
                ("frame_id", "string", false),
                ("x_m", "number", true),
                ("y_m", "number", true),
                ("confidence", "number", false),
            ]),
            "SpatialMemoryStatus",
        ),
        descriptor(
            "memory_list",
            "List local spatial memory entries for this run/profile store",
            SafetyClass::ObserveOnly,
            object_schema(&[("include_stale", "boolean", false)]),
            "SpatialMemoryStatus",
        ),
        descriptor(
            "memory_query",
            "Query local spatial memory entries by name, kind, and effective confidence",
            SafetyClass::ObserveOnly,
            object_schema(&[
                ("query", "string", false),
                ("kind", "SpatialMemoryKind", false),
                ("min_confidence", "number", false),
                ("include_stale", "boolean", false),
            ]),
            "SpatialMemoryStatus",
        ),
        descriptor(
            "memory_clear",
            "Clear local spatial memory for this run/profile store",
            SafetyClass::SimControl,
            object_schema(&[]),
            "SpatialMemoryStatus",
        ),
    ];
    feature_capability_descriptors(descriptors)
}

#[cfg(any(feature = "mavlink-drone", feature = "manipulator"))]
fn feature_capability_descriptors(
    mut descriptors: Vec<CapabilityDescriptor>,
) -> Vec<CapabilityDescriptor> {
    #[cfg(feature = "mavlink-drone")]
    descriptors.extend(drone_capability_descriptors());
    #[cfg(feature = "manipulator")]
    descriptors.extend(manipulator_capability_descriptors());
    descriptors
}

#[cfg(not(any(feature = "mavlink-drone", feature = "manipulator")))]
fn feature_capability_descriptors(
    descriptors: Vec<CapabilityDescriptor>,
) -> Vec<CapabilityDescriptor> {
    descriptors
}

#[cfg(feature = "mavlink-drone")]
fn drone_capability_descriptors() -> Vec<CapabilityDescriptor> {
    vec![
        descriptor(
            "drone_arm",
            "Arm a MAVLink drone adapter after policy and operator gates",
            SafetyClass::PhysicalHighRisk,
            object_schema(&[("token", "string", false), ("approval", "boolean", false)]),
            "DroneCommandStatus",
        ),
        descriptor(
            "drone_disarm",
            "Disarm a MAVLink drone adapter after policy and operator gates",
            SafetyClass::PhysicalHighRisk,
            object_schema(&[("token", "string", false), ("approval", "boolean", false)]),
            "DroneCommandStatus",
        ),
        descriptor(
            "drone_takeoff",
            "Request a MAVLink drone takeoff altitude",
            SafetyClass::PhysicalHighRisk,
            object_schema(&[
                ("token", "string", false),
                ("approval", "boolean", false),
                ("altitude_m", "number", false),
            ]),
            "DroneCommandStatus",
        ),
        descriptor(
            "drone_land",
            "Request a MAVLink drone landing sequence",
            SafetyClass::PhysicalHighRisk,
            object_schema(&[("token", "string", false), ("approval", "boolean", false)]),
            "DroneCommandStatus",
        ),
        descriptor(
            "drone_move_velocity",
            "Request MAVLink drone local-frame velocity movement",
            SafetyClass::PhysicalHighRisk,
            object_schema(&[
                ("token", "string", false),
                ("approval", "boolean", false),
                ("vx_mps", "number", false),
                ("vy_mps", "number", false),
                ("vz_mps", "number", false),
                ("yaw_rate_radps", "number", false),
            ]),
            "DroneCommandStatus",
        ),
        descriptor(
            "drone_fly_to",
            "Request a MAVLink drone global fly-to target",
            SafetyClass::PhysicalHighRisk,
            object_schema(&[
                ("token", "string", false),
                ("approval", "boolean", false),
                ("lat_deg", "number", true),
                ("lon_deg", "number", true),
                ("altitude_m", "number", true),
            ]),
            "DroneCommandStatus",
        ),
    ]
}

#[cfg(feature = "manipulator")]
fn manipulator_capability_descriptors() -> Vec<CapabilityDescriptor> {
    vec![
        descriptor(
            "manipulator_joint_state",
            "Read the versioned manipulator joint state",
            SafetyClass::ObserveOnly,
            object_schema(&[]),
            "ManipulatorJointState",
        ),
        descriptor(
            "manipulator_joint_command",
            "Send a versioned manipulator joint command",
            SafetyClass::PhysicalHighRisk,
            object_schema(&[
                ("token", "string", false),
                ("approval", "boolean", false),
                ("joints", "ManipulatorJointCommandV1", true),
            ]),
            "ManipulatorCommandStatus",
        ),
        descriptor(
            "manipulator_pose_command",
            "Send a versioned manipulator end-effector pose command",
            SafetyClass::PhysicalHighRisk,
            object_schema(&[
                ("token", "string", false),
                ("approval", "boolean", false),
                ("pose", "ManipulatorPoseCommandV1", true),
            ]),
            "ManipulatorCommandStatus",
        ),
        descriptor(
            "manipulator_home",
            "Move the manipulator to its configured home pose",
            SafetyClass::PhysicalHighRisk,
            object_schema(&[("token", "string", false), ("approval", "boolean", false)]),
            "ManipulatorCommandStatus",
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

fn waypoint_schema() -> Value {
    object_schema(&[
        ("id", "string", true),
        ("name", "string", true),
        ("frame_id", "string", false),
        ("x_m", "number", true),
        ("y_m", "number", true),
        ("tolerance_m", "number", false),
    ])
}

fn patrol_zone_schema() -> Value {
    object_schema(&[
        ("id", "string", true),
        ("name", "string", true),
        ("frame_id", "string", false),
        ("waypoint_ids", "array", true),
        ("boundary", "array", false),
    ])
}

fn canonical_name(name: &str) -> &str {
    match name {
        "motors.stop" | "motors/stop" => "stop",
        "camera.aim" | "camera/aim" | "gimbal_aim" | "gimbal.aim" | "gimbal/aim" => "camera_aim",
        "estop/reset" | "estop.reset" => "estop_reset",
        "planner.set_goal" | "planner/set_goal" => "planner_set_goal",
        "planner.cancel" | "planner/cancel" => "planner_cancel",
        "planner.status" | "planner/status" => "planner_status",
        "patrol.start" | "patrol/start" | "patrol_start" => "start_patrol",
        "patrol.stop" | "patrol/stop" | "patrol_stop" => "stop_patrol",
        "patrol.status" | "patrol/status" => "patrol_status",
        "waypoint.create" | "waypoint/create" => "waypoint_create",
        "waypoint.list" | "waypoint/list" => "waypoint_list",
        "waypoint.update" | "waypoint/update" => "waypoint_update",
        "waypoint.delete" | "waypoint/delete" => "waypoint_delete",
        "patrol.zone.create" | "patrol/zone/create" => "patrol_zone_create",
        "patrol.zone.list" | "patrol/zone/list" => "patrol_zone_list",
        "patrol.zone.update" | "patrol/zone/update" => "patrol_zone_update",
        "patrol.zone.delete" | "patrol/zone/delete" => "patrol_zone_delete",
        "patrol.zone.start" | "patrol/zone/start" => "start_patrol_zone",
        "memory.tag"
        | "memory/tag"
        | "memory.tag_location"
        | "memory/tag_location"
        | "memory.tag-location"
        | "memory/tag-location"
        | "memory_tag"
        | "tag_location"
        | "tag-location" => "memory_tag_location",
        "memory.list" | "memory/list" | "list_memory" | "list-memory" => "memory_list",
        "memory.query" | "memory/query" | "query_memory" | "query-memory" => "memory_query",
        "memory.clear" | "memory/clear" | "clear_memory" | "clear-memory" => "memory_clear",
        "drone.arm" | "drone/arm" => "drone_arm",
        "drone.disarm" | "drone/disarm" => "drone_disarm",
        "drone.takeoff" | "drone/takeoff" => "drone_takeoff",
        "drone.land" | "drone/land" => "drone_land",
        "drone.move_velocity"
        | "drone/move_velocity"
        | "drone.move-velocity"
        | "drone/move-velocity" => "drone_move_velocity",
        "drone.fly_to" | "drone/fly_to" | "drone.fly-to" | "drone/fly-to" => "drone_fly_to",
        "manipulator.joint_state"
        | "manipulator/joint_state"
        | "manipulator.joint-state"
        | "manipulator/joint-state" => "manipulator_joint_state",
        "manipulator.joint_command"
        | "manipulator/joint_command"
        | "manipulator.joint-command"
        | "manipulator/joint-command" => "manipulator_joint_command",
        "manipulator.pose_command"
        | "manipulator/pose_command"
        | "manipulator.pose-command"
        | "manipulator/pose-command" => "manipulator_pose_command",
        "manipulator.home" | "manipulator/home" => "manipulator_home",
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

fn optional_bool(args: &Map<String, Value>, key: &str) -> Result<Option<bool>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
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

fn optional_u32(args: &Map<String, Value>, key: &str) -> Result<Option<u32>> {
    optional_u64(args, key)?
        .map(|value| u32::try_from(value).map_err(|_| anyhow!("{key} must fit in u32")))
        .transpose()
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

fn waypoint_spec(args: &Map<String, Value>) -> Result<WaypointSpec> {
    Ok(WaypointSpec {
        id: required_string(args, "id")?,
        name: required_string(args, "name")?,
        frame_id: optional_string(args, "frame_id")?.unwrap_or_else(|| "map".to_string()),
        x_m: required_f64(args, "x_m")?,
        y_m: required_f64(args, "y_m")?,
        tolerance_m: optional_f64(args, "tolerance_m")?.unwrap_or(0.1),
    })
}

fn patrol_zone_spec(args: &Map<String, Value>) -> Result<PatrolZoneSpec> {
    Ok(PatrolZoneSpec {
        id: required_string(args, "id")?,
        name: required_string(args, "name")?,
        frame_id: optional_string(args, "frame_id")?.unwrap_or_else(|| "map".to_string()),
        waypoint_ids: required_string_array(args, "waypoint_ids")?,
        boundary: optional_boundary(args, "boundary")?,
    })
}

fn required_string_array(args: &Map<String, Value>, key: &str) -> Result<Vec<String>> {
    let values = args
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("{key} must be an array of strings"))?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| anyhow!("{key} must contain only strings"))
        })
        .collect()
}

fn optional_boundary(args: &Map<String, Value>, key: &str) -> Result<Vec<ZoneBoundaryPoint>> {
    let Some(value) = args.get(key) else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .ok_or_else(|| anyhow!("{key} must be an array of coordinate objects"))?;
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let object = value
                .as_object()
                .ok_or_else(|| anyhow!("{key}[{index}] must be an object"))?;
            ensure_fields(object, &["x_m", "y_m"])?;
            Ok(ZoneBoundaryPoint {
                x_m: required_f64(object, "x_m")?,
                y_m: required_f64(object, "y_m")?,
            })
        })
        .collect()
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

fn optional_spatial_memory_kind(
    args: &Map<String, Value>,
    key: &str,
) -> Result<Option<SpatialMemoryKind>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|err| anyhow!("{key} must be a valid spatial memory kind: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(any(feature = "mavlink-drone", feature = "manipulator"))]
    use crate::config::Profile;
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

    #[cfg(feature = "mavlink-drone")]
    #[tokio::test]
    async fn drone_capabilities_are_simulated_and_policy_gated() {
        let harness = Harness::new(HarnessConfig {
            policy_mode: PolicyMode::RequireApproval,
            ..HarnessConfig::default()
        })
        .unwrap();
        let registry = CapabilityRegistry::new(harness);

        let err = registry
            .invoke_value("drone_arm", json!({}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("policy require-approval"));

        let armed = registry
            .invoke_value("drone.arm", json!({ "approval": true }))
            .unwrap();
        assert_eq!(armed["ok"], true);
        assert_eq!(armed["command"], "arm");
        assert_eq!(armed["profile"], "sim");
        assert_eq!(armed["simulated"], true);

        let fly_to = registry
            .invoke_value(
                "drone_fly_to",
                json!({
                    "approval": true,
                    "lat_deg": 40.0,
                    "lon_deg": -73.0,
                    "altitude_m": 12.5
                }),
            )
            .unwrap();
        assert_eq!(fly_to["command"], "fly_to");
        assert_eq!(fly_to["args"]["altitude_m"], 12.5);
    }

    #[cfg(feature = "mavlink-drone")]
    #[tokio::test]
    async fn physical_drone_profile_stays_a_gated_skeleton() {
        let err = match Harness::new(HarnessConfig {
            profile: Profile::MavlinkDrone,
            ..HarnessConfig::default()
        }) {
            Ok(_) => panic!("expected mavlink-drone profile to require the physical gate"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("LEASH_ALLOW_PHYSICAL_ACTUATION"));

        let harness = Harness::new(HarnessConfig {
            profile: Profile::MavlinkDrone,
            allow_physical_actuation: true,
            policy_mode: PolicyMode::RequireApproval,
            ..HarnessConfig::default()
        })
        .unwrap();
        let registry = CapabilityRegistry::new(harness);
        let err = registry
            .invoke_value("drone_land", json!({ "approval": true }))
            .unwrap_err()
            .to_string();
        assert!(err.contains("gated skeleton"));
    }

    #[cfg(feature = "manipulator")]
    #[tokio::test]
    async fn manipulator_capabilities_are_simulated_and_policy_gated() {
        let harness = Harness::new(HarnessConfig {
            policy_mode: PolicyMode::RequireApproval,
            ..HarnessConfig::default()
        })
        .unwrap();
        let registry = CapabilityRegistry::new(harness);

        let state = registry
            .invoke_value("manipulator.joint-state", json!({}))
            .unwrap();
        assert_eq!(state["version"], "leash-manipulator-v1");
        assert_eq!(state["simulated"], true);
        assert_eq!(state["joints"][0]["name"], "shoulder_pan");

        let err = registry
            .invoke_value(
                "manipulator_joint_command",
                json!({ "joints": [{ "name": "elbow", "position_rad": 0.4 }] }),
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("policy require-approval"));

        let commanded = registry
            .invoke_value(
                "manipulator.joint-command",
                json!({
                    "approval": true,
                    "joints": [{ "name": "elbow", "position_rad": 0.4 }]
                }),
            )
            .unwrap();
        assert_eq!(commanded["version"], "leash-manipulator-v1");
        assert_eq!(commanded["command"], "joint_command");
        assert_eq!(commanded["simulated"], true);

        let homed = registry
            .invoke_value("manipulator_home", json!({ "approval": true }))
            .unwrap();
        assert_eq!(homed["command"], "home");
    }

    #[cfg(feature = "manipulator")]
    #[tokio::test]
    async fn physical_manipulator_profile_stays_a_gated_skeleton() {
        let err = match Harness::new(HarnessConfig {
            profile: Profile::Manipulator,
            ..HarnessConfig::default()
        }) {
            Ok(_) => panic!("expected manipulator profile to require the physical gate"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("LEASH_ALLOW_PHYSICAL_ACTUATION"));

        let harness = Harness::new(HarnessConfig {
            profile: Profile::Manipulator,
            allow_physical_actuation: true,
            policy_mode: PolicyMode::RequireApproval,
            ..HarnessConfig::default()
        })
        .unwrap();
        let registry = CapabilityRegistry::new(harness);
        let err = registry
            .invoke_value("manipulator_home", json!({ "approval": true }))
            .unwrap_err()
            .to_string();
        assert!(err.contains("gated skeleton"));
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
    async fn waypoint_and_zone_capabilities_execute_in_sim_with_estop_priority() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();
        let registry = CapabilityRegistry::new(harness.clone());

        let waypoints = registry
            .invoke_value(
                "waypoint.create",
                json!({
                    "id": "entry",
                    "name": "Entry",
                    "x_m": 0.25,
                    "y_m": 0.0
                }),
            )
            .unwrap();
        assert_eq!(waypoints["count"], 1);

        let zones = registry
            .invoke_value(
                "patrol.zone.create",
                json!({
                    "id": "front",
                    "name": "Front",
                    "waypoint_ids": ["entry"],
                    "boundary": [
                        {"x_m": 0.0, "y_m": 0.0},
                        {"x_m": 0.5, "y_m": 0.0},
                        {"x_m": 0.5, "y_m": 0.5}
                    ]
                }),
            )
            .unwrap();
        assert_eq!(zones["zones"][0]["id"], "front");

        let started = registry
            .invoke_value(
                "patrol.zone.start",
                json!({"zone_id": "front", "speed_mode": "low"}),
            )
            .unwrap();
        assert_eq!(started["active"], true);
        assert_eq!(started["zone_id"], "front");
        assert_eq!(started["waypoint_index"], 0);

        registry.invoke_value("estop", json!({})).unwrap();
        assert!(!harness.patrol_status().active);
        let error = registry
            .invoke_value("start_patrol_zone", json!({"zone_id": "front"}))
            .unwrap_err()
            .to_string();
        assert!(error.contains("estop is latched"));
    }

    #[tokio::test]
    async fn memory_capabilities_tag_query_list_and_clear() {
        let path = std::env::temp_dir().join(format!(
            "leash-memory-capability-{}-{}.json",
            std::process::id(),
            crate::runtime::now_ms()
        ));
        let harness =
            Harness::new_with_memory_path(HarnessConfig::default(), path.clone()).unwrap();
        let registry = CapabilityRegistry::new(harness);

        let tagged = registry
            .invoke_value_with_origin(
                "tag_location",
                json!({
                    "name": "dock",
                    "frame_id": "map",
                    "x_m": 0.25,
                    "y_m": 0.0,
                    "confidence": 0.95
                }),
                InvocationOrigin::Mcp,
            )
            .unwrap();
        assert_eq!(tagged["ok"], true);
        assert_eq!(tagged["count"], 1);
        assert_eq!(tagged["entries"][0]["name"], "dock");
        assert_eq!(tagged["entries"][0]["kind"], "location");
        assert!(path.exists());

        registry
            .invoke_value(
                "memory_tag_location",
                json!({
                    "name": "cone",
                    "kind": "object",
                    "frame_id": "map",
                    "x_m": 0.4,
                    "y_m": 0.5,
                    "confidence": 0.8
                }),
            )
            .unwrap();
        let queried = registry
            .invoke_value(
                "query_memory",
                json!({ "query": "co", "kind": "object", "min_confidence": 0.7 }),
            )
            .unwrap();
        assert_eq!(queried["count"], 1);
        assert_eq!(queried["entries"][0]["name"], "cone");

        let listed = registry.invoke_value("memory_list", json!({})).unwrap();
        assert_eq!(listed["count"], 2);
        assert!(listed["store_path"]
            .as_str()
            .unwrap()
            .ends_with(path.file_name().unwrap().to_str().unwrap()));

        let cleared = registry.invoke_value("memory_clear", json!({})).unwrap();
        assert_eq!(cleared["count"], 0);

        let _ = std::fs::remove_file(path);
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
