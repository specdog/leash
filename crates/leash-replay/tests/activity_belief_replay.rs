use leash_core::{
    resolve_competing, Activity, ActivityEvent, ActivityFailure, ActivityId, ActivityKind,
    ActivityState, Base, Belief, BeliefId, BeliefSource, Effect, EvidenceId, Frame, FrameName,
    Lineage, MonotonicNanos, Precision, ProducerEpoch, Proposal, ProposalId, ProposalRejection,
    Sequence,
};
use serde::Deserialize;

const SCENARIO: &[u8] = include_bytes!("../fixtures/activity-belief-v1.json");

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    schema_version: String,
    activities: Vec<ActivityReplay>,
    belief: BeliefReplay,
    arbitration: ArbitrationReplay,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivityReplay {
    sequence: u64,
    events: Vec<TimedActivityEvent>,
    expected_state: ExpectedActivityState,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TimedActivityEvent {
    at_ns: u64,
    event: ReplayActivityEvent,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ReplayActivityEvent {
    Start,
    Suspend,
    Resume,
    Succeed,
    Cancel,
    Fail { reason: ReplayFailure },
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReplayFailure {
    StaleBelief,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExpectedActivityState {
    Succeeded,
    Cancelled,
    FailedStaleBelief,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BeliefReplay {
    sequence: u64,
    observed_at_ns: u64,
    expires_at_ns: u64,
    check_at_ns: u64,
    expected_fresh: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArbitrationReplay {
    at_ns: u64,
    first: ProposalReplay,
    second: ProposalReplay,
    expected_selected_sequence: u64,
    expected_rejection: ExpectedRejection,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposalReplay {
    sequence: u64,
    created_at_ns: u64,
    deadline_ns: u64,
    priority: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExpectedRejection {
    LowerPriority,
}

fn epoch(value: u64) -> ProducerEpoch {
    ProducerEpoch::new(value).unwrap()
}

fn sequence(value: u64) -> Sequence {
    Sequence::new(value).unwrap()
}

fn activity_id(value: u64) -> ActivityId {
    ActivityId::new(epoch(1), sequence(value))
}

fn replay_event(event: &ReplayActivityEvent) -> ActivityEvent {
    match event {
        ReplayActivityEvent::Start => ActivityEvent::Start,
        ReplayActivityEvent::Suspend => ActivityEvent::Suspend,
        ReplayActivityEvent::Resume => ActivityEvent::Resume,
        ReplayActivityEvent::Succeed => ActivityEvent::Succeed,
        ReplayActivityEvent::Cancel => ActivityEvent::Cancel,
        ReplayActivityEvent::Fail {
            reason: ReplayFailure::StaleBelief,
        } => ActivityEvent::Fail(ActivityFailure::StaleBelief),
    }
}

fn expected_state(state: ActivityState) -> ExpectedActivityState {
    match state {
        ActivityState::Succeeded => ExpectedActivityState::Succeeded,
        ActivityState::Cancelled => ExpectedActivityState::Cancelled,
        ActivityState::Failed(ActivityFailure::StaleBelief) => {
            ExpectedActivityState::FailedStaleBelief
        }
        other => panic!("fixture ended in unexpected activity state {other:?}"),
    }
}

fn proposal(value: &ProposalReplay, activity: ActivityId) -> Proposal {
    Proposal::new(
        ProposalId::new(epoch(4), sequence(value.sequence)),
        activity,
        Effect::ProposeStop,
        MonotonicNanos::new(value.created_at_ns),
        MonotonicNanos::new(value.deadline_ns),
        value.priority,
        vec![BeliefId::new(epoch(3), sequence(1))].into_boxed_slice(),
    )
    .unwrap()
}

#[test]
fn checked_activity_belief_replay_covers_lifecycle_staleness_and_competition() {
    let scenario: Scenario = serde_json::from_slice(SCENARIO).unwrap();
    assert_eq!(scenario.schema_version, "leash.activity-belief-replay.v1");

    for replay in &scenario.activities {
        let mut activity = Activity::new(
            activity_id(replay.sequence),
            ActivityKind::HoldPosition,
            MonotonicNanos::ZERO,
        );
        for event in &replay.events {
            activity
                .apply(replay_event(&event.event), MonotonicNanos::new(event.at_ns))
                .unwrap();
        }
        assert_eq!(expected_state(activity.state), replay.expected_state);
    }

    let belief = Belief::new(
        BeliefId::new(epoch(3), sequence(scenario.belief.sequence)),
        true,
        BeliefSource::new("replay/lidar").unwrap(),
        Frame::<Base>::new(FrameName::new("base_link").unwrap()),
        MonotonicNanos::new(scenario.belief.observed_at_ns),
        MonotonicNanos::new(scenario.belief.expires_at_ns),
        Precision::new(0.9).unwrap(),
        Lineage::new(
            vec![EvidenceId {
                producer_epoch: epoch(5),
                sequence: sequence(1),
            }]
            .into_boxed_slice(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        belief.is_fresh_at(MonotonicNanos::new(scenario.belief.check_at_ns)),
        scenario.belief.expected_fresh
    );

    let arbitration = resolve_competing(
        proposal(&scenario.arbitration.first, activity_id(1)),
        proposal(&scenario.arbitration.second, activity_id(1)),
        MonotonicNanos::new(scenario.arbitration.at_ns),
    );
    assert_eq!(
        arbitration.selected.unwrap().id.sequence.get(),
        scenario.arbitration.expected_selected_sequence
    );
    assert_eq!(arbitration.rejected.len(), 1);
    assert_eq!(
        arbitration.rejected[0].1,
        match scenario.arbitration.expected_rejection {
            ExpectedRejection::LowerPriority => ProposalRejection::LowerPriority,
        }
    );
}
