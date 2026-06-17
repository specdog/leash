use std::collections::HashSet;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::config::{HarnessConfig, Profile};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ModuleState {
    Planned,
    Starting,
    Running,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum StreamDirection {
    Input,
    Output,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct StreamDescriptor {
    pub name: String,
    pub direction: StreamDirection,
    pub message_type: String,
    pub transport: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct ModuleHealth {
    pub ok: bool,
    pub state: ModuleState,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct ModuleInfo {
    pub name: String,
    pub module_type: String,
    pub state: ModuleState,
    pub health: ModuleHealth,
    pub inputs: Vec<StreamDescriptor>,
    pub outputs: Vec<StreamDescriptor>,
    pub capabilities: Vec<String>,
    pub physical: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct ModuleGraph {
    pub modules: Vec<ModuleInfo>,
}

impl ModuleGraph {
    pub fn new(modules: Vec<ModuleInfo>) -> Result<Self> {
        let mut names = HashSet::with_capacity(modules.len());
        for module in &modules {
            if module.name.trim().is_empty() {
                bail!("module name cannot be empty");
            }
            if !names.insert(module.name.clone()) {
                bail!("duplicate module name '{}'", module.name);
            }
        }
        Ok(Self { modules })
    }

    pub fn modules(&self) -> &[ModuleInfo] {
        &self.modules
    }

    pub fn stream_count(&self) -> usize {
        self.modules
            .iter()
            .map(|module| module.inputs.len() + module.outputs.len())
            .sum()
    }
}

pub fn default_module_graph(config: &HarnessConfig, capabilities: Vec<String>) -> ModuleGraph {
    let driver_name = match config.profile {
        Profile::Sim => "sim-driver",
        Profile::WaveshareUgv => "waveshare-ugv-driver",
    };
    ModuleGraph::new(vec![
        ModuleInfo {
            name: "harness-runtime".to_string(),
            module_type: "runtime".to_string(),
            state: ModuleState::Running,
            health: running_health("runtime ready"),
            inputs: vec![stream("capability_request", StreamDirection::Input, "json")],
            outputs: vec![
                stream("health", StreamDirection::Output, "Health"),
                stream("capabilities", StreamDirection::Output, "Capabilities"),
            ],
            capabilities,
            physical: false,
        },
        ModuleInfo {
            name: driver_name.to_string(),
            module_type: "driver".to_string(),
            state: ModuleState::Running,
            health: running_health("driver ready"),
            inputs: vec![stream("drive_command", StreamDirection::Input, "DriveReq")],
            outputs: vec![stream(
                "odometry",
                StreamDirection::Output,
                "OdometryStatus",
            )],
            capabilities: vec!["drive".to_string(), "stop".to_string(), "estop".to_string()],
            physical: config.profile.is_physical(),
        },
        ModuleInfo {
            name: "telemetry".to_string(),
            module_type: "telemetry".to_string(),
            state: ModuleState::Running,
            health: running_health("telemetry loop ready"),
            inputs: vec![stream("odometry", StreamDirection::Input, "OdometryStatus")],
            outputs: vec![
                stream("telemetry", StreamDirection::Output, "TelemetryFrame"),
                stream("sensors", StreamDirection::Output, "SensorSnapshot"),
            ],
            capabilities: vec!["observe".to_string(), "capture".to_string()],
            physical: false,
        },
    ])
    .expect("default module graph uses unique non-empty names")
}

fn stream(name: &str, direction: StreamDirection, message_type: &str) -> StreamDescriptor {
    StreamDescriptor {
        name: name.to_string(),
        direction,
        message_type: message_type.to_string(),
        transport: "in-process".to_string(),
    }
}

fn running_health(message: &str) -> ModuleHealth {
    ModuleHealth {
        ok: true,
        state: ModuleState::Running,
        message: Some(message.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(name: &str) -> ModuleInfo {
        ModuleInfo {
            name: name.to_string(),
            module_type: "test".to_string(),
            state: ModuleState::Running,
            health: running_health("ok"),
            inputs: Vec::new(),
            outputs: Vec::new(),
            capabilities: Vec::new(),
            physical: false,
        }
    }

    #[test]
    fn rejects_duplicate_module_names() {
        let err = ModuleGraph::new(vec![module("a"), module("a")])
            .unwrap_err()
            .to_string();
        assert!(err.contains("duplicate module name"));
    }

    #[test]
    fn counts_declared_streams() {
        let graph = default_module_graph(&HarnessConfig::default(), vec!["health".to_string()]);
        assert_eq!(graph.modules().len(), 3);
        assert_eq!(graph.stream_count(), 8);
    }
}
