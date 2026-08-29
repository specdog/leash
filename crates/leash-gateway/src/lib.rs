//! Typed transport-edge conversion for Leash runtime v2.

#![forbid(unsafe_code)]

use std::{fmt, time::Duration};

use leash_core::{
    ActuationReason, ControlDenial, ControlEffect, ControlInput, DifferentialDrive, MonotonicNanos,
    NormalizedDrive, OperatorId, SafetyDenial, SafetyState,
};
use leash_runtime::{SupervisorHandle, SupervisorSubmitError};
use serde::{Deserialize, Serialize};

pub const GATEWAY_SCHEMA_VERSION: &str = "leash.gateway-command.v1";

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommandRequest {
    Authorize {
        operator: String,
        expires_at_ns: u64,
    },
    Drive {
        left: f64,
        right: f64,
        deadline_ns: u64,
    },
    Stop {},
    EStop {},
    ResetEStop {
        approved: bool,
    },
    SetPlannerActive {
        active: bool,
    },
    CancelPlanner {},
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct IdDto {
    pub producer_epoch: u64,
    pub sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "effect", rename_all = "snake_case")]
pub enum EffectDto {
    OperatorAuthorized {
        expires_at_ns: u64,
    },
    Actuate {
        reason: &'static str,
        command_id: IdDto,
        evidence_id: IdDto,
        authorized_at_ns: u64,
        left: f64,
        right: f64,
    },
    Denied {
        command_id: IdDto,
        reason: &'static str,
    },
    PlannerChanged {
        active: bool,
    },
    SafetyChanged {
        state: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CommandResponse {
    SafetyAccepted {
        schema_version: &'static str,
        kind: &'static str,
        request_sequence: u64,
    },
    Transitioned {
        schema_version: &'static str,
        proposal_sequence: u64,
        processed_at_ns: u64,
        effects: Vec<EffectDto>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayError {
    InvalidJson(Box<str>),
    InvalidDomain(Box<str>),
    Proposal(SupervisorSubmitError),
    Safety(Box<str>),
    Timeout,
    Supervisor(Box<str>),
    Encode(Box<str>),
}

impl fmt::Display for GatewayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(error) => write!(formatter, "invalid gateway JSON: {error}"),
            Self::InvalidDomain(error) => write!(formatter, "invalid domain command: {error}"),
            Self::Proposal(error) => write!(formatter, "submit domain command: {error}"),
            Self::Safety(error) => write!(formatter, "submit safety command: {error}"),
            Self::Timeout => formatter.write_str("domain transition timed out"),
            Self::Supervisor(error) => write!(formatter, "domain transition failed: {error}"),
            Self::Encode(error) => write!(formatter, "encode gateway response: {error}"),
        }
    }
}

impl std::error::Error for GatewayError {}

pub trait CommandService {
    type Request;
    type Response;
    type Error;

    fn execute(&self, request: Self::Request) -> Result<Self::Response, Self::Error>;
}

/// Transport-neutral query half of a gateway service.
///
/// The associated types let compatibility surfaces retain their frozen wire
/// contracts without introducing HTTP, MCP, CLI, or implementation-crate
/// dependencies here.
pub trait QueryService {
    type Request;
    type Response;
    type Error;

    fn query(&self, request: Self::Request) -> Result<Self::Response, Self::Error>;
}

#[derive(Clone)]
pub struct TypedCommandService {
    supervisor: SupervisorHandle,
    transition_timeout: Duration,
}

impl TypedCommandService {
    pub fn new(
        supervisor: SupervisorHandle,
        transition_timeout: Duration,
    ) -> Result<Self, GatewayError> {
        if transition_timeout.is_zero() {
            return Err(GatewayError::InvalidDomain(
                "transition timeout must be positive".into(),
            ));
        }
        Ok(Self {
            supervisor,
            transition_timeout,
        })
    }

    pub fn decode_and_execute(&self, json: &[u8]) -> Result<Vec<u8>, GatewayError> {
        let request = serde_json::from_slice(json)
            .map_err(|error| GatewayError::InvalidJson(error.to_string().into_boxed_str()))?;
        let response = self.execute(request)?;
        serde_json::to_vec(&response)
            .map_err(|error| GatewayError::Encode(error.to_string().into_boxed_str()))
    }

    fn transition(&self, input: ControlInput) -> Result<CommandResponse, GatewayError> {
        let ticket = self
            .supervisor
            .submit(input)
            .map_err(GatewayError::Proposal)?;
        let Some(result) = ticket
            .wait_timeout(self.transition_timeout)
            .map_err(GatewayError::Proposal)?
        else {
            return Err(GatewayError::Timeout);
        };
        let receipt = result.map_err(GatewayError::Supervisor)?;
        Ok(CommandResponse::Transitioned {
            schema_version: GATEWAY_SCHEMA_VERSION,
            proposal_sequence: receipt.proposal_sequence,
            processed_at_ns: receipt.processed_at.get(),
            effects: receipt.effects.iter().map(effect_dto).collect(),
        })
    }
}

impl CommandService for TypedCommandService {
    type Request = CommandRequest;
    type Response = CommandResponse;
    type Error = GatewayError;

    fn execute(&self, request: Self::Request) -> Result<Self::Response, Self::Error> {
        match request {
            CommandRequest::Authorize {
                operator,
                expires_at_ns,
            } => self.transition(ControlInput::Authorize {
                operator: OperatorId::new(operator)
                    .map_err(|error| GatewayError::InvalidDomain(error.to_string().into()))?,
                expires_at: MonotonicNanos::new(expires_at_ns),
            }),
            CommandRequest::Drive {
                left,
                right,
                deadline_ns,
            } => {
                let left = NormalizedDrive::new(left)
                    .map_err(|error| GatewayError::InvalidDomain(error.to_string().into()))?;
                let right = NormalizedDrive::new(right)
                    .map_err(|error| GatewayError::InvalidDomain(error.to_string().into()))?;
                self.transition(ControlInput::Drive {
                    command: DifferentialDrive::new(left, right),
                    deadline: MonotonicNanos::new(deadline_ns),
                })
            }
            CommandRequest::Stop {} => {
                let request_sequence = self
                    .supervisor
                    .stop()
                    .map_err(|error| GatewayError::Safety(error.to_string().into()))?;
                Ok(CommandResponse::SafetyAccepted {
                    schema_version: GATEWAY_SCHEMA_VERSION,
                    kind: "stop",
                    request_sequence,
                })
            }
            CommandRequest::EStop {} => {
                let request_sequence = self
                    .supervisor
                    .estop()
                    .map_err(|error| GatewayError::Safety(error.to_string().into()))?;
                Ok(CommandResponse::SafetyAccepted {
                    schema_version: GATEWAY_SCHEMA_VERSION,
                    kind: "estop",
                    request_sequence,
                })
            }
            CommandRequest::ResetEStop { approved } => {
                self.transition(ControlInput::ResetEStop { approved })
            }
            CommandRequest::SetPlannerActive { active } => {
                self.transition(ControlInput::SetPlannerActive(active))
            }
            CommandRequest::CancelPlanner {} => {
                self.transition(ControlInput::SetPlannerActive(false))
            }
        }
    }
}

fn effect_dto(effect: &ControlEffect) -> EffectDto {
    match effect {
        ControlEffect::OperatorAuthorized { expires_at } => EffectDto::OperatorAuthorized {
            expires_at_ns: expires_at.get(),
        },
        ControlEffect::Actuate { reason, command } => EffectDto::Actuate {
            reason: actuation_reason(*reason),
            command_id: IdDto {
                producer_epoch: command.command_id().producer_epoch.get(),
                sequence: command.command_id().sequence.get(),
            },
            evidence_id: IdDto {
                producer_epoch: command.evidence_id().producer_epoch.get(),
                sequence: command.evidence_id().sequence.get(),
            },
            authorized_at_ns: command.authorized_at().get(),
            left: command.command().left.get(),
            right: command.command().right.get(),
        },
        ControlEffect::Denied { command_id, reason } => EffectDto::Denied {
            command_id: IdDto {
                producer_epoch: command_id.producer_epoch.get(),
                sequence: command_id.sequence.get(),
            },
            reason: control_denial(*reason),
        },
        ControlEffect::PlannerChanged { active } => EffectDto::PlannerChanged { active: *active },
        ControlEffect::SafetyChanged { state } => EffectDto::SafetyChanged {
            state: safety_state(*state),
        },
    }
}

const fn actuation_reason(reason: ActuationReason) -> &'static str {
    match reason {
        ActuationReason::DriveAccepted => "drive_accepted",
        ActuationReason::OperatorStop => "operator_stop",
        ActuationReason::PlannerCancelled => "planner_cancelled",
        ActuationReason::Deadman => "deadman",
        ActuationReason::Obstacle => "obstacle",
        ActuationReason::StaleEvidence => "stale_evidence",
        ActuationReason::LeaseExpired => "lease_expired",
        ActuationReason::EStop => "estop",
    }
}

const fn control_denial(reason: ControlDenial) -> &'static str {
    match reason {
        ControlDenial::NoOperatorLease => "no_operator_lease",
        ControlDenial::LeaseExpired => "lease_expired",
        ControlDenial::ObstacleBlocked => "obstacle_blocked",
        ControlDenial::LidarStale => "lidar_stale",
        ControlDenial::LocalizationStale => "localization_stale",
        ControlDenial::EStopped => "estopped",
        ControlDenial::ResetRequiresApproval => "reset_requires_approval",
        ControlDenial::Safety(denial) => safety_denial(denial),
    }
}

const fn safety_denial(reason: SafetyDenial) -> &'static str {
    match reason {
        SafetyDenial::Disarmed => "safety_disarmed",
        SafetyDenial::EStopped => "safety_estopped",
        SafetyDenial::Faulted => "safety_faulted",
        SafetyDenial::Expired => "safety_expired",
        SafetyDenial::IssuedInFuture => "safety_issued_in_future",
        SafetyDenial::SequenceExhausted => "safety_sequence_exhausted",
    }
}

const fn safety_state(state: SafetyState) -> &'static str {
    match state {
        SafetyState::Disarmed => "disarmed",
        SafetyState::Ready => "ready",
        SafetyState::Moving => "moving",
        SafetyState::EStopped => "estopped",
        SafetyState::Faulted => "faulted",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use leash_core::{
        Authorized, Clock, ControlKernel, ControlKernelConfig, DurationNanos, ProducerEpoch,
    };
    use leash_runtime::{
        ActuationAcknowledgement, ActuationPort, CpuSafetySupervisor, SafetyKind, SupervisorConfig,
    };

    use super::*;

    struct TestClock(u64);

    impl Clock for TestClock {
        fn now(&mut self) -> MonotonicNanos {
            self.0 += 1_000_000;
            MonotonicNanos::new(self.0)
        }
    }

    #[derive(Debug)]
    struct TestAck;

    impl ActuationAcknowledgement for TestAck {
        fn applied(&self) -> bool {
            true
        }

        fn verified_zero(&self) -> bool {
            false
        }
    }

    #[derive(Clone, Default)]
    struct TestPort {
        safety: Arc<Mutex<Vec<SafetyKind>>>,
    }

    impl ActuationPort for TestPort {
        type Acknowledgement = TestAck;
        type Error = &'static str;

        fn submit_drive(
            &mut self,
            _command: Authorized<DifferentialDrive>,
        ) -> Result<(), Self::Error> {
            Ok(())
        }

        fn request_safety(&mut self, kind: SafetyKind) -> Result<u64, Self::Error> {
            let mut safety = self.safety.lock().unwrap();
            safety.push(kind);
            Ok(safety.len() as u64)
        }

        fn try_acknowledgement(&mut self) -> Result<Option<Self::Acknowledgement>, Self::Error> {
            Ok(None)
        }
    }

    fn service() -> (TypedCommandService, TestPort, CpuSafetySupervisor<TestAck>) {
        let port = TestPort::default();
        let supervisor = CpuSafetySupervisor::spawn(
            ControlKernel::new(ControlKernelConfig {
                command_epoch: ProducerEpoch::new(61).unwrap(),
                evidence_epoch: ProducerEpoch::new(62).unwrap(),
                deadman: DurationNanos::from_millis(50).unwrap(),
            }),
            port.clone(),
            Box::new(TestClock(0)),
            SupervisorConfig {
                proposal_capacity: 4,
                tick_period: Duration::from_millis(1),
            },
        )
        .unwrap();
        let service =
            TypedCommandService::new(supervisor.handle(), Duration::from_millis(100)).unwrap();
        (service, port, supervisor)
    }

    #[test]
    fn json_edge_rejects_unknown_and_unvalidated_drive_fields() {
        let (service, _port, _supervisor) = service();
        assert!(matches!(
            service.decode_and_execute(
                br#"{"command":"drive","left":2.0,"right":0.0,"deadline_ns":1000000000}"#,
            ),
            Err(GatewayError::InvalidDomain(_))
        ));
        assert!(matches!(
            service.decode_and_execute(br#"{"command":"stop","surprise":true}"#),
            Err(GatewayError::InvalidJson(_))
        ));
    }

    #[test]
    fn all_normal_surfaces_share_the_same_typed_transition_service() {
        let (service, _port, _supervisor) = service();
        let response = service
            .execute(CommandRequest::Authorize {
                operator: "operator-a".to_string(),
                expires_at_ns: 1_000_000_000,
            })
            .unwrap();
        let CommandResponse::Transitioned { effects, .. } = response else {
            panic!("expected transition response")
        };
        assert!(matches!(effects[0], EffectDto::OperatorAuthorized { .. }));
    }

    #[test]
    fn stop_and_estop_bypass_normal_transition_waiting() {
        let (service, port, _supervisor) = service();
        let stop = service.execute(CommandRequest::Stop {}).unwrap();
        let estop = service.execute(CommandRequest::EStop {}).unwrap();
        assert!(matches!(
            stop,
            CommandResponse::SafetyAccepted { kind: "stop", .. }
        ));
        assert!(matches!(
            estop,
            CommandResponse::SafetyAccepted { kind: "estop", .. }
        ));
        for _ in 0..100 {
            if port.safety.lock().unwrap().contains(&SafetyKind::EStop) {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(port.safety.lock().unwrap().contains(&SafetyKind::EStop));
    }

    #[test]
    fn planner_cancellation_cannot_remove_an_accepted_safety_request() {
        let (service, port, _supervisor) = service();
        service.execute(CommandRequest::Stop {}).unwrap();
        service.execute(CommandRequest::EStop {}).unwrap();
        let _ = service.execute(CommandRequest::CancelPlanner {});
        for _ in 0..100 {
            let safety = port.safety.lock().unwrap();
            if safety.contains(&SafetyKind::Stop) && safety.contains(&SafetyKind::EStop) {
                break;
            }
            drop(safety);
            std::thread::sleep(Duration::from_millis(1));
        }
        let safety = port.safety.lock().unwrap();
        assert!(safety.contains(&SafetyKind::Stop));
        assert!(safety.contains(&SafetyKind::EStop));
    }
}
