use std::{collections::HashSet, net::SocketAddr, str::FromStr};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::{
    capability::default_capability_descriptors,
    config::{HarnessConfig, PartialHarnessConfig, Profile},
    module::default_module_graph,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum StackTransport {
    Http,
    Mcp,
}

impl StackTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Mcp => "mcp",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct TransportBinding {
    pub kind: StackTransport,
    pub listen: Option<SocketAddr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct StackModule {
    pub name: String,
    pub module_type: String,
    pub required: bool,
    pub physical: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum AdapterCategory {
    Simulation,
    Compatibility,
    MobileBase,
    Drone,
    Manipulator,
    Perception,
    Sensor,
}

impl AdapterCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Simulation => "simulation",
            Self::Compatibility => "compatibility",
            Self::MobileBase => "mobile-base",
            Self::Drone => "drone",
            Self::Manipulator => "manipulator",
            Self::Perception => "perception",
            Self::Sensor => "sensor",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum AdapterMaturity {
    Stable,
    Beta,
    Alpha,
    Experimental,
}

impl AdapterMaturity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
            Self::Alpha => "alpha",
            Self::Experimental => "experimental",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct AdapterProfile {
    pub category: AdapterCategory,
    pub maturity: AdapterMaturity,
    pub capabilities: Vec<String>,
    pub feature_flags: Vec<String>,
    pub required_gates: Vec<String>,
    pub boundary: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct Stack {
    pub name: String,
    pub description: String,
    pub profile: Profile,
    pub transport: TransportBinding,
    pub required_features: Vec<String>,
    pub hardware_required: bool,
    pub adapter: AdapterProfile,
    pub config_overrides: PartialHarnessConfig,
    pub modules: Vec<StackModule>,
    pub command: String,
}

impl Stack {
    pub fn validate(&self) -> Result<()> {
        if self.hardware_required != self.profile.is_physical() {
            bail!(
                "stack '{}' hardware metadata does not match profile '{}'",
                self.name,
                self.profile.as_str()
            );
        }

        for feature in &self.adapter.feature_flags {
            if !self.required_features.contains(feature) {
                bail!(
                    "stack '{}' adapter feature '{}' is missing from required_features",
                    self.name,
                    feature
                );
            }
        }

        if self.adapter.capabilities.is_empty() {
            bail!("stack '{}' adapter must declare capabilities", self.name);
        }

        if self.hardware_required
            && !self
                .adapter
                .required_gates
                .iter()
                .any(|gate| gate == "physical-actuation")
        {
            bail!(
                "stack '{}' physical adapter must declare physical-actuation gate",
                self.name
            );
        }

        let capabilities = default_capability_descriptors()
            .into_iter()
            .map(|descriptor| descriptor.name)
            .collect();
        let graph = default_module_graph(
            &HarnessConfig {
                profile: self.profile,
                ..HarnessConfig::default()
            },
            capabilities,
        );
        let graph_modules = graph
            .modules()
            .iter()
            .map(|module| module.name.as_str())
            .collect::<HashSet<_>>();

        for module in &self.modules {
            if !graph_modules.contains(module.name.as_str()) {
                bail!(
                    "stack '{}' references unknown module '{}'",
                    self.name,
                    module.name
                );
            }
        }
        Ok(())
    }
}

pub fn built_in_stacks() -> Vec<Stack> {
    let stacks = vec![
        sim_http_stack(),
        sim_mcp_stack(),
        bridge_compat_http_stack(),
        waveshare_ugv_http_stack(),
    ];
    #[cfg(feature = "mavlink-drone")]
    {
        let mut stacks = stacks;
        stacks.push(mavlink_drone_sim_stack());
        stacks.push(mavlink_drone_replay_stack());
        stacks.push(mavlink_drone_http_stack());
        stacks
    }
    #[cfg(not(feature = "mavlink-drone"))]
    {
        stacks
    }
}

pub fn find_stack(name: &str) -> Option<Stack> {
    built_in_stacks()
        .into_iter()
        .find(|stack| stack.name == name)
}

pub fn resolve_stack(name: &str) -> Result<Stack> {
    let Some(stack) = find_stack(name) else {
        let names = built_in_stacks()
            .into_iter()
            .map(|stack| stack.name)
            .collect::<Vec<_>>()
            .join(", ");
        bail!("unknown stack '{name}'; expected one of: {names}");
    };
    stack.validate()?;
    Ok(stack)
}

pub fn adapter_profile_for_profile(profile: Profile) -> AdapterProfile {
    match profile {
        Profile::Sim => simulation_adapter_profile(),
        Profile::Replay => replay_adapter_profile(),
        Profile::WaveshareUgv => waveshare_ugv_adapter_profile(),
        Profile::MavlinkDrone => mavlink_drone_adapter_profile(),
    }
}

fn sim_http_stack() -> Stack {
    Stack {
        name: "sim-http".to_string(),
        description: "Simulation HTTP runtime with WebSocket telemetry".to_string(),
        profile: Profile::Sim,
        transport: TransportBinding {
            kind: StackTransport::Http,
            listen: Some(socket("127.0.0.1:8000")),
        },
        required_features: strings(&["sim", "http"]),
        hardware_required: false,
        adapter: simulation_adapter_profile(),
        config_overrides: PartialHarnessConfig {
            listen: Some(socket("127.0.0.1:8000")),
            ..PartialHarnessConfig::default()
        },
        modules: module_refs(Profile::Sim),
        command: "leash run sim-http".to_string(),
    }
}

fn sim_mcp_stack() -> Stack {
    Stack {
        name: "sim-mcp".to_string(),
        description: "Simulation stdio MCP server for local LLM clients".to_string(),
        profile: Profile::Sim,
        transport: TransportBinding {
            kind: StackTransport::Mcp,
            listen: None,
        },
        required_features: strings(&["sim", "mcp"]),
        hardware_required: false,
        adapter: simulation_adapter_profile(),
        config_overrides: PartialHarnessConfig::default(),
        modules: module_refs(Profile::Sim),
        command: "leash run sim-mcp".to_string(),
    }
}

fn bridge_compat_http_stack() -> Stack {
    Stack {
        name: "bridge-compat-http".to_string(),
        description: "Simulation HTTP runtime with bridge compatibility endpoints".to_string(),
        profile: Profile::Sim,
        transport: TransportBinding {
            kind: StackTransport::Http,
            listen: Some(socket("127.0.0.1:8001")),
        },
        required_features: strings(&["sim", "http", "bridge-compat"]),
        hardware_required: false,
        adapter: bridge_compat_adapter_profile(),
        config_overrides: PartialHarnessConfig {
            listen: Some(socket("127.0.0.1:8001")),
            ..PartialHarnessConfig::default()
        },
        modules: module_refs(Profile::Sim),
        command: "leash run bridge-compat-http".to_string(),
    }
}

fn waveshare_ugv_http_stack() -> Stack {
    Stack {
        name: "waveshare-ugv-http".to_string(),
        description: "Gated Waveshare UGV HTTP runtime for bot installs".to_string(),
        profile: Profile::WaveshareUgv,
        transport: TransportBinding {
            kind: StackTransport::Http,
            listen: Some(socket("0.0.0.0:8000")),
        },
        required_features: strings(&["http", "waveshare-ugv"]),
        hardware_required: true,
        adapter: waveshare_ugv_adapter_profile(),
        config_overrides: PartialHarnessConfig {
            listen: Some(socket("0.0.0.0:8000")),
            ..PartialHarnessConfig::default()
        },
        modules: module_refs(Profile::WaveshareUgv),
        command:
            "LEASH_ALLOW_PHYSICAL_ACTUATION=1 leash run waveshare-ugv-http --allow-physical-actuation"
                .to_string(),
    }
}

#[cfg(feature = "mavlink-drone")]
fn mavlink_drone_sim_stack() -> Stack {
    Stack {
        name: "mavlink-drone-sim".to_string(),
        description: "No-hardware MAVLink drone adapter skeleton in simulation".to_string(),
        profile: Profile::Sim,
        transport: TransportBinding {
            kind: StackTransport::Http,
            listen: Some(socket("127.0.0.1:8010")),
        },
        required_features: strings(&["sim", "http", "mavlink-drone"]),
        hardware_required: false,
        adapter: mavlink_drone_sim_adapter_profile(),
        config_overrides: PartialHarnessConfig {
            listen: Some(socket("127.0.0.1:8010")),
            ..PartialHarnessConfig::default()
        },
        modules: module_refs(Profile::Sim),
        command: "leash run mavlink-drone-sim".to_string(),
    }
}

#[cfg(feature = "mavlink-drone")]
fn mavlink_drone_replay_stack() -> Stack {
    Stack {
        name: "mavlink-drone-replay".to_string(),
        description: "Fixture-backed MAVLink drone adapter skeleton for replay proof".to_string(),
        profile: Profile::Replay,
        transport: TransportBinding {
            kind: StackTransport::Mcp,
            listen: None,
        },
        required_features: strings(&["mcp", "mavlink-drone"]),
        hardware_required: false,
        adapter: mavlink_drone_replay_adapter_profile(),
        config_overrides: PartialHarnessConfig::default(),
        modules: module_refs(Profile::Replay),
        command: "leash run mavlink-drone-replay --replay-source examples/replay/sim-basic.jsonl"
            .to_string(),
    }
}

#[cfg(feature = "mavlink-drone")]
fn mavlink_drone_http_stack() -> Stack {
    Stack {
        name: "mavlink-drone-http".to_string(),
        description: "Gated MAVLink drone HTTP runtime skeleton".to_string(),
        profile: Profile::MavlinkDrone,
        transport: TransportBinding {
            kind: StackTransport::Http,
            listen: Some(socket("0.0.0.0:8010")),
        },
        required_features: strings(&["http", "mavlink-drone"]),
        hardware_required: true,
        adapter: mavlink_drone_adapter_profile(),
        config_overrides: PartialHarnessConfig {
            listen: Some(socket("0.0.0.0:8010")),
            mavlink_endpoint: Some("udp://127.0.0.1:14550".to_string()),
            ..PartialHarnessConfig::default()
        },
        modules: module_refs(Profile::MavlinkDrone),
        command:
            "LEASH_ALLOW_PHYSICAL_ACTUATION=1 LEASH_MAVLINK_ENDPOINT=udp://127.0.0.1:14550 leash run mavlink-drone-http --allow-physical-actuation"
                .to_string(),
    }
}

fn simulation_adapter_profile() -> AdapterProfile {
    AdapterProfile {
        category: AdapterCategory::Simulation,
        maturity: AdapterMaturity::Stable,
        capabilities: strings(&[
            "drive",
            "speed_mode",
            "stop",
            "estop",
            "estop_reset",
            "observe",
            "capture",
        ]),
        feature_flags: strings(&["sim"]),
        required_gates: Vec::new(),
        boundary: "in-crate simulation driver; no hardware or external process boundary"
            .to_string(),
    }
}

fn replay_adapter_profile() -> AdapterProfile {
    AdapterProfile {
        category: AdapterCategory::Simulation,
        maturity: AdapterMaturity::Beta,
        capabilities: strings(&["observe", "capture"]),
        feature_flags: Vec::new(),
        required_gates: Vec::new(),
        boundary: "fixture-backed replay source; deterministic and non-physical".to_string(),
    }
}

fn bridge_compat_adapter_profile() -> AdapterProfile {
    AdapterProfile {
        category: AdapterCategory::Compatibility,
        maturity: AdapterMaturity::Beta,
        capabilities: strings(&["drive", "stop", "estop", "observe"]),
        feature_flags: strings(&["sim", "bridge-compat"]),
        required_gates: Vec::new(),
        boundary: "HTTP compatibility shim over the simulation adapter".to_string(),
    }
}

fn waveshare_ugv_adapter_profile() -> AdapterProfile {
    AdapterProfile {
        category: AdapterCategory::MobileBase,
        maturity: AdapterMaturity::Alpha,
        capabilities: strings(&["drive", "speed_mode", "stop", "estop", "observe", "capture"]),
        feature_flags: strings(&["waveshare-ugv"]),
        required_gates: strings(&["physical-actuation", "policy-token-or-approval"]),
        boundary: "feature-gated RobotDriver implementation; serial I/O isolated from core policy"
            .to_string(),
    }
}

fn mavlink_drone_capabilities() -> Vec<String> {
    strings(&[
        "drone_arm",
        "drone_disarm",
        "drone_takeoff",
        "drone_land",
        "drone_move_velocity",
        "drone_fly_to",
        "observe",
        "stop",
    ])
}

fn mavlink_drone_adapter_profile() -> AdapterProfile {
    AdapterProfile {
        category: AdapterCategory::Drone,
        maturity: AdapterMaturity::Experimental,
        capabilities: mavlink_drone_capabilities(),
        feature_flags: strings(&["mavlink-drone"]),
        required_gates: strings(&["physical-actuation", "policy-token-or-approval"]),
        boundary: "feature-gated MAVLink adapter skeleton; network endpoint is configured but flight I/O is intentionally externalized".to_string(),
    }
}

#[cfg(feature = "mavlink-drone")]
fn mavlink_drone_sim_adapter_profile() -> AdapterProfile {
    AdapterProfile {
        maturity: AdapterMaturity::Alpha,
        required_gates: Vec::new(),
        boundary: "simulated MAVLink drone capability surface; no flight hardware or network I/O"
            .to_string(),
        ..mavlink_drone_adapter_profile()
    }
}

#[cfg(feature = "mavlink-drone")]
fn mavlink_drone_replay_adapter_profile() -> AdapterProfile {
    AdapterProfile {
        maturity: AdapterMaturity::Beta,
        required_gates: Vec::new(),
        boundary: "replay-backed MAVLink drone capability surface; deterministic and non-physical"
            .to_string(),
        ..mavlink_drone_adapter_profile()
    }
}

fn module_refs(profile: Profile) -> Vec<StackModule> {
    let driver = match profile {
        Profile::Sim => ("sim-driver", false),
        Profile::Replay => ("replay-driver", false),
        Profile::WaveshareUgv => ("waveshare-ugv-driver", true),
        Profile::MavlinkDrone => ("mavlink-drone-driver", true),
    };
    vec![
        stack_module("harness-runtime", "runtime", true, false),
        stack_module(driver.0, "driver", true, driver.1),
        stack_module("telemetry", "telemetry", true, false),
    ]
}

fn stack_module(name: &str, module_type: &str, required: bool, physical: bool) -> StackModule {
    StackModule {
        name: name.to_string(),
        module_type: module_type.to_string(),
        required,
        physical,
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

fn socket(value: &str) -> SocketAddr {
    SocketAddr::from_str(value).expect("built-in stack socket is valid")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::config::{resolve_config, ConfigRequest};

    #[test]
    fn built_in_stacks_validate() {
        let stacks = built_in_stacks();
        assert!(stacks.iter().any(|stack| stack.name == "sim-http"));
        assert!(stacks.iter().any(|stack| stack.name == "sim-mcp"));
        assert!(stacks
            .iter()
            .any(|stack| stack.name == "bridge-compat-http"));
        assert!(stacks
            .iter()
            .any(|stack| stack.name == "waveshare-ugv-http"));
        #[cfg(feature = "mavlink-drone")]
        {
            assert!(stacks.iter().any(|stack| stack.name == "mavlink-drone-sim"));
            assert!(stacks
                .iter()
                .any(|stack| stack.name == "mavlink-drone-replay"));
            assert!(stacks
                .iter()
                .any(|stack| stack.name == "mavlink-drone-http"));
        }

        for stack in stacks {
            stack.validate().unwrap();
        }
    }

    #[test]
    fn stack_adapter_profiles_form_the_hardware_matrix() {
        let stacks = built_in_stacks();
        let physical = stacks
            .iter()
            .find(|stack| stack.name == "waveshare-ugv-http")
            .unwrap();
        assert_eq!(physical.adapter.category, AdapterCategory::MobileBase);
        assert_eq!(physical.adapter.maturity, AdapterMaturity::Alpha);
        assert!(physical
            .adapter
            .feature_flags
            .contains(&"waveshare-ugv".to_string()));
        assert!(physical
            .adapter
            .required_gates
            .contains(&"physical-actuation".to_string()));
        assert!(physical.adapter.capabilities.contains(&"drive".to_string()));

        let sim = adapter_profile_for_profile(Profile::Sim);
        assert_eq!(sim.category, AdapterCategory::Simulation);
        assert!(sim.required_gates.is_empty());

        #[cfg(feature = "mavlink-drone")]
        {
            let drone = stacks
                .iter()
                .find(|stack| stack.name == "mavlink-drone-http")
                .unwrap();
            assert_eq!(drone.adapter.category, AdapterCategory::Drone);
            assert_eq!(drone.adapter.maturity, AdapterMaturity::Experimental);
            assert!(drone
                .adapter
                .capabilities
                .contains(&"drone_arm".to_string()));
            assert!(drone
                .adapter
                .required_gates
                .contains(&"physical-actuation".to_string()));

            let drone_sim = stacks
                .iter()
                .find(|stack| stack.name == "mavlink-drone-sim")
                .unwrap();
            assert_eq!(drone_sim.adapter.category, AdapterCategory::Drone);
            assert!(!drone_sim.hardware_required);
            assert!(drone_sim.adapter.required_gates.is_empty());
        }
    }

    #[test]
    fn unknown_stack_reports_known_names() {
        let err = resolve_stack("missing").unwrap_err().to_string();
        assert!(err.contains("unknown stack 'missing'"));
        assert!(err.contains("sim-http"));
    }

    #[test]
    fn physical_stack_resolves_before_refusing_without_gate() {
        let stack = resolve_stack("waveshare-ugv-http").unwrap();
        let config = resolve_config(ConfigRequest {
            config_path: None,
            stack: Some(stack.profile),
            stack_defaults: stack.config_overrides,
            env: BTreeMap::new(),
            cli: PartialHarnessConfig::default(),
        })
        .unwrap()
        .config;

        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("physical profile 'waveshare-ugv' refuses to start"));
    }

    #[cfg(feature = "mavlink-drone")]
    #[test]
    fn mavlink_drone_stack_resolves_before_refusing_without_gate() {
        let stack = resolve_stack("mavlink-drone-http").unwrap();
        let config = resolve_config(ConfigRequest {
            config_path: None,
            stack: Some(stack.profile),
            stack_defaults: stack.config_overrides,
            env: BTreeMap::new(),
            cli: PartialHarnessConfig::default(),
        })
        .unwrap()
        .config;

        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("physical profile 'mavlink-drone' refuses to start"));
        assert_eq!(config.mavlink_endpoint, "udp://127.0.0.1:14550");
    }
}
