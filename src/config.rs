use std::{net::SocketAddr, str::FromStr};

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

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
