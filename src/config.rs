use std::{collections::BTreeMap, env, fs, net::SocketAddr, path::PathBuf, str::FromStr};

use anyhow::Context;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::transport::StreamTransportBackend;

const DEFAULT_LISTEN: &str = "127.0.0.1:8000";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    Sim,
    Replay,
    WaveshareUgv,
    MavlinkDrone,
}

impl Profile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sim => "sim",
            Self::Replay => "replay",
            Self::WaveshareUgv => "waveshare-ugv",
            Self::MavlinkDrone => "mavlink-drone",
        }
    }

    pub fn is_physical(self) -> bool {
        matches!(self, Self::WaveshareUgv | Self::MavlinkDrone)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum AcceleratorBackend {
    #[default]
    None,
    Cpu,
    Cuda,
}

impl AcceleratorBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Cpu => "cpu",
            Self::Cuda => "cuda",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum AgentProvider {
    #[default]
    DeterministicTest,
    #[serde(rename = "openai-compatible-http")]
    #[value(name = "openai-compatible-http")]
    OpenAiCompatibleHttp,
    LocalHttp,
}

impl AgentProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DeterministicTest => "deterministic-test",
            Self::OpenAiCompatibleHttp => "openai-compatible-http",
            Self::LocalHttp => "local-http",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum PolicyMode {
    DryRun,
    #[default]
    RequireToken,
    RequireApproval,
    Deny,
}

impl PolicyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DryRun => "dry-run",
            Self::RequireToken => "require-token",
            Self::RequireApproval => "require-approval",
            Self::Deny => "deny",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct HarnessConfig {
    pub role: String,
    pub profile: Profile,
    pub stream_transport: StreamTransportBackend,
    pub replay_source: Option<PathBuf>,
    pub replay_speed: f64,
    pub listen: SocketAddr,
    pub allow_untokened_drive: bool,
    pub allow_physical_actuation: bool,
    pub deadman_ms: u64,
    pub soft_odometry_limit_m: f64,
    pub serial_port: String,
    pub serial_baud: u32,
    pub drive_invert: bool,
    pub drive_swap: bool,
    pub mavlink_endpoint: String,
    pub accelerator: AcceleratorBackend,
    pub require_accelerator: bool,
    pub resource_sampling: bool,
    pub agent_provider: AgentProvider,
    pub agent_model: String,
    pub agent_base_url: Option<String>,
    #[serde(skip_serializing)]
    pub agent_api_key: Option<String>,
    pub agent_timeout_ms: u64,
    pub policy_mode: PolicyMode,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PartialHarnessConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<Profile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_transport: Option<StreamTransportBackend>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replay_source: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replay_speed: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listen: Option<SocketAddr>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_untokened_drive: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_physical_actuation: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadman_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub soft_odometry_limit_m: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial_port: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serial_baud: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drive_invert: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drive_swap: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mavlink_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accelerator: Option<AcceleratorBackend>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_accelerator: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_sampling: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_provider: Option<AgentProvider>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_mode: Option<PolicyMode>,
}

#[derive(Debug, Clone)]
pub struct ConfigRequest {
    pub config_path: Option<PathBuf>,
    pub stack: Option<Profile>,
    pub stack_defaults: PartialHarnessConfig,
    pub env: BTreeMap<String, String>,
    pub cli: PartialHarnessConfig,
}

impl ConfigRequest {
    pub fn from_process(
        config_path: Option<PathBuf>,
        stack: Option<Profile>,
        cli: PartialHarnessConfig,
    ) -> Self {
        Self {
            config_path,
            stack,
            stack_defaults: PartialHarnessConfig::default(),
            env: env::vars()
                .filter(|(key, _)| key.starts_with("LEASH_"))
                .collect(),
            cli,
        }
    }

    pub fn with_stack_defaults(mut self, defaults: PartialHarnessConfig) -> Self {
        self.stack_defaults = defaults;
        self
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolvedHarnessConfig {
    #[serde(flatten)]
    pub config: HarnessConfig,
    pub physical: bool,
    pub physical_actuation_enabled: bool,
    pub network_bind: String,
    pub config_file: Option<String>,
    pub precedence: Vec<&'static str>,
    pub fields: Vec<ResolvedConfigField>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResolvedConfigField {
    pub name: &'static str,
    pub value: Value,
    pub source: String,
    pub attention: Option<&'static str>,
}

#[derive(Debug, Clone)]
struct Resolved<T> {
    value: T,
    source: String,
}

#[derive(Debug, Clone)]
struct ConfigBuilder {
    role: Resolved<String>,
    profile: Resolved<Profile>,
    stream_transport: Resolved<StreamTransportBackend>,
    replay_source: Resolved<Option<PathBuf>>,
    replay_speed: Resolved<f64>,
    listen: Resolved<SocketAddr>,
    allow_untokened_drive: Resolved<bool>,
    allow_physical_actuation: Resolved<bool>,
    deadman_ms: Resolved<u64>,
    soft_odometry_limit_m: Resolved<f64>,
    serial_port: Resolved<String>,
    serial_baud: Resolved<u32>,
    drive_invert: Resolved<bool>,
    drive_swap: Resolved<bool>,
    mavlink_endpoint: Resolved<String>,
    accelerator: Resolved<AcceleratorBackend>,
    require_accelerator: Resolved<bool>,
    resource_sampling: Resolved<bool>,
    agent_provider: Resolved<AgentProvider>,
    agent_model: Resolved<String>,
    agent_base_url: Resolved<Option<String>>,
    agent_api_key: Resolved<Option<String>>,
    agent_timeout_ms: Resolved<u64>,
    policy_mode: Resolved<PolicyMode>,
    config_file: Option<String>,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            role: "robot".to_string(),
            profile: Profile::Sim,
            stream_transport: StreamTransportBackend::LocalPubsub,
            replay_source: None,
            replay_speed: 1.0,
            listen: SocketAddr::from_str(DEFAULT_LISTEN).expect("valid default listen address"),
            allow_untokened_drive: true,
            allow_physical_actuation: false,
            deadman_ms: 400,
            soft_odometry_limit_m: 0.0,
            serial_port: "/dev/ttyTHS1".to_string(),
            serial_baud: 115_200,
            drive_invert: false,
            drive_swap: false,
            mavlink_endpoint: "udp://127.0.0.1:14550".to_string(),
            accelerator: AcceleratorBackend::None,
            require_accelerator: false,
            resource_sampling: false,
            agent_provider: AgentProvider::DeterministicTest,
            agent_model: "deterministic-test".to_string(),
            agent_base_url: None,
            agent_api_key: None,
            agent_timeout_ms: 10_000,
            policy_mode: PolicyMode::RequireToken,
        }
    }
}

impl HarnessConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.profile.is_physical()
            && !self.allow_physical_actuation
            && std::env::var("LEASH_ALLOW_PHYSICAL_ACTUATION")
                .ok()
                .as_deref()
                != Some("1")
        {
            anyhow::bail!(
                "physical profile '{}' refuses to start without LEASH_ALLOW_PHYSICAL_ACTUATION=1 or --allow-physical-actuation",
                self.profile.as_str()
            );
        }
        if self.profile == Profile::Replay && self.replay_source.is_none() {
            anyhow::bail!("profile 'replay' requires --replay-source or LEASH_REPLAY_SOURCE");
        }
        if self.replay_source.is_some() && self.profile != Profile::Replay {
            anyhow::bail!("replay_source requires profile 'replay'");
        }
        if self.profile == Profile::MavlinkDrone && self.mavlink_endpoint.trim().is_empty() {
            anyhow::bail!(
                "profile 'mavlink-drone' requires LEASH_MAVLINK_ENDPOINT or mavlink_endpoint"
            );
        }
        if !self.replay_speed.is_finite() || self.replay_speed <= 0.0 {
            anyhow::bail!("replay_speed must be a finite positive number");
        }
        if self.agent_timeout_ms == 0 {
            anyhow::bail!("agent_timeout_ms must be at least 1");
        }
        match self.agent_provider {
            AgentProvider::DeterministicTest => {}
            AgentProvider::OpenAiCompatibleHttp => {
                require_non_empty(
                    self.agent_base_url.as_deref(),
                    "agent provider openai-compatible-http requires LEASH_AGENT_BASE_URL or agent_base_url",
                )?;
                require_non_empty(
                    self.agent_api_key.as_deref(),
                    "agent provider openai-compatible-http requires LEASH_AGENT_API_KEY or agent_api_key",
                )?;
            }
            AgentProvider::LocalHttp => {
                require_non_empty(
                    self.agent_base_url.as_deref(),
                    "agent provider local-http requires LEASH_AGENT_BASE_URL or agent_base_url",
                )?;
            }
        }
        crate::accelerator::resolve_accelerator(self.accelerator, self.require_accelerator)?;
        Ok(())
    }
}

pub fn resolve_config(request: ConfigRequest) -> anyhow::Result<ResolvedHarnessConfig> {
    let mut builder = ConfigBuilder::default();

    if let Some(path) = resolved_config_path(request.config_path) {
        let config = read_config_file(&path)?;
        let source = format!("config-file:{}", path.display());
        builder.config_file = Some(path.display().to_string());
        builder.apply_partial(config, |_| source.clone());
    }

    if let Some(profile) = request.stack {
        builder.apply_partial(
            PartialHarnessConfig {
                profile: Some(profile),
                ..PartialHarnessConfig::default()
            },
            |_| format!("stack:{}", profile.as_str()),
        );
    }
    builder.apply_partial(request.stack_defaults, |_| "stack-default".to_string());

    builder.apply_profile_defaults();
    builder.apply_partial(env_overrides(&request.env)?, env_source);
    builder.apply_partial(request.cli, |_| "cli".to_string());
    builder.apply_profile_defaults();

    Ok(builder.finish())
}

fn resolved_config_path(explicit: Option<PathBuf>) -> Option<PathBuf> {
    if explicit.is_some() {
        return explicit;
    }
    let home = env::var_os("HOME")?;
    let path = PathBuf::from(home).join(".config/leash/config.json");
    path.exists().then_some(path)
}

fn read_config_file(path: &PathBuf) -> anyhow::Result<PartialHarnessConfig> {
    let text = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

fn env_overrides(env: &BTreeMap<String, String>) -> anyhow::Result<PartialHarnessConfig> {
    Ok(PartialHarnessConfig {
        role: env.get("LEASH_ROLE").cloned(),
        profile: parse_env(env, "LEASH_PROFILE", parse_profile)?,
        stream_transport: parse_env(env, "LEASH_STREAM_TRANSPORT", parse_stream_transport)?,
        replay_source: env.get("LEASH_REPLAY_SOURCE").map(PathBuf::from),
        replay_speed: parse_env(env, "LEASH_REPLAY_SPEED", parse_f64)?,
        listen: parse_env(env, "LEASH_LISTEN", parse_socket_addr)?,
        allow_untokened_drive: parse_env(env, "LEASH_ALLOW_UNTOKENED_DRIVE", parse_bool)?,
        allow_physical_actuation: parse_env(env, "LEASH_ALLOW_PHYSICAL_ACTUATION", parse_bool)?,
        deadman_ms: parse_env(env, "LEASH_DEADMAN_MS", parse_u64)?,
        soft_odometry_limit_m: parse_env(env, "LEASH_SOFT_ODOMETRY_LIMIT_M", parse_f64)?,
        serial_port: env.get("LEASH_SERIAL_PORT").cloned(),
        serial_baud: parse_env(env, "LEASH_SERIAL_BAUD", parse_u32)?,
        drive_invert: parse_env(env, "LEASH_DRIVE_INVERT", parse_bool)?,
        drive_swap: parse_env(env, "LEASH_DRIVE_SWAP", parse_bool)?,
        mavlink_endpoint: env.get("LEASH_MAVLINK_ENDPOINT").cloned(),
        accelerator: parse_env(env, "LEASH_ACCELERATOR", parse_accelerator)?,
        require_accelerator: parse_env(env, "LEASH_REQUIRE_ACCELERATOR", parse_bool)?,
        resource_sampling: parse_env(env, "LEASH_RESOURCE_SAMPLING", parse_bool)?,
        agent_provider: parse_env(env, "LEASH_AGENT_PROVIDER", parse_agent_provider)?,
        agent_model: env.get("LEASH_AGENT_MODEL").cloned(),
        agent_base_url: env.get("LEASH_AGENT_BASE_URL").cloned(),
        agent_api_key: env.get("LEASH_AGENT_API_KEY").cloned(),
        agent_timeout_ms: parse_env(env, "LEASH_AGENT_TIMEOUT_MS", parse_u64)?,
        policy_mode: parse_env(env, "LEASH_POLICY_MODE", parse_policy_mode)?,
    })
}

fn parse_env<T>(
    env: &BTreeMap<String, String>,
    key: &'static str,
    parse: impl FnOnce(&str) -> anyhow::Result<T>,
) -> anyhow::Result<Option<T>> {
    env.get(key)
        .map(|value| parse(value).with_context(|| format!("parse {key}")))
        .transpose()
}

fn parse_profile(value: &str) -> anyhow::Result<Profile> {
    match value {
        "sim" => Ok(Profile::Sim),
        "replay" => Ok(Profile::Replay),
        "waveshare-ugv" => Ok(Profile::WaveshareUgv),
        "mavlink-drone" => Ok(Profile::MavlinkDrone),
        _ => anyhow::bail!("expected sim, replay, waveshare-ugv, or mavlink-drone"),
    }
}

fn parse_accelerator(value: &str) -> anyhow::Result<AcceleratorBackend> {
    match value {
        "none" => Ok(AcceleratorBackend::None),
        "cpu" => Ok(AcceleratorBackend::Cpu),
        "cuda" => Ok(AcceleratorBackend::Cuda),
        _ => anyhow::bail!("expected none, cpu, or cuda"),
    }
}

fn parse_agent_provider(value: &str) -> anyhow::Result<AgentProvider> {
    match value {
        "deterministic-test" => Ok(AgentProvider::DeterministicTest),
        "openai-compatible-http" => Ok(AgentProvider::OpenAiCompatibleHttp),
        "local-http" => Ok(AgentProvider::LocalHttp),
        _ => anyhow::bail!("expected deterministic-test, openai-compatible-http, or local-http"),
    }
}

fn parse_policy_mode(value: &str) -> anyhow::Result<PolicyMode> {
    match value {
        "dry-run" => Ok(PolicyMode::DryRun),
        "require-token" => Ok(PolicyMode::RequireToken),
        "require-approval" => Ok(PolicyMode::RequireApproval),
        "deny" => Ok(PolicyMode::Deny),
        _ => anyhow::bail!("expected dry-run, require-token, require-approval, or deny"),
    }
}

fn parse_stream_transport(value: &str) -> anyhow::Result<StreamTransportBackend> {
    match value {
        "memory" => Ok(StreamTransportBackend::Memory),
        "local-pubsub" => Ok(StreamTransportBackend::LocalPubsub),
        _ => anyhow::bail!("expected memory or local-pubsub"),
    }
}

fn parse_socket_addr(value: &str) -> anyhow::Result<SocketAddr> {
    Ok(SocketAddr::from_str(value)?)
}

fn parse_bool(value: &str) -> anyhow::Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => anyhow::bail!("expected true/false or 1/0"),
    }
}

fn parse_u64(value: &str) -> anyhow::Result<u64> {
    Ok(value.parse()?)
}

fn parse_u32(value: &str) -> anyhow::Result<u32> {
    Ok(value.parse()?)
}

fn parse_f64(value: &str) -> anyhow::Result<f64> {
    Ok(value.parse()?)
}

fn env_source(field: &str) -> String {
    format!("env:{}", env_var_for_field(field))
}

fn env_var_for_field(field: &str) -> &'static str {
    match field {
        "role" => "LEASH_ROLE",
        "profile" => "LEASH_PROFILE",
        "stream_transport" => "LEASH_STREAM_TRANSPORT",
        "replay_source" => "LEASH_REPLAY_SOURCE",
        "replay_speed" => "LEASH_REPLAY_SPEED",
        "listen" => "LEASH_LISTEN",
        "allow_untokened_drive" => "LEASH_ALLOW_UNTOKENED_DRIVE",
        "allow_physical_actuation" => "LEASH_ALLOW_PHYSICAL_ACTUATION",
        "deadman_ms" => "LEASH_DEADMAN_MS",
        "soft_odometry_limit_m" => "LEASH_SOFT_ODOMETRY_LIMIT_M",
        "serial_port" => "LEASH_SERIAL_PORT",
        "serial_baud" => "LEASH_SERIAL_BAUD",
        "drive_invert" => "LEASH_DRIVE_INVERT",
        "drive_swap" => "LEASH_DRIVE_SWAP",
        "mavlink_endpoint" => "LEASH_MAVLINK_ENDPOINT",
        "accelerator" => "LEASH_ACCELERATOR",
        "require_accelerator" => "LEASH_REQUIRE_ACCELERATOR",
        "resource_sampling" => "LEASH_RESOURCE_SAMPLING",
        "agent_provider" => "LEASH_AGENT_PROVIDER",
        "agent_model" => "LEASH_AGENT_MODEL",
        "agent_base_url" => "LEASH_AGENT_BASE_URL",
        "agent_api_key" => "LEASH_AGENT_API_KEY",
        "agent_timeout_ms" => "LEASH_AGENT_TIMEOUT_MS",
        "policy_mode" => "LEASH_POLICY_MODE",
        _ => "LEASH_UNKNOWN",
    }
}

impl Default for ConfigBuilder {
    fn default() -> Self {
        let config = HarnessConfig::default();
        Self {
            role: Resolved::defaulted(config.role),
            profile: Resolved::defaulted(config.profile),
            stream_transport: Resolved::defaulted(config.stream_transport),
            replay_source: Resolved::defaulted(config.replay_source),
            replay_speed: Resolved::defaulted(config.replay_speed),
            listen: Resolved::defaulted(config.listen),
            allow_untokened_drive: Resolved::defaulted(config.allow_untokened_drive),
            allow_physical_actuation: Resolved::defaulted(config.allow_physical_actuation),
            deadman_ms: Resolved::defaulted(config.deadman_ms),
            soft_odometry_limit_m: Resolved::defaulted(config.soft_odometry_limit_m),
            serial_port: Resolved::defaulted(config.serial_port),
            serial_baud: Resolved::defaulted(config.serial_baud),
            drive_invert: Resolved::defaulted(config.drive_invert),
            drive_swap: Resolved::defaulted(config.drive_swap),
            mavlink_endpoint: Resolved::defaulted(config.mavlink_endpoint),
            accelerator: Resolved::defaulted(config.accelerator),
            require_accelerator: Resolved::defaulted(config.require_accelerator),
            resource_sampling: Resolved::defaulted(config.resource_sampling),
            agent_provider: Resolved::defaulted(config.agent_provider),
            agent_model: Resolved::defaulted(config.agent_model),
            agent_base_url: Resolved::defaulted(config.agent_base_url),
            agent_api_key: Resolved::defaulted(config.agent_api_key),
            agent_timeout_ms: Resolved::defaulted(config.agent_timeout_ms),
            policy_mode: Resolved::defaulted(config.policy_mode),
            config_file: None,
        }
    }
}

impl<T> Resolved<T> {
    fn defaulted(value: T) -> Self {
        Self {
            value,
            source: "default".to_string(),
        }
    }

    fn set(&mut self, value: T, source: String) {
        self.value = value;
        self.source = source;
    }
}

impl ConfigBuilder {
    fn apply_partial(
        &mut self,
        partial: PartialHarnessConfig,
        source: impl Fn(&'static str) -> String,
    ) {
        if let Some(value) = partial.role {
            self.role.set(value, source("role"));
        }
        if let Some(value) = partial.profile {
            self.profile.set(value, source("profile"));
        }
        if let Some(value) = partial.stream_transport {
            self.stream_transport.set(value, source("stream_transport"));
        }
        if let Some(value) = partial.replay_source {
            self.replay_source.set(Some(value), source("replay_source"));
        }
        if let Some(value) = partial.replay_speed {
            self.replay_speed.set(value, source("replay_speed"));
        }
        if let Some(value) = partial.listen {
            self.listen.set(value, source("listen"));
        }
        if let Some(value) = partial.allow_untokened_drive {
            self.allow_untokened_drive
                .set(value, source("allow_untokened_drive"));
        }
        if let Some(value) = partial.allow_physical_actuation {
            self.allow_physical_actuation
                .set(value, source("allow_physical_actuation"));
        }
        if let Some(value) = partial.deadman_ms {
            self.deadman_ms.set(value, source("deadman_ms"));
        }
        if let Some(value) = partial.soft_odometry_limit_m {
            self.soft_odometry_limit_m
                .set(value, source("soft_odometry_limit_m"));
        }
        if let Some(value) = partial.serial_port {
            self.serial_port.set(value, source("serial_port"));
        }
        if let Some(value) = partial.serial_baud {
            self.serial_baud.set(value, source("serial_baud"));
        }
        if let Some(value) = partial.drive_invert {
            self.drive_invert.set(value, source("drive_invert"));
        }
        if let Some(value) = partial.drive_swap {
            self.drive_swap.set(value, source("drive_swap"));
        }
        if let Some(value) = partial.mavlink_endpoint {
            self.mavlink_endpoint.set(value, source("mavlink_endpoint"));
        }
        if let Some(value) = partial.accelerator {
            self.accelerator.set(value, source("accelerator"));
        }
        if let Some(value) = partial.require_accelerator {
            self.require_accelerator
                .set(value, source("require_accelerator"));
        }
        if let Some(value) = partial.resource_sampling {
            self.resource_sampling
                .set(value, source("resource_sampling"));
        }
        if let Some(value) = partial.agent_provider {
            self.agent_provider.set(value, source("agent_provider"));
        }
        if let Some(value) = partial.agent_model {
            self.agent_model.set(value, source("agent_model"));
        }
        if let Some(value) = partial.agent_base_url {
            self.agent_base_url
                .set(Some(value), source("agent_base_url"));
        }
        if let Some(value) = partial.agent_api_key {
            self.agent_api_key.set(Some(value), source("agent_api_key"));
        }
        if let Some(value) = partial.agent_timeout_ms {
            self.agent_timeout_ms.set(value, source("agent_timeout_ms"));
        }
        if let Some(value) = partial.policy_mode {
            self.policy_mode.set(value, source("policy_mode"));
        }
    }

    fn apply_profile_defaults(&mut self) {
        if self.replay_source.value.is_some() && self.profile.source == "default" {
            self.profile
                .set(Profile::Replay, "replay-source".to_string());
        }
        if self.profile.value == Profile::WaveshareUgv
            && self.allow_untokened_drive.source == "default"
        {
            self.allow_untokened_drive
                .set(false, "stack:waveshare-ugv".to_string());
        }
        if self.profile.value == Profile::MavlinkDrone
            && self.allow_untokened_drive.source == "default"
        {
            self.allow_untokened_drive
                .set(false, "stack:mavlink-drone".to_string());
        }
    }

    fn finish(self) -> ResolvedHarnessConfig {
        let config = HarnessConfig {
            role: self.role.value,
            profile: self.profile.value,
            stream_transport: self.stream_transport.value,
            replay_source: self.replay_source.value,
            replay_speed: self.replay_speed.value,
            listen: self.listen.value,
            allow_untokened_drive: self.allow_untokened_drive.value,
            allow_physical_actuation: self.allow_physical_actuation.value,
            deadman_ms: self.deadman_ms.value,
            soft_odometry_limit_m: self.soft_odometry_limit_m.value,
            serial_port: self.serial_port.value,
            serial_baud: self.serial_baud.value,
            drive_invert: self.drive_invert.value,
            drive_swap: self.drive_swap.value,
            mavlink_endpoint: self.mavlink_endpoint.value,
            accelerator: self.accelerator.value,
            require_accelerator: self.require_accelerator.value,
            resource_sampling: self.resource_sampling.value,
            agent_provider: self.agent_provider.value,
            agent_model: self.agent_model.value,
            agent_base_url: self.agent_base_url.value,
            agent_api_key: self.agent_api_key.value,
            agent_timeout_ms: self.agent_timeout_ms.value,
            policy_mode: self.policy_mode.value,
        };
        let physical = config.profile.is_physical();
        let physical_actuation_enabled = config.allow_physical_actuation;
        let network_bind = config.listen.to_string();
        let fields = vec![
            field("role", json!(config.role), self.role.source, None),
            field(
                "profile",
                json!(config.profile.as_str()),
                self.profile.source,
                physical.then_some("physical-profile"),
            ),
            field(
                "stream_transport",
                json!(config.stream_transport.as_str()),
                self.stream_transport.source,
                Some("module-streams"),
            ),
            field(
                "replay_source",
                json!(config
                    .replay_source
                    .as_ref()
                    .map(|path| path.display().to_string())),
                self.replay_source.source,
                config.replay_source.is_some().then_some("replay"),
            ),
            field(
                "replay_speed",
                json!(config.replay_speed),
                self.replay_speed.source,
                (config.profile == Profile::Replay).then_some("replay"),
            ),
            field(
                "listen",
                json!(config.listen.to_string()),
                self.listen.source,
                Some("network-bind"),
            ),
            field(
                "allow_untokened_drive",
                json!(config.allow_untokened_drive),
                self.allow_untokened_drive.source,
                Some("drive-auth"),
            ),
            field(
                "allow_physical_actuation",
                json!(config.allow_physical_actuation),
                self.allow_physical_actuation.source,
                Some("physical-actuation"),
            ),
            field(
                "deadman_ms",
                json!(config.deadman_ms),
                self.deadman_ms.source,
                Some("safety"),
            ),
            field(
                "soft_odometry_limit_m",
                json!(config.soft_odometry_limit_m),
                self.soft_odometry_limit_m.source,
                Some("safety"),
            ),
            field(
                "serial_port",
                json!(config.serial_port),
                self.serial_port.source,
                physical.then_some("physical-device"),
            ),
            field(
                "serial_baud",
                json!(config.serial_baud),
                self.serial_baud.source,
                physical.then_some("physical-device"),
            ),
            field(
                "drive_invert",
                json!(config.drive_invert),
                self.drive_invert.source,
                physical.then_some("physical-drive-map"),
            ),
            field(
                "drive_swap",
                json!(config.drive_swap),
                self.drive_swap.source,
                physical.then_some("physical-drive-map"),
            ),
            field(
                "mavlink_endpoint",
                json!(config.mavlink_endpoint),
                self.mavlink_endpoint.source,
                (config.profile == Profile::MavlinkDrone).then_some("mavlink-network"),
            ),
            field(
                "accelerator",
                json!(config.accelerator.as_str()),
                self.accelerator.source,
                (config.accelerator != AcceleratorBackend::None).then_some("accelerator"),
            ),
            field(
                "require_accelerator",
                json!(config.require_accelerator),
                self.require_accelerator.source,
                config.require_accelerator.then_some("accelerator-required"),
            ),
            field(
                "resource_sampling",
                json!(config.resource_sampling),
                self.resource_sampling.source,
                config.resource_sampling.then_some("resource-monitor"),
            ),
            field(
                "agent_provider",
                json!(config.agent_provider.as_str()),
                self.agent_provider.source,
                Some("agent-model"),
            ),
            field(
                "agent_model",
                json!(config.agent_model),
                self.agent_model.source,
                Some("agent-model"),
            ),
            field(
                "agent_base_url",
                json!(config.agent_base_url),
                self.agent_base_url.source,
                matches!(
                    config.agent_provider,
                    AgentProvider::OpenAiCompatibleHttp | AgentProvider::LocalHttp
                )
                .then_some("agent-endpoint"),
            ),
            field(
                "agent_api_key",
                json!(config.agent_api_key),
                self.agent_api_key.source,
                (config.agent_provider == AgentProvider::OpenAiCompatibleHttp).then_some("secret"),
            ),
            field(
                "agent_timeout_ms",
                json!(config.agent_timeout_ms),
                self.agent_timeout_ms.source,
                Some("agent-model"),
            ),
            field(
                "policy_mode",
                json!(config.policy_mode.as_str()),
                self.policy_mode.source,
                Some("safety-policy"),
            ),
        ];

        ResolvedHarnessConfig {
            config,
            physical,
            physical_actuation_enabled,
            network_bind,
            config_file: self.config_file,
            precedence: vec![
                "default",
                "config-file",
                "stack-default",
                "environment",
                "cli",
            ],
            fields,
        }
    }
}

fn field(
    name: &'static str,
    value: Value,
    source: String,
    attention: Option<&'static str>,
) -> ResolvedConfigField {
    ResolvedConfigField {
        name,
        value: redact_value(name, value),
        source,
        attention,
    }
}

fn redact_value(name: &str, value: Value) -> Value {
    if !value.is_string() {
        return value;
    }
    let name = name.to_ascii_lowercase();
    let sensitive = ["token", "secret", "password", "key"]
        .iter()
        .any(|needle| name.contains(needle));
    if sensitive {
        json!("<redacted>")
    } else {
        value
    }
}

fn require_non_empty(value: Option<&str>, message: &'static str) -> anyhow::Result<()> {
    if value.is_some_and(|value| !value.trim().is_empty()) {
        Ok(())
    } else {
        anyhow::bail!(message)
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;

    #[test]
    fn resolves_precedence_with_sources() {
        let config_path = write_temp_config(
            "precedence",
            r#"{"role":"file-bot","deadman_ms":900,"listen":"127.0.0.1:7000"}"#,
        );
        let env = BTreeMap::from([
            ("LEASH_ROLE".to_string(), "env-bot".to_string()),
            ("LEASH_DEADMAN_MS".to_string(), "250".to_string()),
        ]);
        let resolved = resolve_config(ConfigRequest {
            config_path: Some(config_path.clone()),
            stack: None,
            stack_defaults: PartialHarnessConfig::default(),
            env,
            cli: PartialHarnessConfig {
                role: Some("cli-bot".to_string()),
                ..PartialHarnessConfig::default()
            },
        })
        .unwrap();

        assert_eq!(resolved.config.role, "cli-bot");
        assert_eq!(resolved.config.deadman_ms, 250);
        assert_eq!(resolved.config.listen.to_string(), "127.0.0.1:7000");
        assert_eq!(source_for(&resolved, "role"), "cli");
        assert_eq!(source_for(&resolved, "deadman_ms"), "env:LEASH_DEADMAN_MS");
        assert!(source_for(&resolved, "listen").starts_with("config-file:"));
        let _ = fs::remove_file(config_path);
    }

    #[test]
    fn physical_profile_defaults_to_tokened_drive_and_disabled_actuation() {
        let resolved = resolve_config(ConfigRequest {
            config_path: None,
            stack: Some(Profile::WaveshareUgv),
            stack_defaults: PartialHarnessConfig::default(),
            env: BTreeMap::new(),
            cli: PartialHarnessConfig::default(),
        })
        .unwrap();

        assert_eq!(resolved.config.profile, Profile::WaveshareUgv);
        assert!(!resolved.config.allow_untokened_drive);
        assert!(!resolved.config.allow_physical_actuation);
        assert_eq!(
            source_for(&resolved, "allow_untokened_drive"),
            "stack:waveshare-ugv"
        );
        assert_eq!(
            attention_for(&resolved, "allow_physical_actuation"),
            Some("physical-actuation")
        );
    }

    #[test]
    fn resolves_accelerator_precedence_with_sources() {
        let config_path = write_temp_config(
            "accelerator",
            r#"{"accelerator":"cpu","require_accelerator":true}"#,
        );
        let env = BTreeMap::from([("LEASH_ACCELERATOR".to_string(), "cuda".to_string())]);
        let resolved = resolve_config(ConfigRequest {
            config_path: Some(config_path.clone()),
            stack: None,
            stack_defaults: PartialHarnessConfig::default(),
            env,
            cli: PartialHarnessConfig {
                accelerator: Some(AcceleratorBackend::Cpu),
                ..PartialHarnessConfig::default()
            },
        })
        .unwrap();

        assert_eq!(resolved.config.accelerator, AcceleratorBackend::Cpu);
        assert!(resolved.config.require_accelerator);
        assert_eq!(source_for(&resolved, "accelerator"), "cli");
        assert!(source_for(&resolved, "require_accelerator").starts_with("config-file:"));
        assert_eq!(attention_for(&resolved, "accelerator"), Some("accelerator"));
        let _ = fs::remove_file(config_path);
    }

    #[test]
    fn resolves_stream_transport_from_env() {
        let env = BTreeMap::from([("LEASH_STREAM_TRANSPORT".to_string(), "memory".to_string())]);
        let resolved = resolve_config(ConfigRequest {
            config_path: None,
            stack: None,
            stack_defaults: PartialHarnessConfig::default(),
            env,
            cli: PartialHarnessConfig::default(),
        })
        .unwrap();

        assert_eq!(
            resolved.config.stream_transport,
            StreamTransportBackend::Memory
        );
        assert_eq!(
            source_for(&resolved, "stream_transport"),
            "env:LEASH_STREAM_TRANSPORT"
        );
    }

    #[test]
    fn resolves_resource_sampling_from_env_and_cli() {
        let env = BTreeMap::from([("LEASH_RESOURCE_SAMPLING".to_string(), "true".to_string())]);
        let resolved = resolve_config(ConfigRequest {
            config_path: None,
            stack: None,
            stack_defaults: PartialHarnessConfig::default(),
            env,
            cli: PartialHarnessConfig {
                resource_sampling: Some(false),
                ..PartialHarnessConfig::default()
            },
        })
        .unwrap();

        assert!(!resolved.config.resource_sampling);
        assert_eq!(source_for(&resolved, "resource_sampling"), "cli");
        assert_eq!(attention_for(&resolved, "resource_sampling"), None);
    }

    #[test]
    fn resolves_policy_mode_from_env_and_cli() {
        let env = BTreeMap::from([("LEASH_POLICY_MODE".to_string(), "deny".to_string())]);
        let resolved = resolve_config(ConfigRequest {
            config_path: None,
            stack: None,
            stack_defaults: PartialHarnessConfig::default(),
            env,
            cli: PartialHarnessConfig {
                policy_mode: Some(PolicyMode::RequireApproval),
                ..PartialHarnessConfig::default()
            },
        })
        .unwrap();

        assert_eq!(resolved.config.policy_mode, PolicyMode::RequireApproval);
        assert_eq!(source_for(&resolved, "policy_mode"), "cli");
        assert_eq!(
            value_for(&resolved, "policy_mode"),
            json!("require-approval")
        );
        assert_eq!(
            attention_for(&resolved, "policy_mode"),
            Some("safety-policy")
        );
    }

    #[test]
    fn deterministic_agent_provider_is_default_and_no_network() {
        let resolved = resolve_config(ConfigRequest {
            config_path: None,
            stack: None,
            stack_defaults: PartialHarnessConfig::default(),
            env: BTreeMap::new(),
            cli: PartialHarnessConfig::default(),
        })
        .unwrap();

        assert_eq!(
            resolved.config.agent_provider,
            AgentProvider::DeterministicTest
        );
        assert_eq!(resolved.config.agent_model, "deterministic-test");
        assert!(resolved.config.agent_base_url.is_none());
        assert!(resolved.config.agent_api_key.is_none());
        resolved.config.validate().unwrap();
    }

    #[test]
    fn hosted_agent_provider_redacts_secret_in_resolved_output() {
        let env = BTreeMap::from([
            (
                "LEASH_AGENT_PROVIDER".to_string(),
                "openai-compatible-http".to_string(),
            ),
            (
                "LEASH_AGENT_BASE_URL".to_string(),
                "https://example.test/v1".to_string(),
            ),
            (
                "LEASH_AGENT_API_KEY".to_string(),
                "super-secret".to_string(),
            ),
        ]);
        let resolved = resolve_config(ConfigRequest {
            config_path: None,
            stack: None,
            stack_defaults: PartialHarnessConfig::default(),
            env,
            cli: PartialHarnessConfig::default(),
        })
        .unwrap();

        resolved.config.validate().unwrap();
        assert_eq!(
            resolved.config.agent_provider,
            AgentProvider::OpenAiCompatibleHttp
        );
        assert_eq!(value_for(&resolved, "agent_api_key"), json!("<redacted>"));
        assert_eq!(
            source_for(&resolved, "agent_api_key"),
            "env:LEASH_AGENT_API_KEY"
        );
        assert_eq!(attention_for(&resolved, "agent_api_key"), Some("secret"));
        let output = serde_json::to_string(&resolved).unwrap();
        assert!(!output.contains("super-secret"));
    }

    #[test]
    fn hosted_agent_provider_requires_api_key() {
        let env = BTreeMap::from([
            (
                "LEASH_AGENT_PROVIDER".to_string(),
                "openai-compatible-http".to_string(),
            ),
            (
                "LEASH_AGENT_BASE_URL".to_string(),
                "https://example.test/v1".to_string(),
            ),
        ]);
        let resolved = resolve_config(ConfigRequest {
            config_path: None,
            stack: None,
            stack_defaults: PartialHarnessConfig::default(),
            env,
            cli: PartialHarnessConfig::default(),
        })
        .unwrap();

        let err = resolved.config.validate().unwrap_err().to_string();

        assert!(err.contains("LEASH_AGENT_API_KEY"));
    }

    #[test]
    fn local_agent_provider_requires_base_url() {
        let resolved = resolve_config(ConfigRequest {
            config_path: None,
            stack: None,
            stack_defaults: PartialHarnessConfig::default(),
            env: BTreeMap::from([("LEASH_AGENT_PROVIDER".to_string(), "local-http".to_string())]),
            cli: PartialHarnessConfig::default(),
        })
        .unwrap();

        let err = resolved.config.validate().unwrap_err().to_string();

        assert!(err.contains("LEASH_AGENT_BASE_URL"));
    }

    #[test]
    fn replay_source_defaults_profile_to_replay() {
        let env = BTreeMap::from([(
            "LEASH_REPLAY_SOURCE".to_string(),
            "examples/replay/sim-basic.jsonl".to_string(),
        )]);
        let resolved = resolve_config(ConfigRequest {
            config_path: None,
            stack: None,
            stack_defaults: PartialHarnessConfig::default(),
            env,
            cli: PartialHarnessConfig::default(),
        })
        .unwrap();

        assert_eq!(resolved.config.profile, Profile::Replay);
        assert_eq!(
            resolved.config.replay_source,
            Some(PathBuf::from("examples/replay/sim-basic.jsonl"))
        );
        assert_eq!(source_for(&resolved, "profile"), "replay-source");
        assert_eq!(
            source_for(&resolved, "replay_source"),
            "env:LEASH_REPLAY_SOURCE"
        );
        assert_eq!(attention_for(&resolved, "replay_source"), Some("replay"));
    }

    #[test]
    fn rejects_required_accelerator_without_backend() {
        let config = HarnessConfig {
            require_accelerator: true,
            ..HarnessConfig::default()
        };
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("required"));
    }

    #[test]
    fn redacts_sensitive_field_names() {
        assert_eq!(
            redact_value("pilot_token", json!("abc")),
            json!("<redacted>")
        );
        assert_eq!(redact_value("api_key", json!("abc")), json!("<redacted>"));
        assert_eq!(
            redact_value("allow_untokened_drive", json!(true)),
            json!(true)
        );
        assert_eq!(redact_value("role", json!("abc")), json!("abc"));
    }

    fn source_for(resolved: &ResolvedHarnessConfig, name: &str) -> String {
        resolved
            .fields
            .iter()
            .find(|field| field.name == name)
            .unwrap()
            .source
            .clone()
    }

    fn attention_for<'a>(resolved: &'a ResolvedHarnessConfig, name: &str) -> Option<&'a str> {
        resolved
            .fields
            .iter()
            .find(|field| field.name == name)
            .unwrap()
            .attention
    }

    fn value_for(resolved: &ResolvedHarnessConfig, name: &str) -> Value {
        resolved
            .fields
            .iter()
            .find(|field| field.name == name)
            .unwrap()
            .value
            .clone()
    }

    fn write_temp_config(name: &str, body: &str) -> PathBuf {
        let path = env::temp_dir().join(format!(
            "leash-{name}-{}-{}.json",
            std::process::id(),
            crate::runtime::now_ms()
        ));
        fs::write(&path, body).unwrap();
        path
    }
}
