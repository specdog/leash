use std::{
    fs,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::{TelemetryFrame, TelemetryStreamFrame};

pub const REPLAY_FORMAT_VERSION: &str = "leash-replay-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ReplayEventKind {
    Telemetry,
    Sensors,
    Camera,
    Command,
    RawFrame,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct ReplayEvent {
    #[serde(default = "default_replay_format")]
    pub format: String,
    pub ts_ms: u128,
    #[serde(default)]
    pub seq: u64,
    pub kind: ReplayEventKind,
    pub data: Value,
}

impl ReplayEvent {
    pub fn new(ts_ms: u128, seq: u64, kind: ReplayEventKind, data: Value) -> Self {
        Self {
            format: REPLAY_FORMAT_VERSION.to_string(),
            ts_ms,
            seq,
            kind,
            data,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReplayRecording {
    events: Vec<ReplayEvent>,
}

impl ReplayRecording {
    pub fn new(events: Vec<ReplayEvent>) -> Self {
        Self {
            events: sorted_events(events),
        }
    }

    pub fn read_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = fs::read_to_string(path)
            .with_context(|| format!("read replay recording {}", path.display()))?;
        Self::from_jsonl(&text)
            .with_context(|| format!("parse replay recording {}", path.display()))
    }

    pub fn write_path(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        fs::write(path, self.to_jsonl())
            .with_context(|| format!("write replay recording {}", path.display()))
    }

    pub fn from_jsonl(text: &str) -> Result<Self> {
        let mut indexed = Vec::new();
        for (line_index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let event: ReplayEvent = serde_json::from_str(line)
                .with_context(|| format!("parse replay event on line {}", line_index + 1))?;
            if event.format != REPLAY_FORMAT_VERSION {
                bail!(
                    "unsupported replay format '{}' on line {}",
                    event.format,
                    line_index + 1
                );
            }
            indexed.push((line_index, event));
        }
        indexed.sort_by(|(left_index, left), (right_index, right)| {
            left.ts_ms
                .cmp(&right.ts_ms)
                .then_with(|| left.seq.cmp(&right.seq))
                .then_with(|| left.kind.cmp(&right.kind))
                .then_with(|| left_index.cmp(right_index))
        });
        Ok(Self {
            events: indexed.into_iter().map(|(_, event)| event).collect(),
        })
    }

    pub fn to_jsonl(&self) -> String {
        let mut lines = self
            .events
            .iter()
            .map(|event| serde_json::to_string(event).expect("replay event serializes"))
            .collect::<Vec<_>>()
            .join("\n");
        lines.push('\n');
        lines
    }

    pub fn events(&self) -> &[ReplayEvent] {
        &self.events
    }

    pub fn telemetry_streams(&self) -> Result<Vec<(u128, TelemetryStreamFrame)>> {
        self.events
            .iter()
            .filter(|event| event.kind == ReplayEventKind::Telemetry)
            .map(|event| {
                let frame = serde_json::from_value(event.data.clone()).with_context(|| {
                    format!("parse telemetry replay event at {}ms", event.ts_ms)
                })?;
                Ok((event.ts_ms, frame))
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct ReplayPlayback {
    telemetry: Arc<Vec<(u128, TelemetryStreamFrame)>>,
    speed: f64,
    started_at: Instant,
}

impl ReplayPlayback {
    pub fn from_path(path: impl AsRef<Path>, speed: f64) -> Result<Self> {
        let recording = ReplayRecording::read_path(path)?;
        Self::new(recording, speed)
    }

    pub fn new(recording: ReplayRecording, speed: f64) -> Result<Self> {
        validate_replay_speed(speed)?;
        let telemetry = recording.telemetry_streams()?;
        if telemetry.is_empty() {
            bail!("replay recording does not contain telemetry events");
        }
        Ok(Self {
            telemetry: Arc::new(telemetry),
            speed,
            started_at: Instant::now(),
        })
    }

    pub fn telemetry_at_elapsed(&self, elapsed: Duration) -> Option<TelemetryStreamFrame> {
        let replay_ms = replay_elapsed_ms(elapsed, self.speed);
        telemetry_at(&self.telemetry, replay_ms)
    }

    pub fn telemetry_now(&self) -> Option<TelemetryStreamFrame> {
        self.telemetry_at_elapsed(self.started_at.elapsed())
    }
}

pub fn scaled_delay(previous_ts_ms: u128, next_ts_ms: u128, speed: f64) -> Result<Duration> {
    validate_replay_speed(speed)?;
    let delta = next_ts_ms.saturating_sub(previous_ts_ms) as f64;
    Ok(Duration::from_millis((delta / speed).round() as u64))
}

pub fn validate_replay_speed(speed: f64) -> Result<()> {
    if !speed.is_finite() || speed <= 0.0 {
        bail!("replay speed must be a finite positive number");
    }
    Ok(())
}

fn telemetry_at(
    telemetry: &[(u128, TelemetryStreamFrame)],
    replay_ms: u128,
) -> Option<TelemetryStreamFrame> {
    let index = telemetry.partition_point(|(ts_ms, _)| *ts_ms <= replay_ms);
    if index == 0 {
        telemetry.first().map(|(_, frame)| frame.clone())
    } else {
        telemetry.get(index - 1).map(|(_, frame)| frame.clone())
    }
}

fn replay_elapsed_ms(elapsed: Duration, speed: f64) -> u128 {
    (elapsed.as_millis() as f64 * speed).floor() as u128
}

fn sorted_events(mut events: Vec<ReplayEvent>) -> Vec<ReplayEvent> {
    events.sort_by(|left, right| {
        left.ts_ms
            .cmp(&right.ts_ms)
            .then_with(|| left.seq.cmp(&right.seq))
            .then_with(|| left.kind.cmp(&right.kind))
    });
    events
}

fn default_replay_format() -> String {
    REPLAY_FORMAT_VERSION.to_string()
}

pub fn replay_telemetry_source(frame: TelemetryFrame) -> TelemetryFrame {
    TelemetryFrame {
        profile: "replay".to_string(),
        source: "replay".to_string(),
        ..frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AcceleratorBackend;
    use crate::module::ModuleInfo;
    use crate::types::{
        BatteryStatus, CameraStatus, CommandStreamState, Health, OdometryStatus, RawFrameStatus,
        SafetyStreamState, SensorSnapshot, SpeedMode,
    };

    #[test]
    fn jsonl_replay_events_are_sorted_deterministically() {
        let text = format!(
            "{}\n{}\n{}",
            serde_json::json!({
                "format": REPLAY_FORMAT_VERSION,
                "ts_ms": 20,
                "seq": 2,
                "kind": "command",
                "data": {"left_cmd": 0.0}
            }),
            serde_json::json!({
                "format": REPLAY_FORMAT_VERSION,
                "ts_ms": 10,
                "seq": 1,
                "kind": "telemetry",
                "data": telemetry_frame(10)
            }),
            serde_json::json!({
                "format": REPLAY_FORMAT_VERSION,
                "ts_ms": 20,
                "seq": 1,
                "kind": "camera",
                "data": {"status": "simulated"}
            })
        );

        let recording = ReplayRecording::from_jsonl(&text).unwrap();

        assert_eq!(recording.events()[0].ts_ms, 10);
        assert_eq!(recording.events()[1].kind, ReplayEventKind::Camera);
        assert_eq!(recording.events()[2].kind, ReplayEventKind::Command);
    }

    #[test]
    fn scaled_delay_honors_speed_and_backwards_timestamps() {
        assert_eq!(
            scaled_delay(100, 200, 2.0).unwrap(),
            Duration::from_millis(50)
        );
        assert_eq!(
            scaled_delay(200, 100, 1.0).unwrap(),
            Duration::from_millis(0)
        );
        assert!(scaled_delay(0, 1, 0.0).is_err());
    }

    #[test]
    fn playback_selects_latest_telemetry_for_elapsed_time() {
        let recording = ReplayRecording::new(vec![
            ReplayEvent::new(
                0,
                0,
                ReplayEventKind::Telemetry,
                serde_json::to_value(telemetry_frame(0)).unwrap(),
            ),
            ReplayEvent::new(
                100,
                1,
                ReplayEventKind::Telemetry,
                serde_json::to_value(telemetry_frame(100)).unwrap(),
            ),
        ]);
        let playback = ReplayPlayback::new(recording, 1.0).unwrap();

        assert_eq!(
            playback
                .telemetry_at_elapsed(Duration::from_millis(75))
                .unwrap()
                .ts_ms,
            0
        );
        assert_eq!(
            playback
                .telemetry_at_elapsed(Duration::from_millis(120))
                .unwrap()
                .ts_ms,
            100
        );
    }

    #[test]
    fn bundled_fixture_is_valid_replay_jsonl() {
        let text = include_str!("../examples/replay/sim-basic.jsonl");
        let recording = ReplayRecording::from_jsonl(text).unwrap();

        assert!(recording
            .events()
            .iter()
            .any(|event| event.kind == ReplayEventKind::Telemetry));
        assert!(recording.telemetry_streams().unwrap().len() >= 2);
    }

    fn telemetry_frame(ts_ms: u128) -> TelemetryStreamFrame {
        let telemetry = TelemetryFrame {
            ts_ms,
            robot: "robot".to_string(),
            profile: "replay".to_string(),
            battery_v: Some(12.3),
            battery_pct: Some(91.7),
            left_cmd: 0.0,
            right_cmd: 0.0,
            odometry_left: Some(0.0),
            odometry_right: Some(0.0),
            session_id: None,
            deadman_ok: true,
            estop: false,
            stopped_by_deadman: false,
            soft_odometry_limited: false,
            soft_odometry_limit_m: 0.0,
            speed_mode: SpeedMode::Medium,
            max_speed: SpeedMode::Medium.cap(),
            sensors: SensorSnapshot {
                battery: BatteryStatus {
                    status: "available".to_string(),
                    voltage_v: Some(12.3),
                    level_pct: Some(91.7),
                },
                odometry: OdometryStatus {
                    status: "available".to_string(),
                    left_m: Some(0.0),
                    right_m: Some(0.0),
                },
                camera: CameraStatus {
                    status: "simulated".to_string(),
                    health: "healthy".to_string(),
                    stream_url: None,
                    snapshot_url: None,
                },
                raw_frame: RawFrameStatus {
                    status: "available".to_string(),
                    source: "replay".to_string(),
                    last_ms: Some(ts_ms),
                    payload: None,
                },
            },
            vision: Default::default(),
            workers: Vec::new(),
            resource: None,
            source: "replay".to_string(),
        };
        TelemetryStreamFrame {
            kind: "telemetry".to_string(),
            ts_ms,
            telemetry,
            health: Health {
                ok: true,
                mode: "replay".to_string(),
                replay: true,
                role: "robot".to_string(),
                profile: "replay".to_string(),
                uptime_ms: ts_ms,
                estop: false,
                deadman_ok: true,
                physical_actuation_enabled: false,
                accelerator: crate::accelerator::resolve_accelerator(
                    AcceleratorBackend::None,
                    false,
                )
                .unwrap(),
                modules: Vec::<ModuleInfo>::new(),
            },
            command: CommandStreamState {
                left_cmd: 0.0,
                right_cmd: 0.0,
                session_id: None,
                speed_mode: SpeedMode::Medium,
                max_speed: SpeedMode::Medium.cap(),
            },
            safety: SafetyStreamState {
                estop: false,
                deadman_ok: true,
                stopped_by_deadman: false,
                soft_odometry_limited: false,
                soft_odometry_limit_m: 0.0,
                physical_actuation_enabled: false,
            },
            visualization: Default::default(),
        }
    }
}
