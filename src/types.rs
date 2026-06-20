use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{accelerator::AcceleratorStatus, capability::CapabilityDescriptor, module::ModuleInfo};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum SpeedMode {
    Low,
    #[default]
    Medium,
    High,
}

impl SpeedMode {
    pub fn cap(self) -> f64 {
        match self {
            Self::Low => 0.22,
            Self::Medium => 0.35,
            Self::High => 1.0,
        }
    }
}

pub const VISUALIZATION_FRAME_VERSION: &str = "leash-visualization-v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct Health {
    pub ok: bool,
    pub mode: String,
    pub replay: bool,
    pub role: String,
    pub profile: String,
    pub uptime_ms: u128,
    pub estop: bool,
    pub deadman_ok: bool,
    pub physical_actuation_enabled: bool,
    pub accelerator: AcceleratorStatus,
    pub modules: Vec<ModuleInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct Capabilities {
    pub ok: bool,
    pub mode: String,
    pub replay: bool,
    pub role: String,
    pub profile: String,
    pub physical: bool,
    pub stream_transport: String,
    pub endpoints: Vec<String>,
    pub mcp_tools: Vec<String>,
    pub speed_modes: Vec<SpeedMode>,
    pub accelerator: AcceleratorStatus,
    pub modules: Vec<ModuleInfo>,
    pub capabilities: Vec<CapabilityDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct TelemetryFrame {
    pub ts_ms: u128,
    pub robot: String,
    pub profile: String,
    pub battery_v: Option<f64>,
    pub left_cmd: f64,
    pub right_cmd: f64,
    pub odometry_left: Option<f64>,
    pub odometry_right: Option<f64>,
    pub session_id: Option<String>,
    pub deadman_ok: bool,
    pub estop: bool,
    pub stopped_by_deadman: bool,
    pub soft_odometry_limited: bool,
    pub soft_odometry_limit_m: f64,
    pub speed_mode: SpeedMode,
    pub max_speed: f64,
    pub sensors: SensorSnapshot,
    pub resource: Option<ResourceSample>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct ResourceSample {
    pub sampled_at_ms: u128,
    pub process_id: u32,
    pub cpu_time_ticks: Option<u64>,
    pub memory_rss_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct RunLogEntry {
    pub timestamp: u128,
    pub run_id: String,
    pub module: String,
    pub event: String,
    pub level: String,
    pub fields: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct AgentMessage {
    pub id: u64,
    pub ts_ms: u128,
    pub source: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct AgentMessageAck {
    pub ok: bool,
    pub message: AgentMessage,
    pub response: Option<AgentModelResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct AgentMessageList {
    pub ok: bool,
    pub messages: Vec<AgentMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct AgentModelResponse {
    pub ok: bool,
    pub provider: String,
    pub model: String,
    pub prompt: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct TelemetryStreamFrame {
    pub kind: String,
    pub ts_ms: u128,
    pub telemetry: TelemetryFrame,
    pub health: Health,
    pub command: CommandStreamState,
    pub safety: SafetyStreamState,
    #[serde(default)]
    pub visualization: VisualizationFrame,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct CommandStreamState {
    pub left_cmd: f64,
    pub right_cmd: f64,
    pub session_id: Option<String>,
    pub speed_mode: SpeedMode,
    pub max_speed: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct SafetyStreamState {
    pub estop: bool,
    pub deadman_ok: bool,
    pub stopped_by_deadman: bool,
    pub soft_odometry_limited: bool,
    pub soft_odometry_limit_m: f64,
    pub physical_actuation_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct VisualizationFrame {
    pub version: String,
    pub ts_ms: u128,
    pub robot: String,
    pub profile: String,
    pub pose: Pose2d,
    pub path: VisualizationPath,
    pub occupancy_grid: OccupancyGridFrame,
    pub point_cloud: PointCloudMetadata,
    pub detections: Vec<DetectionFrame>,
    pub command: CommandOverlay,
}

impl Default for VisualizationFrame {
    fn default() -> Self {
        Self {
            version: VISUALIZATION_FRAME_VERSION.to_string(),
            ts_ms: 0,
            robot: String::new(),
            profile: String::new(),
            pose: Pose2d::default(),
            path: VisualizationPath::default(),
            occupancy_grid: OccupancyGridFrame::default(),
            point_cloud: PointCloudMetadata::default(),
            detections: Vec::new(),
            command: CommandOverlay::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct Pose2d {
    pub frame_id: String,
    pub x_m: f64,
    pub y_m: f64,
    pub yaw_rad: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct VisualizationPath {
    pub frame_id: String,
    pub poses: Vec<Pose2d>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct OccupancyGridFrame {
    pub frame_id: String,
    pub width: u32,
    pub height: u32,
    pub resolution_m: f64,
    pub origin: Pose2d,
    pub cells: Vec<i8>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct PointCloudMetadata {
    pub frame_id: String,
    pub point_count: u32,
    pub fields: Vec<String>,
    pub source: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct DetectionFrame {
    pub frame_id: String,
    pub id: String,
    pub label: String,
    pub confidence: f64,
    pub x_m: f64,
    pub y_m: f64,
    pub width_m: f64,
    pub height_m: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct CommandOverlay {
    pub left_cmd: f64,
    pub right_cmd: f64,
    pub speed_mode: SpeedMode,
    pub max_speed: f64,
    pub estop: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct SensorSnapshot {
    pub battery: BatteryStatus,
    pub odometry: OdometryStatus,
    pub camera: CameraStatus,
    pub raw_frame: RawFrameStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct BatteryStatus {
    pub status: String,
    pub voltage_v: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct OdometryStatus {
    pub status: String,
    pub left_m: Option<f64>,
    pub right_m: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct CameraStatus {
    pub status: String,
    pub health: String,
    pub stream_url: Option<String>,
    pub snapshot_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct RawFrameStatus {
    pub status: String,
    pub source: String,
    pub last_ms: Option<u128>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct CaptureResult {
    pub ok: bool,
    pub source: String,
    pub content_type: String,
    pub byte_len: usize,
    pub captured_at_ms: u128,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct DriveOutcome {
    pub ok: bool,
    pub left: f64,
    pub right: f64,
    pub speed_mode: SpeedMode,
    pub max_speed: f64,
    pub stopped_by_deadman: bool,
    pub soft_odometry_limited: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visualization_frame_round_trips_as_versioned_json() {
        let frame = VisualizationFrame {
            version: VISUALIZATION_FRAME_VERSION.to_string(),
            ts_ms: 42,
            robot: "robot".to_string(),
            profile: "sim".to_string(),
            pose: Pose2d {
                frame_id: "map".to_string(),
                x_m: 1.0,
                y_m: 2.0,
                yaw_rad: 0.5,
            },
            path: VisualizationPath {
                frame_id: "map".to_string(),
                poses: vec![Pose2d {
                    frame_id: "map".to_string(),
                    x_m: 1.0,
                    y_m: 2.0,
                    yaw_rad: 0.5,
                }],
            },
            occupancy_grid: OccupancyGridFrame {
                frame_id: "map".to_string(),
                width: 2,
                height: 2,
                resolution_m: 0.25,
                origin: Pose2d::default(),
                cells: vec![0, 0, 50, 100],
            },
            point_cloud: PointCloudMetadata {
                frame_id: "base_link".to_string(),
                point_count: 0,
                fields: vec!["x".to_string(), "y".to_string(), "z".to_string()],
                source: "sim".to_string(),
            },
            detections: vec![DetectionFrame {
                frame_id: "camera".to_string(),
                id: "det-1".to_string(),
                label: "fixture".to_string(),
                confidence: 0.9,
                x_m: 0.1,
                y_m: 0.2,
                width_m: 0.3,
                height_m: 0.4,
            }],
            command: CommandOverlay {
                left_cmd: 0.1,
                right_cmd: 0.1,
                speed_mode: SpeedMode::Low,
                max_speed: SpeedMode::Low.cap(),
                estop: false,
            },
        };

        let value = serde_json::to_value(&frame).unwrap();
        assert_eq!(value["version"], VISUALIZATION_FRAME_VERSION);
        assert_eq!(value["pose"]["frame_id"], "map");
        assert_eq!(
            value["occupancy_grid"]["cells"].as_array().unwrap().len(),
            4
        );

        let parsed: VisualizationFrame = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.version, VISUALIZATION_FRAME_VERSION);
        assert_eq!(parsed.path.poses.len(), 1);
        assert_eq!(parsed.detections[0].label, "fixture");
    }
}
