//! Typed compatibility gateway shared by the legacy HTTP, MCP, and CLI edges.
//!
//! The legacy runtime remains the sole owner of its configured robot driver.
//! This facade deliberately delegates through the existing capability policy
//! boundary, so switching transports cannot bypass dry-run, approval, session,
//! or physical-actuation gates.

use anyhow::{anyhow, Result};
use leash_gateway::{CommandService, QueryService};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    capability::InvocationOrigin,
    module::ModuleGraph,
    runtime::Harness,
    types::{
        Capabilities, CaptureResult, DriveOutcome, Health, OperatorTokenStatus, SpeedMode,
        TelemetryFrame,
    },
};

#[derive(Debug, Clone, PartialEq)]
pub enum GatewayCommand {
    Authorize {
        token: String,
        ttl_secs: Option<u64>,
        speed_mode: Option<SpeedMode>,
    },
    Drive {
        token: Option<String>,
        left: f64,
        right: f64,
        speed_mode: Option<SpeedMode>,
        approval: Option<bool>,
    },
    SetSpeedMode {
        token: Option<String>,
        speed_mode: SpeedMode,
    },
    Stop,
    EStop,
    ResetEStop {
        token: Option<String>,
        approval: Option<bool>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthorizationResponse {
    pub ok: bool,
    pub ttl_secs: u64,
    pub speed_mode: SpeedMode,
    pub operator_token: OperatorTokenStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EStopResponse {
    pub ok: bool,
    pub estop: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeedModeResponse {
    pub ok: bool,
    pub speed_mode: SpeedMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DryRunResponse {
    pub ok: bool,
    pub dry_run: bool,
    pub capability: String,
    pub safety: String,
    pub origin: String,
    pub policy_mode: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GatewayCommandResponse {
    Authorization(AuthorizationResponse),
    Drive(DriveOutcome),
    EStop(EStopResponse),
    SpeedMode(SpeedModeResponse),
    DryRun(DryRunResponse),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayQuery {
    Health,
    Capabilities,
    Modules,
    Observe,
    Capture,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GatewayQueryResponse {
    Health(Health),
    Capabilities(Capabilities),
    Modules(ModuleGraph),
    Observe(Box<TelemetryFrame>),
    Capture(CaptureResult),
}

/// One cloneable application service used by every legacy transport adapter.
#[derive(Clone)]
pub struct TransportGateway {
    harness: Harness,
    origin: InvocationOrigin,
}

impl TransportGateway {
    pub fn new(harness: Harness, origin: InvocationOrigin) -> Self {
        Self { harness, origin }
    }

    pub fn execute(&self, command: GatewayCommand) -> Result<GatewayCommandResponse> {
        <Self as CommandService>::execute(self, command)
    }

    pub fn query(&self, query: GatewayQuery) -> Result<GatewayQueryResponse> {
        <Self as QueryService>::query(self, query)
    }

    pub fn harness(&self) -> &Harness {
        &self.harness
    }

    pub fn health(&self) -> Health {
        self.harness.health()
    }

    pub fn capabilities(&self) -> Capabilities {
        self.harness.capabilities()
    }

    pub fn modules(&self) -> ModuleGraph {
        self.harness.module_graph()
    }

    pub fn observe(&self) -> TelemetryFrame {
        self.harness.telemetry()
    }

    pub fn capture(&self) -> CaptureResult {
        self.harness.capture()
    }
}

impl CommandService for TransportGateway {
    type Request = GatewayCommand;
    type Response = GatewayCommandResponse;
    type Error = anyhow::Error;

    fn execute(&self, command: Self::Request) -> Result<Self::Response> {
        let (capability, args) = match command {
            GatewayCommand::Authorize {
                token,
                ttl_secs,
                speed_mode,
            } => (
                "authorize",
                json!({
                    "token": token,
                    "ttl_secs": ttl_secs,
                    "speed_mode": speed_mode,
                }),
            ),
            GatewayCommand::Drive {
                token,
                left,
                right,
                speed_mode,
                approval,
            } => (
                "drive",
                json!({
                    "token": token,
                    "left": left,
                    "right": right,
                    "speed_mode": speed_mode,
                    "approval": approval,
                }),
            ),
            GatewayCommand::SetSpeedMode { token, speed_mode } => (
                "speed_mode",
                json!({ "token": token, "speed_mode": speed_mode }),
            ),
            GatewayCommand::Stop => ("stop", json!({})),
            GatewayCommand::EStop => ("estop", json!({})),
            GatewayCommand::ResetEStop { token, approval } => (
                "estop_reset",
                json!({ "token": token, "approval": approval }),
            ),
        };
        let value = self
            .harness
            .capability_registry()
            .invoke_value_with_origin(capability, args, self.origin)?;
        if value.get("dry_run").and_then(serde_json::Value::as_bool) == Some(true) {
            return serde_json::from_value(value)
                .map(GatewayCommandResponse::DryRun)
                .map_err(Into::into);
        }
        match capability {
            "authorize" => serde_json::from_value(value)
                .map(GatewayCommandResponse::Authorization)
                .map_err(Into::into),
            "drive" | "stop" => serde_json::from_value(value)
                .map(GatewayCommandResponse::Drive)
                .map_err(Into::into),
            "estop" | "estop_reset" => serde_json::from_value(value)
                .map(GatewayCommandResponse::EStop)
                .map_err(Into::into),
            "speed_mode" => serde_json::from_value(value)
                .map(GatewayCommandResponse::SpeedMode)
                .map_err(Into::into),
            _ => Err(anyhow!("unsupported typed gateway command")),
        }
    }
}

impl QueryService for TransportGateway {
    type Request = GatewayQuery;
    type Response = GatewayQueryResponse;
    type Error = anyhow::Error;

    fn query(&self, query: Self::Request) -> Result<Self::Response> {
        Ok(match query {
            GatewayQuery::Health => GatewayQueryResponse::Health(self.harness.health()),
            GatewayQuery::Capabilities => {
                GatewayQueryResponse::Capabilities(self.harness.capabilities())
            }
            GatewayQuery::Modules => GatewayQueryResponse::Modules(self.harness.module_graph()),
            GatewayQuery::Observe => {
                GatewayQueryResponse::Observe(Box::new(self.harness.telemetry()))
            }
            GatewayQuery::Capture => GatewayQueryResponse::Capture(self.harness.capture()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::PolicyMode, HarnessConfig};

    fn gateway() -> TransportGateway {
        TransportGateway::new(
            Harness::new(HarnessConfig::default()).unwrap(),
            InvocationOrigin::Runtime,
        )
    }

    #[tokio::test]
    async fn typed_commands_preserve_legacy_response_shapes() {
        let gateway = gateway();
        let response = gateway.execute(GatewayCommand::Stop).unwrap();
        let GatewayCommandResponse::Drive(response) = response else {
            panic!("stop must preserve DriveOutcome")
        };
        assert_eq!(response.left, 0.0);
        assert_eq!(response.right, 0.0);

        let response = gateway.execute(GatewayCommand::EStop).unwrap();
        assert_eq!(
            response,
            GatewayCommandResponse::EStop(EStopResponse {
                ok: true,
                estop: true,
            })
        );
    }

    #[tokio::test]
    async fn typed_queries_return_concrete_contract_types() {
        let gateway = gateway();
        assert!(matches!(
            gateway.query(GatewayQuery::Health).unwrap(),
            GatewayQueryResponse::Health(Health { ok: true, .. })
        ));
        assert!(matches!(
            gateway.query(GatewayQuery::Modules).unwrap(),
            GatewayQueryResponse::Modules(_)
        ));
    }

    #[tokio::test]
    async fn typed_commands_preserve_policy_dry_run_response() {
        let config = HarnessConfig {
            policy_mode: PolicyMode::DryRun,
            ..HarnessConfig::default()
        };
        let gateway = TransportGateway::new(Harness::new(config).unwrap(), InvocationOrigin::Http);
        let response = gateway
            .execute(GatewayCommand::Drive {
                token: None,
                left: 0.2,
                right: 0.2,
                speed_mode: None,
                approval: None,
            })
            .unwrap();
        assert!(matches!(
            response,
            GatewayCommandResponse::DryRun(DryRunResponse {
                ok: true,
                dry_run: true,
                ..
            })
        ));
    }
}
