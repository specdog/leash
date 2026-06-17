use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::{runtime::Harness, types::SpeedMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum SafetyClass {
    ObserveOnly,
    SimControl,
    PhysicalStop,
    PhysicalMotion,
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
        match canonical_name(name) {
            "health" => {
                let args = args_object(args)?;
                ensure_fields(&args, &[])?;
                serde_json::to_value(self.harness.health()).map_err(Into::into)
            }
            "capabilities" => {
                let args = args_object(args)?;
                ensure_fields(&args, &[])?;
                serde_json::to_value(self.harness.capabilities()).map_err(Into::into)
            }
            "observe" => {
                let args = args_object(args)?;
                ensure_fields(&args, &[])?;
                serde_json::to_value(self.harness.telemetry()).map_err(Into::into)
            }
            "capture" => {
                let args = args_object(args)?;
                ensure_fields(&args, &[])?;
                serde_json::to_value(self.harness.capture()).map_err(Into::into)
            }
            "authorize" => {
                let args = args_object(args)?;
                ensure_fields(&args, &["token", "ttl_secs", "speed_mode"])?;
                let token = required_string(&args, "token")?;
                let ttl_secs = optional_u64(&args, "ttl_secs")?.unwrap_or(120);
                let speed_mode = optional_speed_mode(&args, "speed_mode")?.unwrap_or_default();
                self.harness.authorize(token, ttl_secs, speed_mode)?;
                Ok(json!({ "ok": true, "ttl_secs": ttl_secs, "speed_mode": speed_mode }))
            }
            "drive" => {
                let args = args_object(args)?;
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
                let args = args_object(args)?;
                ensure_fields(&args, &["token", "speed_mode"])?;
                let token = optional_string(&args, "token")?;
                let speed_mode = required_speed_mode(&args, "speed_mode")?;
                self.harness.set_speed_mode(token.as_deref(), speed_mode)?;
                Ok(json!({ "ok": true, "speed_mode": speed_mode }))
            }
            "stop" => {
                let args = args_object(args)?;
                ensure_fields(&args, &[])?;
                serde_json::to_value(self.harness.stop()?).map_err(Into::into)
            }
            "estop" => {
                let args = args_object(args)?;
                ensure_fields(&args, &[])?;
                self.harness.estop()?;
                Ok(json!({ "ok": true, "estop": true }))
            }
            "estop_reset" => {
                let args = args_object(args)?;
                ensure_fields(&args, &[])?;
                self.harness.reset_estop();
                Ok(json!({ "ok": true, "estop": false }))
            }
            other => Err(anyhow!("unknown capability '{other}'")),
        }
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
            SafetyClass::SimControl,
            object_schema(&[]),
            "EstopResp",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{types::TelemetryFrame, HarnessConfig};

    #[tokio::test]
    async fn descriptors_are_unique() {
        let registry = CapabilityRegistry::new(Harness::new(HarnessConfig::default()).unwrap());
        let names = registry.names();
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(names.len(), unique.len());
    }

    #[tokio::test]
    async fn rejects_invalid_drive_args_before_command_changes() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();
        let registry = CapabilityRegistry::new(harness.clone());
        let err = registry
            .invoke_value("drive", json!({ "left": 0.2 }))
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
}
