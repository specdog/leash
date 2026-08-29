use std::collections::BTreeMap;

use leash_harness::{
    adapter::waveshare_drive_values,
    types::{AppliedActionEvidence, APPLIED_ACTION_SCHEMA_VERSION},
    Harness, HarnessConfig, ReplayRecording,
};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
struct ContractFixture {
    format: String,
    shapes: BTreeMap<String, Vec<String>>,
    waveshare_zero_command: Value,
}

fn fixture() -> ContractFixture {
    serde_json::from_str(include_str!("../examples/contracts/runtime-v1-shapes.json"))
        .expect("runtime v1 contract fixture must parse")
}

fn sorted_keys(value: Value) -> Vec<String> {
    let mut keys = value
        .as_object()
        .expect("contract value must be an object")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    keys
}

fn assert_shape(fixture: &ContractFixture, name: &str, value: Value) {
    assert_eq!(
        sorted_keys(value),
        fixture.shapes[name],
        "{name} wire fields changed; review and update the v1 contract fixture"
    );
}

#[tokio::test]
async fn runtime_v1_public_shapes_are_reviewed_golden_contracts() {
    let fixture = fixture();
    assert_eq!(fixture.format, "leash-runtime-v1-contract-shapes");

    let harness = Harness::new(HarnessConfig::default()).unwrap();
    assert_shape(
        &fixture,
        "health",
        serde_json::to_value(harness.health()).unwrap(),
    );
    assert_shape(
        &fixture,
        "capabilities",
        serde_json::to_value(harness.capabilities()).unwrap(),
    );
    assert_shape(
        &fixture,
        "telemetry",
        serde_json::to_value(harness.telemetry()).unwrap(),
    );
    assert_shape(
        &fixture,
        "applied_action_page",
        serde_json::to_value(harness.applied_action_evidence(0, 1).unwrap()).unwrap(),
    );

    let evidence = AppliedActionEvidence {
        schema_version: APPLIED_ACTION_SCHEMA_VERSION.to_string(),
        authority: "leash".to_string(),
        producer_epoch: 1,
        action_sequence: 1,
        interval_start_ns: 1,
        interval_end_ns: 2,
        requested_left: 0.0,
        requested_right: 0.0,
        clamped_left: 0.0,
        clamped_right: 0.0,
        applied_left: 0.0,
        applied_right: 0.0,
        speed_scale: 0.22,
        safety_flags: 0,
        valid: true,
        armed: false,
        deadman_active: false,
        collision_clamped: false,
    };
    assert_shape(
        &fixture,
        "applied_action",
        serde_json::to_value(evidence).unwrap(),
    );
    assert_shape(
        &fixture,
        "cognition_boundary",
        serde_json::to_value(harness.cognition_boundary()).unwrap(),
    );

    let replay = ReplayRecording::from_jsonl(include_str!(
        "../examples/replay/waveshare-ugv-sensors.jsonl"
    ))
    .unwrap();
    assert_shape(
        &fixture,
        "replay_event",
        serde_json::to_value(&replay.events()[0]).unwrap(),
    );
}

#[test]
fn waveshare_zero_command_is_invariant_under_wiring_transforms() {
    let fixture = fixture();
    for invert in [false, true] {
        for swap in [false, true] {
            let (left, right) = waveshare_drive_values(0.0, 0.0, invert, swap);
            assert_eq!(
                json!({"T": 1, "L": left, "R": right}),
                fixture.waveshare_zero_command
            );
        }
    }
}
