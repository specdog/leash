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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct Stack {
    pub name: String,
    pub description: String,
    pub profile: Profile,
    pub transport: TransportBinding,
    pub required_features: Vec<String>,
    pub hardware_required: bool,
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
    vec![
        sim_http_stack(),
        sim_mcp_stack(),
        bridge_compat_http_stack(),
        waveshare_ugv_http_stack(),
    ]
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

fn module_refs(profile: Profile) -> Vec<StackModule> {
    let driver = match profile {
        Profile::Sim => ("sim-driver", false),
        Profile::WaveshareUgv => ("waveshare-ugv-driver", true),
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

        for stack in stacks {
            stack.validate().unwrap();
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
}
