//! Camera-only authority does not create or refresh a pilot/wheel lease.
use anyhow::{anyhow, bail, Result};
use serde::Serialize;
use std::{
    io::Read,
    time::{Duration, Instant},
};

pub const DEFAULT_SPEED: u32 = 8;
pub const DEFAULT_ACCEL: u32 = 4;
pub const MAX_SPEED: u32 = 40;
pub const MAX_ACCEL: u32 = 20;

#[derive(Clone, Serialize)]
pub struct HeadAuthorityStatus {
    pub enabled: bool,
    pub token_configured: bool,
    pub grants_wheel_authority: bool,
    pub physical_feedback: &'static str,
    pub min_request_interval_ms: u64,
    pub last_attempt_at_ms: Option<u128>,
    pub last_error: Option<String>,
}

pub(crate) struct HeadAuthority {
    enabled: bool,
    token: Option<String>,
    last_attempt: Option<Instant>,
    last_attempt_at_ms: Option<u128>,
    last_error: Option<String>,
}

impl HeadAuthority {
    pub(crate) fn from_env(physical_gate: bool) -> Result<Self> {
        let requested = match std::env::var("LEASH_ALLOW_CAMERA_AIM") {
            Err(std::env::VarError::NotPresent) => false,
            Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "on" => true,
                "0" | "false" | "off" | "" => false,
                _ => bail!("LEASH_ALLOW_CAMERA_AIM must be true or false"),
            },
            Err(_) => bail!("LEASH_ALLOW_CAMERA_AIM is not valid text"),
        };
        let path = std::env::var_os("LEASH_CAMERA_AIM_AUTH_TOKEN_FILE")
            .or_else(|| std::env::var_os("LEASH_OPERATOR_AUTH_TOKEN_FILE"));
        let token = if requested {
            path.map(|path| {
                let file = std::fs::File::open(path)
                    .map_err(|_| anyhow!("camera authority file is unreadable"))?;
                let mut bytes = Vec::new();
                file.take(4097)
                    .read_to_end(&mut bytes)
                    .map_err(|_| anyhow!("camera authority file is unreadable"))?;
                if bytes.len() > 4096 {
                    bail!("camera authority file exceeds 4096 bytes");
                }
                let token = String::from_utf8(bytes)
                    .map_err(|_| anyhow!("camera authority is not UTF-8"))?;
                let token = token.trim().to_owned();
                if token.is_empty() {
                    bail!("camera authority is empty");
                }
                Ok(token)
            })
            .transpose()?
        } else {
            None
        };
        Ok(Self {
            enabled: requested && physical_gate,
            token,
            last_attempt: None,
            last_attempt_at_ms: None,
            last_error: None,
        })
    }

    pub(crate) fn accepts(&self, supplied: Option<&str>) -> bool {
        self.enabled
            && self
                .token
                .as_deref()
                .zip(supplied)
                .is_some_and(|(expected, supplied)| {
                    expected.len() == supplied.len()
                        && expected
                            .bytes()
                            .zip(supplied.bytes())
                            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
                            == 0
                })
    }

    pub(crate) fn begin_attempt(&mut self, now_ms: u128) -> Result<()> {
        if self
            .last_attempt
            .is_some_and(|at| at.elapsed() < Duration::from_millis(250))
        {
            bail!("camera aim requests must be at least 250 ms apart");
        }
        self.last_attempt = Some(Instant::now());
        self.last_attempt_at_ms = Some(now_ms);
        self.last_error = None;
        Ok(())
    }

    pub(crate) fn failed(&mut self, error: &anyhow::Error) {
        self.last_error = Some(error.to_string());
    }

    pub(crate) fn status(&self) -> HeadAuthorityStatus {
        HeadAuthorityStatus {
            enabled: self.enabled,
            token_configured: self.token.is_some(),
            grants_wheel_authority: false,
            physical_feedback: "unknown",
            min_request_interval_ms: 250,
            last_attempt_at_ms: self.last_attempt_at_ms,
            last_error: self.last_error.clone(),
        }
    }
}
