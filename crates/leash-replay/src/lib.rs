//! Deterministic, I/O-free replay for the Leash control kernel.

#![forbid(unsafe_code)]

use std::fmt;

use leash_core::{
    ControlEffect, ControlInput, ControlKernel, ControlKernelConfig, Controller, DifferentialDrive,
    DurationNanos, KernelError, MonotonicNanos, NormalizedDrive, OperatorId, ProducerEpoch,
    Sequence, StopReason, Tick,
};
use serde::{Deserialize, Serialize};

pub const REPLAY_SCHEMA_VERSION: &str = "leash.control-replay.v1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayScenario {
    pub schema_version: String,
    pub config: ReplayKernelConfig,
    pub events: Vec<ReplayEvent>,
    pub expected: ReplayExpectation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayKernelConfig {
    pub command_epoch: u64,
    pub evidence_epoch: u64,
    pub deadman_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayEvent {
    pub at_ns: u64,
    pub sequence: u64,
    pub input: ReplayInput,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReplayInput {
    Idle,
    Authorize {
        operator: String,
        expires_at_ns: u64,
    },
    Drive {
        left: f64,
        right: f64,
        deadline_ns: u64,
    },
    Stop {
        reason: ReplayStopReason,
    },
    EStop,
    ResetEStop {
        approved: bool,
    },
    UpdateEvidence {
        obstacle_blocked: bool,
        lidar_fresh: bool,
        localization_fresh: bool,
    },
    SetPlannerActive {
        active: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayStopReason {
    Operator,
    PlannerCancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayExpectation {
    pub final_state_digest: u64,
    pub event_digest: u64,
    pub effect_counts: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayTransition {
    pub at: MonotonicNanos,
    pub sequence: Sequence,
    pub effect_digests: Box<[u64]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayReport {
    pub final_state_digest: u64,
    pub event_digest: u64,
    pub transitions: Box<[ReplayTransition]>,
}

impl ReplayReport {
    pub fn effect_counts(&self) -> Vec<usize> {
        self.transitions
            .iter()
            .map(|transition| transition.effect_digests.len())
            .collect()
    }
}

#[derive(Debug)]
pub enum ReplayError {
    Json(serde_json::Error),
    UnsupportedSchema(String),
    EmptyScenario,
    InvalidEpoch(&'static str),
    InvalidSequence { event: usize },
    InvalidOperator { event: usize },
    InvalidDrive { event: usize },
    Kernel { event: usize, source: KernelError },
}

impl fmt::Display for ReplayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "decode replay scenario: {error}"),
            Self::UnsupportedSchema(version) => {
                write!(formatter, "unsupported replay schema {version}")
            }
            Self::EmptyScenario => formatter.write_str("replay scenario has no events"),
            Self::InvalidEpoch(name) => write!(formatter, "invalid replay {name}"),
            Self::InvalidSequence { event } => {
                write!(formatter, "invalid replay sequence at event {event}")
            }
            Self::InvalidOperator { event } => {
                write!(formatter, "invalid replay operator at event {event}")
            }
            Self::InvalidDrive { event } => {
                write!(formatter, "invalid replay drive at event {event}")
            }
            Self::Kernel { event, source } => {
                write!(formatter, "kernel rejected replay event {event}: {source}")
            }
        }
    }
}

impl std::error::Error for ReplayError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::Kernel { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub enum VerificationError {
    Replay(ReplayError),
    FinalStateDigest {
        expected: u64,
        actual: u64,
    },
    EventDigest {
        expected: u64,
        actual: u64,
    },
    EffectCounts {
        expected: Vec<usize>,
        actual: Vec<usize>,
    },
}

impl fmt::Display for VerificationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "replay oracle mismatch: {self:?}")
    }
}

impl std::error::Error for VerificationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Replay(error) => Some(error),
            _ => None,
        }
    }
}

impl ReplayScenario {
    pub fn from_json(bytes: &[u8]) -> Result<Self, ReplayError> {
        serde_json::from_slice(bytes).map_err(ReplayError::Json)
    }

    pub fn run(&self) -> Result<ReplayReport, ReplayError> {
        if self.schema_version != REPLAY_SCHEMA_VERSION {
            return Err(ReplayError::UnsupportedSchema(self.schema_version.clone()));
        }
        if self.events.is_empty() {
            return Err(ReplayError::EmptyScenario);
        }
        let command_epoch = ProducerEpoch::new(self.config.command_epoch)
            .map_err(|_| ReplayError::InvalidEpoch("command epoch"))?;
        let evidence_epoch = ProducerEpoch::new(self.config.evidence_epoch)
            .map_err(|_| ReplayError::InvalidEpoch("evidence epoch"))?;
        let mut kernel = ControlKernel::new(ControlKernelConfig {
            command_epoch,
            evidence_epoch,
            deadman: DurationNanos::new(self.config.deadman_ns),
        });
        let mut digest = StableDigest::new();
        let mut transitions = Vec::with_capacity(self.events.len());
        for (index, event) in self.events.iter().enumerate() {
            let sequence = Sequence::new(event.sequence)
                .map_err(|_| ReplayError::InvalidSequence { event: index })?;
            let input = event.input.to_domain(index)?;
            let effects = kernel
                .step(Tick::new(MonotonicNanos::new(event.at_ns), sequence, input))
                .map_err(|source| ReplayError::Kernel {
                    event: index,
                    source,
                })?;
            digest.u64(event.at_ns);
            digest.u64(effects.len() as u64);
            let effect_digests = effects
                .iter()
                .map(ControlEffect::stable_digest)
                .inspect(|effect_digest| digest.u64(*effect_digest))
                .collect::<Vec<_>>()
                .into_boxed_slice();
            transitions.push(ReplayTransition {
                at: MonotonicNanos::new(event.at_ns),
                sequence,
                effect_digests,
            });
        }
        Ok(ReplayReport {
            final_state_digest: kernel.state_digest(),
            event_digest: digest.finish(),
            transitions: transitions.into_boxed_slice(),
        })
    }

    pub fn verify(&self) -> Result<ReplayReport, VerificationError> {
        let report = self.run().map_err(VerificationError::Replay)?;
        if report.final_state_digest != self.expected.final_state_digest {
            return Err(VerificationError::FinalStateDigest {
                expected: self.expected.final_state_digest,
                actual: report.final_state_digest,
            });
        }
        if report.event_digest != self.expected.event_digest {
            return Err(VerificationError::EventDigest {
                expected: self.expected.event_digest,
                actual: report.event_digest,
            });
        }
        let actual = report.effect_counts();
        if actual != self.expected.effect_counts {
            return Err(VerificationError::EffectCounts {
                expected: self.expected.effect_counts.clone(),
                actual,
            });
        }
        Ok(report)
    }
}

impl ReplayInput {
    fn to_domain(&self, event: usize) -> Result<ControlInput, ReplayError> {
        match self {
            Self::Idle => Ok(ControlInput::Idle),
            Self::Authorize {
                operator,
                expires_at_ns,
            } => Ok(ControlInput::Authorize {
                operator: OperatorId::new(operator.clone())
                    .map_err(|_| ReplayError::InvalidOperator { event })?,
                expires_at: MonotonicNanos::new(*expires_at_ns),
            }),
            Self::Drive {
                left,
                right,
                deadline_ns,
            } => {
                let left =
                    NormalizedDrive::new(*left).map_err(|_| ReplayError::InvalidDrive { event })?;
                let right = NormalizedDrive::new(*right)
                    .map_err(|_| ReplayError::InvalidDrive { event })?;
                Ok(ControlInput::Drive {
                    command: DifferentialDrive::new(left, right),
                    deadline: MonotonicNanos::new(*deadline_ns),
                })
            }
            Self::Stop { reason } => Ok(ControlInput::Stop {
                reason: match reason {
                    ReplayStopReason::Operator => StopReason::Operator,
                    ReplayStopReason::PlannerCancelled => StopReason::PlannerCancelled,
                },
            }),
            Self::EStop => Ok(ControlInput::EStop),
            Self::ResetEStop { approved } => Ok(ControlInput::ResetEStop {
                approved: *approved,
            }),
            Self::UpdateEvidence {
                obstacle_blocked,
                lidar_fresh,
                localization_fresh,
            } => Ok(ControlInput::UpdateEvidence {
                obstacle_blocked: *obstacle_blocked,
                lidar_fresh: *lidar_fresh,
                localization_fresh: *localization_fresh,
            }),
            Self::SetPlannerActive { active } => Ok(ControlInput::SetPlannerActive(*active)),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct StableDigest(u64);

impl StableDigest {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x00000100000001b3;

    const fn new() -> Self {
        Self(Self::OFFSET)
    }

    fn u64(&mut self, value: u64) {
        for byte in value.to_le_bytes() {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    const fn finish(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAFETY_SCENARIO: &[u8] = include_bytes!("../fixtures/control-safety-v1.json");

    #[test]
    fn checked_in_safety_scenario_matches_the_cross_architecture_oracle() {
        let scenario = ReplayScenario::from_json(SAFETY_SCENARIO).unwrap();
        let first = scenario.verify().unwrap();
        let second = scenario.verify().unwrap();
        assert_eq!(first, second);
        assert_eq!(first.final_state_digest, 14_161_983_491_101_435_343);
        assert_eq!(first.event_digest, 922_937_285_294_098_901);
    }

    #[test]
    fn malformed_inputs_fail_before_or_at_the_exact_event() {
        let mut scenario = ReplayScenario::from_json(SAFETY_SCENARIO).unwrap();
        scenario.events[1].sequence = 0;
        assert!(matches!(
            scenario.run(),
            Err(ReplayError::InvalidSequence { event: 1 })
        ));

        let mut scenario = ReplayScenario::from_json(SAFETY_SCENARIO).unwrap();
        scenario.events[3].input = ReplayInput::Drive {
            left: 2.0,
            right: 0.0,
            deadline_ns: 20_000_000,
        };
        assert!(matches!(
            scenario.run(),
            Err(ReplayError::InvalidDrive { event: 3 })
        ));
    }

    #[test]
    fn oracle_mismatch_reports_the_specific_proof_that_changed() {
        let mut scenario = ReplayScenario::from_json(SAFETY_SCENARIO).unwrap();
        scenario.expected.event_digest = 1;
        assert!(matches!(
            scenario.verify(),
            Err(VerificationError::EventDigest { .. })
        ));
    }

    #[test]
    fn strict_json_rejects_unknown_fields() {
        let json = br#"{
            "schema_version":"leash.control-replay.v1",
            "config":{"command_epoch":1,"evidence_epoch":2,"deadman_ns":3,"surprise":true},
            "events":[],
            "expected":{"final_state_digest":0,"event_digest":0,"effect_counts":[]}
        }"#;
        assert!(matches!(
            ReplayScenario::from_json(json),
            Err(ReplayError::Json(_))
        ));
    }
}
