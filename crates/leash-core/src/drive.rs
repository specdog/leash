use crate::{DomainError, MonotonicNanos, ProducerEpoch, Sequence};

#[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd)]
pub struct NormalizedDrive(f64);

impl NormalizedDrive {
    pub const ZERO: Self = Self(0.0);

    pub fn new(value: f64) -> Result<Self, DomainError> {
        if !value.is_finite() {
            return Err(DomainError::NonFinite("normalized drive"));
        }
        if !(-1.0..=1.0).contains(&value) {
            return Err(DomainError::OutOfRange("normalized drive"));
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DifferentialDrive {
    pub left: NormalizedDrive,
    pub right: NormalizedDrive,
}

impl DifferentialDrive {
    pub const STOP: Self = Self {
        left: NormalizedDrive::ZERO,
        right: NormalizedDrive::ZERO,
    };

    pub const fn new(left: NormalizedDrive, right: NormalizedDrive) -> Self {
        Self { left, right }
    }

    pub fn is_stop(self) -> bool {
        self.left == NormalizedDrive::ZERO && self.right == NormalizedDrive::ZERO
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommandId {
    pub producer_epoch: ProducerEpoch,
    pub sequence: Sequence,
}

impl CommandId {
    pub const fn new(producer_epoch: ProducerEpoch, sequence: Sequence) -> Self {
        Self {
            producer_epoch,
            sequence,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EvidenceId {
    pub producer_epoch: ProducerEpoch,
    pub sequence: Sequence,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate<C> {
    pub id: CommandId,
    pub issued_at: MonotonicNanos,
    pub deadline: MonotonicNanos,
    pub command: C,
}

impl<C> Candidate<C> {
    pub const fn new(
        id: CommandId,
        issued_at: MonotonicNanos,
        deadline: MonotonicNanos,
        command: C,
    ) -> Self {
        Self {
            id,
            issued_at,
            deadline,
            command,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyState {
    Disarmed,
    Ready,
    Moving,
    EStopped,
    Faulted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyDenial {
    Disarmed,
    EStopped,
    Faulted,
    Expired,
    IssuedInFuture,
    SequenceExhausted,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Authorized<C> {
    command_id: CommandId,
    evidence_id: EvidenceId,
    authorized_at: MonotonicNanos,
    command: C,
}

impl<C> Authorized<C> {
    pub const fn command_id(&self) -> CommandId {
        self.command_id
    }

    pub const fn evidence_id(&self) -> EvidenceId {
        self.evidence_id
    }

    pub const fn authorized_at(&self) -> MonotonicNanos {
        self.authorized_at
    }

    pub fn command(&self) -> &C {
        &self.command
    }

    pub fn into_command(self) -> C {
        self.command
    }
}

#[derive(Debug, Clone)]
pub struct SafetyGate {
    state: SafetyState,
    evidence_epoch: ProducerEpoch,
    next_evidence: Sequence,
}

impl SafetyGate {
    pub fn new(evidence_epoch: ProducerEpoch) -> Self {
        Self {
            state: SafetyState::Disarmed,
            evidence_epoch,
            next_evidence: Sequence::new(1).expect("one is non-zero"),
        }
    }

    pub const fn state(&self) -> SafetyState {
        self.state
    }

    pub fn set_state(&mut self, state: SafetyState) {
        self.state = state;
    }

    pub fn authorize<C>(
        &mut self,
        candidate: Candidate<C>,
        now: MonotonicNanos,
    ) -> Result<Authorized<C>, SafetyDenial> {
        match self.state {
            SafetyState::Disarmed => return Err(SafetyDenial::Disarmed),
            SafetyState::EStopped => return Err(SafetyDenial::EStopped),
            SafetyState::Faulted => return Err(SafetyDenial::Faulted),
            SafetyState::Ready | SafetyState::Moving => {}
        }
        if candidate.issued_at > now {
            return Err(SafetyDenial::IssuedInFuture);
        }
        if candidate.deadline < now {
            return Err(SafetyDenial::Expired);
        }
        let evidence_id = EvidenceId {
            producer_epoch: self.evidence_epoch,
            sequence: self.next_evidence,
        };
        self.next_evidence = self
            .next_evidence
            .next()
            .map_err(|_| SafetyDenial::SequenceExhausted)?;
        Ok(Authorized {
            command_id: candidate.id,
            evidence_id,
            authorized_at: now,
            command: candidate.command,
        })
    }
}

/// Raw floating-point values cannot become drive commands without validation.
///
/// ```compile_fail
/// use leash_core::DifferentialDrive;
/// let _command = DifferentialDrive::new(0.5, 0.5);
/// ```
///
/// An actuator authorization cannot be forged directly by a gateway.
///
/// ```compile_fail
/// use leash_core::{Authorized, DifferentialDrive};
/// let _forged = Authorized::<DifferentialDrive> {
///     command: DifferentialDrive::STOP,
/// };
/// ```
#[cfg(doctest)]
struct DriveCompileContract;

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(deadline: u64) -> Candidate<DifferentialDrive> {
        Candidate::new(
            CommandId::new(ProducerEpoch::new(7).unwrap(), Sequence::new(1).unwrap()),
            MonotonicNanos::new(10),
            MonotonicNanos::new(deadline),
            DifferentialDrive::STOP,
        )
    }

    #[test]
    fn normalized_drive_rejects_invalid_values() {
        assert_eq!(
            NormalizedDrive::new(f64::NAN),
            Err(DomainError::NonFinite("normalized drive"))
        );
        assert_eq!(
            NormalizedDrive::new(1.01),
            Err(DomainError::OutOfRange("normalized drive"))
        );
        assert_eq!(NormalizedDrive::new(-1.0).unwrap().get(), -1.0);
    }

    #[test]
    fn safety_gate_is_fail_closed_and_checks_time() {
        let mut gate = SafetyGate::new(ProducerEpoch::new(9).unwrap());
        assert_eq!(
            gate.authorize(candidate(20), MonotonicNanos::new(15)),
            Err(SafetyDenial::Disarmed)
        );

        gate.set_state(SafetyState::Ready);
        assert_eq!(
            gate.authorize(candidate(14), MonotonicNanos::new(15)),
            Err(SafetyDenial::Expired)
        );
        let authorized = gate
            .authorize(candidate(20), MonotonicNanos::new(15))
            .unwrap();
        assert!(authorized.command().is_stop());
        assert_eq!(authorized.evidence_id().sequence.get(), 1);
    }
}
