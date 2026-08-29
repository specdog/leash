use core::fmt;

use crate::{
    Authorized, Candidate, CommandId, Controller, DifferentialDrive, DomainError, DurationNanos,
    Effects, MonotonicNanos, ProducerEpoch, SafetyDenial, SafetyGate, SafetyState, Sequence, Tick,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OperatorId(Box<str>);

impl OperatorId {
    pub fn new(value: impl Into<Box<str>>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.is_empty() {
            return Err(DomainError::Empty("operator id"));
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
        {
            return Err(DomainError::InvalidCharacter("operator id"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlKernelConfig {
    pub command_epoch: ProducerEpoch,
    pub evidence_epoch: ProducerEpoch,
    pub deadman: DurationNanos,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ControlInput {
    Idle,
    Authorize {
        operator: OperatorId,
        expires_at: MonotonicNanos,
    },
    Drive {
        command: DifferentialDrive,
        deadline: MonotonicNanos,
    },
    Stop {
        reason: StopReason,
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
    SetPlannerActive(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Operator,
    PlannerCancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActuationReason {
    DriveAccepted,
    OperatorStop,
    PlannerCancelled,
    Deadman,
    Obstacle,
    StaleEvidence,
    LeaseExpired,
    EStop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlDenial {
    NoOperatorLease,
    LeaseExpired,
    ObstacleBlocked,
    LidarStale,
    LocalizationStale,
    EStopped,
    ResetRequiresApproval,
    Safety(SafetyDenial),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ControlEffect {
    OperatorAuthorized {
        expires_at: MonotonicNanos,
    },
    Actuate {
        reason: ActuationReason,
        command: Authorized<DifferentialDrive>,
    },
    Denied {
        command_id: CommandId,
        reason: ControlDenial,
    },
    PlannerChanged {
        active: bool,
    },
    SafetyChanged {
        state: SafetyState,
    },
}

impl ControlEffect {
    pub fn stable_digest(&self) -> u64 {
        let mut digest = StableDigest::new();
        match self {
            Self::OperatorAuthorized { expires_at } => {
                digest.u8(0);
                digest.u64(expires_at.get());
            }
            Self::Actuate { reason, command } => {
                digest.u8(1);
                digest.u8(actuation_reason_tag(*reason));
                digest.command_id(command.command_id());
                digest.u64(command.evidence_id().producer_epoch.get());
                digest.u64(command.evidence_id().sequence.get());
                digest.u64(command.authorized_at().get());
                digest.u64(command.command().left.get().to_bits());
                digest.u64(command.command().right.get().to_bits());
            }
            Self::Denied { command_id, reason } => {
                digest.u8(2);
                digest.command_id(*command_id);
                digest.u8(control_denial_tag(*reason));
            }
            Self::PlannerChanged { active } => {
                digest.u8(3);
                digest.bool(*active);
            }
            Self::SafetyChanged { state } => {
                digest.u8(4);
                digest.u8(safety_tag(*state));
            }
        }
        digest.finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelError {
    TimeReversed,
    SequenceNotIncreasing,
    CommandSequenceExhausted,
    EffectCapacityExceeded,
}

impl fmt::Display for KernelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TimeReversed => formatter.write_str("control tick time moved backwards"),
            Self::SequenceNotIncreasing => {
                formatter.write_str("control tick sequence did not increase")
            }
            Self::CommandSequenceExhausted => {
                formatter.write_str("control command sequence exhausted")
            }
            Self::EffectCapacityExceeded => formatter.write_str("control effect capacity exceeded"),
        }
    }
}

impl std::error::Error for KernelError {}

#[derive(Debug, Clone)]
struct OperatorLease {
    operator: OperatorId,
    expires_at: MonotonicNanos,
}

#[derive(Debug, Clone)]
pub struct ControlKernel {
    config: ControlKernelConfig,
    gate: SafetyGate,
    last_tick: MonotonicNanos,
    last_tick_sequence: Option<Sequence>,
    next_command: Sequence,
    operator: Option<OperatorLease>,
    applied: DifferentialDrive,
    last_nonzero_drive_at: Option<MonotonicNanos>,
    obstacle_blocked: bool,
    lidar_fresh: bool,
    localization_fresh: bool,
    planner_active: bool,
}

impl ControlKernel {
    pub fn new(config: ControlKernelConfig) -> Self {
        Self {
            gate: SafetyGate::new(config.evidence_epoch),
            config,
            last_tick: MonotonicNanos::ZERO,
            last_tick_sequence: None,
            next_command: Sequence::new(1).expect("one is non-zero"),
            operator: None,
            applied: DifferentialDrive::STOP,
            last_nonzero_drive_at: None,
            obstacle_blocked: false,
            lidar_fresh: false,
            localization_fresh: false,
            planner_active: false,
        }
    }

    pub const fn safety_state(&self) -> SafetyState {
        self.gate.state()
    }

    pub const fn applied_drive(&self) -> DifferentialDrive {
        self.applied
    }

    pub const fn planner_active(&self) -> bool {
        self.planner_active
    }

    pub fn operator(&self) -> Option<&OperatorId> {
        self.operator.as_ref().map(|lease| &lease.operator)
    }

    pub fn state_digest(&self) -> u64 {
        let mut digest = StableDigest::new();
        digest.u64(self.config.command_epoch.get());
        digest.u64(self.gate.evidence_epoch().get());
        digest.u64(self.config.deadman.get());
        digest.u8(safety_tag(self.gate.state()));
        digest.u64(self.last_tick.get());
        digest.u64(self.last_tick_sequence.map_or(0, Sequence::get));
        digest.u64(self.next_command.get());
        digest.u64(self.gate.next_evidence_sequence().get());
        digest.bool(self.obstacle_blocked);
        digest.bool(self.lidar_fresh);
        digest.bool(self.localization_fresh);
        digest.bool(self.planner_active);
        digest.u64(self.applied.left.get().to_bits());
        digest.u64(self.applied.right.get().to_bits());
        match self.last_nonzero_drive_at {
            Some(at) => {
                digest.bool(true);
                digest.u64(at.get());
            }
            None => digest.bool(false),
        }
        match &self.operator {
            Some(lease) => {
                digest.bool(true);
                digest.bytes(lease.operator.as_str().as_bytes());
                digest.u64(lease.expires_at.get());
            }
            None => digest.bool(false),
        }
        digest.finish()
    }

    fn command_id(&mut self) -> Result<CommandId, KernelError> {
        let id = CommandId::new(self.config.command_epoch, self.next_command);
        self.next_command = self
            .next_command
            .next()
            .map_err(|_| KernelError::CommandSequenceExhausted)?;
        Ok(id)
    }

    fn push(
        effects: &mut Effects<ControlEffect>,
        effect: ControlEffect,
    ) -> Result<(), KernelError> {
        effects
            .push(effect)
            .map_err(|_| KernelError::EffectCapacityExceeded)
    }

    fn denial_for_safety(error: SafetyDenial) -> ControlDenial {
        match error {
            SafetyDenial::EStopped => ControlDenial::EStopped,
            other => ControlDenial::Safety(other),
        }
    }

    fn issue_stop(
        &mut self,
        at: MonotonicNanos,
        reason: ActuationReason,
        effects: &mut Effects<ControlEffect>,
    ) -> Result<(), KernelError> {
        let command_id = self.command_id()?;
        let command = self
            .gate
            .authorize_stop(command_id, at)
            .map_err(|_| KernelError::CommandSequenceExhausted)?;
        self.applied = DifferentialDrive::STOP;
        self.last_nonzero_drive_at = None;
        Self::push(effects, ControlEffect::Actuate { reason, command })?;
        if self.planner_active {
            self.planner_active = false;
            Self::push(effects, ControlEffect::PlannerChanged { active: false })?;
        }
        Ok(())
    }

    fn expire_authority(
        &mut self,
        at: MonotonicNanos,
        effects: &mut Effects<ControlEffect>,
    ) -> Result<(), KernelError> {
        let lease_expired = self
            .operator
            .as_ref()
            .is_some_and(|lease| at > lease.expires_at);
        if lease_expired {
            self.operator = None;
            self.gate.set_state(SafetyState::Disarmed);
            if !self.applied.is_stop() || self.planner_active {
                self.issue_stop(at, ActuationReason::LeaseExpired, effects)?;
            }
            Self::push(
                effects,
                ControlEffect::SafetyChanged {
                    state: SafetyState::Disarmed,
                },
            )?;
        }
        Ok(())
    }

    fn enforce_deadman(
        &mut self,
        at: MonotonicNanos,
        effects: &mut Effects<ControlEffect>,
    ) -> Result<(), KernelError> {
        let expired = self.last_nonzero_drive_at.is_some_and(|last| {
            at.duration_since(last)
                .is_ok_and(|elapsed| elapsed > self.config.deadman)
        });
        if expired {
            self.issue_stop(at, ActuationReason::Deadman, effects)?;
            if self.operator.is_some() {
                self.gate.set_state(SafetyState::Ready);
            }
        }
        Ok(())
    }

    fn evidence_stop_reason(&self) -> Option<ActuationReason> {
        if self.obstacle_blocked {
            Some(ActuationReason::Obstacle)
        } else if !self.lidar_fresh || self.planner_active && !self.localization_fresh {
            Some(ActuationReason::StaleEvidence)
        } else {
            None
        }
    }

    fn drive_denial(
        &self,
        at: MonotonicNanos,
        require_localization: bool,
    ) -> Option<ControlDenial> {
        match &self.operator {
            None => return Some(ControlDenial::NoOperatorLease),
            Some(lease) if at > lease.expires_at => return Some(ControlDenial::LeaseExpired),
            Some(_) => {}
        }
        if self.obstacle_blocked {
            return Some(ControlDenial::ObstacleBlocked);
        }
        if !self.lidar_fresh {
            return Some(ControlDenial::LidarStale);
        }
        if require_localization && !self.localization_fresh {
            return Some(ControlDenial::LocalizationStale);
        }
        None
    }

    fn process(
        &mut self,
        at: MonotonicNanos,
        input: ControlInput,
        effects: &mut Effects<ControlEffect>,
    ) -> Result<(), KernelError> {
        match input {
            ControlInput::Idle => {}
            ControlInput::Authorize {
                operator,
                expires_at,
            } => {
                if self.gate.state() == SafetyState::EStopped {
                    let command_id = self.command_id()?;
                    Self::push(
                        effects,
                        ControlEffect::Denied {
                            command_id,
                            reason: ControlDenial::EStopped,
                        },
                    )?;
                    return Ok(());
                }
                if expires_at < at {
                    let command_id = self.command_id()?;
                    Self::push(
                        effects,
                        ControlEffect::Denied {
                            command_id,
                            reason: ControlDenial::LeaseExpired,
                        },
                    )?;
                    return Ok(());
                }
                self.operator = Some(OperatorLease {
                    operator,
                    expires_at,
                });
                self.gate.set_state(SafetyState::Ready);
                Self::push(effects, ControlEffect::OperatorAuthorized { expires_at })?;
                Self::push(
                    effects,
                    ControlEffect::SafetyChanged {
                        state: self.gate.state(),
                    },
                )?;
            }
            ControlInput::Drive { command, deadline } => {
                let command_id = self.command_id()?;
                if let Some(reason) = self.drive_denial(at, false) {
                    Self::push(effects, ControlEffect::Denied { command_id, reason })?;
                    return Ok(());
                }
                let candidate = Candidate::new(command_id, at, deadline, command);
                match self.gate.authorize(candidate, at) {
                    Ok(command) => {
                        self.applied = *command.command();
                        if self.applied.is_stop() {
                            self.last_nonzero_drive_at = None;
                            self.gate.set_state(SafetyState::Ready);
                        } else {
                            self.last_nonzero_drive_at = Some(at);
                            self.gate.set_state(SafetyState::Moving);
                        }
                        Self::push(
                            effects,
                            ControlEffect::Actuate {
                                reason: ActuationReason::DriveAccepted,
                                command,
                            },
                        )?;
                    }
                    Err(error) => Self::push(
                        effects,
                        ControlEffect::Denied {
                            command_id,
                            reason: Self::denial_for_safety(error),
                        },
                    )?,
                }
            }
            ControlInput::Stop { reason } => {
                let reason = match reason {
                    StopReason::Operator => ActuationReason::OperatorStop,
                    StopReason::PlannerCancelled => ActuationReason::PlannerCancelled,
                };
                self.issue_stop(at, reason, effects)?;
                if self.operator.is_some() && self.gate.state() != SafetyState::EStopped {
                    self.gate.set_state(SafetyState::Ready);
                }
            }
            ControlInput::EStop => {
                self.operator = None;
                self.gate.set_state(SafetyState::EStopped);
                self.issue_stop(at, ActuationReason::EStop, effects)?;
                Self::push(
                    effects,
                    ControlEffect::SafetyChanged {
                        state: SafetyState::EStopped,
                    },
                )?;
            }
            ControlInput::ResetEStop { approved } => {
                if !approved {
                    let command_id = self.command_id()?;
                    Self::push(
                        effects,
                        ControlEffect::Denied {
                            command_id,
                            reason: ControlDenial::ResetRequiresApproval,
                        },
                    )?;
                } else {
                    self.operator = None;
                    self.gate.set_state(SafetyState::Disarmed);
                    Self::push(
                        effects,
                        ControlEffect::SafetyChanged {
                            state: SafetyState::Disarmed,
                        },
                    )?;
                }
            }
            ControlInput::UpdateEvidence {
                obstacle_blocked,
                lidar_fresh,
                localization_fresh,
            } => {
                self.obstacle_blocked = obstacle_blocked;
                self.lidar_fresh = lidar_fresh;
                self.localization_fresh = localization_fresh;
                if !self.applied.is_stop() || self.planner_active {
                    if let Some(reason) = self.evidence_stop_reason() {
                        self.issue_stop(at, reason, effects)?;
                        if self.operator.is_some() {
                            self.gate.set_state(SafetyState::Ready);
                        }
                    }
                }
            }
            ControlInput::SetPlannerActive(active) => {
                if active {
                    let command_id = self.command_id()?;
                    if let Some(reason) = self.drive_denial(at, true) {
                        Self::push(effects, ControlEffect::Denied { command_id, reason })?;
                    } else if !self.planner_active {
                        self.planner_active = true;
                        Self::push(effects, ControlEffect::PlannerChanged { active: true })?;
                    }
                } else if self.planner_active {
                    self.issue_stop(at, ActuationReason::PlannerCancelled, effects)?;
                }
            }
        }
        Ok(())
    }
}

impl Controller for ControlKernel {
    type Input = ControlInput;
    type Output = ControlEffect;
    type Error = KernelError;

    fn step(&mut self, tick: Tick<Self::Input>) -> Result<Effects<Self::Output>, Self::Error> {
        if tick.at < self.last_tick {
            return Err(KernelError::TimeReversed);
        }
        if self
            .last_tick_sequence
            .is_some_and(|sequence| tick.sequence <= sequence)
        {
            return Err(KernelError::SequenceNotIncreasing);
        }
        self.last_tick = tick.at;
        self.last_tick_sequence = Some(tick.sequence);
        let mut effects = Effects::none(tick.at);
        self.expire_authority(tick.at, &mut effects)?;
        self.enforce_deadman(tick.at, &mut effects)?;
        self.process(tick.at, tick.input, &mut effects)?;
        Ok(effects)
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

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    fn u8(&mut self, value: u8) {
        self.bytes(&[value]);
    }

    fn bool(&mut self, value: bool) {
        self.u8(u8::from(value));
    }

    fn command_id(&mut self, value: CommandId) {
        self.u64(value.producer_epoch.get());
        self.u64(value.sequence.get());
    }

    const fn finish(self) -> u64 {
        self.0
    }
}

const fn safety_tag(state: SafetyState) -> u8 {
    match state {
        SafetyState::Disarmed => 0,
        SafetyState::Ready => 1,
        SafetyState::Moving => 2,
        SafetyState::EStopped => 3,
        SafetyState::Faulted => 4,
    }
}

const fn actuation_reason_tag(reason: ActuationReason) -> u8 {
    match reason {
        ActuationReason::DriveAccepted => 0,
        ActuationReason::OperatorStop => 1,
        ActuationReason::PlannerCancelled => 2,
        ActuationReason::Deadman => 3,
        ActuationReason::Obstacle => 4,
        ActuationReason::StaleEvidence => 5,
        ActuationReason::LeaseExpired => 6,
        ActuationReason::EStop => 7,
    }
}

const fn control_denial_tag(reason: ControlDenial) -> u8 {
    match reason {
        ControlDenial::NoOperatorLease => 0,
        ControlDenial::LeaseExpired => 1,
        ControlDenial::ObstacleBlocked => 2,
        ControlDenial::LidarStale => 3,
        ControlDenial::LocalizationStale => 4,
        ControlDenial::EStopped => 5,
        ControlDenial::ResetRequiresApproval => 6,
        ControlDenial::Safety(SafetyDenial::Disarmed) => 7,
        ControlDenial::Safety(SafetyDenial::EStopped) => 8,
        ControlDenial::Safety(SafetyDenial::Faulted) => 9,
        ControlDenial::Safety(SafetyDenial::Expired) => 10,
        ControlDenial::Safety(SafetyDenial::IssuedInFuture) => 11,
        ControlDenial::Safety(SafetyDenial::SequenceExhausted) => 12,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NormalizedDrive;

    fn kernel() -> ControlKernel {
        ControlKernel::new(ControlKernelConfig {
            command_epoch: ProducerEpoch::new(11).unwrap(),
            evidence_epoch: ProducerEpoch::new(12).unwrap(),
            deadman: DurationNanos::from_millis(50).unwrap(),
        })
    }

    fn drive(value: f64) -> DifferentialDrive {
        let value = NormalizedDrive::new(value).unwrap();
        DifferentialDrive::new(value, value)
    }

    fn run_scenario() -> (u64, u64, Vec<(u64, u64)>) {
        let mut kernel = kernel();
        let mut effect_counts = Vec::new();
        let mut event_digest = StableDigest::new();
        let inputs = [
            (
                0,
                ControlInput::UpdateEvidence {
                    obstacle_blocked: false,
                    lidar_fresh: true,
                    localization_fresh: true,
                },
            ),
            (
                1,
                ControlInput::Authorize {
                    operator: OperatorId::new("operator-a").unwrap(),
                    expires_at: MonotonicNanos::new(1_000_000_000),
                },
            ),
            (2, ControlInput::SetPlannerActive(true)),
            (
                10_000_000,
                ControlInput::Drive {
                    command: drive(0.5),
                    deadline: MonotonicNanos::new(20_000_000),
                },
            ),
            (
                20_000_000,
                ControlInput::UpdateEvidence {
                    obstacle_blocked: true,
                    lidar_fresh: true,
                    localization_fresh: true,
                },
            ),
            (
                30_000_000,
                ControlInput::UpdateEvidence {
                    obstacle_blocked: false,
                    lidar_fresh: true,
                    localization_fresh: true,
                },
            ),
            (
                31_000_000,
                ControlInput::Drive {
                    command: drive(0.25),
                    deadline: MonotonicNanos::new(40_000_000),
                },
            ),
            (90_000_001, ControlInput::Idle),
            (
                91_000_000,
                ControlInput::UpdateEvidence {
                    obstacle_blocked: false,
                    lidar_fresh: false,
                    localization_fresh: true,
                },
            ),
            (
                92_000_000,
                ControlInput::Drive {
                    command: drive(0.1),
                    deadline: MonotonicNanos::new(100_000_000),
                },
            ),
            (93_000_000, ControlInput::EStop),
            (
                94_000_000,
                ControlInput::Drive {
                    command: drive(0.1),
                    deadline: MonotonicNanos::new(100_000_000),
                },
            ),
            (95_000_000, ControlInput::ResetEStop { approved: false }),
            (96_000_000, ControlInput::ResetEStop { approved: true }),
            (
                97_000_000,
                ControlInput::Stop {
                    reason: StopReason::Operator,
                },
            ),
            (
                98_000_000,
                ControlInput::UpdateEvidence {
                    obstacle_blocked: false,
                    lidar_fresh: true,
                    localization_fresh: true,
                },
            ),
            (
                99_000_000,
                ControlInput::Authorize {
                    operator: OperatorId::new("operator-b").unwrap(),
                    expires_at: MonotonicNanos::new(110_000_000),
                },
            ),
            (100_000_000, ControlInput::SetPlannerActive(true)),
            (
                101_000_000,
                ControlInput::Drive {
                    command: drive(0.2),
                    deadline: MonotonicNanos::new(105_000_000),
                },
            ),
            (111_000_000, ControlInput::Idle),
        ];

        for (index, (at, input)) in inputs.into_iter().enumerate() {
            let effects = kernel
                .step(Tick::new(
                    MonotonicNanos::new(at),
                    Sequence::new(index as u64 + 1).unwrap(),
                    input,
                ))
                .unwrap();
            effect_counts.push((at, effects.len() as u64));
            event_digest.u64(at);
            event_digest.u64(effects.len() as u64);
            for effect in effects.iter() {
                event_digest.u64(effect.stable_digest());
            }
        }
        (kernel.state_digest(), event_digest.finish(), effect_counts)
    }

    #[test]
    fn scenario_is_deterministic_and_covers_safety_continuations() {
        let first = run_scenario();
        let second = run_scenario();
        assert_eq!(first, second);
        assert_eq!(first.0, 14_161_983_491_101_435_343);
        assert_eq!(first.1, 922_937_285_294_098_901);
    }

    #[test]
    fn stop_is_authorized_while_disarmed_and_estopped() {
        let mut kernel = kernel();
        let disarmed = kernel
            .step(Tick::new(
                MonotonicNanos::new(1),
                Sequence::new(1).unwrap(),
                ControlInput::Stop {
                    reason: StopReason::Operator,
                },
            ))
            .unwrap();
        assert!(matches!(
            disarmed.iter().next(),
            Some(ControlEffect::Actuate { command, .. }) if command.command().is_stop()
        ));

        kernel
            .step(Tick::new(
                MonotonicNanos::new(2),
                Sequence::new(2).unwrap(),
                ControlInput::EStop,
            ))
            .unwrap();
        let estopped = kernel
            .step(Tick::new(
                MonotonicNanos::new(3),
                Sequence::new(3).unwrap(),
                ControlInput::Stop {
                    reason: StopReason::Operator,
                },
            ))
            .unwrap();
        assert!(matches!(
            estopped.iter().next(),
            Some(ControlEffect::Actuate { command, .. }) if command.command().is_stop()
        ));
    }

    #[test]
    fn reversed_tick_time_is_rejected_without_state_change() {
        let mut kernel = kernel();
        kernel
            .step(Tick::new(
                MonotonicNanos::new(10),
                Sequence::new(1).unwrap(),
                ControlInput::Idle,
            ))
            .unwrap();
        let before = kernel.state_digest();
        assert_eq!(
            kernel.step(Tick::new(
                MonotonicNanos::new(9),
                Sequence::new(2).unwrap(),
                ControlInput::Idle,
            )),
            Err(KernelError::TimeReversed)
        );
        assert_eq!(kernel.state_digest(), before);
        assert_eq!(
            kernel.step(Tick::new(
                MonotonicNanos::new(11),
                Sequence::new(1).unwrap(),
                ControlInput::Idle,
            )),
            Err(KernelError::SequenceNotIncreasing)
        );
        assert_eq!(kernel.state_digest(), before);
    }
}
