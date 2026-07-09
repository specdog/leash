use std::collections::HashSet;

use anyhow::{bail, Result};
use serde_json::Value;

use crate::types::OperatorSessionRecording;

pub const OPERATOR_SESSION_FORMAT: &str = "leash-operator-session-v1";

pub fn validate_operator_session(recording: &OperatorSessionRecording) -> Result<()> {
    if recording.format != OPERATOR_SESSION_FORMAT {
        bail!("unsupported operator session format '{}'", recording.format);
    }
    if recording.fleet_name.trim().is_empty() {
        bail!("operator session fleet_name cannot be empty");
    }
    if recording.ended_at_ms < recording.started_at_ms {
        bail!("operator session ended_at_ms precedes started_at_ms");
    }
    let duration_ms = recording
        .ended_at_ms
        .saturating_sub(recording.started_at_ms);
    let mut robot_ids = HashSet::new();
    for robot in &recording.robots {
        if robot.id.trim().is_empty() || robot.name.trim().is_empty() {
            bail!("operator session robot id and name cannot be empty");
        }
        if !robot_ids.insert(robot.id.as_str()) {
            bail!("operator session has duplicate robot id '{}'", robot.id);
        }
    }

    let mut previous_offset = 0;
    for (index, event) in recording.events.iter().enumerate() {
        if !robot_ids.contains(event.robot_id.as_str()) {
            bail!(
                "operator session event {index} references unknown robot '{}'",
                event.robot_id
            );
        }
        if index > 0 && event.offset_ms < previous_offset {
            bail!("operator session events must be sorted by offset_ms");
        }
        if u128::from(event.offset_ms) > duration_ms {
            bail!("operator session event {index} exceeds recording duration");
        }
        if recording
            .started_at_ms
            .checked_add(u128::from(event.offset_ms))
            != Some(event.ts_ms)
        {
            bail!("operator session event {index} timestamp does not match its offset");
        }
        reject_sensitive_fields(&event.data, "data")?;
        previous_offset = event.offset_ms;
    }
    Ok(())
}

fn reject_sensitive_fields(value: &Value, path: &str) -> Result<()> {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                let normalized = key.to_ascii_lowercase().replace('-', "_");
                if matches!(
                    normalized.as_str(),
                    "token" | "password" | "secret" | "credential" | "base_url"
                ) {
                    bail!("operator session contains sensitive field {path}.{key}");
                }
                reject_sensitive_fields(value, &format!("{path}.{key}"))?;
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                reject_sensitive_fields(value, &format!("{path}[{index}]"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{OperatorSessionEventKind, OperatorSessionRecording};

    #[test]
    fn bundled_operator_session_is_safe_and_replayable() {
        let recording: OperatorSessionRecording =
            serde_json::from_str(include_str!("../examples/replay/operator-session.json")).unwrap();
        validate_operator_session(&recording).unwrap();
        assert!(recording
            .events
            .iter()
            .any(|event| event.kind == OperatorSessionEventKind::JoystickDrive));
        assert!(recording
            .events
            .iter()
            .any(|event| event.kind == OperatorSessionEventKind::CameraRecovery));
        assert!(
            recording
                .events
                .iter()
                .filter(|event| event.kind == OperatorSessionEventKind::Summary)
                .count()
                >= 2
        );
    }

    #[test]
    fn raw_tokens_and_private_urls_are_rejected_recursively() {
        let mut recording: OperatorSessionRecording =
            serde_json::from_str(include_str!("../examples/replay/operator-session.json")).unwrap();
        recording.events[0].data = serde_json::json!({"nested": {"token": "private"}});
        assert!(validate_operator_session(&recording)
            .unwrap_err()
            .to_string()
            .contains("sensitive field"));
        recording.events[0].data = serde_json::json!({"base_url": "http://private"});
        assert!(validate_operator_session(&recording).is_err());
    }
}
