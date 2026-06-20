use std::collections::{HashMap, HashSet};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::{
    config::{HarnessConfig, Profile},
    transport::StreamTransportBackend,
};

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
    pub id: usize,
    pub name: String,
    pub module_type: String,
    pub state: ModuleState,
    pub health: ModuleHealth,
    pub dependencies: Vec<String>,
    pub inputs: Vec<StreamDescriptor>,
    pub outputs: Vec<StreamDescriptor>,
    pub capabilities: Vec<String>,
    pub physical: bool,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct ModuleGraph {
    pub modules: Vec<ModuleInfo>,
}

impl ModuleGraph {
    pub fn new(modules: Vec<ModuleInfo>) -> Result<Self> {
        let mut names = HashSet::with_capacity(modules.len());
        let mut ids = HashSet::with_capacity(modules.len());
        for module in &modules {
            if !ids.insert(module.id) {
                bail!("duplicate module id '{}'", module.id);
            }
            if module.name.trim().is_empty() {
                bail!("module name cannot be empty");
            }
            if !names.insert(module.name.clone()) {
                bail!("duplicate module name '{}'", module.name);
            }
        }
        for module in &modules {
            for dependency in &module.dependencies {
                if dependency == &module.name {
                    bail!("module '{}' cannot depend on itself", module.name);
                }
                if !names.contains(dependency) {
                    bail!(
                        "module '{}' depends on unknown module '{}'",
                        module.name,
                        dependency
                    );
                }
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

    pub fn to_dot(&self) -> String {
        let mut dot = String::from("digraph leash_module_graph {\n");
        dot.push_str("  graph [rankdir=LR];\n");
        dot.push_str("  node [shape=box];\n");

        for module in &self.modules {
            let risk = if module.physical {
                "physical"
            } else {
                "non-physical"
            };
            dot.push_str(&format!(
                "  {} [label=\"{}\\n{}\\n{}\", color=\"{}\"];\n",
                dot_id(module.id),
                escape_dot(&module.name),
                escape_dot(&module.module_type),
                risk,
                if module.physical { "red" } else { "black" }
            ));
        }

        for from in &self.modules {
            for output in &from.outputs {
                for to in &self.modules {
                    if from.id == to.id {
                        continue;
                    }
                    for input in &to.inputs {
                        if output.name == input.name && output.message_type == input.message_type {
                            dot.push_str(&format!(
                                "  {} -> {} [label=\"{}:{} via {}\"];\n",
                                dot_id(from.id),
                                dot_id(to.id),
                                escape_dot(&output.name),
                                escape_dot(&output.message_type),
                                escape_dot(&output.transport)
                            ));
                        }
                    }
                }
            }
        }

        dot.push_str("}\n");
        dot
    }
}

#[derive(Debug, Clone)]
pub struct ModuleCoordinator {
    modules: Vec<ModuleInfo>,
    start_failures: HashMap<String, String>,
}

impl ModuleCoordinator {
    pub fn new(graph: ModuleGraph) -> Self {
        Self {
            modules: graph.modules,
            start_failures: HashMap::new(),
        }
    }

    pub fn with_start_failure(mut self, module_name: &str, message: impl Into<String>) -> Self {
        self.start_failures
            .insert(module_name.to_string(), message.into());
        self
    }

    pub fn start(&mut self) -> Result<()> {
        let order = self.dependency_order()?;
        let mut started = Vec::new();

        for index in order {
            self.set_health(index, ModuleState::Starting, true, "starting");
            let module_name = self.modules[index].name.clone();
            if let Some(message) = self.start_failures.get(&module_name).cloned() {
                self.set_health(index, ModuleState::Failed, false, message.clone());
                self.stop_started_after_failure(&started);
                bail!("module '{}' failed to start: {}", module_name, message);
            }
            self.set_health(index, ModuleState::Running, true, "running");
            started.push(index);
        }

        Ok(())
    }

    pub fn stop(&mut self) {
        let Ok(order) = self.dependency_order() else {
            for index in (0..self.modules.len()).rev() {
                if matches!(
                    self.modules[index].state,
                    ModuleState::Starting | ModuleState::Running
                ) {
                    self.set_health(index, ModuleState::Stopped, true, "stopped");
                }
            }
            return;
        };
        for index in order.into_iter().rev() {
            if matches!(
                self.modules[index].state,
                ModuleState::Starting | ModuleState::Running
            ) {
                self.set_health(index, ModuleState::Stopped, true, "stopped");
            }
        }
    }

    pub fn graph(&self) -> ModuleGraph {
        ModuleGraph::new(self.modules.clone()).expect("coordinator preserves valid module graph")
    }

    pub fn modules(&self) -> &[ModuleInfo] {
        &self.modules
    }

    pub fn is_healthy(&self) -> bool {
        self.modules
            .iter()
            .all(|module| module.health.ok && module.state == ModuleState::Running)
    }

    fn dependency_order(&self) -> Result<Vec<usize>> {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Visit {
            New,
            Active,
            Done,
        }

        let by_name: HashMap<&str, usize> = self
            .modules
            .iter()
            .enumerate()
            .map(|(index, module)| (module.name.as_str(), index))
            .collect();
        let mut visits = vec![Visit::New; self.modules.len()];
        let mut order = Vec::with_capacity(self.modules.len());

        fn visit(
            index: usize,
            modules: &[ModuleInfo],
            by_name: &HashMap<&str, usize>,
            visits: &mut [Visit],
            order: &mut Vec<usize>,
        ) -> Result<()> {
            match visits[index] {
                Visit::Done => return Ok(()),
                Visit::Active => bail!("module dependency cycle at '{}'", modules[index].name),
                Visit::New => {}
            }

            visits[index] = Visit::Active;
            for dependency in &modules[index].dependencies {
                let Some(dependency_index) = by_name.get(dependency.as_str()).copied() else {
                    bail!(
                        "module '{}' depends on unknown module '{}'",
                        modules[index].name,
                        dependency
                    );
                };
                visit(dependency_index, modules, by_name, visits, order)?;
            }
            visits[index] = Visit::Done;
            order.push(index);
            Ok(())
        }

        for index in 0..self.modules.len() {
            visit(index, &self.modules, &by_name, &mut visits, &mut order)?;
        }
        Ok(order)
    }

    fn stop_started_after_failure(&mut self, started: &[usize]) {
        for index in started.iter().copied().rev() {
            self.set_health(
                index,
                ModuleState::Stopped,
                true,
                "stopped after startup failure",
            );
        }
    }

    fn set_health(
        &mut self,
        index: usize,
        state: ModuleState,
        ok: bool,
        message: impl Into<String>,
    ) {
        let module = &mut self.modules[index];
        module.state = state;
        module.health = ModuleHealth {
            ok,
            state,
            message: Some(message.into()),
        };
    }
}

pub fn default_module_graph(config: &HarnessConfig, capabilities: Vec<String>) -> ModuleGraph {
    let driver_name = match config.profile {
        Profile::Sim => "sim-driver",
        Profile::Replay => "replay-driver",
        Profile::WaveshareUgv => "waveshare-ugv-driver",
        Profile::MavlinkDrone => "mavlink-drone-driver",
    };
    let driver_capabilities = match config.profile {
        Profile::MavlinkDrone => vec![
            "drone_arm".to_string(),
            "drone_disarm".to_string(),
            "drone_takeoff".to_string(),
            "drone_land".to_string(),
            "drone_move_velocity".to_string(),
            "drone_fly_to".to_string(),
            "stop".to_string(),
            "estop".to_string(),
        ],
        _ => vec!["drive".to_string(), "stop".to_string(), "estop".to_string()],
    };
    let driver_input = match config.profile {
        Profile::MavlinkDrone => "FlightCommand",
        _ => "DriveReq",
    };
    let driver_output = match config.profile {
        Profile::MavlinkDrone => "DroneCommandStatus",
        _ => "OdometryStatus",
    };
    let transport = config.stream_transport;
    ModuleGraph::new(vec![
        ModuleInfo {
            id: 0,
            name: "harness-runtime".to_string(),
            module_type: "runtime".to_string(),
            state: ModuleState::Planned,
            health: planned_health(),
            dependencies: Vec::new(),
            inputs: vec![stream(
                "capability_request",
                StreamDirection::Input,
                "json",
                transport,
            )],
            outputs: vec![
                stream("health", StreamDirection::Output, "Health", transport),
                stream(
                    "capabilities",
                    StreamDirection::Output,
                    "Capabilities",
                    transport,
                ),
            ],
            capabilities,
            physical: false,
            required: true,
        },
        ModuleInfo {
            id: 1,
            name: driver_name.to_string(),
            module_type: "driver".to_string(),
            state: ModuleState::Planned,
            health: planned_health(),
            dependencies: vec!["harness-runtime".to_string()],
            inputs: vec![stream(
                if config.profile == Profile::MavlinkDrone {
                    "flight_command"
                } else {
                    "drive_command"
                },
                StreamDirection::Input,
                driver_input,
                transport,
            )],
            outputs: vec![stream(
                if config.profile == Profile::MavlinkDrone {
                    "flight_status"
                } else {
                    "odometry"
                },
                StreamDirection::Output,
                driver_output,
                transport,
            )],
            capabilities: driver_capabilities,
            physical: config.profile.is_physical(),
            required: true,
        },
        ModuleInfo {
            id: 2,
            name: "telemetry".to_string(),
            module_type: "telemetry".to_string(),
            state: ModuleState::Planned,
            health: planned_health(),
            dependencies: vec![driver_name.to_string()],
            inputs: vec![stream(
                "odometry",
                StreamDirection::Input,
                "OdometryStatus",
                transport,
            )],
            outputs: vec![
                stream(
                    "telemetry",
                    StreamDirection::Output,
                    "TelemetryFrame",
                    transport,
                ),
                stream(
                    "sensors",
                    StreamDirection::Output,
                    "SensorSnapshot",
                    transport,
                ),
            ],
            capabilities: vec!["observe".to_string(), "capture".to_string()],
            physical: false,
            required: true,
        },
    ])
    .expect("default module graph uses unique non-empty names")
}

fn dot_id(id: usize) -> String {
    format!("module_{id}")
}

fn escape_dot(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn stream(
    name: &str,
    direction: StreamDirection,
    message_type: &str,
    transport: StreamTransportBackend,
) -> StreamDescriptor {
    StreamDescriptor {
        name: name.to_string(),
        direction,
        message_type: message_type.to_string(),
        transport: transport.as_str().to_string(),
    }
}

fn planned_health() -> ModuleHealth {
    ModuleHealth {
        ok: true,
        state: ModuleState::Planned,
        message: Some("planned".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(name: &str) -> ModuleInfo {
        ModuleInfo {
            id: 0,
            name: name.to_string(),
            module_type: "test".to_string(),
            state: ModuleState::Planned,
            health: planned_health(),
            dependencies: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            capabilities: Vec::new(),
            physical: false,
            required: true,
        }
    }

    fn module_with_id(id: usize, name: &str) -> ModuleInfo {
        ModuleInfo { id, ..module(name) }
    }

    #[test]
    fn rejects_duplicate_module_names() {
        let err = ModuleGraph::new(vec![module_with_id(0, "a"), module_with_id(1, "a")])
            .unwrap_err()
            .to_string();
        assert!(err.contains("duplicate module name"));
    }

    #[test]
    fn rejects_duplicate_module_ids() {
        let err = ModuleGraph::new(vec![module_with_id(0, "a"), module_with_id(0, "b")])
            .unwrap_err()
            .to_string();
        assert!(err.contains("duplicate module id"));
    }

    #[test]
    fn counts_declared_streams() {
        let graph = default_module_graph(&HarnessConfig::default(), vec!["health".to_string()]);
        assert_eq!(graph.modules().len(), 3);
        assert_eq!(graph.stream_count(), 8);
    }

    #[test]
    fn exports_dot_with_edges_and_risk_metadata() {
        let graph = default_module_graph(
            &HarnessConfig {
                profile: Profile::WaveshareUgv,
                ..HarnessConfig::default()
            },
            vec!["drive".to_string()],
        );
        let dot = graph.to_dot();
        assert!(dot.contains("digraph leash_module_graph"));
        assert!(dot.contains("waveshare-ugv-driver"));
        assert!(dot.contains("physical"));
        assert!(dot.contains("module_1 -> module_2"));
        assert!(dot.contains("odometry:OdometryStatus via local-pubsub"));
    }

    #[test]
    fn graph_stream_transport_can_switch_backends() {
        let graph = default_module_graph(
            &HarnessConfig {
                stream_transport: StreamTransportBackend::Memory,
                ..HarnessConfig::default()
            },
            vec!["observe".to_string()],
        );

        assert!(graph
            .modules()
            .iter()
            .flat_map(|module| module.inputs.iter().chain(module.outputs.iter()))
            .all(|stream| stream.transport == "memory"));
    }

    #[test]
    fn coordinator_starts_in_dependency_order() {
        let graph = default_module_graph(&HarnessConfig::default(), vec!["health".to_string()]);
        let mut coordinator = ModuleCoordinator::new(graph);
        coordinator.start().unwrap();

        let modules = coordinator.modules();
        assert_eq!(modules[0].state, ModuleState::Running);
        assert_eq!(modules[1].state, ModuleState::Running);
        assert_eq!(modules[2].state, ModuleState::Running);
        assert!(coordinator.is_healthy());
    }

    #[test]
    fn coordinator_cleans_up_started_modules_after_start_failure() {
        let graph = default_module_graph(&HarnessConfig::default(), vec!["drive".to_string()]);
        let mut coordinator =
            ModuleCoordinator::new(graph).with_start_failure("telemetry", "loop refused");

        let err = coordinator.start().unwrap_err().to_string();
        assert!(err.contains("telemetry"));

        let modules = coordinator.modules();
        assert_eq!(modules[0].state, ModuleState::Stopped);
        assert_eq!(modules[1].state, ModuleState::Stopped);
        assert_eq!(modules[2].state, ModuleState::Failed);
        assert!(!coordinator.is_healthy());
    }

    #[test]
    fn coordinator_stop_is_idempotent_during_partial_startup() {
        let mut runtime = module_with_id(0, "runtime");
        runtime.state = ModuleState::Running;
        runtime.health = ModuleHealth {
            ok: true,
            state: ModuleState::Running,
            message: Some("running".to_string()),
        };
        let mut driver = module_with_id(1, "driver");
        driver.dependencies = vec!["runtime".to_string()];
        driver.state = ModuleState::Starting;
        driver.health = ModuleHealth {
            ok: true,
            state: ModuleState::Starting,
            message: Some("starting".to_string()),
        };
        let graph = ModuleGraph::new(vec![runtime, driver]).unwrap();
        let mut coordinator = ModuleCoordinator::new(graph);

        coordinator.stop();
        coordinator.stop();

        assert!(coordinator
            .modules()
            .iter()
            .all(|module| module.state == ModuleState::Stopped));
    }

    #[test]
    fn failed_optional_module_does_not_leave_runtime_starting() {
        let mut optional = module_with_id(1, "optional-viewer");
        optional.required = false;
        optional.dependencies = vec!["runtime".to_string()];
        let graph = ModuleGraph::new(vec![module_with_id(0, "runtime"), optional]).unwrap();
        let mut coordinator =
            ModuleCoordinator::new(graph).with_start_failure("optional-viewer", "not available");

        let _ = coordinator.start().unwrap_err();

        assert!(coordinator
            .modules()
            .iter()
            .all(|module| { matches!(module.state, ModuleState::Stopped | ModuleState::Failed) }));
    }
}
