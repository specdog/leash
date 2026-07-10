use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    accelerator::AcceleratorStatus, capability::CapabilityDescriptor,
    localization::LocalizationProviderStatus, module::ModuleInfo, stack::AdapterProfile,
    worker::ExternalWorkerStatus,
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
pub const SENSOR_CONTRACT_VERSION: &str = "leash-sensors-v1";
pub const LOCALIZATION_FRAME_VERSION: &str = "leash-localization-v1";
pub const MAX_IMU_LINEAR_ACCELERATION_MPS2: f64 = 1_000.0;
pub const MAX_IMU_ANGULAR_VELOCITY_RADPS: f64 = 1_000.0;

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
    pub physical_navigation_enabled: bool,
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
    #[serde(default)]
    pub physical_navigation_enabled: bool,
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
    pub localization: LocalizationFrame,
    #[serde(default)]
    pub localization_provider: LocalizationProviderStatus,
    #[serde(default)]
    pub map: MapMetadata,
    #[serde(default)]
    pub occupancy_grid: OccupancyGridFrame,
    #[serde(default)]
    pub costmap: CostmapFrame,
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

impl TelemetryStreamFrame {
    pub fn validate(&self) -> Result<(), TelemetryContractError> {
        if self.kind != "telemetry" {
            return Err(TelemetryContractError::InvalidKind);
        }
        if self.ts_ms != self.telemetry.ts_ms {
            return Err(TelemetryContractError::TimestampMismatch);
        }
        if self.telemetry.sensors.version != SENSOR_CONTRACT_VERSION {
            return Err(TelemetryContractError::UnsupportedSensorVersion);
        }
        if self.visualization.version != VISUALIZATION_FRAME_VERSION {
            return Err(TelemetryContractError::UnsupportedVisualizationVersion);
        }
        self.telemetry
            .sensors
            .range_scan
            .validate()
            .map_err(TelemetryContractError::RangeScan)?;
        self.telemetry
            .sensors
            .imu
            .validate()
            .map_err(TelemetryContractError::Imu)?;
        self.telemetry
            .localization
            .validate()
            .map_err(TelemetryContractError::Localization)?;
        self.visualization
            .localization
            .validate()
            .map_err(TelemetryContractError::VisualizationLocalization)?;
        if self.telemetry.map != self.visualization.map
            || self.telemetry.occupancy_grid != self.visualization.occupancy_grid
            || self.telemetry.costmap != self.visualization.costmap
            || self.telemetry.localization != self.visualization.localization
            || self.telemetry.localization_provider != self.visualization.localization_provider
            || self.telemetry.sensors.range_scan != self.visualization.range_scan
            || self.telemetry.sensors.imu != self.visualization.imu
        {
            return Err(TelemetryContractError::VisualizationMismatch);
        }
        if self.telemetry.occupancy_grid.cells.len()
            != self.telemetry.occupancy_grid.width as usize
                * self.telemetry.occupancy_grid.height as usize
            || self.telemetry.costmap.costs.len()
                != self.telemetry.costmap.width as usize * self.telemetry.costmap.height as usize
        {
            return Err(TelemetryContractError::GridSizeMismatch);
        }
        if !self.telemetry.localization.map.map_id.is_empty()
            && self.telemetry.localization.map.map_id != self.telemetry.map.map_id
        {
            return Err(TelemetryContractError::MapIdentityMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum TelemetryContractError {
    InvalidKind,
    TimestampMismatch,
    UnsupportedSensorVersion,
    UnsupportedVisualizationVersion,
    RangeScan(SensorContractError),
    Imu(SensorContractError),
    Localization(LocalizationContractError),
    VisualizationLocalization(LocalizationContractError),
    VisualizationMismatch,
    GridSizeMismatch,
    MapIdentityMismatch,
}

impl std::fmt::Display for TelemetryContractError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidKind => formatter.write_str("stream frame kind must be telemetry"),
            Self::TimestampMismatch => {
                formatter.write_str("stream and telemetry timestamps do not match")
            }
            Self::UnsupportedSensorVersion => {
                formatter.write_str("unsupported sensor contract version")
            }
            Self::UnsupportedVisualizationVersion => {
                formatter.write_str("unsupported visualization frame version")
            }
            Self::RangeScan(error) => write!(formatter, "invalid range scan: {error}"),
            Self::Imu(error) => write!(formatter, "invalid IMU: {error}"),
            Self::Localization(error) => write!(formatter, "invalid localization: {error}"),
            Self::VisualizationLocalization(error) => {
                write!(formatter, "invalid visualization localization: {error}")
            }
            Self::VisualizationMismatch => {
                formatter.write_str("telemetry and visualization mapping fields do not match")
            }
            Self::GridSizeMismatch => {
                formatter.write_str("mapping grid dimensions do not match cell counts")
            }
            Self::MapIdentityMismatch => {
                formatter.write_str("localization and map identity do not match")
            }
        }
    }
}

impl std::error::Error for TelemetryContractError {}

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
    #[serde(default)]
    pub physical_navigation_enabled: bool,
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
    #[serde(default)]
    pub range_scan: RangeScanStatus,
    #[serde(default)]
    pub imu: ImuStatus,
    #[serde(default)]
    pub localization: LocalizationFrame,
    #[serde(default)]
    pub localization_provider: LocalizationProviderStatus,
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
            range_scan: RangeScanStatus::default(),
            imu: ImuStatus::default(),
            localization: LocalizationFrame::default(),
            localization_provider: LocalizationProviderStatus::default(),
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

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct MapIdentity {
    pub map_id: String,
    pub map_revision: String,
    pub frame_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct PoseWithCovariance2d {
    pub pose: Pose2d,
    /// Row-major 3x3 covariance for x, y, and yaw.
    pub covariance: Vec<f64>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum LocalizationStatus {
    Initializing,
    Tracking,
    Degraded,
    Stale,
    Lost,
    #[default]
    Unavailable,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct LocalizationHealth {
    pub status: LocalizationStatus,
    #[serde(default)]
    pub last_update_ms: Option<u128>,
    pub message: String,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct LocalizationFrame {
    #[serde(default = "default_localization_frame_version")]
    pub version: String,
    pub ts_ms: u128,
    #[serde(default)]
    pub map: MapIdentity,
    #[serde(default)]
    pub pose: Option<PoseWithCovariance2d>,
    #[serde(default)]
    pub health: LocalizationHealth,
}

impl Default for LocalizationFrame {
    fn default() -> Self {
        Self {
            version: default_localization_frame_version(),
            ts_ms: 0,
            map: MapIdentity::default(),
            pose: None,
            health: LocalizationHealth::default(),
        }
    }
}

impl LocalizationFrame {
    pub fn validate(&self) -> Result<(), LocalizationContractError> {
        if self.version != LOCALIZATION_FRAME_VERSION {
            return Err(LocalizationContractError::UnsupportedVersion);
        }
        let needs_pose = matches!(
            self.health.status,
            LocalizationStatus::Tracking | LocalizationStatus::Degraded | LocalizationStatus::Stale
        );
        if needs_pose && self.pose.is_none() {
            return Err(LocalizationContractError::MissingPose);
        }
        if matches!(self.health.status, LocalizationStatus::Lost)
            && self
                .health
                .error
                .as_deref()
                .is_none_or(|error| error.trim().is_empty())
        {
            return Err(LocalizationContractError::MissingError);
        }
        if needs_pose
            && (self.map.map_id.trim().is_empty()
                || self.map.map_revision.trim().is_empty()
                || self.map.frame_id.trim().is_empty())
        {
            return Err(LocalizationContractError::EmptyMapIdentity);
        }
        if let Some(localized_pose) = &self.pose {
            if localized_pose.pose.frame_id != self.map.frame_id {
                return Err(LocalizationContractError::PoseFrameMismatch);
            }
            if localized_pose.covariance.len() != 9 {
                return Err(LocalizationContractError::InvalidCovarianceLength);
            }
            if localized_pose
                .covariance
                .iter()
                .any(|value| !value.is_finite())
            {
                return Err(LocalizationContractError::NonFiniteCovariance);
            }
            if [0, 4, 8]
                .into_iter()
                .any(|index| localized_pose.covariance[index] < 0.0)
            {
                return Err(LocalizationContractError::NegativeCovariance);
            }
            if self.health.last_update_ms != Some(localized_pose.pose.ts_ms) {
                return Err(LocalizationContractError::TimestampMismatch);
            }
        }
        Ok(())
    }
}

fn default_localization_frame_version() -> String {
    LOCALIZATION_FRAME_VERSION.to_string()
}

#[derive(Debug, PartialEq, Eq)]
pub enum LocalizationContractError {
    UnsupportedVersion,
    MissingPose,
    MissingError,
    EmptyMapIdentity,
    PoseFrameMismatch,
    InvalidCovarianceLength,
    NonFiniteCovariance,
    NegativeCovariance,
    TimestampMismatch,
}

impl std::fmt::Display for LocalizationContractError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::UnsupportedVersion => "unsupported localization frame version",
            Self::MissingPose => "localization state requires a pose",
            Self::MissingError => "lost localization state requires an error",
            Self::EmptyMapIdentity => "localized pose requires a complete map identity",
            Self::PoseFrameMismatch => "localized pose frame does not match map frame",
            Self::InvalidCovarianceLength => "localized pose covariance must contain 9 values",
            Self::NonFiniteCovariance => "localized pose covariance must be finite",
            Self::NegativeCovariance => "localized pose covariance diagonal cannot be negative",
            Self::TimestampMismatch => {
                "localization health timestamp does not match the pose timestamp"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for LocalizationContractError {}

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
    #[serde(default)]
    pub map: Option<MapIdentity>,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum OperatorSessionEventKind {
    Summary,
    OperatorOwnership,
    JoystickDrive,
    JoystickCamera,
    CameraFailure,
    CameraRecovery,
    FrameHealth,
    Telemetry,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct OperatorSessionRobot {
    pub id: String,
    pub name: String,
    pub role: String,
    pub location: String,
    pub video_transport: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct OperatorSessionEvent {
    pub offset_ms: u64,
    pub ts_ms: u128,
    pub robot_id: String,
    pub kind: OperatorSessionEventKind,
    pub data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct OperatorSessionRecording {
    pub format: String,
    pub fleet_name: String,
    pub started_at_ms: u128,
    pub ended_at_ms: u128,
    pub robots: Vec<OperatorSessionRobot>,
    pub events: Vec<OperatorSessionEvent>,
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
    #[serde(default)]
    pub map: Option<MapIdentity>,
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
    #[serde(default = "default_sensor_contract_version")]
    pub version: String,
    pub battery: BatteryStatus,
    pub odometry: OdometryStatus,
    pub camera: CameraStatus,
    pub raw_frame: RawFrameStatus,
    #[serde(default)]
    pub range_scan: RangeScanStatus,
    #[serde(default)]
    pub imu: ImuStatus,
}

fn default_sensor_contract_version() -> String {
    SENSOR_CONTRACT_VERSION.to_string()
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum SensorDataStatus {
    Available,
    Stale,
    Malformed,
    Disconnected,
    #[default]
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct PlanarRangeScan {
    /// Sensor capture time in Unix epoch milliseconds.
    pub ts_ms: u128,
    /// Coordinate frame containing the scan; angles are about positive Z.
    pub frame_id: String,
    /// Angle of the first sample in radians.
    pub angle_min_rad: f64,
    /// Angle of the last sample in radians.
    pub angle_max_rad: f64,
    /// Signed angular separation between adjacent samples in radians.
    pub angle_increment_rad: f64,
    /// Minimum valid range in meters.
    pub range_min_m: f64,
    /// Maximum valid range in meters.
    pub range_max_m: f64,
    /// Ranges in scan order. `None` is an explicitly invalid sample.
    pub ranges_m: Vec<Option<f64>>,
    /// Optional non-negative device intensity values in scan order.
    #[serde(default)]
    pub intensities: Vec<Option<f64>>,
    /// Optional measured revolutions per second for the completed scan.
    #[serde(default)]
    pub scan_rate_hz: Option<f64>,
}

impl PlanarRangeScan {
    pub fn validate(&self) -> Result<(), SensorContractError> {
        if self.frame_id.trim().is_empty() {
            return Err(SensorContractError::EmptyFrameId);
        }
        for (name, value) in [
            ("angle_min_rad", self.angle_min_rad),
            ("angle_max_rad", self.angle_max_rad),
            ("angle_increment_rad", self.angle_increment_rad),
            ("range_min_m", self.range_min_m),
            ("range_max_m", self.range_max_m),
        ] {
            if !value.is_finite() {
                return Err(SensorContractError::NonFinite(name));
            }
        }
        if self.ranges_m.is_empty() {
            return Err(SensorContractError::EmptyScan);
        }
        if self.angle_increment_rad == 0.0 {
            return Err(SensorContractError::ZeroAngleIncrement);
        }
        if self.range_min_m < 0.0 || self.range_max_m <= self.range_min_m {
            return Err(SensorContractError::InvalidRangeBounds);
        }
        let expected_angle_max =
            self.angle_min_rad + self.angle_increment_rad * (self.ranges_m.len() - 1) as f64;
        if (expected_angle_max - self.angle_max_rad).abs() > 1e-6 {
            return Err(SensorContractError::AngleCountMismatch);
        }
        if (self.angle_max_rad - self.angle_min_rad).abs() > std::f64::consts::TAU + 1e-6 {
            return Err(SensorContractError::ScanSpanExceedsFullTurn);
        }
        if !self.intensities.is_empty() && self.intensities.len() != self.ranges_m.len() {
            return Err(SensorContractError::IntensityCountMismatch);
        }
        if self
            .scan_rate_hz
            .is_some_and(|scan_rate_hz| !scan_rate_hz.is_finite() || scan_rate_hz <= 0.0)
        {
            return Err(SensorContractError::InvalidScanRate);
        }
        for (index, range) in self.ranges_m.iter().enumerate() {
            if let Some(range) = range {
                if !range.is_finite() || *range < self.range_min_m || *range > self.range_max_m {
                    return Err(SensorContractError::InvalidRangeSample(index));
                }
            }
        }
        for (index, intensity) in self.intensities.iter().enumerate() {
            if intensity.is_some_and(|value| !value.is_finite() || value < 0.0) {
                return Err(SensorContractError::InvalidIntensitySample(index));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct Vector3Si {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vector3Si {
    fn values(self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct Quaternion {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct ImuSample {
    /// Sensor capture time in Unix epoch milliseconds.
    pub ts_ms: u128,
    /// Right-handed body frame: +X forward, +Y left, +Z up.
    pub frame_id: String,
    /// Linear acceleration in meters per second squared.
    pub linear_acceleration_mps2: Vector3Si,
    /// Angular velocity in radians per second.
    pub angular_velocity_radps: Vector3Si,
    /// Optional body orientation as an approximately unit-length XYZW quaternion.
    #[serde(default)]
    pub orientation_xyzw: Option<Quaternion>,
}

impl ImuSample {
    pub fn validate(&self) -> Result<(), SensorContractError> {
        if self.frame_id.trim().is_empty() {
            return Err(SensorContractError::EmptyFrameId);
        }
        for value in self.linear_acceleration_mps2.values() {
            if !value.is_finite() {
                return Err(SensorContractError::NonFinite("linear_acceleration_mps2"));
            }
            if value.abs() > MAX_IMU_LINEAR_ACCELERATION_MPS2 {
                return Err(SensorContractError::ImuAccelerationOutOfBounds);
            }
        }
        for value in self.angular_velocity_radps.values() {
            if !value.is_finite() {
                return Err(SensorContractError::NonFinite("angular_velocity_radps"));
            }
            if value.abs() > MAX_IMU_ANGULAR_VELOCITY_RADPS {
                return Err(SensorContractError::ImuAngularVelocityOutOfBounds);
            }
        }
        if let Some(orientation) = self.orientation_xyzw {
            let values = [orientation.x, orientation.y, orientation.z, orientation.w];
            if values.iter().any(|value| !value.is_finite()) {
                return Err(SensorContractError::NonFinite("orientation_xyzw"));
            }
            let norm_squared = values.iter().map(|value| value * value).sum::<f64>();
            if !(0.25..=2.25).contains(&norm_squared) {
                return Err(SensorContractError::InvalidQuaternionNorm);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct RangeScanStatus {
    pub status: SensorDataStatus,
    pub source: String,
    #[serde(default)]
    pub last_ms: Option<u128>,
    #[serde(default)]
    pub sample: Option<PlanarRangeScan>,
    #[serde(default)]
    pub error: Option<String>,
}

impl RangeScanStatus {
    pub fn validate(&self) -> Result<(), SensorContractError> {
        validate_sensor_status(self.status, self.last_ms, self.error.as_deref())?;
        if matches!(
            self.status,
            SensorDataStatus::Available | SensorDataStatus::Stale
        ) && self.sample.is_none()
        {
            return Err(SensorContractError::MissingSample);
        }
        if let Some(sample) = &self.sample {
            sample.validate()?;
            if self.last_ms.is_some_and(|last_ms| last_ms != sample.ts_ms) {
                return Err(SensorContractError::TimestampMismatch);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct ImuStatus {
    pub status: SensorDataStatus,
    pub source: String,
    #[serde(default)]
    pub last_ms: Option<u128>,
    #[serde(default)]
    pub sample: Option<ImuSample>,
    #[serde(default)]
    pub error: Option<String>,
}

impl ImuStatus {
    pub fn validate(&self) -> Result<(), SensorContractError> {
        validate_sensor_status(self.status, self.last_ms, self.error.as_deref())?;
        if matches!(
            self.status,
            SensorDataStatus::Available | SensorDataStatus::Stale
        ) && self.sample.is_none()
        {
            return Err(SensorContractError::MissingSample);
        }
        if let Some(sample) = &self.sample {
            sample.validate()?;
            if self.last_ms.is_some_and(|last_ms| last_ms != sample.ts_ms) {
                return Err(SensorContractError::TimestampMismatch);
            }
        }
        Ok(())
    }
}

fn validate_sensor_status(
    status: SensorDataStatus,
    last_ms: Option<u128>,
    error: Option<&str>,
) -> Result<(), SensorContractError> {
    if matches!(
        status,
        SensorDataStatus::Available | SensorDataStatus::Stale
    ) && last_ms.is_none()
    {
        return Err(SensorContractError::MissingTimestamp);
    }
    if matches!(
        status,
        SensorDataStatus::Malformed | SensorDataStatus::Disconnected
    ) && error.is_none_or(|error| error.trim().is_empty())
    {
        return Err(SensorContractError::MissingError);
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum SensorContractError {
    EmptyFrameId,
    NonFinite(&'static str),
    EmptyScan,
    ZeroAngleIncrement,
    InvalidRangeBounds,
    AngleCountMismatch,
    ScanSpanExceedsFullTurn,
    IntensityCountMismatch,
    InvalidScanRate,
    InvalidRangeSample(usize),
    InvalidIntensitySample(usize),
    ImuAccelerationOutOfBounds,
    ImuAngularVelocityOutOfBounds,
    InvalidQuaternionNorm,
    MissingSample,
    MissingTimestamp,
    TimestampMismatch,
    MissingError,
}

impl std::fmt::Display for SensorContractError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyFrameId => write!(formatter, "sensor frame_id cannot be empty"),
            Self::NonFinite(field) => write!(formatter, "sensor field {field} must be finite"),
            Self::EmptyScan => write!(formatter, "range scan cannot be empty"),
            Self::ZeroAngleIncrement => {
                write!(formatter, "range scan angle increment cannot be zero")
            }
            Self::InvalidRangeBounds => {
                write!(formatter, "range scan limits must satisfy 0 <= min < max")
            }
            Self::AngleCountMismatch => {
                write!(
                    formatter,
                    "range scan angle_max does not match sample count"
                )
            }
            Self::ScanSpanExceedsFullTurn => {
                write!(formatter, "range scan span exceeds one full turn")
            }
            Self::IntensityCountMismatch => write!(
                formatter,
                "range scan intensity count does not match range count"
            ),
            Self::InvalidScanRate => {
                write!(formatter, "range scan rate must be positive and finite")
            }
            Self::InvalidRangeSample(index) => write!(
                formatter,
                "range scan sample {index} is non-finite or outside declared limits"
            ),
            Self::InvalidIntensitySample(index) => write!(
                formatter,
                "range scan intensity {index} is non-finite or negative"
            ),
            Self::ImuAccelerationOutOfBounds => {
                write!(formatter, "IMU acceleration exceeds the contract bound")
            }
            Self::ImuAngularVelocityOutOfBounds => {
                write!(formatter, "IMU angular velocity exceeds the contract bound")
            }
            Self::InvalidQuaternionNorm => {
                write!(formatter, "IMU orientation quaternion norm is invalid")
            }
            Self::MissingSample => {
                write!(
                    formatter,
                    "available or stale sensor status requires a sample"
                )
            }
            Self::MissingTimestamp => write!(
                formatter,
                "available or stale sensor status requires last_ms"
            ),
            Self::TimestampMismatch => write!(
                formatter,
                "sensor status timestamp does not match the sample timestamp"
            ),
            Self::MissingError => write!(
                formatter,
                "malformed or disconnected sensor status requires an error"
            ),
        }
    }
}

impl std::error::Error for SensorContractError {}

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
            range_scan: RangeScanStatus::default(),
            imu: ImuStatus::default(),
            localization: LocalizationFrame::default(),
            localization_provider: LocalizationProviderStatus::default(),
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
        assert_eq!(parsed.localization, LocalizationFrame::default());
    }

    #[test]
    fn planar_scan_validation_covers_angles_ranges_and_intensities() {
        let mut scan = crate::adapter::simulated_range_scan(42).sample.unwrap();
        scan.validate().unwrap();

        scan.angle_max_rad += 0.1;
        assert_eq!(
            scan.validate().unwrap_err(),
            SensorContractError::AngleCountMismatch
        );
        scan.angle_max_rad -= 0.1;
        scan.ranges_m[2] = Some(scan.range_max_m + 1.0);
        assert_eq!(
            scan.validate().unwrap_err(),
            SensorContractError::InvalidRangeSample(2)
        );
        scan.ranges_m[2] = None;
        scan.intensities.pop();
        assert_eq!(
            scan.validate().unwrap_err(),
            SensorContractError::IntensityCountMismatch
        );
    }

    #[test]
    fn imu_validation_rejects_non_finite_bounds_and_bad_quaternion() {
        let mut imu = crate::adapter::simulated_imu_sample(7).sample.unwrap();
        imu.validate().unwrap();

        imu.linear_acceleration_mps2.x = f64::NAN;
        assert_eq!(
            imu.validate().unwrap_err(),
            SensorContractError::NonFinite("linear_acceleration_mps2")
        );
        imu.linear_acceleration_mps2.x = MAX_IMU_LINEAR_ACCELERATION_MPS2 + 1.0;
        assert_eq!(
            imu.validate().unwrap_err(),
            SensorContractError::ImuAccelerationOutOfBounds
        );
        imu.linear_acceleration_mps2.x = 0.0;
        imu.orientation_xyzw = Some(Quaternion {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 2.0,
        });
        assert_eq!(
            imu.validate().unwrap_err(),
            SensorContractError::InvalidQuaternionNorm
        );
    }

    #[test]
    fn sensor_status_validates_timestamp_and_error_state() {
        let mut scan = crate::adapter::simulated_range_scan(42);
        scan.validate().unwrap();
        scan.last_ms = Some(41);
        assert_eq!(
            scan.validate().unwrap_err(),
            SensorContractError::TimestampMismatch
        );

        let disconnected = ImuStatus {
            status: SensorDataStatus::Disconnected,
            source: "fixture".to_string(),
            last_ms: Some(42),
            sample: None,
            error: Some("device disconnected".to_string()),
        };
        disconnected.validate().unwrap();
    }

    #[test]
    fn older_sensor_snapshot_defaults_new_contracts() {
        let old = serde_json::json!({
            "battery": {"status": "available", "voltage_v": 12.3},
            "odometry": {"status": "available", "left_m": 0.0, "right_m": 0.0},
            "camera": {
                "status": "simulated",
                "health": "healthy",
                "stream_url": null,
                "snapshot_url": null
            },
            "raw_frame": {"status": "available", "source": "legacy", "last_ms": 1}
        });

        let parsed: SensorSnapshot = serde_json::from_value(old).unwrap();

        assert_eq!(parsed.version, SENSOR_CONTRACT_VERSION);
        assert_eq!(parsed.range_scan.status, SensorDataStatus::Unavailable);
        assert_eq!(parsed.imu.status, SensorDataStatus::Unavailable);
    }

    #[test]
    fn localization_frame_validates_version_pose_covariance_and_health() {
        let mut frame = LocalizationFrame {
            version: LOCALIZATION_FRAME_VERSION.to_string(),
            ts_ms: 42,
            map: MapIdentity {
                map_id: "map-a".to_string(),
                map_revision: "revision-1".to_string(),
                frame_id: "map".to_string(),
            },
            pose: Some(PoseWithCovariance2d {
                pose: Pose2d {
                    ts_ms: 42,
                    frame_id: "map".to_string(),
                    x_m: 1.0,
                    y_m: 2.0,
                    yaw_rad: 0.25,
                },
                covariance: vec![0.01, 0.0, 0.0, 0.0, 0.02, 0.0, 0.0, 0.0, 0.03],
            }),
            health: LocalizationHealth {
                status: LocalizationStatus::Tracking,
                last_update_ms: Some(42),
                message: "tracking".to_string(),
                error: None,
            },
        };
        frame.validate().unwrap();

        frame.version = "old".to_string();
        assert_eq!(
            frame.validate().unwrap_err(),
            LocalizationContractError::UnsupportedVersion
        );
        frame.version = LOCALIZATION_FRAME_VERSION.to_string();
        frame.pose.as_mut().unwrap().covariance.pop();
        assert_eq!(
            frame.validate().unwrap_err(),
            LocalizationContractError::InvalidCovarianceLength
        );
        frame.pose = None;
        assert_eq!(
            frame.validate().unwrap_err(),
            LocalizationContractError::MissingPose
        );

        frame.health.status = LocalizationStatus::Lost;
        frame.health.error = Some("provider disconnected".to_string());
        frame.validate().unwrap();
    }

    #[test]
    fn checked_in_localization_fixture_covers_tracking_stale_and_lost() {
        #[derive(Deserialize)]
        struct Fixture {
            format: String,
            frames: Vec<LocalizationFrame>,
        }

        let fixture: Fixture = serde_json::from_str(include_str!(
            "../examples/replay/localization-contract-states.json"
        ))
        .unwrap();

        assert_eq!(fixture.format, "leash-localization-fixture-v1");
        assert_eq!(fixture.frames.len(), 3);
        for frame in &fixture.frames {
            frame.validate().unwrap();
        }
        assert_eq!(
            fixture.frames[0].health.status,
            LocalizationStatus::Tracking
        );
        assert_eq!(fixture.frames[1].health.status, LocalizationStatus::Stale);
        assert_eq!(fixture.frames[2].health.status, LocalizationStatus::Lost);
    }

    #[test]
    fn checked_in_sensor_state_fixture_is_valid() {
        #[derive(Deserialize)]
        struct Fixture {
            format: String,
            cases: Vec<FixtureCase>,
        }
        #[derive(Deserialize)]
        struct FixtureCase {
            name: String,
            range_scan: RangeScanStatus,
            imu: ImuStatus,
        }

        let fixture: Fixture = serde_json::from_str(include_str!(
            "../examples/replay/sensor-contract-states.json"
        ))
        .unwrap();

        assert_eq!(fixture.format, "leash-sensor-contract-fixture-v1");
        assert_eq!(fixture.cases.len(), 4);
        for case in &fixture.cases {
            case.range_scan
                .validate()
                .unwrap_or_else(|error| panic!("{} range scan: {error}", case.name));
            case.imu
                .validate()
                .unwrap_or_else(|error| panic!("{} IMU: {error}", case.name));
        }
        assert_eq!(fixture.cases[0].name, "valid");
        assert_eq!(
            fixture.cases[1].range_scan.status,
            SensorDataStatus::Malformed
        );
        assert_eq!(fixture.cases[2].imu.status, SensorDataStatus::Stale);
        assert_eq!(
            fixture.cases[3].range_scan.status,
            SensorDataStatus::Disconnected
        );
    }
}
