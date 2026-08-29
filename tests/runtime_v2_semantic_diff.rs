use leash_core::{
    ControlInput, ControlKernel, ControlKernelConfig, Controller, DifferentialDrive, DurationNanos,
    MonotonicNanos, NormalizedDrive, OperatorId, ProducerEpoch, SafetyState, Sequence, StopReason,
    Tick,
};
use leash_harness::{Harness, HarnessConfig, SpeedMode};
use serde::Deserialize;

const SHADOW_FIXTURE: &str = include_str!("../examples/contracts/runtime-v1-v2-shadow.json");
const OPERATOR: &str = "runtime-v2-shadow";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShadowFixture {
    format: String,
    events: Vec<ShadowEvent>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShadowEvent {
    at_ns: u64,
    kind: String,
    #[serde(default)]
    drive_left: f64,
    #[serde(default)]
    drive_right: f64,
    left: f64,
    right: f64,
    estop: bool,
}

#[derive(Debug, PartialEq)]
struct SemanticState {
    left: f64,
    right: f64,
    estop: bool,
}

fn normalized_drive(left: f64, right: f64) -> DifferentialDrive {
    DifferentialDrive::new(
        NormalizedDrive::new(left).expect("fixture left drive is normalized"),
        NormalizedDrive::new(right).expect("fixture right drive is normalized"),
    )
}

#[tokio::test]
async fn v1_and_v2_match_recorded_control_semantics_without_hardware() {
    let fixture: ShadowFixture = serde_json::from_str(SHADOW_FIXTURE).expect("valid fixture");
    assert_eq!(fixture.format, "leash-runtime-v1-v2-shadow-v1");

    let v1 = Harness::new(HarnessConfig::default()).expect("simulation harness");
    assert!(!v1.capabilities().physical);
    assert!(!v1.health().physical_actuation_enabled);

    let mut v2 = ControlKernel::new(ControlKernelConfig {
        command_epoch: ProducerEpoch::new(71).unwrap(),
        evidence_epoch: ProducerEpoch::new(72).unwrap(),
        deadman: DurationNanos::from_millis(400).unwrap(),
    });

    for (index, event) in fixture.events.iter().enumerate() {
        let at = MonotonicNanos::new(event.at_ns);
        let input = match event.kind.as_str() {
            "evidence" => ControlInput::UpdateEvidence {
                obstacle_blocked: false,
                lidar_fresh: true,
                localization_fresh: true,
            },
            "authorize" => {
                v1.authorize(OPERATOR.to_string(), 30, SpeedMode::High)
                    .expect("v1 authorization");
                ControlInput::Authorize {
                    operator: OperatorId::new(OPERATOR).unwrap(),
                    expires_at: MonotonicNanos::new(event.at_ns + 30_000_000_000),
                }
            }
            "drive" => {
                v1.drive(
                    Some(OPERATOR),
                    event.drive_left,
                    event.drive_right,
                    Some(SpeedMode::High),
                )
                .expect("v1 simulated drive");
                ControlInput::Drive {
                    command: normalized_drive(event.drive_left, event.drive_right),
                    deadline: MonotonicNanos::new(event.at_ns + 100_000_000),
                }
            }
            "stop" => {
                v1.stop().expect("v1 simulated stop");
                ControlInput::Stop {
                    reason: StopReason::Operator,
                }
            }
            "estop" => {
                v1.estop().expect("v1 simulated estop");
                ControlInput::EStop
            }
            "reset_estop" => {
                v1.reset_estop(Some(OPERATOR)).expect("v1 estop reset");
                ControlInput::ResetEStop { approved: true }
            }
            other => panic!("unknown shadow event {other}"),
        };

        v2.step(Tick::new(
            at,
            Sequence::new((index + 1) as u64).unwrap(),
            input,
        ))
        .expect("v2 transition");

        let v1_frame = v1.telemetry();
        let v1_state = SemanticState {
            left: v1_frame.left_cmd,
            right: v1_frame.right_cmd,
            estop: v1_frame.estop,
        };
        let v2_drive = v2.applied_drive();
        let v2_state = SemanticState {
            left: v2_drive.left.get(),
            right: v2_drive.right.get(),
            estop: v2.safety_state() == SafetyState::EStopped,
        };
        let expected = SemanticState {
            left: event.left,
            right: event.right,
            estop: event.estop,
        };

        assert_eq!(v1_state, expected, "v1 diverged at event {index}");
        assert_eq!(v2_state, expected, "v2 diverged at event {index}");
        assert_eq!(v1_state, v2_state, "semantic diff at event {index}");
        println!(
            "shadow event={index} kind={} left={} right={} estop={} match=true",
            event.kind, expected.left, expected.right, expected.estop
        );
    }
}
