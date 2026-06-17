use std::{collections::BTreeMap, env, fs, net::SocketAddr, path::PathBuf, str::FromStr};

use anyhow::Context;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const DEFAULT_LISTEN: &str = "127.0.0.1:8000";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    Sim,
    WaveshareUgv,
}

impl Profile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sim => "sim",
            Self::WaveshareUgv => "waveshare-ugv",
        }
    }

    pub fn is_physical(self) -> bool {
        matches!(self, Self::WaveshareUgv)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct HarnessConfig {
    pub role: String,
    pub profile: Profile,
    pub listen: SocketAddr,
    pub allow_untokened_drive: bool,
    pub allow_physical_actuation: bool,
    pub deadman_ms: u64,
    pub soft_odometry_limit_m: f64,
    pub serial_port: String,
    pub serial_baud: u32,
    pub drive_invert: bool,
    pub drive_swap: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartialHarnessConfig {
    pub role: Option<String>,
    pub profile: Option<Profile>,
    pub listen: Option<SocketAddr>,
    pub allow_untokened_drive: Option<bool>,
    pub allow_physical_actuation: Option<bool>,
    pub deadman_ms: Option<u64>,
    pub soft_odometry_limit_m: Option<f64>,
    pub serial_port: Option<String>,
    pub serial_baud: Option<u32>,
    pub drive_invert: Option<bool>,
    pub drive_swap: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct ConfigRequest {
    pub config_path: Option<PathBuf>,
    pub blueprint: Option<Profile>,
    pub env: BTreeMap<String, String>,
    pub cli: PartialHarnessConfig,
}

impl ConfigRequest {
    pub fn from_process(
        config_path: Option<PathBuf>,
        blueprint: Option<Profile>,
        cli: PartialHarnessConfig,
    ) -> Self {
        Self {
            config_path,
            blueprint,
            env: env::vars()
                .filter(|(key, _)| key.starts_with("LEASH_"))
                .collect(),
            cli,
        }
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
    listen: Resolved<SocketAddr>,
    allow_untokened_drive: Resolved<bool>,
    allow_physical_actuation: Resolved<bool>,
    deadman_ms: Resolved<u64>,
    soft_odometry_limit_m: Resolved<f64>,
    serial_port: Resolved<String>,
    serial_baud: Resolved<u32>,
    drive_invert: Resolved<bool>,
    drive_swap: Resolved<bool>,
    config_file: Option<String>,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            role: "robot".to_string(),
            profile: Profile::Sim,
            listen: SocketAddr::from_str(DEFAULT_LISTEN).expect("valid default listen address"),
            allow_untokened_drive: true,
            allow_physical_actuation: false,
            deadman_ms: 400,
            soft_odometry_limit_m: 0.0,
            serial_port: "/dev/ttyTHS1".to_string(),
            serial_baud: 115_200,
            drive_invert: false,
            drive_swap: false,
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

    if let Some(profile) = request.blueprint {
        builder.apply_partial(
            PartialHarnessConfig {
                profile: Some(profile),
                ..PartialHarnessConfig::default()
            },
            |_| format!("blueprint:{}", profile.as_str()),
        );
    }

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
        listen: parse_env(env, "LEASH_LISTEN", parse_socket_addr)?,
        allow_untokened_drive: parse_env(env, "LEASH_ALLOW_UNTOKENED_DRIVE", parse_bool)?,
        allow_physical_actuation: parse_env(env, "LEASH_ALLOW_PHYSICAL_ACTUATION", parse_bool)?,
        deadman_ms: parse_env(env, "LEASH_DEADMAN_MS", parse_u64)?,
        soft_odometry_limit_m: parse_env(env, "LEASH_SOFT_ODOMETRY_LIMIT_M", parse_f64)?,
        serial_port: env.get("LEASH_SERIAL_PORT").cloned(),
        serial_baud: parse_env(env, "LEASH_SERIAL_BAUD", parse_u32)?,
        drive_invert: parse_env(env, "LEASH_DRIVE_INVERT", parse_bool)?,
        drive_swap: parse_env(env, "LEASH_DRIVE_SWAP", parse_bool)?,
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
        "waveshare-ugv" => Ok(Profile::WaveshareUgv),
        _ => anyhow::bail!("expected sim or waveshare-ugv"),
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
        "listen" => "LEASH_LISTEN",
        "allow_untokened_drive" => "LEASH_ALLOW_UNTOKENED_DRIVE",
        "allow_physical_actuation" => "LEASH_ALLOW_PHYSICAL_ACTUATION",
        "deadman_ms" => "LEASH_DEADMAN_MS",
        "soft_odometry_limit_m" => "LEASH_SOFT_ODOMETRY_LIMIT_M",
        "serial_port" => "LEASH_SERIAL_PORT",
        "serial_baud" => "LEASH_SERIAL_BAUD",
        "drive_invert" => "LEASH_DRIVE_INVERT",
        "drive_swap" => "LEASH_DRIVE_SWAP",
        _ => "LEASH_UNKNOWN",
    }
}

impl Default for ConfigBuilder {
    fn default() -> Self {
        let config = HarnessConfig::default();
        Self {
            role: Resolved::defaulted(config.role),
            profile: Resolved::defaulted(config.profile),
            listen: Resolved::defaulted(config.listen),
            allow_untokened_drive: Resolved::defaulted(config.allow_untokened_drive),
            allow_physical_actuation: Resolved::defaulted(config.allow_physical_actuation),
            deadman_ms: Resolved::defaulted(config.deadman_ms),
            soft_odometry_limit_m: Resolved::defaulted(config.soft_odometry_limit_m),
            serial_port: Resolved::defaulted(config.serial_port),
            serial_baud: Resolved::defaulted(config.serial_baud),
            drive_invert: Resolved::defaulted(config.drive_invert),
            drive_swap: Resolved::defaulted(config.drive_swap),
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
    }

    fn apply_profile_defaults(&mut self) {
        if self.profile.value == Profile::WaveshareUgv
            && self.allow_untokened_drive.source == "default"
        {
            self.allow_untokened_drive
                .set(false, "blueprint:waveshare-ugv".to_string());
        }
    }

    fn finish(self) -> ResolvedHarnessConfig {
        let config = HarnessConfig {
            role: self.role.value,
            profile: self.profile.value,
            listen: self.listen.value,
            allow_untokened_drive: self.allow_untokened_drive.value,
            allow_physical_actuation: self.allow_physical_actuation.value,
            deadman_ms: self.deadman_ms.value,
            soft_odometry_limit_m: self.soft_odometry_limit_m.value,
            serial_port: self.serial_port.value,
            serial_baud: self.serial_baud.value,
            drive_invert: self.drive_invert.value,
            drive_swap: self.drive_swap.value,
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
                "blueprint-default",
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
    let name = name.to_ascii_lowercase();
    let sensitive = ["token", "secret", "password", "key"]
        .iter()
        .any(|needle| name.contains(needle));
    if sensitive && !value.is_null() {
        json!("<redacted>")
    } else {
        value
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
            blueprint: None,
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
            blueprint: Some(Profile::WaveshareUgv),
            env: BTreeMap::new(),
            cli: PartialHarnessConfig::default(),
        })
        .unwrap();

        assert_eq!(resolved.config.profile, Profile::WaveshareUgv);
        assert!(!resolved.config.allow_untokened_drive);
        assert!(!resolved.config.allow_physical_actuation);
        assert_eq!(
            source_for(&resolved, "allow_untokened_drive"),
            "blueprint:waveshare-ugv"
        );
        assert_eq!(
            attention_for(&resolved, "allow_physical_actuation"),
            Some("physical-actuation")
        );
    }

    #[test]
    fn redacts_sensitive_field_names() {
        assert_eq!(
            redact_value("pilot_token", json!("abc")),
            json!("<redacted>")
        );
        assert_eq!(redact_value("api_key", json!("abc")), json!("<redacted>"));
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
