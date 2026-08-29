use crate::{
    CommandId, DifferentialDrive, DomainError, EvidenceId, Frame, Map, MonotonicNanos, Odom, Pose2,
    ProducerEpoch, Sequence,
};

pub const ACTIVITY_SCHEMA_VERSION: &str = "leash.activity.v1";
pub const BELIEF_SCHEMA_VERSION: &str = "leash.belief.v1";
pub const PROPOSAL_SCHEMA_VERSION: &str = "leash.proposal.v1";

macro_rules! entity_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name {
            pub producer_epoch: ProducerEpoch,
            pub sequence: Sequence,
        }

        impl $name {
            pub const fn new(producer_epoch: ProducerEpoch, sequence: Sequence) -> Self {
                Self {
                    producer_epoch,
                    sequence,
                }
            }
        }
    };
}

entity_id!(ActivityId);
entity_id!(BeliefId);
entity_id!(ProposalId);

#[derive(Debug, Clone, PartialEq)]
pub enum ActivityKind {
    HoldPosition,
    Navigate { goal: Pose2<Map> },
    Patrol { route: Box<[Pose2<Map>]> },
    Observe { subject: Box<str> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityFailure {
    InvalidIntent,
    StaleBelief,
    SafetyDenied,
    ActuatorFault,
    PlannerFault,
    DeadlineExpired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityState {
    Created,
    Running,
    Suspended,
    Succeeded,
    Failed(ActivityFailure),
    Cancelled,
}

impl ActivityState {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed(_) | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityEvent {
    Start,
    Suspend,
    Resume,
    Succeed,
    Fail(ActivityFailure),
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityTransitionError {
    Illegal {
        from: ActivityState,
        event: ActivityEvent,
    },
    TimeReversed {
        from: ActivityState,
        event: ActivityEvent,
        previous: MonotonicNanos,
        attempted: MonotonicNanos,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    pub schema_version: &'static str,
    pub id: ActivityId,
    pub kind: ActivityKind,
    pub state: ActivityState,
    pub created_at: MonotonicNanos,
    pub updated_at: MonotonicNanos,
}

impl Activity {
    pub const fn new(id: ActivityId, kind: ActivityKind, at: MonotonicNanos) -> Self {
        Self {
            schema_version: ACTIVITY_SCHEMA_VERSION,
            id,
            kind,
            state: ActivityState::Created,
            created_at: at,
            updated_at: at,
        }
    }

    pub fn apply(
        &mut self,
        event: ActivityEvent,
        at: MonotonicNanos,
    ) -> Result<ActivityState, ActivityTransitionError> {
        let next = match (self.state, event) {
            (ActivityState::Created, ActivityEvent::Start)
            | (ActivityState::Suspended, ActivityEvent::Resume) => ActivityState::Running,
            (ActivityState::Running, ActivityEvent::Suspend) => ActivityState::Suspended,
            (ActivityState::Running, ActivityEvent::Succeed) => ActivityState::Succeeded,
            (
                ActivityState::Created | ActivityState::Running | ActivityState::Suspended,
                ActivityEvent::Fail(reason),
            ) => ActivityState::Failed(reason),
            (
                ActivityState::Created | ActivityState::Running | ActivityState::Suspended,
                ActivityEvent::Cancel,
            ) => ActivityState::Cancelled,
            (from, event) => return Err(ActivityTransitionError::Illegal { from, event }),
        };
        if at < self.updated_at {
            return Err(ActivityTransitionError::TimeReversed {
                from: self.state,
                event,
                previous: self.updated_at,
                attempted: at,
            });
        }
        self.state = next;
        self.updated_at = at;
        Ok(next)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    Hold,
    Navigate(Pose2<Map>),
    DirectDrive(DifferentialDrive),
    Stop,
    Observe(Box<str>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Observation {
    BasePose(Pose2<Odom>),
    ObstacleBlocked(bool),
    Localization(Pose2<Map>),
    BatteryFraction(Precision),
    SemanticLabel {
        label: Box<str>,
        confidence: Precision,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputeIntent {
    OccupancyProjection,
    LidarTransform,
    CameraNormalization,
    PredictiveStep,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    ProposeDrive(DifferentialDrive),
    ProposeStop,
    SetNavigationGoal(Pose2<Map>),
    RequestObservation(Box<str>),
    RequestCompute(ComputeIntent),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BeliefSource(Box<str>);

impl BeliefSource {
    pub fn new(value: impl Into<Box<str>>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.is_empty() {
            return Err(DomainError::Empty("belief source"));
        }
        if !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':' | b'/')
        }) {
            return Err(DomainError::InvalidCharacter("belief source"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Precision(f64);

impl Precision {
    pub fn new(value: f64) -> Result<Self, DomainError> {
        if !value.is_finite() {
            return Err(DomainError::NonFinite("belief precision"));
        }
        if !(0.0..=1.0).contains(&value) {
            return Err(DomainError::OutOfRange("belief precision"));
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lineage(Box<[EvidenceId]>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineageError {
    Empty,
}

impl Lineage {
    pub fn new(values: impl Into<Box<[EvidenceId]>>) -> Result<Self, LineageError> {
        let values = values.into();
        if values.is_empty() {
            return Err(LineageError::Empty);
        }
        Ok(Self(values))
    }

    pub fn as_slice(&self) -> &[EvidenceId] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeliefError {
    ExpiresBeforeObservation,
    EmptyLineage,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Belief<T, F> {
    pub schema_version: &'static str,
    pub id: BeliefId,
    pub value: T,
    pub source: BeliefSource,
    pub frame: Frame<F>,
    pub observed_at: MonotonicNanos,
    pub expires_at: MonotonicNanos,
    pub precision: Precision,
    pub lineage: Lineage,
}

impl<T, F> Belief<T, F> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: BeliefId,
        value: T,
        source: BeliefSource,
        frame: Frame<F>,
        observed_at: MonotonicNanos,
        expires_at: MonotonicNanos,
        precision: Precision,
        lineage: Lineage,
    ) -> Result<Self, BeliefError> {
        if expires_at < observed_at {
            return Err(BeliefError::ExpiresBeforeObservation);
        }
        if lineage.as_slice().is_empty() {
            return Err(BeliefError::EmptyLineage);
        }
        Ok(Self {
            schema_version: BELIEF_SCHEMA_VERSION,
            id,
            value,
            source,
            frame,
            observed_at,
            expires_at,
            precision,
            lineage,
        })
    }

    pub fn is_fresh_at(&self, now: MonotonicNanos) -> bool {
        self.observed_at <= now && now <= self.expires_at
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalError {
    DeadlineBeforeCreation,
    EmptyBeliefLineage,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    pub schema_version: &'static str,
    pub id: ProposalId,
    pub activity_id: ActivityId,
    pub effect: Effect,
    pub created_at: MonotonicNanos,
    pub deadline: MonotonicNanos,
    pub priority: u8,
    pub belief_lineage: Box<[BeliefId]>,
}

impl Proposal {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: ProposalId,
        activity_id: ActivityId,
        effect: Effect,
        created_at: MonotonicNanos,
        deadline: MonotonicNanos,
        priority: u8,
        belief_lineage: impl Into<Box<[BeliefId]>>,
    ) -> Result<Self, ProposalError> {
        if deadline < created_at {
            return Err(ProposalError::DeadlineBeforeCreation);
        }
        let belief_lineage = belief_lineage.into();
        if belief_lineage.is_empty() {
            return Err(ProposalError::EmptyBeliefLineage);
        }
        Ok(Self {
            schema_version: PROPOSAL_SCHEMA_VERSION,
            id,
            activity_id,
            effect,
            created_at,
            deadline,
            priority,
            belief_lineage,
        })
    }

    pub fn is_fresh_at(&self, now: MonotonicNanos) -> bool {
        self.created_at <= now && now <= self.deadline
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalRejection {
    Stale,
    LowerPriority,
    TieBrokenById,
    SafetyDenied,
    Superseded,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Arbitration {
    pub selected: Option<Proposal>,
    pub rejected: Box<[(Proposal, ProposalRejection)]>,
}

pub fn resolve_competing(first: Proposal, second: Proposal, now: MonotonicNanos) -> Arbitration {
    match (first.is_fresh_at(now), second.is_fresh_at(now)) {
        (false, false) => Arbitration {
            selected: None,
            rejected: Box::new([
                (first, ProposalRejection::Stale),
                (second, ProposalRejection::Stale),
            ]),
        },
        (false, true) => Arbitration {
            selected: Some(second),
            rejected: Box::new([(first, ProposalRejection::Stale)]),
        },
        (true, false) => Arbitration {
            selected: Some(first),
            rejected: Box::new([(second, ProposalRejection::Stale)]),
        },
        (true, true) => {
            let first_wins = first.priority > second.priority
                || first.priority == second.priority && first.id <= second.id;
            if first_wins {
                let reason = if first.priority == second.priority {
                    ProposalRejection::TieBrokenById
                } else {
                    ProposalRejection::LowerPriority
                };
                Arbitration {
                    selected: Some(first),
                    rejected: Box::new([(second, reason)]),
                }
            } else {
                let reason = if first.priority == second.priority {
                    ProposalRejection::TieBrokenById
                } else {
                    ProposalRejection::LowerPriority
                };
                Arbitration {
                    selected: Some(second),
                    rejected: Box::new([(first, reason)]),
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Proposed(ProposalId),
    Rejected {
        proposal_id: ProposalId,
        reason: ProposalRejection,
    },
    Authorized {
        proposal_id: ProposalId,
        command_id: CommandId,
        evidence_id: EvidenceId,
    },
    ActivitySucceeded(ActivityId),
    ActivityFailed {
        activity_id: ActivityId,
        reason: ActivityFailure,
    },
    ActivityCancelled(ActivityId),
}

/// A belief frame is part of its type and cannot be silently relabeled.
///
/// ```compile_fail
/// use leash_core::{Base, Belief, Map};
/// fn consume_map(_: Belief<bool, Map>) {}
/// fn wrong(value: Belief<bool, Base>) { consume_map(value); }
/// ```
#[cfg(doctest)]
struct ActivityCompileContract;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Base, FrameName, Meters, Radians, SafetyState};

    fn epoch(value: u64) -> ProducerEpoch {
        ProducerEpoch::new(value).unwrap()
    }

    fn activity_id(value: u64) -> ActivityId {
        ActivityId::new(epoch(1), Sequence::new(value).unwrap())
    }

    fn belief_id(value: u64) -> BeliefId {
        BeliefId::new(epoch(2), Sequence::new(value).unwrap())
    }

    fn proposal_id(value: u64) -> ProposalId {
        ProposalId::new(epoch(3), Sequence::new(value).unwrap())
    }

    fn evidence(value: u64) -> EvidenceId {
        EvidenceId {
            producer_epoch: epoch(4),
            sequence: Sequence::new(value).unwrap(),
        }
    }

    fn frame<F>(name: &str) -> Frame<F> {
        Frame::new(FrameName::new(name).unwrap())
    }

    fn goal() -> Pose2<Map> {
        Pose2::new(
            frame("map"),
            Meters::new(1.0).unwrap(),
            Meters::new(2.0).unwrap(),
            Radians::new(0.5).unwrap(),
        )
    }

    #[test]
    fn activity_state_machine_covers_start_suspend_resume_and_terminals() {
        let mut activity = Activity::new(
            activity_id(1),
            ActivityKind::Navigate { goal: goal() },
            MonotonicNanos::new(1),
        );
        assert_eq!(
            activity.apply(ActivityEvent::Start, MonotonicNanos::new(2)),
            Ok(ActivityState::Running)
        );
        assert_eq!(
            activity.apply(ActivityEvent::Suspend, MonotonicNanos::new(3)),
            Ok(ActivityState::Suspended)
        );
        assert_eq!(
            activity.apply(ActivityEvent::Resume, MonotonicNanos::new(4)),
            Ok(ActivityState::Running)
        );
        assert_eq!(
            activity.apply(ActivityEvent::Succeed, MonotonicNanos::new(5)),
            Ok(ActivityState::Succeeded)
        );
        assert!(activity.state.is_terminal());
        assert_eq!(
            activity.apply(ActivityEvent::Cancel, MonotonicNanos::new(6)),
            Err(ActivityTransitionError::Illegal {
                from: ActivityState::Succeeded,
                event: ActivityEvent::Cancel,
            })
        );
    }

    #[test]
    fn activity_cancel_and_fail_are_total_from_every_non_terminal_state() {
        for state in [
            ActivityState::Created,
            ActivityState::Running,
            ActivityState::Suspended,
        ] {
            let mut cancelled = Activity::new(
                activity_id(1),
                ActivityKind::HoldPosition,
                MonotonicNanos::new(1),
            );
            cancelled.state = state;
            assert_eq!(
                cancelled.apply(ActivityEvent::Cancel, MonotonicNanos::new(2)),
                Ok(ActivityState::Cancelled)
            );
            let mut failed = Activity::new(
                activity_id(2),
                ActivityKind::HoldPosition,
                MonotonicNanos::new(1),
            );
            failed.state = state;
            assert_eq!(
                failed.apply(
                    ActivityEvent::Fail(ActivityFailure::SafetyDenied),
                    MonotonicNanos::new(2),
                ),
                Ok(ActivityState::Failed(ActivityFailure::SafetyDenied))
            );
        }
    }

    #[test]
    fn activity_time_reversal_is_distinct_from_an_illegal_transition() {
        let mut activity = Activity::new(
            activity_id(1),
            ActivityKind::HoldPosition,
            MonotonicNanos::new(10),
        );
        assert_eq!(
            activity.apply(ActivityEvent::Start, MonotonicNanos::new(9)),
            Err(ActivityTransitionError::TimeReversed {
                from: ActivityState::Created,
                event: ActivityEvent::Start,
                previous: MonotonicNanos::new(10),
                attempted: MonotonicNanos::new(9),
            })
        );
        assert_eq!(activity.state, ActivityState::Created);
    }

    #[test]
    fn belief_retains_source_frame_precision_expiry_and_lineage() {
        let belief = Belief::new(
            belief_id(1),
            true,
            BeliefSource::new("lidar/front").unwrap(),
            frame::<Base>("base_link"),
            MonotonicNanos::new(10),
            MonotonicNanos::new(20),
            Precision::new(0.9).unwrap(),
            Lineage::new(Box::new([evidence(1)]) as Box<[EvidenceId]>).unwrap(),
        )
        .unwrap();
        assert!(belief.is_fresh_at(MonotonicNanos::new(20)));
        assert!(!belief.is_fresh_at(MonotonicNanos::new(21)));
        assert_eq!(belief.source.as_str(), "lidar/front");
        assert_eq!(belief.frame.name().as_str(), "base_link");
        assert_eq!(belief.lineage.as_slice(), [evidence(1)]);
    }

    fn proposal(id: u64, priority: u8, deadline: u64) -> Proposal {
        Proposal::new(
            proposal_id(id),
            activity_id(1),
            Effect::SetNavigationGoal(goal()),
            MonotonicNanos::new(10),
            MonotonicNanos::new(deadline),
            priority,
            Box::new([belief_id(1)]) as Box<[BeliefId]>,
        )
        .unwrap()
    }

    #[test]
    fn competing_proposals_resolve_by_freshness_priority_then_id() {
        let arbitration = resolve_competing(
            proposal(2, 10, 20),
            proposal(1, 10, 20),
            MonotonicNanos::new(15),
        );
        assert_eq!(arbitration.selected.unwrap().id, proposal_id(1));
        assert_eq!(arbitration.rejected[0].1, ProposalRejection::TieBrokenById);

        let arbitration = resolve_competing(
            proposal(1, 100, 11),
            proposal(2, 1, 20),
            MonotonicNanos::new(15),
        );
        assert_eq!(arbitration.selected.unwrap().id, proposal_id(2));
        assert_eq!(arbitration.rejected[0].1, ProposalRejection::Stale);
    }

    #[test]
    fn proposal_and_belief_constructors_reject_incomplete_temporal_evidence() {
        assert_eq!(
            Lineage::new(Box::<[EvidenceId]>::default()),
            Err(LineageError::Empty)
        );
        assert_eq!(
            Proposal::new(
                proposal_id(1),
                activity_id(1),
                Effect::ProposeStop,
                MonotonicNanos::new(20),
                MonotonicNanos::new(10),
                1,
                Box::new([belief_id(1)]) as Box<[BeliefId]>,
            ),
            Err(ProposalError::DeadlineBeforeCreation)
        );
        let _ = SafetyState::Disarmed;
    }
}
