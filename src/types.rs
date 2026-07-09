use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    accelerator::AcceleratorStatus, capability::CapabilityDescriptor, module::ModuleInfo,
    stack::AdapterProfile, worker::ExternalWorkerStatus,
};

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
pub const OCCUPANCY_UNKNOWN: i8 = -1;
pub const OCCUPANCY_FREE: i8 = 0;
pub const OCCUPANCY_OCCUPIED: i8 = 100;
pub const COST_FREE: u8 = 0;
pub const COST_LETHAL: u8 = 254;
pub const COST_UNKNOWN: u8 = 255;
pub const MANIPULATOR_SCHEMA_VERSION: &str = "leash-manipulator-v1";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct OperatorTokenStatus {
    pub active: bool,
    pub owner_id: Option<String>,
    pub expires_in_ms: Option<u64>,
    pub speed_mode: Option<SpeedMode>,
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
    #[serde(default)]
    pub operator_token: OperatorTokenStatus,
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
    pub adapter: AdapterProfile,
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
    #[serde(default)]
    pub battery_v: Option<f64>,
    #[serde(default)]
    pub battery_pct: Option<f64>,
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
    #[serde(default)]
    pub vision: VisionResult,
    #[serde(default)]
    pub workers: Vec<ExternalWorkerStatus>,
    #[serde(default)]
    pub motion_events: Vec<MotionEvent>,
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
    #[serde(default)]
    pub map: MapMetadata,
    pub pose: Pose2d,
    #[serde(default)]
    pub twist: Twist2d,
    pub path: VisualizationPath,
    pub occupancy_grid: OccupancyGridFrame,
    #[serde(default)]
    pub costmap: CostmapFrame,
    pub point_cloud: PointCloudMetadata,
    pub detections: Vec<DetectionFrame>,
    pub command: CommandOverlay,
    #[serde(default)]
    pub autonomy: AutonomyOverlay,
}

impl Default for VisualizationFrame {
    fn default() -> Self {
        Self {
            version: VISUALIZATION_FRAME_VERSION.to_string(),
            ts_ms: 0,
            robot: String::new(),
            profile: String::new(),
            map: MapMetadata::default(),
            pose: Pose2d::default(),
            twist: Twist2d::default(),
            path: VisualizationPath::default(),
            occupancy_grid: OccupancyGridFrame::default(),
            costmap: CostmapFrame::default(),
            point_cloud: PointCloudMetadata::default(),
            detections: Vec::new(),
            command: CommandOverlay::default(),
            autonomy: AutonomyOverlay::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct Pose2d {
    #[serde(default)]
    pub ts_ms: u128,
    pub frame_id: String,
    pub x_m: f64,
    pub y_m: f64,
    pub yaw_rad: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct Twist2d {
    #[serde(default)]
    pub ts_ms: u128,
    pub frame_id: String,
    pub linear_x_mps: f64,
    pub linear_y_mps: f64,
    pub angular_z_radps: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct VisualizationPath {
    #[serde(default)]
    pub ts_ms: u128,
    pub frame_id: String,
    pub poses: Vec<Pose2d>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct MapMetadata {
    #[serde(default)]
    pub ts_ms: u128,
    pub map_id: String,
    pub frame_id: String,
    pub width: u32,
    pub height: u32,
    pub resolution_m: f64,
    pub origin: Pose2d,
    pub cell_order: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct OccupancyGridFrame {
    #[serde(default)]
    pub ts_ms: u128,
    pub frame_id: String,
    pub width: u32,
    pub height: u32,
    pub resolution_m: f64,
    pub origin: Pose2d,
    #[serde(default)]
    pub metadata: MapMetadata,
    pub cells: Vec<i8>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct CostmapFrame {
    #[serde(default)]
    pub ts_ms: u128,
    pub frame_id: String,
    pub width: u32,
    pub height: u32,
    pub resolution_m: f64,
    pub origin: Pose2d,
    #[serde(default)]
    pub metadata: MapMetadata,
    pub costs: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct PlannerGoal {
    pub frame_id: String,
    pub x_m: f64,
    pub y_m: f64,
    pub tolerance_m: f64,
    pub speed_mode: SpeedMode,
}

impl Default for PlannerGoal {
    fn default() -> Self {
        Self {
            frame_id: "map".to_string(),
            x_m: 0.0,
            y_m: 0.0,
            tolerance_m: 0.1,
            speed_mode: SpeedMode::Low,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct PlannerStatus {
    pub ok: bool,
    pub active: bool,
    pub status: String,
    pub message: String,
    pub goal: Option<PlannerGoal>,
    pub path: VisualizationPath,
    pub last_drive: Option<DriveOutcome>,
}

impl Default for PlannerStatus {
    fn default() -> Self {
        Self {
            ok: true,
            active: false,
            status: "idle".to_string(),
            message: "planner idle".to_string(),
            goal: None,
            path: VisualizationPath::default(),
            last_drive: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum PatrolStrategy {
    #[default]
    Coverage,
    Frontier,
    Random,
}

impl PatrolStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Coverage => "coverage",
            Self::Frontier => "frontier",
            Self::Random => "random",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct PatrolStatus {
    pub ok: bool,
    pub active: bool,
    pub status: String,
    pub message: String,
    pub strategy: Option<PatrolStrategy>,
    pub speed_mode: SpeedMode,
    pub goal: Option<PlannerGoal>,
    pub path: VisualizationPath,
    pub visited_cells: Vec<String>,
    #[serde(default)]
    pub zone_id: Option<String>,
    #[serde(default)]
    pub waypoint_index: Option<usize>,
}

impl Default for PatrolStatus {
    fn default() -> Self {
        Self {
            ok: true,
            active: false,
            status: "idle".to_string(),
            message: "patrol idle".to_string(),
            strategy: None,
            speed_mode: SpeedMode::Low,
            goal: None,
            path: VisualizationPath::default(),
            visited_cells: Vec::new(),
            zone_id: None,
            waypoint_index: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct SavedWaypoint {
    pub id: String,
    pub name: String,
    pub frame_id: String,
    pub x_m: f64,
    pub y_m: f64,
    pub tolerance_m: f64,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct SavedWaypointList {
    pub ok: bool,
    pub store_path: String,
    pub count: usize,
    pub waypoints: Vec<SavedWaypoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct ZoneBoundaryPoint {
    pub x_m: f64,
    pub y_m: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct PatrolZone {
    pub id: String,
    pub name: String,
    pub frame_id: String,
    pub waypoint_ids: Vec<String>,
    pub boundary: Vec<ZoneBoundaryPoint>,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct PatrolZoneList {
    pub ok: bool,
    pub store_path: String,
    pub count: usize,
    pub zones: Vec<PatrolZone>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum MotionEventKind {
    Detected,
    Updated,
    Cleared,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct MotionEvent {
    pub event_id: String,
    pub ts_ms: u128,
    pub source: String,
    pub frame_id: String,
    pub kind: MotionEventKind,
    pub confidence: f64,
    pub x_m: Option<f64>,
    pub y_m: Option<f64>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum SpatialMemoryKind {
    #[default]
    Location,
    Object,
}

impl SpatialMemoryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Location => "location",
            Self::Object => "object",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct SpatialMemoryEntry {
    pub name: String,
    pub kind: SpatialMemoryKind,
    pub frame_id: String,
    pub x_m: f64,
    pub y_m: f64,
    pub observed_at_ms: u128,
    pub updated_at_ms: u128,
    pub confidence: f64,
    pub effective_confidence: f64,
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct SpatialMemoryStatus {
    pub ok: bool,
    pub store_path: String,
    pub count: usize,
    pub entries: Vec<SpatialMemoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct AutonomyOverlay {
    #[serde(default)]
    pub ts_ms: u128,
    pub mode: String,
    pub active: bool,
    pub status: String,
    pub strategy: Option<PatrolStrategy>,
    pub goal: Option<PlannerGoal>,
    pub visited_cells: Vec<String>,
}

impl Default for AutonomyOverlay {
    fn default() -> Self {
        Self {
            ts_ms: 0,
            mode: "idle".to_string(),
            active: false,
            status: "idle".to_string(),
            strategy: None,
            goal: None,
            visited_cells: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct PointCloudMetadata {
    #[serde(default)]
    pub ts_ms: u128,
    pub frame_id: String,
    pub point_count: u32,
    pub fields: Vec<String>,
    pub source: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct DetectionFrame {
    #[serde(default)]
    pub ts_ms: u128,
    pub frame_id: String,
    pub id: String,
    pub label: String,
    pub confidence: f64,
    pub x_m: f64,
    pub y_m: f64,
    pub width_m: f64,
    pub height_m: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct ImageObservation {
    #[serde(default)]
    pub ts_ms: u128,
    pub frame_id: String,
    pub source: String,
    pub width_px: u32,
    pub height_px: u32,
    pub content_type: String,
    pub byte_len: usize,
    pub sha256: Option<String>,
}

impl Default for ImageObservation {
    fn default() -> Self {
        Self {
            ts_ms: 0,
            frame_id: "camera".to_string(),
            source: "unknown".to_string(),
            width_px: 0,
            height_px: 0,
            content_type: "application/octet-stream".to_string(),
            byte_len: 0,
            sha256: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct VisionResult {
    pub ok: bool,
    pub status: String,
    pub source: String,
    pub observed_at_ms: u128,
    pub duration_ms: u128,
    pub detections: Vec<DetectionFrame>,
    pub error: Option<String>,
}

impl Default for VisionResult {
    fn default() -> Self {
        Self {
            ok: false,
            status: "unavailable".to_string(),
            source: "none".to_string(),
            observed_at_ms: 0,
            duration_ms: 0,
            detections: Vec::new(),
            error: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct CommandOverlay {
    #[serde(default)]
    pub ts_ms: u128,
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
    #[serde(default)]
    pub voltage_v: Option<f64>,
    #[serde(default)]
    pub level_pct: Option<f64>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct CameraStreamFailure {
    pub ts_ms: u128,
    pub owner: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct CameraStreamHealth {
    pub ok: bool,
    pub status: String,
    pub device: String,
    pub device_available: bool,
    pub active_owner: Option<String>,
    pub active_since_ms: Option<u128>,
    pub recovery_generation: u64,
    pub recovery_count: u64,
    pub last_recovery_ms: Option<u128>,
    pub recent_failures: Vec<CameraStreamFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct CameraRecoveryResponse {
    pub ok: bool,
    pub recovery_requested: bool,
    pub previous_owner: Option<String>,
    pub recovery_generation: u64,
    pub recovery_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct RawFrameStatus {
    pub status: String,
    pub source: String,
    #[serde(default)]
    pub last_ms: Option<u128>,
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct CameraAimOutcome {
    pub ok: bool,
    pub pan_deg: f64,
    pub tilt_deg: f64,
    pub speed: u32,
    pub accel: u32,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct DroneCommandStatus {
    pub ok: bool,
    pub command: String,
    pub profile: String,
    pub simulated: bool,
    pub status: String,
    pub message: String,
    pub mavlink_endpoint: Option<String>,
    #[serde(default)]
    pub args: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct ManipulatorJoint {
    pub name: String,
    pub position_rad: f64,
    pub velocity_radps: f64,
    pub effort_nm: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct ManipulatorJointState {
    pub version: String,
    pub ok: bool,
    pub profile: String,
    pub simulated: bool,
    pub source: String,
    pub joints: Vec<ManipulatorJoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct ManipulatorCommandStatus {
    pub version: String,
    pub ok: bool,
    pub command: String,
    pub profile: String,
    pub simulated: bool,
    pub status: String,
    pub message: String,
    #[serde(default)]
    pub args: Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visualization_frame_round_trips_as_versioned_json() {
        let map = MapMetadata {
            ts_ms: 42,
            map_id: "sim-local".to_string(),
            frame_id: "map".to_string(),
            width: 2,
            height: 2,
            resolution_m: 0.25,
            origin: Pose2d {
                ts_ms: 42,
                frame_id: "map".to_string(),
                x_m: -0.25,
                y_m: -0.25,
                yaw_rad: 0.0,
            },
            cell_order: "row-major".to_string(),
        };
        let frame = VisualizationFrame {
            version: VISUALIZATION_FRAME_VERSION.to_string(),
            ts_ms: 42,
            robot: "robot".to_string(),
            profile: "sim".to_string(),
            map: map.clone(),
            pose: Pose2d {
                ts_ms: 42,
                frame_id: "map".to_string(),
                x_m: 1.0,
                y_m: 2.0,
                yaw_rad: 0.5,
            },
            twist: Twist2d {
                ts_ms: 42,
                frame_id: "base_link".to_string(),
                linear_x_mps: 0.3,
                linear_y_mps: 0.0,
                angular_z_radps: 0.1,
            },
            path: VisualizationPath {
                ts_ms: 42,
                frame_id: "map".to_string(),
                poses: vec![Pose2d {
                    ts_ms: 42,
                    frame_id: "map".to_string(),
                    x_m: 1.0,
                    y_m: 2.0,
                    yaw_rad: 0.5,
                }],
            },
            occupancy_grid: OccupancyGridFrame {
                ts_ms: 42,
                frame_id: "map".to_string(),
                width: 2,
                height: 2,
                resolution_m: 0.25,
                origin: map.origin.clone(),
                metadata: map.clone(),
                cells: vec![OCCUPANCY_FREE, OCCUPANCY_FREE, 50, OCCUPANCY_OCCUPIED],
            },
            costmap: CostmapFrame {
                ts_ms: 42,
                frame_id: "map".to_string(),
                width: 2,
                height: 2,
                resolution_m: 0.25,
                origin: map.origin.clone(),
                metadata: map,
                costs: vec![COST_FREE, 10, COST_LETHAL, COST_UNKNOWN],
            },
            point_cloud: PointCloudMetadata {
                ts_ms: 42,
                frame_id: "base_link".to_string(),
                point_count: 0,
                fields: vec!["x".to_string(), "y".to_string(), "z".to_string()],
                source: "sim".to_string(),
            },
            detections: vec![DetectionFrame {
                ts_ms: 42,
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
                ts_ms: 42,
                left_cmd: 0.1,
                right_cmd: 0.1,
                speed_mode: SpeedMode::Low,
                max_speed: SpeedMode::Low.cap(),
                estop: false,
            },
            autonomy: AutonomyOverlay::default(),
        };

        let value = serde_json::to_value(&frame).unwrap();
        assert_eq!(value["version"], VISUALIZATION_FRAME_VERSION);
        assert_eq!(value["map"]["frame_id"], "map");
        assert_eq!(value["pose"]["frame_id"], "map");
        assert_eq!(value["twist"]["frame_id"], "base_link");
        assert_eq!(
            value["occupancy_grid"]["cells"].as_array().unwrap().len(),
            4
        );
        assert_eq!(value["costmap"]["costs"].as_array().unwrap().len(), 4);

        let parsed: VisualizationFrame = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.version, VISUALIZATION_FRAME_VERSION);
        assert_eq!(parsed.map.cell_order, "row-major");
        assert_eq!(parsed.twist.frame_id, "base_link");
        assert_eq!(parsed.path.poses.len(), 1);
        assert_eq!(parsed.costmap.costs[2], COST_LETHAL);
        assert_eq!(parsed.detections[0].label, "fixture");
    }

    #[test]
    fn old_visualization_json_defaults_new_mapping_fields() {
        let value = serde_json::json!({
            "version": VISUALIZATION_FRAME_VERSION,
            "ts_ms": 7,
            "robot": "robot",
            "profile": "sim",
            "pose": {
                "frame_id": "map",
                "x_m": 1.0,
                "y_m": 2.0,
                "yaw_rad": 0.5
            },
            "path": {
                "frame_id": "map",
                "poses": []
            },
            "occupancy_grid": {
                "frame_id": "map",
                "width": 1,
                "height": 1,
                "resolution_m": 0.25,
                "origin": {
                    "frame_id": "map",
                    "x_m": 0.0,
                    "y_m": 0.0,
                    "yaw_rad": 0.0
                },
                "cells": [0]
            },
            "point_cloud": {
                "frame_id": "base_link",
                "point_count": 0,
                "fields": ["x", "y", "z"],
                "source": "sim"
            },
            "detections": [],
            "command": {
                "left_cmd": 0.0,
                "right_cmd": 0.0,
                "speed_mode": "medium",
                "max_speed": 0.35,
                "estop": false
            }
        });

        let parsed: VisualizationFrame = serde_json::from_value(value).unwrap();

        assert_eq!(parsed.pose.ts_ms, 0);
        assert_eq!(parsed.twist, Twist2d::default());
        assert_eq!(parsed.map, MapMetadata::default());
        assert_eq!(parsed.costmap, CostmapFrame::default());
        assert_eq!(parsed.autonomy, AutonomyOverlay::default());
    }
}
