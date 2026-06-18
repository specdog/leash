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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct TelemetryStreamFrame {
    pub kind: String,
    pub ts_ms: u128,
    pub telemetry: TelemetryFrame,
    pub health: Health,
    pub command: CommandStreamState,
    pub safety: SafetyStreamState,
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
