use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(feature = "waveshare-ugv")]
use std::io::{BufRead, BufReader, ErrorKind, Write};
#[cfg(feature = "waveshare-ugv")]
use std::thread;

#[cfg(feature = "waveshare-ugv")]
use anyhow::Context;
use anyhow::{anyhow, Result};
use parking_lot::{Mutex, RwLock};
#[cfg(feature = "waveshare-ugv")]
use serde_json::json;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::{sync::broadcast, time};
use tracing::{debug, warn};

#[cfg(feature = "waveshare-ugv")]
use crate::adapter::waveshare_drive_values;

#[cfg(feature = "mavlink-drone")]
use crate::types::DroneCommandStatus;
#[cfg(feature = "manipulator")]
use crate::types::{
    ManipulatorCommandStatus, ManipulatorJoint, ManipulatorJointState, MANIPULATOR_SCHEMA_VERSION,
};
use crate::{
    accelerator::{resolve_accelerator, AcceleratorStatus},
    adapter::{GimbalAdapter, MobileBaseAdapter},
    capability::{default_capability_descriptors, CapabilityRegistry},
    config::{HarnessConfig, Profile},
    memory::{
        default_spatial_memory_path, SpatialMemoryQuery, SpatialMemoryStore, SpatialMemoryTag,
    },
    module::{default_module_graph, ModuleCoordinator, ModuleGraph},
    navigation::{navigation_path_for_memory, NavigationStore, PatrolZoneSpec, WaypointSpec},
    perception::PerceptionRuntime,
    replay::{replay_telemetry_source, ReplayPlayback},
    transport::{new_stream_transport, StreamSubscriber, StreamTransport},
    types::{
        AgentMessage, AgentModelResponse, BatteryStatus, CameraAimOutcome, CameraStatus,
        Capabilities, CaptureResult, CommandOverlay, CommandStreamState, CostmapFrame,
        DriveOutcome, Health, ImageObservation, MapMetadata, MotionEvent, MotionEventKind,
        OccupancyGridFrame, OdometryStatus, OperatorTokenStatus, PatrolStatus, PatrolStrategy,
        PatrolZoneList, PlannerGoal, PlannerStatus, PointCloudMetadata, Pose2d, RawFrameStatus,
        ResourceSample, SafetyStreamState, SavedWaypointList, SensorSnapshot, SpatialMemoryStatus,
        SpeedMode, TelemetryFrame, TelemetryStreamFrame, Twist2d, VisionResult, VisualizationFrame,
        VisualizationPath, COST_FREE, COST_LETHAL, OCCUPANCY_FREE, OCCUPANCY_OCCUPIED,
        VISUALIZATION_FRAME_VERSION,
    },
};

const AGENT_MESSAGE_LIMIT: usize = 128;
const DASHBOARD_EVENT_LIMIT: usize = 64;
const PLANNER_GRID_WIDTH: usize = 4;
const PLANNER_GRID_HEIGHT: usize = 4;
const PLANNER_GRID_CELLS: usize = PLANNER_GRID_WIDTH * PLANNER_GRID_HEIGHT;
const PLANNER_RESOLUTION_M: f64 = 0.25;
const PLANNER_ORIGIN_X_M: f64 = -0.5;
const PLANNER_ORIGIN_Y_M: f64 = -0.5;
const PLANNER_BLOCKED_CELLS: &[(usize, usize)] = &[(1, 1)];
const PLANNER_STEP_CMD: f64 = 0.2;
pub const CAMERA_PAN_MIN_DEG: f64 = -180.0;
pub const CAMERA_PAN_MAX_DEG: f64 = 180.0;
pub const CAMERA_TILT_MIN_DEG: f64 = -30.0;
pub const CAMERA_TILT_MAX_DEG: f64 = 90.0;
static HARNESS_INSTANCE_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
struct PatrolState {
    status: PatrolStatus,
    visited: [bool; PLANNER_GRID_CELLS],
}

impl Default for PatrolState {
    fn default() -> Self {
        Self {
            status: PatrolStatus::default(),
            visited: [false; PLANNER_GRID_CELLS],
        }
    }
}

impl PatrolState {
    fn mark_visited(&mut self, cell: PlannerCell) {
        self.visited[cell.index()] = true;
        self.status.visited_cells = visited_cell_labels(&self.visited);
    }

    fn status_with_visited(&self) -> PatrolStatus {
        let mut status = self.status.clone();
        status.visited_cells = visited_cell_labels(&self.visited);
        status
    }
}

trait RobotDriver: MobileBaseAdapter + GimbalAdapter {
    #[cfg(feature = "waveshare-ugv")]
    fn telemetry_reader(&self) -> Result<Option<Box<dyn serialport::SerialPort>>> {
        Ok(None)
    }

    #[cfg(feature = "waveshare-ugv")]
    fn enable_telemetry(&self) -> Result<()> {
        Ok(())
    }

    #[cfg(feature = "waveshare-ugv")]
    fn request_telemetry(&self) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
struct SimDriver;

impl RobotDriver for SimDriver {}

impl MobileBaseAdapter for SimDriver {
    fn drive(&self, left: f64, right: f64) -> Result<()> {
        debug!(left, right, "sim drive");
        Ok(())
    }
}

impl GimbalAdapter for SimDriver {
    fn aim_camera(&self, pan_deg: f64, tilt_deg: f64, speed: u32, accel: u32) -> Result<()> {
        debug!(pan_deg, tilt_deg, speed, accel, "sim camera aim");
        Ok(())
    }
}

#[derive(Debug)]
struct ReplayDriver;

impl RobotDriver for ReplayDriver {}

impl MobileBaseAdapter for ReplayDriver {
    fn drive(&self, left: f64, right: f64) -> Result<()> {
        debug!(left, right, "replay drive ignored");
        Ok(())
    }
}

impl GimbalAdapter for ReplayDriver {
    fn aim_camera(&self, pan_deg: f64, tilt_deg: f64, speed: u32, accel: u32) -> Result<()> {
        debug!(pan_deg, tilt_deg, speed, accel, "replay camera aim ignored");
        Ok(())
    }
}

#[cfg(feature = "mavlink-drone")]
#[derive(Debug)]
struct MavlinkDroneDriver {
    endpoint: String,
}

#[cfg(feature = "mavlink-drone")]
impl MavlinkDroneDriver {
    fn open(config: &HarnessConfig) -> Result<Self> {
        Ok(Self {
            endpoint: config.mavlink_endpoint.clone(),
        })
    }
}

#[cfg(feature = "mavlink-drone")]
impl RobotDriver for MavlinkDroneDriver {}

#[cfg(feature = "mavlink-drone")]
impl MobileBaseAdapter for MavlinkDroneDriver {
    fn drive(&self, left: f64, right: f64) -> Result<()> {
        debug!(
            endpoint = self.endpoint,
            left, right, "mavlink drone skeleton ignored differential drive command"
        );
        Ok(())
    }
}

#[cfg(feature = "mavlink-drone")]
impl GimbalAdapter for MavlinkDroneDriver {}

#[cfg(feature = "manipulator")]
#[derive(Debug)]
struct ManipulatorDriver;

#[cfg(feature = "manipulator")]
impl ManipulatorDriver {
    fn open(_config: &HarnessConfig) -> Self {
        Self
    }
}

#[cfg(feature = "manipulator")]
impl RobotDriver for ManipulatorDriver {}

#[cfg(feature = "manipulator")]
impl MobileBaseAdapter for ManipulatorDriver {
    fn drive(&self, left: f64, right: f64) -> Result<()> {
        debug!(
            left,
            right, "manipulator skeleton ignored differential drive command"
        );
        Ok(())
    }
}

#[cfg(feature = "manipulator")]
impl GimbalAdapter for ManipulatorDriver {}

#[cfg(feature = "waveshare-ugv")]
struct WaveshareUgvDriver {
    writer: Mutex<Box<dyn serialport::SerialPort>>,
    drive_invert: bool,
    drive_swap: bool,
}

#[cfg(feature = "waveshare-ugv")]
impl WaveshareUgvDriver {
    fn open(config: &HarnessConfig) -> Result<Self> {
        let port = serialport::new(&config.serial_port, config.serial_baud)
            .timeout(Duration::from_millis(200))
            .open()
            .with_context(|| {
                format!(
                    "open Waveshare UGV serial port {} @ {}",
                    config.serial_port, config.serial_baud
                )
            })?;
        Ok(Self {
            writer: Mutex::new(port),
            drive_invert: config.drive_invert,
            drive_swap: config.drive_swap,
        })
    }

    fn write_json(&self, payload: Value, context: &'static str) -> Result<()> {
        let line = payload.to_string() + "\n";
        let mut writer = self.writer.lock();
        writer.write_all(line.as_bytes()).context(context)?;
        writer.flush().context(context)?;
        Ok(())
    }
}

#[cfg(feature = "waveshare-ugv")]
impl RobotDriver for WaveshareUgvDriver {
    fn telemetry_reader(&self) -> Result<Option<Box<dyn serialport::SerialPort>>> {
        let writer = self.writer.lock();
        writer
            .try_clone()
            .map(Some)
            .context("clone Waveshare UGV serial port for telemetry")
    }

    fn enable_telemetry(&self) -> Result<()> {
        self.write_json(
            json!({"T": 142, "cmd": 100}),
            "set Waveshare UGV telemetry interval",
        )?;
        self.write_json(
            json!({"T": 131, "cmd": 1}),
            "enable Waveshare UGV telemetry flow",
        )?;
        self.request_telemetry()
    }

    fn request_telemetry(&self) -> Result<()> {
        self.write_json(json!({"T": 130}), "request Waveshare UGV base telemetry")
    }
}

#[cfg(feature = "waveshare-ugv")]
impl MobileBaseAdapter for WaveshareUgvDriver {
    fn drive(&self, left: f64, right: f64) -> Result<()> {
        let (left, right) = waveshare_drive_values(left, right, self.drive_invert, self.drive_swap);
        self.write_json(
            json!({"T": 1, "L": left, "R": right}),
            "write Waveshare UGV drive command",
        )
    }
}

#[cfg(feature = "waveshare-ugv")]
impl GimbalAdapter for WaveshareUgvDriver {
    fn aim_camera(&self, pan_deg: f64, tilt_deg: f64, speed: u32, accel: u32) -> Result<()> {
        self.write_json(
            json!({
                "T": 133,
                "X": pan_deg,
                "Y": tilt_deg,
                "SPD": speed,
                "ACC": accel
            }),
            "write Waveshare UGV camera gimbal command",
        )
    }
}

#[derive(Debug, Clone)]
struct PilotSession {
    expires_at: Instant,
    speed_mode: SpeedMode,
}

#[derive(Debug, Clone)]
struct CommandState {
    left_cmd: f64,
    right_cmd: f64,
    last_cmd_at: Option<Instant>,
    active_session_id: Option<String>,
    speed_mode: SpeedMode,
    estop: bool,
    stopped_by_deadman: bool,
    soft_odometry_limited: bool,
}

impl Default for CommandState {
    fn default() -> Self {
        Self {
            left_cmd: 0.0,
            right_cmd: 0.0,
            last_cmd_at: None,
            active_session_id: None,
            speed_mode: SpeedMode::default(),
            estop: false,
            stopped_by_deadman: false,
            soft_odometry_limited: false,
        }
    }
}

#[derive(Debug, Clone)]
struct RawTelemetry {
    battery_v: Option<f64>,
    battery_pct: Option<f64>,
    odometry_left: Option<f64>,
    odometry_right: Option<f64>,
    source: String,
    last_raw_frame_ms: Option<u128>,
    last_raw_payload: Option<Value>,
}

impl RawTelemetry {
    fn sim() -> Self {
        Self {
            battery_v: Some(12.3),
            battery_pct: battery_percent_from_voltage(12.3),
            odometry_left: Some(0.0),
            odometry_right: Some(0.0),
            source: "sim".to_string(),
            last_raw_frame_ms: Some(now_ms()),
            last_raw_payload: None,
        }
    }

    fn physical(source: &str) -> Self {
        Self {
            battery_v: None,
            battery_pct: None,
            odometry_left: None,
            odometry_right: None,
            source: source.to_string(),
            last_raw_frame_ms: None,
            last_raw_payload: None,
        }
    }

    fn replay() -> Self {
        Self {
            battery_v: None,
            battery_pct: None,
            odometry_left: None,
            odometry_right: None,
            source: "replay".to_string(),
            last_raw_frame_ms: None,
            last_raw_payload: None,
        }
    }
}

#[derive(Clone)]
pub struct Harness {
    config: HarnessConfig,
    started_at: Instant,
    driver: Arc<dyn RobotDriver>,
    command: Arc<Mutex<CommandState>>,
    sessions: Arc<Mutex<HashMap<String, PilotSession>>>,
    raw: Arc<RwLock<RawTelemetry>>,
    telemetry_tx: broadcast::Sender<TelemetryFrame>,
    stream_transport: Arc<dyn StreamTransport>,
    agent_messages: Arc<Mutex<VecDeque<AgentMessage>>>,
    dashboard_events: Arc<Mutex<VecDeque<String>>>,
    planner: Arc<Mutex<PlannerStatus>>,
    patrol: Arc<Mutex<PatrolState>>,
    spatial_memory: Arc<SpatialMemoryStore>,
    navigation: Arc<NavigationStore>,
    perception: PerceptionRuntime,
    agent_seq: Arc<AtomicU64>,
    replay: Option<ReplayPlayback>,
    coordinator: Arc<RwLock<ModuleCoordinator>>,
    accelerator: AcceleratorStatus,
}

impl Harness {
    pub fn new(config: HarnessConfig) -> Result<Self> {
        Self::new_inner(config, None)
    }

    #[cfg(test)]
    pub(crate) fn new_with_memory_path(config: HarnessConfig, path: PathBuf) -> Result<Self> {
        Self::new_inner(config, Some(path))
    }

    fn new_inner(config: HarnessConfig, memory_path: Option<PathBuf>) -> Result<Self> {
        config.validate()?;
        let accelerator = resolve_accelerator(config.accelerator, config.require_accelerator)?;
        let instance_id = HARNESS_INSTANCE_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
        let memory_path =
            memory_path.unwrap_or_else(|| default_spatial_memory_path(&config, instance_id));
        let navigation_path = navigation_path_for_memory(&memory_path);
        let spatial_memory = Arc::new(SpatialMemoryStore::open(memory_path)?);
        let navigation = Arc::new(NavigationStore::open(navigation_path)?);

        let driver: Arc<dyn RobotDriver> = match config.profile {
            Profile::Sim => Arc::new(SimDriver),
            Profile::Replay => Arc::new(ReplayDriver),
            Profile::WaveshareUgv => open_physical_driver(&config)?,
            Profile::MavlinkDrone => open_physical_driver(&config)?,
            Profile::Manipulator => open_physical_driver(&config)?,
        };

        let raw = match config.profile {
            Profile::Sim => RawTelemetry::sim(),
            Profile::Replay => RawTelemetry::replay(),
            Profile::WaveshareUgv => RawTelemetry::physical("waveshare-ugv"),
            Profile::MavlinkDrone => RawTelemetry::physical("mavlink-drone"),
            Profile::Manipulator => RawTelemetry::physical("manipulator"),
        };

        let replay = config
            .replay_source
            .as_ref()
            .map(|path| ReplayPlayback::from_path(path, config.replay_speed))
            .transpose()?;

        let capabilities = default_capability_descriptors()
            .into_iter()
            .map(|descriptor| descriptor.name)
            .collect();
        let mut coordinator = ModuleCoordinator::new(default_module_graph(&config, capabilities));
        coordinator.start()?;

        let (telemetry_tx, _) = broadcast::channel(128);
        let stream_transport = new_stream_transport(config.stream_transport);
        let harness = Self {
            config,
            started_at: Instant::now(),
            driver,
            command: Arc::new(Mutex::new(CommandState::default())),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            raw: Arc::new(RwLock::new(raw)),
            telemetry_tx,
            stream_transport,
            agent_messages: Arc::new(Mutex::new(VecDeque::new())),
            dashboard_events: Arc::new(Mutex::new(VecDeque::new())),
            planner: Arc::new(Mutex::new(PlannerStatus::default())),
            patrol: Arc::new(Mutex::new(PatrolState::default())),
            spatial_memory,
            navigation,
            perception: PerceptionRuntime::fake(),
            agent_seq: Arc::new(AtomicU64::new(0)),
            replay,
            coordinator: Arc::new(RwLock::new(coordinator)),
            accelerator,
        };
        harness.spawn_deadman();
        harness.spawn_planner_loop();
        harness.spawn_patrol_loop();
        #[cfg(feature = "waveshare-ugv")]
        harness.spawn_waveshare_telemetry();
        harness.spawn_telemetry_loop();
        Ok(harness)
    }

    pub fn config(&self) -> &HarnessConfig {
        &self.config
    }

    pub fn subscribe_telemetry(&self) -> broadcast::Receiver<TelemetryFrame> {
        self.telemetry_tx.subscribe()
    }

    pub fn subscribe_stream(&self, stream: &str) -> Result<StreamSubscriber> {
        self.stream_transport.subscribe(stream)
    }

    pub fn stream_transport(&self) -> Arc<dyn StreamTransport> {
        self.stream_transport.clone()
    }

    pub fn capability_registry(&self) -> CapabilityRegistry {
        CapabilityRegistry::new(self.clone())
    }

    pub fn submit_agent_message(
        &self,
        source: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<AgentMessage> {
        let text = text.into();
        let text = text.trim();
        if text.is_empty() {
            return Err(anyhow!("agent message text cannot be empty"));
        }
        let source = source.into();
        let source = match source.trim() {
            "" => "unknown".to_string(),
            source => source.to_string(),
        };
        let message = AgentMessage {
            id: self.agent_seq.fetch_add(1, Ordering::Relaxed) + 1,
            ts_ms: now_ms(),
            source,
            text: text.to_string(),
        };

        {
            let mut messages = self.agent_messages.lock();
            if messages.len() == AGENT_MESSAGE_LIMIT {
                messages.pop_front();
            }
            messages.push_back(message.clone());
        }

        if let Ok(payload) = serde_json::to_value(&message) {
            let _ = self.stream_transport.publish("agent", payload);
        }
        tracing::info!(
            message_id = message.id,
            source = %message.source,
            "agent message queued"
        );
        Ok(message)
    }

    pub fn agent_messages(&self) -> Vec<AgentMessage> {
        self.agent_messages.lock().iter().cloned().collect()
    }

    pub fn record_dashboard_event(&self, event: impl Into<String>) {
        let mut events = self.dashboard_events.lock();
        if events.len() == DASHBOARD_EVENT_LIMIT {
            events.pop_front();
        }
        events.push_back(format!("{} {}", now_ms(), event.into()));
    }

    pub fn dashboard_events(&self) -> Vec<String> {
        self.dashboard_events.lock().iter().cloned().collect()
    }

    pub fn planner_status(&self) -> PlannerStatus {
        self.planner.lock().clone()
    }

    pub fn set_planner_goal(&self, goal: PlannerGoal) -> Result<PlannerStatus> {
        if self.config.profile != Profile::Sim {
            return Err(anyhow!("planner is only available for the sim profile"));
        }
        if goal.frame_id != "map" {
            return Err(anyhow!("planner goals must use frame_id 'map'"));
        }
        if goal.tolerance_m <= 0.0 {
            return Err(anyhow!("planner goal tolerance_m must be positive"));
        }

        let ts_ms = now_ms();
        let current = self.current_sim_pose(ts_ms);
        let Some(start) = planner_cell_for_xy(current.x_m, current.y_m) else {
            return Err(anyhow!("current sim pose is outside the planner grid"));
        };
        let Some(goal_cell) = planner_cell_for_xy(goal.x_m, goal.y_m) else {
            return Err(anyhow!("planner goal is outside the planner grid"));
        };

        let Some(cells) = planner_route(start, goal_cell) else {
            let status = PlannerStatus {
                ok: false,
                active: false,
                status: "blocked".to_string(),
                message: "planner goal is blocked by the sim occupancy grid".to_string(),
                goal: Some(goal),
                path: VisualizationPath {
                    ts_ms,
                    frame_id: "map".to_string(),
                    poses: Vec::new(),
                },
                last_drive: None,
            };
            *self.planner.lock() = status.clone();
            return Ok(status);
        };

        let status = PlannerStatus {
            ok: true,
            active: true,
            status: "active".to_string(),
            message: "planner goal accepted".to_string(),
            goal: Some(goal),
            path: VisualizationPath {
                ts_ms,
                frame_id: "map".to_string(),
                poses: cells
                    .into_iter()
                    .map(|cell| planner_pose_for_cell(cell, ts_ms))
                    .collect(),
            },
            last_drive: None,
        };
        *self.planner.lock() = status;
        self.planner_step();
        Ok(self.planner_status())
    }

    pub fn cancel_planner_goal(&self) -> Result<PlannerStatus> {
        self.cancel_patrol_state("stopped", "patrol stopped by planner cancel");
        self.cancel_planner_state("cancelled", "planner goal cancelled");
        let outcome = self.stop_without_planner_cancel()?;
        let mut planner = self.planner.lock();
        planner.last_drive = Some(outcome);
        Ok(planner.clone())
    }

    pub fn patrol_status(&self) -> PatrolStatus {
        self.patrol.lock().status_with_visited()
    }

    pub fn tag_spatial_memory(&self, tag: SpatialMemoryTag) -> Result<SpatialMemoryStatus> {
        self.spatial_memory.tag(tag)
    }

    pub fn spatial_memory(&self) -> SpatialMemoryStatus {
        self.spatial_memory.list()
    }

    pub fn query_spatial_memory(&self, query: SpatialMemoryQuery) -> Result<SpatialMemoryStatus> {
        self.spatial_memory.query(query)
    }

    pub fn clear_spatial_memory(&self) -> Result<SpatialMemoryStatus> {
        self.spatial_memory.clear()
    }

    pub fn waypoints(&self) -> SavedWaypointList {
        self.navigation.waypoints()
    }

    pub fn create_waypoint(&self, spec: WaypointSpec) -> Result<SavedWaypointList> {
        self.navigation.create_waypoint(spec)
    }

    pub fn update_waypoint(&self, spec: WaypointSpec) -> Result<SavedWaypointList> {
        self.navigation.update_waypoint(spec)
    }

    pub fn delete_waypoint(&self, id: &str) -> Result<SavedWaypointList> {
        self.navigation.delete_waypoint(id)
    }

    pub fn patrol_zones(&self) -> PatrolZoneList {
        self.navigation.zones()
    }

    pub fn create_patrol_zone(&self, spec: PatrolZoneSpec) -> Result<PatrolZoneList> {
        self.navigation.create_zone(spec)
    }

    pub fn update_patrol_zone(&self, spec: PatrolZoneSpec) -> Result<PatrolZoneList> {
        self.navigation.update_zone(spec)
    }

    pub fn delete_patrol_zone(&self, id: &str) -> Result<PatrolZoneList> {
        self.navigation.delete_zone(id)
    }

    pub fn start_patrol_zone(&self, zone_id: &str, speed_mode: SpeedMode) -> Result<PatrolStatus> {
        if !matches!(self.config.profile, Profile::Sim | Profile::Replay) {
            return Err(anyhow!(
                "patrol zones are only available for sim and replay profiles"
            ));
        }
        if self.command.lock().estop {
            return Err(anyhow!("estop is latched; reset it before starting patrol"));
        }
        let zone = self
            .navigation
            .zone(zone_id)
            .ok_or_else(|| anyhow!("patrol zone '{zone_id}' does not exist"))?;
        let waypoint_id = zone
            .waypoint_ids
            .first()
            .ok_or_else(|| anyhow!("patrol zone '{zone_id}' has no waypoints"))?;
        let waypoint = self
            .navigation
            .waypoint(waypoint_id)
            .ok_or_else(|| anyhow!("patrol zone waypoint '{waypoint_id}' does not exist"))?;
        let goal = PlannerGoal {
            frame_id: waypoint.frame_id,
            x_m: waypoint.x_m,
            y_m: waypoint.y_m,
            tolerance_m: waypoint.tolerance_m,
            speed_mode,
        };

        let planner = if self.config.profile == Profile::Sim {
            self.set_planner_goal(goal.clone())?
        } else {
            let status = PlannerStatus {
                ok: true,
                active: true,
                status: "replay".to_string(),
                message: "replay patrol zone selected without actuation".to_string(),
                goal: Some(goal.clone()),
                path: VisualizationPath {
                    ts_ms: now_ms(),
                    frame_id: zone.frame_id.clone(),
                    poses: zone
                        .waypoint_ids
                        .iter()
                        .filter_map(|id| self.navigation.waypoint(id))
                        .map(|waypoint| Pose2d {
                            ts_ms: now_ms(),
                            frame_id: waypoint.frame_id,
                            x_m: waypoint.x_m,
                            y_m: waypoint.y_m,
                            yaw_rad: 0.0,
                        })
                        .collect(),
                },
                last_drive: None,
            };
            *self.planner.lock() = status.clone();
            status
        };

        let mut patrol = self.patrol.lock();
        patrol.status = patrol_status(PatrolStatusUpdate {
            ok: planner.ok,
            active: planner.active,
            status: &planner.status,
            message: if self.config.profile == Profile::Replay {
                "replay patrol zone selected without actuation"
            } else {
                "patrol zone goal accepted"
            },
            strategy: Some(PatrolStrategy::Coverage),
            speed_mode,
            goal: Some(goal),
            path: planner.path,
        });
        patrol.status.zone_id = Some(zone.id);
        patrol.status.waypoint_index = Some(0);
        Ok(patrol.status_with_visited())
    }

    pub fn start_patrol(
        &self,
        strategy: PatrolStrategy,
        speed_mode: SpeedMode,
    ) -> Result<PatrolStatus> {
        if self.config.profile != Profile::Sim {
            return Err(anyhow!("patrol is only available for the sim profile"));
        }

        let ts_ms = now_ms();
        let current = self.current_sim_pose(ts_ms);
        let Some(start) = planner_cell_for_xy(current.x_m, current.y_m) else {
            return Err(anyhow!("current sim pose is outside the patrol grid"));
        };

        let Some(goal_cell) = ({
            let mut patrol = self.patrol.lock();
            patrol.mark_visited(start);
            select_patrol_goal(strategy, start, &patrol.visited)
        }) else {
            let mut patrol = self.patrol.lock();
            patrol.status = patrol_status(PatrolStatusUpdate {
                ok: false,
                active: false,
                status: "no-goal",
                message: "patrol could not find a safe reachable goal",
                strategy: Some(strategy),
                speed_mode,
                goal: None,
                path: VisualizationPath {
                    ts_ms,
                    frame_id: "map".to_string(),
                    poses: Vec::new(),
                },
            });
            return Ok(patrol.status_with_visited());
        };

        let goal = planner_goal_for_cell(goal_cell, speed_mode);
        let planner = self.set_planner_goal(goal.clone())?;
        let mut patrol = self.patrol.lock();
        if planner.ok && planner.active {
            patrol.status = patrol_status(PatrolStatusUpdate {
                ok: true,
                active: true,
                status: "active",
                message: "patrol goal accepted",
                strategy: Some(strategy),
                speed_mode,
                goal: Some(goal),
                path: planner.path,
            });
        } else {
            patrol.status = patrol_status(PatrolStatusUpdate {
                ok: false,
                active: false,
                status: &planner.status,
                message: &planner.message,
                strategy: Some(strategy),
                speed_mode,
                goal: Some(goal),
                path: planner.path,
            });
        }
        Ok(patrol.status_with_visited())
    }

    pub fn stop_patrol(&self) -> Result<PatrolStatus> {
        self.cancel_patrol_state("stopped", "patrol stopped");
        let planner = self.cancel_planner_goal()?;
        let mut patrol = self.patrol.lock();
        patrol.status.path = planner.path;
        Ok(patrol.status_with_visited())
    }

    #[cfg(test)]
    fn mark_all_patrol_cells_visited_for_test(&self) {
        let mut patrol = self.patrol.lock();
        for index in 0..PLANNER_GRID_CELLS {
            let cell = PlannerCell {
                col: index % PLANNER_GRID_WIDTH,
                row: index / PLANNER_GRID_WIDTH,
            };
            if patrol_cell_clear(cell) {
                patrol.visited[index] = true;
            }
        }
        patrol.status.visited_cells = visited_cell_labels(&patrol.visited);
    }

    pub fn agent_model_response(&self, text: &str) -> Result<Option<AgentModelResponse>> {
        crate::agent::complete(&self.config, text)
    }

    pub fn module_graph(&self) -> ModuleGraph {
        self.coordinator.read().graph()
    }

    pub fn health(&self) -> Health {
        let command = self.command.lock().clone();
        let coordinator = self.coordinator.read();
        Health {
            ok: coordinator.is_healthy(),
            mode: self.runtime_mode().to_string(),
            replay: self.replay.is_some(),
            role: self.config.role.clone(),
            profile: self.config.profile.as_str().to_string(),
            uptime_ms: self.started_at.elapsed().as_millis(),
            estop: command.estop,
            deadman_ok: !command.stopped_by_deadman,
            physical_actuation_enabled: self.physical_actuation_enabled(),
            operator_token: self.operator_token_status(),
            accelerator: self.accelerator.clone(),
            modules: coordinator.graph().modules,
        }
    }

    pub fn capabilities(&self) -> Capabilities {
        Capabilities {
            ok: true,
            mode: self.runtime_mode().to_string(),
            replay: self.replay.is_some(),
            role: self.config.role.clone(),
            profile: self.config.profile.as_str().to_string(),
            physical: self.config.profile.is_physical(),
            adapter: crate::stack::adapter_profile_for_profile(self.config.profile),
            stream_transport: self.config.stream_transport.as_str().to_string(),
            endpoints: vec![
                "GET /".to_string(),
                "GET /dashboard".to_string(),
                "POST /dashboard/authorize".to_string(),
                "POST /dashboard/stop".to_string(),
                "POST /dashboard/estop".to_string(),
                "POST /dashboard/estop-reset".to_string(),
                "POST /dashboard/capture".to_string(),
                "GET /health".to_string(),
                "GET /capabilities".to_string(),
                "GET /telemetry".to_string(),
                "GET /events/telemetry".to_string(),
                "GET /sse/telemetry".to_string(),
                "GET /sensors".to_string(),
                "GET /camera/status".to_string(),
                "GET /camera/stream/health".to_string(),
                "POST /camera/stream/recover".to_string(),
                "GET /camera/snapshot".to_string(),
                "GET /camera/stream.mjpg".to_string(),
                "GET /agent".to_string(),
                "GET /agent/messages".to_string(),
                "POST /agent/messages".to_string(),
                "POST /pilot/authorize".to_string(),
                "GET /waypoints".to_string(),
                "GET /patrol/zones".to_string(),
                "POST /patrol/zones/:zone_id/start".to_string(),
                "GET /patrol/status".to_string(),
                "POST /patrol/stop".to_string(),
                "POST /drive".to_string(),
                "POST /motors/stop".to_string(),
                "POST /estop".to_string(),
                "POST /estop/reset".to_string(),
                "WS /ws/telemetry".to_string(),
            ],
            mcp_tools: vec![
                "health".to_string(),
                "capabilities".to_string(),
                "observe".to_string(),
                "invoke_capability".to_string(),
                "stop".to_string(),
                "estop".to_string(),
                "capture".to_string(),
                "modules".to_string(),
            ],
            speed_modes: vec![SpeedMode::Low, SpeedMode::Medium, SpeedMode::High],
            accelerator: self.accelerator.clone(),
            modules: self.coordinator.read().graph().modules,
            capabilities: self.capability_registry().descriptors().to_vec(),
        }
    }

    pub fn telemetry(&self) -> TelemetryFrame {
        if let Some(frame) = self.replay.as_ref().and_then(ReplayPlayback::telemetry_now) {
            return self.telemetry_with_vision(replay_telemetry_source(frame.telemetry));
        }

        let now = now_ms();
        let command = self.command.lock().clone();
        let raw = self.raw.read().clone();
        let sensors = sensor_snapshot(&raw);
        let resource = self.config.resource_sampling.then(current_resource_sample);
        let workers = if self.config.profile == Profile::Sim {
            vec![crate::worker::simulated_perception_worker_status()]
        } else {
            Vec::new()
        };
        let telemetry = TelemetryFrame {
            ts_ms: now,
            robot: self.config.role.clone(),
            profile: self.config.profile.as_str().to_string(),
            battery_v: raw.battery_v,
            battery_pct: raw.battery_pct,
            left_cmd: command.left_cmd,
            right_cmd: command.right_cmd,
            odometry_left: raw.odometry_left,
            odometry_right: raw.odometry_right,
            session_id: command.active_session_id.as_deref().map(operator_owner_id),
            deadman_ok: !command.stopped_by_deadman,
            estop: command.estop,
            stopped_by_deadman: command.stopped_by_deadman,
            soft_odometry_limited: command.soft_odometry_limited,
            soft_odometry_limit_m: self.config.soft_odometry_limit_m,
            speed_mode: command.speed_mode,
            max_speed: command.speed_mode.cap(),
            sensors,
            vision: VisionResult::default(),
            workers,
            motion_events: Vec::new(),
            resource,
            source: raw.source,
        };
        self.telemetry_with_vision(telemetry)
    }

    fn telemetry_with_vision(&self, mut telemetry: TelemetryFrame) -> TelemetryFrame {
        let observation = ImageObservation {
            ts_ms: telemetry.ts_ms,
            frame_id: "camera".to_string(),
            source: telemetry.sensors.raw_frame.source.clone(),
            width_px: 640,
            height_px: 480,
            content_type: "image/simulated".to_string(),
            byte_len: 0,
            sha256: None,
        };
        if matches!(self.config.profile, Profile::Sim | Profile::Replay) {
            telemetry.vision = self.perception.observe(observation);
            if telemetry.workers.is_empty() {
                telemetry.workers = vec![crate::worker::simulated_perception_worker_status()];
            }
            if telemetry.motion_events.is_empty()
                && (telemetry.left_cmd.abs() > f64::EPSILON
                    || telemetry.right_cmd.abs() > f64::EPSILON)
            {
                let source = if self.config.profile == Profile::Replay {
                    "replay-motion"
                } else {
                    "simulated-motion"
                };
                telemetry.motion_events.push(MotionEvent {
                    event_id: format!("{source}-{}", telemetry.ts_ms),
                    ts_ms: telemetry.ts_ms,
                    source: source.to_string(),
                    frame_id: "map".to_string(),
                    kind: MotionEventKind::Detected,
                    confidence: 1.0,
                    x_m: telemetry
                        .odometry_left
                        .zip(telemetry.odometry_right)
                        .map(|(left, right)| (left + right) / 2.0),
                    y_m: Some(0.0),
                });
            }
        }
        telemetry
    }

    pub fn telemetry_stream_frame(&self) -> TelemetryStreamFrame {
        let telemetry = self.telemetry();
        self.stream_frame_from_telemetry(telemetry)
    }

    fn stream_frame_from_telemetry(&self, telemetry: TelemetryFrame) -> TelemetryStreamFrame {
        let health = self.health();
        let visualization = self.visualization_frame_from_telemetry(&telemetry);
        TelemetryStreamFrame {
            kind: "telemetry".to_string(),
            ts_ms: telemetry.ts_ms,
            command: CommandStreamState {
                left_cmd: telemetry.left_cmd,
                right_cmd: telemetry.right_cmd,
                session_id: telemetry.session_id.clone(),
                speed_mode: telemetry.speed_mode,
                max_speed: telemetry.max_speed,
            },
            safety: SafetyStreamState {
                estop: telemetry.estop,
                deadman_ok: telemetry.deadman_ok,
                stopped_by_deadman: telemetry.stopped_by_deadman,
                soft_odometry_limited: telemetry.soft_odometry_limited,
                soft_odometry_limit_m: telemetry.soft_odometry_limit_m,
                physical_actuation_enabled: self.physical_actuation_enabled(),
            },
            visualization,
            telemetry,
            health,
        }
    }

    fn visualization_frame_from_telemetry(&self, telemetry: &TelemetryFrame) -> VisualizationFrame {
        let left_m = telemetry.odometry_left.unwrap_or_default();
        let right_m = telemetry.odometry_right.unwrap_or_default();
        let x_m = round3((left_m + right_m) / 2.0);
        let yaw_rad = round3((right_m - left_m) * 0.25);
        let map_origin = Pose2d {
            ts_ms: telemetry.ts_ms,
            frame_id: "map".to_string(),
            x_m: -0.5,
            y_m: -0.5,
            yaw_rad: 0.0,
        };
        let map = MapMetadata {
            ts_ms: telemetry.ts_ms,
            map_id: "sim-local".to_string(),
            frame_id: "map".to_string(),
            width: 4,
            height: 4,
            resolution_m: 0.25,
            origin: map_origin.clone(),
            cell_order: "row-major".to_string(),
        };
        let planner_path = self.planner.lock().path.clone();
        let pose = Pose2d {
            ts_ms: telemetry.ts_ms,
            frame_id: "map".to_string(),
            x_m,
            y_m: 0.0,
            yaw_rad,
        };
        let twist = Twist2d {
            ts_ms: telemetry.ts_ms,
            frame_id: "base_link".to_string(),
            linear_x_mps: round3(
                (telemetry.left_cmd + telemetry.right_cmd) * 0.5 * telemetry.max_speed,
            ),
            linear_y_mps: 0.0,
            angular_z_radps: round3(
                (telemetry.right_cmd - telemetry.left_cmd) * telemetry.max_speed,
            ),
        };
        VisualizationFrame {
            version: VISUALIZATION_FRAME_VERSION.to_string(),
            ts_ms: telemetry.ts_ms,
            robot: telemetry.robot.clone(),
            profile: telemetry.profile.clone(),
            map: map.clone(),
            pose: pose.clone(),
            twist,
            path: if planner_path.poses.is_empty() {
                VisualizationPath {
                    ts_ms: telemetry.ts_ms,
                    frame_id: "map".to_string(),
                    poses: vec![
                        Pose2d {
                            ts_ms: telemetry.ts_ms,
                            frame_id: "map".to_string(),
                            x_m: 0.0,
                            y_m: 0.0,
                            yaw_rad: 0.0,
                        },
                        pose.clone(),
                    ],
                }
            } else {
                planner_path
            },
            occupancy_grid: OccupancyGridFrame {
                ts_ms: telemetry.ts_ms,
                frame_id: "map".to_string(),
                width: 4,
                height: 4,
                resolution_m: 0.25,
                origin: map_origin.clone(),
                metadata: map.clone(),
                cells: planner_occupancy_cells(),
            },
            costmap: CostmapFrame {
                ts_ms: telemetry.ts_ms,
                frame_id: "map".to_string(),
                width: 4,
                height: 4,
                resolution_m: 0.25,
                origin: map_origin,
                metadata: map,
                costs: planner_costs(),
            },
            point_cloud: PointCloudMetadata {
                ts_ms: telemetry.ts_ms,
                frame_id: "base_link".to_string(),
                point_count: 0,
                fields: vec!["x".to_string(), "y".to_string(), "z".to_string()],
                source: telemetry.sensors.raw_frame.source.clone(),
            },
            detections: telemetry.vision.detections.clone(),
            command: CommandOverlay {
                ts_ms: telemetry.ts_ms,
                left_cmd: telemetry.left_cmd,
                right_cmd: telemetry.right_cmd,
                speed_mode: telemetry.speed_mode,
                max_speed: telemetry.max_speed,
                estop: telemetry.estop,
            },
            autonomy: self.autonomy_overlay(telemetry.ts_ms),
        }
    }

    fn runtime_mode(&self) -> &'static str {
        if self.replay.is_some() {
            "replay"
        } else {
            "live"
        }
    }

    pub fn physical_actuation_enabled(&self) -> bool {
        self.config.profile.is_physical()
            && (self.config.allow_physical_actuation
                || std::env::var("LEASH_ALLOW_PHYSICAL_ACTUATION")
                    .ok()
                    .as_deref()
                    == Some("1"))
    }

    pub fn authorize(&self, token: String, ttl_secs: u64, speed_mode: SpeedMode) -> Result<()> {
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err(anyhow!("token cannot be empty"));
        }

        let should_stop_previous_owner = {
            let command = self.command.lock();
            command
                .active_session_id
                .as_deref()
                .is_some_and(|active| active != token)
                && (command.left_cmd != 0.0 || command.right_cmd != 0.0)
        };
        if should_stop_previous_owner {
            self.driver.stop()?;
            let mut command = self.command.lock();
            command.left_cmd = 0.0;
            command.right_cmd = 0.0;
            command.last_cmd_at = None;
        }

        let mut sessions = self.sessions.lock();
        sessions.clear();
        sessions.insert(
            token.clone(),
            PilotSession {
                expires_at: Instant::now() + Duration::from_secs(ttl_secs.max(1)),
                speed_mode,
            },
        );
        let mut command = self.command.lock();
        if command.active_session_id.as_deref() != Some(token.as_str()) {
            command.active_session_id = None;
        }
        Ok(())
    }

    pub fn operator_token_status(&self) -> OperatorTokenStatus {
        let now = Instant::now();
        let mut sessions = self.sessions.lock();
        sessions.retain(|_, session| session.expires_at > now);
        let Some((token, session)) = sessions.iter().next() else {
            return OperatorTokenStatus::default();
        };
        let expires_in_ms = session
            .expires_at
            .saturating_duration_since(now)
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        OperatorTokenStatus {
            active: true,
            owner_id: Some(operator_owner_id(token)),
            expires_in_ms: Some(expires_in_ms),
            speed_mode: Some(session.speed_mode),
        }
    }

    pub fn set_speed_mode(&self, token: Option<&str>, speed_mode: SpeedMode) -> Result<()> {
        self.validate_session(token)?;
        self.command.lock().speed_mode = speed_mode;
        Ok(())
    }

    pub fn drive(
        &self,
        token: Option<&str>,
        left: f64,
        right: f64,
        speed_mode: Option<SpeedMode>,
    ) -> Result<DriveOutcome> {
        let session = self.validate_session(token)?;
        let speed_mode = speed_mode.or(session.map(|session| session.speed_mode));
        if let Some(speed_mode) = speed_mode {
            self.command.lock().speed_mode = speed_mode;
        }

        let mut command = self.command.lock();
        if command.estop {
            return Err(anyhow!("estop is latched; call estop/reset before driving"));
        }

        let max_speed = command.speed_mode.cap();
        let mut left = clamp(left, -max_speed, max_speed);
        let mut right = clamp(right, -max_speed, max_speed);
        command.soft_odometry_limited = self.soft_odometry_limit_reached(left, right);
        if command.soft_odometry_limited {
            left = 0.0;
            right = 0.0;
        }

        self.driver.drive(left, right)?;
        command.left_cmd = left;
        command.right_cmd = right;
        command.last_cmd_at = Some(Instant::now());
        command.active_session_id = token.map(ToOwned::to_owned);
        command.stopped_by_deadman = false;
        drop(command);

        self.advance_sim_odometry(left, right);

        let command = self.command.lock().clone();
        Ok(DriveOutcome {
            ok: true,
            left,
            right,
            speed_mode: command.speed_mode,
            max_speed,
            stopped_by_deadman: command.stopped_by_deadman,
            soft_odometry_limited: command.soft_odometry_limited,
        })
    }

    pub fn camera_aim(
        &self,
        token: Option<&str>,
        pan_deg: f64,
        tilt_deg: f64,
        speed: Option<u32>,
        accel: Option<u32>,
    ) -> Result<CameraAimOutcome> {
        self.validate_session(token)?;
        {
            let command = self.command.lock();
            if command.estop {
                return Err(anyhow!(
                    "estop is latched; call estop/reset before camera aim"
                ));
            }
        }

        let pan_deg = clamp(pan_deg, CAMERA_PAN_MIN_DEG, CAMERA_PAN_MAX_DEG);
        let tilt_deg = clamp(tilt_deg, CAMERA_TILT_MIN_DEG, CAMERA_TILT_MAX_DEG);
        let speed = speed.unwrap_or(0);
        let accel = accel.unwrap_or(0);
        self.driver.aim_camera(pan_deg, tilt_deg, speed, accel)?;
        Ok(CameraAimOutcome {
            ok: true,
            pan_deg,
            tilt_deg,
            speed,
            accel,
        })
    }

    pub fn stop(&self) -> Result<DriveOutcome> {
        self.cancel_patrol_state("stopped", "patrol movement stopped");
        self.cancel_planner_state("stopped", "planner movement stopped");
        self.stop_without_planner_cancel()
    }

    fn stop_without_planner_cancel(&self) -> Result<DriveOutcome> {
        self.driver.stop()?;
        let mut command = self.command.lock();
        command.left_cmd = 0.0;
        command.right_cmd = 0.0;
        command.last_cmd_at = Some(Instant::now());
        command.stopped_by_deadman = false;
        Ok(DriveOutcome {
            ok: true,
            left: 0.0,
            right: 0.0,
            speed_mode: command.speed_mode,
            max_speed: command.speed_mode.cap(),
            stopped_by_deadman: false,
            soft_odometry_limited: command.soft_odometry_limited,
        })
    }

    pub fn estop(&self) -> Result<()> {
        self.cancel_patrol_state("estop", "patrol movement cancelled by estop");
        self.cancel_planner_state("estop", "planner movement cancelled by estop");
        self.driver.stop()?;
        let mut command = self.command.lock();
        command.left_cmd = 0.0;
        command.right_cmd = 0.0;
        command.estop = true;
        command.stopped_by_deadman = false;
        Ok(())
    }

    fn current_sim_pose(&self, ts_ms: u128) -> Pose2d {
        let raw = self.raw.read();
        let left_m = raw.odometry_left.unwrap_or_default();
        let right_m = raw.odometry_right.unwrap_or_default();
        Pose2d {
            ts_ms,
            frame_id: "map".to_string(),
            x_m: round3((left_m + right_m) / 2.0),
            y_m: 0.0,
            yaw_rad: round3((right_m - left_m) * 0.25),
        }
    }

    fn planner_step(&self) {
        let (goal, path) = {
            let planner = self.planner.lock();
            if !planner.active {
                return;
            }
            let Some(goal) = planner.goal.clone() else {
                return;
            };
            (goal, planner.path.clone())
        };

        let now = now_ms();
        let current = self.current_sim_pose(now);
        let goal_distance = distance2d(current.x_m, current.y_m, goal.x_m, goal.y_m);
        if goal_distance <= goal.tolerance_m {
            self.cancel_planner_state("reached", "planner goal reached");
            let _ = self.stop_without_planner_cancel();
            return;
        }

        let next = path
            .poses
            .iter()
            .find(|pose| distance2d(current.x_m, current.y_m, pose.x_m, pose.y_m) > 0.05)
            .cloned()
            .unwrap_or_else(|| Pose2d {
                ts_ms: now,
                frame_id: "map".to_string(),
                x_m: goal.x_m,
                y_m: goal.y_m,
                yaw_rad: 0.0,
            });
        let (left, right) = planner_drive_command(&current, &next);

        match self.drive(None, left, right, Some(goal.speed_mode)) {
            Ok(outcome) => {
                let mut planner = self.planner.lock();
                planner.last_drive = Some(outcome.clone());
                if outcome.soft_odometry_limited {
                    planner.ok = false;
                    planner.active = false;
                    planner.status = "limited".to_string();
                    planner.message = "planner stopped by soft odometry limit".to_string();
                } else {
                    planner.ok = true;
                    planner.active = true;
                    planner.status = "active".to_string();
                    planner.message = "planner driving toward goal".to_string();
                }
            }
            Err(err) => {
                let mut planner = self.planner.lock();
                planner.ok = false;
                planner.active = false;
                planner.status = "stopped".to_string();
                planner.message = format!("planner drive stopped: {err}");
            }
        }
    }

    fn cancel_planner_state(&self, status: &str, message: &str) {
        let mut planner = self.planner.lock();
        if planner.goal.is_none() && planner.path.poses.is_empty() {
            return;
        }
        planner.ok = matches!(
            status,
            "cancelled" | "idle" | "reached" | "stopped" | "estop"
        );
        planner.active = false;
        planner.status = status.to_string();
        planner.message = message.to_string();
    }

    fn patrol_step(&self) {
        let ts_ms = now_ms();
        let current = self.current_sim_pose(ts_ms);
        let Some(start) = planner_cell_for_xy(current.x_m, current.y_m) else {
            self.cancel_patrol_state("no-goal", "patrol current pose is outside the grid");
            return;
        };

        let planner = self.planner_status();
        let next = {
            let mut patrol = self.patrol.lock();
            if !patrol.status.active {
                return;
            }
            patrol.mark_visited(start);
            patrol.status.path = planner.path.clone();
            patrol.status.goal = planner.goal.clone();

            if planner.active {
                return;
            }

            if matches!(planner.status.as_str(), "limited" | "stopped" | "estop") {
                patrol.status.ok = false;
                patrol.status.active = false;
                patrol.status.status = planner.status;
                patrol.status.message = "patrol stopped by planner safety state".to_string();
                return;
            }

            let strategy = patrol.status.strategy.unwrap_or_default();
            let speed_mode = patrol.status.speed_mode;
            let Some(goal_cell) = select_patrol_goal(strategy, start, &patrol.visited) else {
                patrol.status.ok = false;
                patrol.status.active = false;
                patrol.status.status = "no-goal".to_string();
                patrol.status.message = "patrol could not find a safe reachable goal".to_string();
                patrol.status.goal = None;
                patrol.status.path = VisualizationPath {
                    ts_ms,
                    frame_id: "map".to_string(),
                    poses: Vec::new(),
                };
                return;
            };
            (
                planner_goal_for_cell(goal_cell, speed_mode),
                strategy,
                speed_mode,
            )
        };

        let (goal, strategy, speed_mode) = next;
        let planner = match self.set_planner_goal(goal.clone()) {
            Ok(planner) => planner,
            Err(err) => {
                let mut patrol = self.patrol.lock();
                patrol.status.ok = false;
                patrol.status.active = false;
                patrol.status.status = "no-goal".to_string();
                patrol.status.message = format!("patrol planner failed: {err}");
                return;
            }
        };
        let mut patrol = self.patrol.lock();
        patrol.status = patrol_status(PatrolStatusUpdate {
            ok: planner.ok,
            active: planner.active,
            status: &planner.status,
            message: &planner.message,
            strategy: Some(strategy),
            speed_mode,
            goal: Some(goal),
            path: planner.path,
        });
    }

    fn cancel_patrol_state(&self, status: &str, message: &str) {
        let mut patrol = self.patrol.lock();
        if !patrol.status.active && patrol.status.goal.is_none() {
            return;
        }
        patrol.status.ok = matches!(status, "stopped" | "cancelled" | "reached" | "idle");
        patrol.status.active = false;
        patrol.status.status = status.to_string();
        patrol.status.message = message.to_string();
    }

    fn autonomy_overlay(&self, ts_ms: u128) -> crate::types::AutonomyOverlay {
        let patrol = self.patrol_status();
        crate::types::AutonomyOverlay {
            ts_ms,
            mode: if patrol.active {
                "patrol".to_string()
            } else {
                "idle".to_string()
            },
            active: patrol.active,
            status: patrol.status,
            strategy: patrol.strategy,
            goal: patrol.goal,
            visited_cells: patrol.visited_cells,
        }
    }

    pub fn reset_estop(&self, token: Option<&str>) -> Result<()> {
        self.validate_session(token)?;
        let mut command = self.command.lock();
        command.estop = false;
        command.stopped_by_deadman = false;
        Ok(())
    }

    pub fn capture(&self) -> CaptureResult {
        let frame = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="320" height="240"><rect width="320" height="240" fill="#101820"/><text x="18" y="120" fill="#f6f1d1" font-family="monospace" font-size="18">leash {}</text></svg>"##,
            self.config.role
        );
        let mut hasher = Sha256::new();
        hasher.update(frame.as_bytes());
        CaptureResult {
            ok: true,
            source: format!("{}-capture", self.config.profile.as_str()),
            content_type: "image/svg+xml".to_string(),
            byte_len: frame.len(),
            captured_at_ms: now_ms(),
            sha256: format!("{:x}", hasher.finalize()),
        }
    }

    #[cfg(feature = "mavlink-drone")]
    pub fn drone_command(
        &self,
        command: &str,
        token: Option<&str>,
        args: Value,
    ) -> Result<DroneCommandStatus> {
        self.validate_session(token)?;
        let simulated = matches!(self.config.profile, Profile::Sim | Profile::Replay);
        if self.config.profile == Profile::MavlinkDrone {
            return Err(anyhow!(
                "MAVLink drone profile is a gated skeleton; configure a concrete MAVLink adapter before executing '{command}'"
            ));
        }
        if !simulated {
            return Err(anyhow!(
                "drone capability '{command}' requires sim, replay, or mavlink-drone profile"
            ));
        }

        Ok(DroneCommandStatus {
            ok: true,
            command: command.to_string(),
            profile: self.config.profile.as_str().to_string(),
            simulated,
            status: "simulated".to_string(),
            message: format!("simulated MAVLink drone {command} accepted"),
            mavlink_endpoint: None,
            args,
        })
    }

    #[cfg(feature = "manipulator")]
    pub fn manipulator_joint_state(&self) -> ManipulatorJointState {
        let simulated = matches!(self.config.profile, Profile::Sim | Profile::Replay);
        ManipulatorJointState {
            version: MANIPULATOR_SCHEMA_VERSION.to_string(),
            ok: true,
            profile: self.config.profile.as_str().to_string(),
            simulated,
            source: if simulated {
                "mock-arm".to_string()
            } else {
                "manipulator-skeleton".to_string()
            },
            joints: vec![
                ManipulatorJoint {
                    name: "shoulder_pan".to_string(),
                    position_rad: 0.0,
                    velocity_radps: 0.0,
                    effort_nm: Some(0.0),
                },
                ManipulatorJoint {
                    name: "elbow".to_string(),
                    position_rad: 0.0,
                    velocity_radps: 0.0,
                    effort_nm: Some(0.0),
                },
                ManipulatorJoint {
                    name: "wrist".to_string(),
                    position_rad: 0.0,
                    velocity_radps: 0.0,
                    effort_nm: Some(0.0),
                },
            ],
        }
    }

    #[cfg(feature = "manipulator")]
    pub fn manipulator_command(
        &self,
        command: &str,
        token: Option<&str>,
        args: Value,
    ) -> Result<ManipulatorCommandStatus> {
        self.validate_session(token)?;
        let simulated = matches!(self.config.profile, Profile::Sim | Profile::Replay);
        if self.config.profile == Profile::Manipulator {
            return Err(anyhow!(
                "manipulator profile is a gated skeleton; configure a concrete manipulator adapter before executing '{command}'"
            ));
        }
        if !simulated {
            return Err(anyhow!(
                "manipulator capability '{command}' requires sim, replay, or manipulator profile"
            ));
        }

        Ok(ManipulatorCommandStatus {
            version: MANIPULATOR_SCHEMA_VERSION.to_string(),
            ok: true,
            command: command.to_string(),
            profile: self.config.profile.as_str().to_string(),
            simulated,
            status: "simulated".to_string(),
            message: format!("mock manipulator {command} accepted"),
            args,
        })
    }

    fn validate_session(&self, token: Option<&str>) -> Result<Option<PilotSession>> {
        if self.config.allow_untokened_drive && token.is_none() {
            return Ok(None);
        }
        let token = token.ok_or_else(|| anyhow!("missing pilot token"))?;
        let mut sessions = self.sessions.lock();
        let Some(session) = sessions.get(token).cloned() else {
            return Err(anyhow!("invalid pilot token"));
        };
        if Instant::now() > session.expires_at {
            sessions.remove(token);
            return Err(anyhow!("expired pilot token"));
        }
        Ok(Some(session))
    }

    fn soft_odometry_limit_reached(&self, left: f64, right: f64) -> bool {
        if self.config.soft_odometry_limit_m <= 0.0 || left <= 0.0 && right <= 0.0 {
            return false;
        }
        let raw = self.raw.read();
        let Some(left_m) = raw.odometry_left else {
            return false;
        };
        let Some(right_m) = raw.odometry_right else {
            return false;
        };
        ((left_m + right_m) / 2.0).abs() >= self.config.soft_odometry_limit_m
    }

    fn advance_sim_odometry(&self, left: f64, right: f64) {
        if self.config.profile != Profile::Sim {
            return;
        }
        let mut raw = self.raw.write();
        raw.odometry_left = Some(round3(raw.odometry_left.unwrap_or_default() + left * 0.03));
        raw.odometry_right = Some(round3(
            raw.odometry_right.unwrap_or_default() + right * 0.03,
        ));
        raw.last_raw_frame_ms = Some(now_ms());
    }

    fn spawn_deadman(&self) {
        let harness = self.clone();
        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_millis(50));
            loop {
                interval.tick().await;
                let should_stop = {
                    let command = harness.command.lock();
                    if command.left_cmd == 0.0 && command.right_cmd == 0.0 {
                        false
                    } else {
                        command.last_cmd_at.is_some_and(|at| {
                            at.elapsed().as_millis() > harness.config.deadman_ms as u128
                        })
                    }
                };
                if should_stop {
                    if let Err(err) = harness.driver.stop() {
                        warn!(?err, "deadman stop failed");
                    }
                    let mut command = harness.command.lock();
                    command.left_cmd = 0.0;
                    command.right_cmd = 0.0;
                    command.stopped_by_deadman = true;
                }
            }
        });
    }

    fn spawn_planner_loop(&self) {
        let harness = self.clone();
        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_millis(100));
            loop {
                interval.tick().await;
                harness.planner_step();
            }
        });
    }

    fn spawn_patrol_loop(&self) {
        let harness = self.clone();
        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_millis(150));
            loop {
                interval.tick().await;
                harness.patrol_step();
            }
        });
    }

    #[cfg(feature = "waveshare-ugv")]
    fn spawn_waveshare_telemetry(&self) {
        if self.config.profile != Profile::WaveshareUgv {
            return;
        }

        match self.driver.telemetry_reader() {
            Ok(Some(port)) => {
                let raw = self.raw.clone();
                thread::spawn(move || read_waveshare_telemetry_loop(port, raw));
            }
            Ok(None) => {}
            Err(err) => warn!(?err, "waveshare telemetry reader unavailable"),
        }

        let driver = self.driver.clone();
        tokio::spawn(async move {
            let mut failures = 0_u64;
            if let Err(err) = driver.enable_telemetry() {
                warn!(?err, "enable Waveshare telemetry failed");
            }
            let mut interval = time::interval(Duration::from_millis(500));
            loop {
                interval.tick().await;
                if let Err(err) = driver.request_telemetry() {
                    failures += 1;
                    if failures == 1 || failures.is_multiple_of(20) {
                        warn!(?err, "request Waveshare telemetry failed");
                    }
                } else {
                    failures = 0;
                }
            }
        });
    }

    fn spawn_telemetry_loop(&self) {
        let harness = self.clone();
        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_millis(50));
            loop {
                interval.tick().await;
                let telemetry = harness.telemetry();
                let _ = harness.telemetry_tx.send(telemetry.clone());
                if let Ok(payload) = serde_json::to_value(harness.telemetry_stream_frame()) {
                    let _ = harness.stream_transport.publish("telemetry", payload);
                }
            }
        });
    }
}

fn operator_owner_id(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    format!("operator-{}", &format!("{digest:x}")[..12])
}

#[cfg(feature = "waveshare-ugv")]
fn read_waveshare_telemetry_loop(
    port: Box<dyn serialport::SerialPort>,
    raw: Arc<RwLock<RawTelemetry>>,
) {
    let mut reader = BufReader::new(port);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => continue,
            Ok(_) => {
                let Some(frame) = parse_waveshare_frame(&line) else {
                    continue;
                };
                if apply_waveshare_frame(&raw, frame) {
                    continue;
                }
            }
            Err(err)
                if matches!(
                    err.kind(),
                    ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(err) => {
                warn!(?err, "read Waveshare telemetry failed");
                thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

#[cfg(feature = "waveshare-ugv")]
fn parse_waveshare_frame(line: &str) -> Option<Value> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Some(value);
    }
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<Value>(&trimmed[start..=end]).ok()
}

#[cfg(feature = "waveshare-ugv")]
fn apply_waveshare_frame(raw: &Arc<RwLock<RawTelemetry>>, frame: Value) -> bool {
    if !is_waveshare_base_feedback(&frame) {
        return false;
    }

    let mut next = raw.write();
    if let Some(voltage) = waveshare_voltage(&frame) {
        next.battery_v = Some(round3(voltage));
        next.battery_pct = battery_percent_from_voltage(voltage);
    }
    if let Some(left_m) = waveshare_odometry_m(&frame, "odl", &["odometry_left", "left_m"]) {
        next.odometry_left = Some(round3(left_m));
    }
    if let Some(right_m) = waveshare_odometry_m(&frame, "odr", &["odometry_right", "right_m"]) {
        next.odometry_right = Some(round3(right_m));
    }
    next.last_raw_frame_ms = Some(now_ms());
    next.last_raw_payload = Some(frame);
    true
}

#[cfg(feature = "waveshare-ugv")]
fn is_waveshare_base_feedback(frame: &Value) -> bool {
    frame
        .get("T")
        .and_then(json_number)
        .is_some_and(|kind| (kind - 1001.0).abs() < f64::EPSILON)
        || frame.get("v").is_some()
        || frame.get("odl").is_some()
        || frame.get("odr").is_some()
}

#[cfg(feature = "waveshare-ugv")]
fn waveshare_voltage(frame: &Value) -> Option<f64> {
    const DIRECT_KEYS: &[&str] = &[
        "battery_v",
        "voltage_v",
        "loadVoltage_V",
        "load_voltage_v",
        "busVoltage_V",
        "bus_voltage_v",
        "vbat",
        "VBAT",
        "voltage",
    ];
    for key in DIRECT_KEYS {
        if let Some(value) = frame.get(*key).and_then(json_number) {
            return normalize_voltage(value);
        }
    }
    frame
        .get("v")
        .and_then(json_number)
        .and_then(normalize_voltage)
}

#[cfg(feature = "waveshare-ugv")]
fn normalize_voltage(value: f64) -> Option<f64> {
    let voltage = if value > 100.0 { value / 100.0 } else { value };
    (voltage > 3.0 && voltage < 30.0).then_some(voltage)
}

#[cfg(feature = "waveshare-ugv")]
fn waveshare_odometry_m(frame: &Value, centimeters_key: &str, meter_keys: &[&str]) -> Option<f64> {
    for key in meter_keys {
        if let Some(value) = frame.get(*key).and_then(json_number) {
            return Some(value);
        }
    }
    frame
        .get(centimeters_key)
        .and_then(json_number)
        .map(|centimeters| centimeters / 100.0)
}

#[cfg(feature = "waveshare-ugv")]
fn json_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn battery_percent_from_voltage(voltage: f64) -> Option<f64> {
    if !(3.0..=30.0).contains(&voltage) {
        return None;
    }
    Some(round1(clamp(
        (voltage - 9.0) / (12.6 - 9.0) * 100.0,
        0.0,
        100.0,
    )))
}

fn open_physical_driver(config: &HarnessConfig) -> Result<Arc<dyn RobotDriver>> {
    match config.profile {
        Profile::Sim => Ok(Arc::new(SimDriver)),
        Profile::Replay => Ok(Arc::new(ReplayDriver)),
        Profile::WaveshareUgv => {
            #[cfg(feature = "waveshare-ugv")]
            {
                Ok(Arc::new(WaveshareUgvDriver::open(config)?))
            }
            #[cfg(not(feature = "waveshare-ugv"))]
            {
                let _ = config;
                Err(anyhow!(
                    "profile 'waveshare-ugv' requires building with --features waveshare-ugv"
                ))
            }
        }
        Profile::MavlinkDrone => {
            #[cfg(feature = "mavlink-drone")]
            {
                Ok(Arc::new(MavlinkDroneDriver::open(config)?))
            }
            #[cfg(not(feature = "mavlink-drone"))]
            {
                let _ = config;
                Err(anyhow!(
                    "profile 'mavlink-drone' requires building with --features mavlink-drone"
                ))
            }
        }
        Profile::Manipulator => {
            #[cfg(feature = "manipulator")]
            {
                Ok(Arc::new(ManipulatorDriver::open(config)))
            }
            #[cfg(not(feature = "manipulator"))]
            {
                let _ = config;
                Err(anyhow!(
                    "profile 'manipulator' requires building with --features manipulator"
                ))
            }
        }
    }
}

fn configured_camera_device_exists() -> bool {
    let device = std::env::var("LEASH_CAMERA_DEVICE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "/dev/video0".to_string());
    std::path::Path::new(&device).exists()
}

fn sensor_snapshot(raw: &RawTelemetry) -> SensorSnapshot {
    let camera = if configured_camera_device_exists() {
        CameraStatus {
            status: "available".to_string(),
            health: "healthy".to_string(),
            stream_url: Some("/camera/stream.mjpg".to_string()),
            snapshot_url: Some("/camera/snapshot".to_string()),
        }
    } else {
        CameraStatus {
            status: "simulated".to_string(),
            health: "healthy".to_string(),
            stream_url: None,
            snapshot_url: None,
        }
    };

    SensorSnapshot {
        battery: BatteryStatus {
            status: if raw.battery_v.is_some() || raw.battery_pct.is_some() {
                "available"
            } else {
                "unavailable"
            }
            .to_string(),
            voltage_v: raw.battery_v,
            level_pct: raw.battery_pct,
        },
        odometry: OdometryStatus {
            status: if raw.odometry_left.is_some() || raw.odometry_right.is_some() {
                "available"
            } else {
                "unavailable"
            }
            .to_string(),
            left_m: raw.odometry_left,
            right_m: raw.odometry_right,
        },
        camera,
        raw_frame: RawFrameStatus {
            status: if raw.last_raw_frame_ms.is_some() {
                "available"
            } else {
                "missing"
            }
            .to_string(),
            source: raw.source.clone(),
            last_ms: raw.last_raw_frame_ms,
            payload: raw.last_raw_payload.clone(),
        },
    }
}

fn clamp(value: f64, min: f64, max: f64) -> f64 {
    value.max(min).min(max)
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PlannerCell {
    col: usize,
    row: usize,
}

impl PlannerCell {
    fn index(self) -> usize {
        self.row * PLANNER_GRID_WIDTH + self.col
    }
}

fn planner_occupancy_cells() -> Vec<i8> {
    let mut cells = vec![OCCUPANCY_FREE; PLANNER_GRID_CELLS];
    for &(col, row) in PLANNER_BLOCKED_CELLS {
        cells[row * PLANNER_GRID_WIDTH + col] = OCCUPANCY_OCCUPIED;
    }
    cells
}

fn planner_costs() -> Vec<u8> {
    planner_occupancy_cells()
        .into_iter()
        .map(|cell| {
            if cell == OCCUPANCY_OCCUPIED {
                COST_LETHAL
            } else {
                COST_FREE
            }
        })
        .collect()
}

fn planner_cell_for_xy(x_m: f64, y_m: f64) -> Option<PlannerCell> {
    let max_x = PLANNER_ORIGIN_X_M + PLANNER_GRID_WIDTH as f64 * PLANNER_RESOLUTION_M;
    let max_y = PLANNER_ORIGIN_Y_M + PLANNER_GRID_HEIGHT as f64 * PLANNER_RESOLUTION_M;
    if x_m < PLANNER_ORIGIN_X_M || y_m < PLANNER_ORIGIN_Y_M || x_m >= max_x || y_m >= max_y {
        return None;
    }

    let col = ((x_m - PLANNER_ORIGIN_X_M) / PLANNER_RESOLUTION_M).floor() as usize;
    let row = ((y_m - PLANNER_ORIGIN_Y_M) / PLANNER_RESOLUTION_M).floor() as usize;
    Some(PlannerCell { col, row })
}

fn planner_pose_for_cell(cell: PlannerCell, ts_ms: u128) -> Pose2d {
    Pose2d {
        ts_ms,
        frame_id: "map".to_string(),
        x_m: round3(PLANNER_ORIGIN_X_M + (cell.col as f64 + 0.5) * PLANNER_RESOLUTION_M),
        y_m: round3(PLANNER_ORIGIN_Y_M + (cell.row as f64 + 0.5) * PLANNER_RESOLUTION_M),
        yaw_rad: 0.0,
    }
}

fn planner_route(start: PlannerCell, goal: PlannerCell) -> Option<Vec<PlannerCell>> {
    if planner_cell_blocked(start) || planner_cell_blocked(goal) {
        return None;
    }

    let mut queue = VecDeque::from([start]);
    let mut seen = [false; PLANNER_GRID_CELLS];
    let mut previous = [None; PLANNER_GRID_CELLS];
    seen[start.index()] = true;

    while let Some(cell) = queue.pop_front() {
        if cell == goal {
            break;
        }
        for next in planner_neighbors(cell) {
            let index = next.index();
            if seen[index] || planner_cell_blocked(next) {
                continue;
            }
            seen[index] = true;
            previous[index] = Some(cell);
            queue.push_back(next);
        }
    }

    if !seen[goal.index()] {
        return None;
    }

    let mut cells = vec![goal];
    let mut cursor = goal;
    while cursor != start {
        cursor = previous[cursor.index()]?;
        cells.push(cursor);
    }
    cells.reverse();
    Some(cells)
}

fn planner_cell_blocked(cell: PlannerCell) -> bool {
    PLANNER_BLOCKED_CELLS
        .iter()
        .any(|&(col, row)| col == cell.col && row == cell.row)
}

fn planner_neighbors(cell: PlannerCell) -> Vec<PlannerCell> {
    let mut neighbors = Vec::with_capacity(4);
    if cell.col > 0 {
        neighbors.push(PlannerCell {
            col: cell.col - 1,
            row: cell.row,
        });
    }
    if cell.col + 1 < PLANNER_GRID_WIDTH {
        neighbors.push(PlannerCell {
            col: cell.col + 1,
            row: cell.row,
        });
    }
    if cell.row > 0 {
        neighbors.push(PlannerCell {
            col: cell.col,
            row: cell.row - 1,
        });
    }
    if cell.row + 1 < PLANNER_GRID_HEIGHT {
        neighbors.push(PlannerCell {
            col: cell.col,
            row: cell.row + 1,
        });
    }
    neighbors
}

fn planner_drive_command(current: &Pose2d, next: &Pose2d) -> (f64, f64) {
    let dx = next.x_m - current.x_m;
    let dy = next.y_m - current.y_m;
    if dx.abs() >= dy.abs() {
        let direction = if dx >= 0.0 { 1.0 } else { -1.0 };
        (PLANNER_STEP_CMD * direction, PLANNER_STEP_CMD * direction)
    } else if dy >= 0.0 {
        (PLANNER_STEP_CMD * 0.5, PLANNER_STEP_CMD)
    } else {
        (PLANNER_STEP_CMD, PLANNER_STEP_CMD * 0.5)
    }
}

fn distance2d(ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt()
}

struct PatrolStatusUpdate<'a> {
    ok: bool,
    active: bool,
    status: &'a str,
    message: &'a str,
    strategy: Option<PatrolStrategy>,
    speed_mode: SpeedMode,
    goal: Option<PlannerGoal>,
    path: VisualizationPath,
}

fn patrol_status(update: PatrolStatusUpdate<'_>) -> PatrolStatus {
    PatrolStatus {
        ok: update.ok,
        active: update.active,
        status: update.status.to_string(),
        message: update.message.to_string(),
        strategy: update.strategy,
        speed_mode: update.speed_mode,
        goal: update.goal,
        path: update.path,
        visited_cells: Vec::new(),
        zone_id: None,
        waypoint_index: None,
    }
}

fn planner_goal_for_cell(cell: PlannerCell, speed_mode: SpeedMode) -> PlannerGoal {
    let pose = planner_pose_for_cell(cell, now_ms());
    PlannerGoal {
        frame_id: "map".to_string(),
        x_m: pose.x_m,
        y_m: pose.y_m,
        tolerance_m: 0.1,
        speed_mode,
    }
}

fn select_patrol_goal(
    strategy: PatrolStrategy,
    start: PlannerCell,
    visited: &[bool; PLANNER_GRID_CELLS],
) -> Option<PlannerCell> {
    let mut candidates = patrol_goal_candidates(start, visited);
    match strategy {
        PatrolStrategy::Coverage => candidates.into_iter().next(),
        PatrolStrategy::Frontier => candidates.into_iter().find(|cell| {
            planner_neighbors(*cell)
                .iter()
                .any(|near| visited[near.index()])
        }),
        PatrolStrategy::Random => {
            if candidates.is_empty() {
                None
            } else {
                let seed = visited
                    .iter()
                    .enumerate()
                    .filter_map(|(index, seen)| seen.then_some(index + 1))
                    .sum::<usize>()
                    + start.index();
                let index = seed % candidates.len();
                Some(candidates.swap_remove(index))
            }
        }
    }
}

fn patrol_goal_candidates(
    start: PlannerCell,
    visited: &[bool; PLANNER_GRID_CELLS],
) -> Vec<PlannerCell> {
    let mut cells = Vec::new();
    for row in 0..PLANNER_GRID_HEIGHT {
        for col in 0..PLANNER_GRID_WIDTH {
            let cell = PlannerCell { col, row };
            if cell == start || visited[cell.index()] || !patrol_cell_clear(cell) {
                continue;
            }
            if planner_route(start, cell).is_some() {
                cells.push(cell);
            }
        }
    }
    cells
}

fn patrol_cell_clear(cell: PlannerCell) -> bool {
    !planner_cell_blocked(cell)
}

fn visited_cell_labels(visited: &[bool; PLANNER_GRID_CELLS]) -> Vec<String> {
    visited
        .iter()
        .enumerate()
        .filter(|(_, seen)| **seen)
        .map(|(index, _)| {
            let cell = PlannerCell {
                col: index % PLANNER_GRID_WIDTH,
                row: index / PLANNER_GRID_WIDTH,
            };
            planner_cell_label(cell)
        })
        .collect()
}

fn planner_cell_label(cell: PlannerCell) -> String {
    format!("{},{}", cell.col, cell.row)
}

fn current_resource_sample() -> ResourceSample {
    ResourceSample {
        sampled_at_ms: now_ms(),
        process_id: std::process::id(),
        cpu_time_ticks: process_cpu_time_ticks(),
        memory_rss_bytes: process_memory_rss_bytes(),
    }
}

#[cfg(target_os = "linux")]
fn process_cpu_time_ticks() -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let (_, fields) = stat.rsplit_once(") ")?;
    let fields = fields.split_whitespace().collect::<Vec<_>>();
    let utime = fields.get(11)?.parse::<u64>().ok()?;
    let stime = fields.get(12)?.parse::<u64>().ok()?;
    Some(utime + stime)
}

#[cfg(not(target_os = "linux"))]
fn process_cpu_time_ticks() -> Option<u64> {
    None
}

#[cfg(target_os = "linux")]
fn process_memory_rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let rss_line = status
        .lines()
        .find(|line| line.strip_prefix("VmRSS:").is_some())?;
    let kb = rss_line.split_whitespace().nth(1)?.parse::<u64>().ok()?;
    Some(kb * 1024)
}

#[cfg(not(target_os = "linux"))]
fn process_memory_rss_bytes() -> Option<u64> {
    None
}

pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after unix epoch")
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sim_harness_drives_and_deadman_stops() {
        let config = HarnessConfig {
            deadman_ms: 20,
            ..HarnessConfig::default()
        };
        let harness = Harness::new(config).unwrap();

        let outcome = harness.drive(None, 1.0, 1.0, Some(SpeedMode::Low)).unwrap();
        assert_eq!(outcome.left, SpeedMode::Low.cap());
        assert_eq!(outcome.right, SpeedMode::Low.cap());

        time::sleep(Duration::from_millis(80)).await;
        let telemetry = harness.telemetry();
        assert_eq!(telemetry.left_cmd, 0.0);
        assert_eq!(telemetry.right_cmd, 0.0);
        assert!(telemetry.stopped_by_deadman);
    }

    #[test]
    fn physical_profile_requires_explicit_gate() {
        let config = HarnessConfig {
            profile: Profile::WaveshareUgv,
            ..HarnessConfig::default()
        };
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("LEASH_ALLOW_PHYSICAL_ACTUATION"));
    }

    #[tokio::test]
    async fn capture_is_deterministic_for_role() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();
        assert_eq!(harness.capture().sha256, harness.capture().sha256);
    }

    #[tokio::test]
    async fn resource_samples_are_disabled_by_default() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();

        assert!(harness.telemetry().resource.is_none());
    }

    #[tokio::test]
    async fn resource_samples_can_be_enabled() {
        let harness = Harness::new(HarnessConfig {
            resource_sampling: true,
            ..HarnessConfig::default()
        })
        .unwrap();

        let sample = harness.telemetry().resource.unwrap();
        assert_eq!(sample.process_id, std::process::id());
    }

    #[tokio::test]
    async fn health_includes_running_module_state() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();
        let health = harness.health();

        assert!(health.ok);
        assert_eq!(
            health.accelerator.active,
            crate::config::AcceleratorBackend::None
        );
        assert!(health.accelerator.probes.iter().any(|probe| probe.backend
            == crate::config::AcceleratorBackend::Cpu
            && probe.available));
        assert_eq!(health.modules.len(), 3);
        assert!(health
            .modules
            .iter()
            .all(|module| module.state == crate::module::ModuleState::Running));
    }

    #[tokio::test]
    async fn a_new_operator_deterministically_takes_ownership_without_exposing_tokens() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();
        harness
            .authorize("first-private-token".to_string(), 30, SpeedMode::Low)
            .unwrap();
        harness
            .drive(Some("first-private-token"), 0.1, 0.1, None)
            .unwrap();

        harness
            .authorize("second-private-token".to_string(), 30, SpeedMode::High)
            .unwrap();

        let status = harness.operator_token_status();
        assert!(status.active);
        assert_eq!(
            status.owner_id.as_deref(),
            Some(operator_owner_id("second-private-token").as_str())
        );
        assert_eq!(status.speed_mode, Some(SpeedMode::High));
        assert!(status
            .expires_in_ms
            .is_some_and(|ttl| ttl > 0 && ttl <= 30_000));
        assert_eq!(harness.telemetry().left_cmd, 0.0);
        assert!(harness
            .drive(Some("first-private-token"), 0.1, 0.1, None)
            .unwrap_err()
            .to_string()
            .contains("invalid pilot token"));
        harness
            .drive(Some("second-private-token"), 0.1, 0.1, None)
            .unwrap();

        let serialized = serde_json::to_string(&harness.health()).unwrap();
        assert!(!serialized.contains("first-private-token"));
        assert!(!serialized.contains("second-private-token"));
        assert_eq!(
            harness.telemetry().session_id.as_deref(),
            Some(operator_owner_id("second-private-token").as_str())
        );
    }

    #[tokio::test]
    async fn cpu_accelerator_is_reported_without_hardware() {
        let harness = Harness::new(HarnessConfig {
            accelerator: crate::config::AcceleratorBackend::Cpu,
            require_accelerator: true,
            ..HarnessConfig::default()
        })
        .unwrap();

        let health = harness.health();
        assert!(health.ok);
        assert_eq!(
            health.accelerator.active,
            crate::config::AcceleratorBackend::Cpu
        );
        assert!(health.accelerator.available);
        assert!(health.accelerator.probes.iter().any(|probe| probe.backend
            == crate::config::AcceleratorBackend::Cpu
            && probe.selected
            && probe.available));
    }

    #[tokio::test]
    async fn capabilities_include_accelerator_probe_inventory() {
        let harness = Harness::new(HarnessConfig {
            accelerator: crate::config::AcceleratorBackend::Cuda,
            ..HarnessConfig::default()
        })
        .unwrap();

        let capabilities = harness.capabilities();
        assert_eq!(capabilities.stream_transport, "local-pubsub");
        assert_eq!(
            capabilities.adapter.category,
            crate::stack::AdapterCategory::Simulation
        );
        assert_eq!(
            capabilities.adapter.maturity,
            crate::stack::AdapterMaturity::Stable
        );
        assert!(capabilities.adapter.required_gates.is_empty());
        assert_eq!(
            capabilities.accelerator.requested,
            crate::config::AcceleratorBackend::Cuda
        );
        assert_eq!(
            capabilities.accelerator.active,
            crate::config::AcceleratorBackend::Cpu
        );
        assert!(capabilities
            .accelerator
            .probes
            .iter()
            .any(
                |probe| probe.backend == crate::config::AcceleratorBackend::Cuda
                    && !probe.available
            ));
    }

    #[tokio::test]
    async fn telemetry_is_published_to_selected_stream_transport() {
        let harness = Harness::new(HarnessConfig {
            stream_transport: crate::transport::StreamTransportBackend::Memory,
            ..HarnessConfig::default()
        })
        .unwrap();
        let mut receiver = harness.subscribe_stream("telemetry").unwrap();

        let message = receiver.recv().await.unwrap();

        assert_eq!(message.stream, "telemetry");
        assert_eq!(message.payload["kind"], "telemetry");
        assert_eq!(message.payload["telemetry"]["profile"], "sim");
        assert_eq!(message.payload["telemetry"]["vision"]["status"], "ok");
        assert_eq!(
            message.payload["telemetry"]["workers"][0]["name"],
            "simulated-perception"
        );
        assert_eq!(message.payload["telemetry"]["workers"][0]["healthy"], true);
        assert_eq!(message.payload["telemetry"]["workers"][0]["restarts"], 0);
        assert_eq!(
            message.payload["telemetry"]["workers"][0]["last_error"],
            Value::Null
        );
        assert_eq!(
            message.payload["visualization"]["detections"][0]["label"],
            "sim-fixture"
        );
        assert!(message.payload["health"]["modules"].is_array());
        assert_eq!(message.payload["safety"]["deadman_ok"], true);
    }

    #[tokio::test]
    async fn telemetry_stream_frame_includes_visualization_frame() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();
        harness.drive(None, 0.2, 0.2, Some(SpeedMode::Low)).unwrap();

        let frame = harness.telemetry_stream_frame();

        assert_eq!(frame.visualization.version, VISUALIZATION_FRAME_VERSION);
        assert_eq!(frame.visualization.robot, "robot");
        assert_eq!(frame.visualization.profile, "sim");
        assert_eq!(frame.visualization.map.frame_id, "map");
        assert_eq!(frame.visualization.map.map_id, "sim-local");
        assert_eq!(frame.visualization.map.cell_order, "row-major");
        assert_eq!(frame.visualization.pose.frame_id, "map");
        assert!(frame.visualization.pose.x_m > 0.0);
        assert_eq!(frame.visualization.twist.frame_id, "base_link");
        assert!(frame.visualization.twist.linear_x_mps > 0.0);
        assert_eq!(frame.visualization.path.poses.len(), 2);
        assert_eq!(frame.visualization.occupancy_grid.cells.len(), 16);
        assert_eq!(
            frame.visualization.occupancy_grid.metadata,
            frame.visualization.map
        );
        assert_eq!(frame.visualization.costmap.costs.len(), 16);
        assert_eq!(
            frame.visualization.costmap.metadata,
            frame.visualization.map
        );
        assert_eq!(frame.visualization.point_cloud.fields, ["x", "y", "z"]);
        assert_eq!(frame.telemetry.vision.status, "ok");
        assert_eq!(
            frame.visualization.detections,
            frame.telemetry.vision.detections
        );
        assert_eq!(frame.visualization.detections[0].label, "sim-fixture");
        assert_eq!(frame.visualization.command.left_cmd, 0.2);
        assert_eq!(frame.visualization.command.speed_mode, SpeedMode::Low);
    }

    #[tokio::test]
    async fn planner_rejects_blocked_sim_goal_without_motion() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();

        let status = harness
            .set_planner_goal(PlannerGoal {
                x_m: -0.25,
                y_m: -0.25,
                ..PlannerGoal::default()
            })
            .unwrap();

        assert!(!status.ok);
        assert!(!status.active);
        assert_eq!(status.status, "blocked");
        assert!(status.path.poses.is_empty());
        assert_eq!(harness.telemetry().left_cmd, 0.0);
        assert_eq!(harness.telemetry().right_cmd, 0.0);
    }

    #[tokio::test]
    async fn planner_accepts_goal_and_cancel_stops_motion() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();

        let status = harness
            .set_planner_goal(PlannerGoal {
                x_m: 0.25,
                y_m: 0.0,
                ..PlannerGoal::default()
            })
            .unwrap();

        assert!(status.ok);
        assert!(status.active);
        assert!(status.path.poses.len() >= 2);
        assert!(status
            .last_drive
            .as_ref()
            .is_some_and(|drive| drive.left > 0.0));
        assert!(harness.telemetry().left_cmd > 0.0);

        let status = harness.cancel_planner_goal().unwrap();

        assert!(!status.active);
        assert_eq!(status.status, "cancelled");
        assert_eq!(harness.telemetry().left_cmd, 0.0);
        assert_eq!(harness.telemetry().right_cmd, 0.0);
    }

    #[tokio::test]
    async fn stop_and_estop_cancel_planner_movement() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();
        harness
            .set_planner_goal(PlannerGoal {
                x_m: 0.25,
                y_m: 0.0,
                ..PlannerGoal::default()
            })
            .unwrap();

        harness.stop().unwrap();

        let status = harness.planner_status();
        assert!(!status.active);
        assert_eq!(status.status, "stopped");

        let harness = Harness::new(HarnessConfig::default()).unwrap();
        harness
            .set_planner_goal(PlannerGoal {
                x_m: 0.25,
                y_m: 0.0,
                ..PlannerGoal::default()
            })
            .unwrap();

        harness.estop().unwrap();

        let status = harness.planner_status();
        assert!(!status.active);
        assert_eq!(status.status, "estop");
    }

    #[tokio::test]
    async fn planner_stops_when_soft_odometry_limit_is_reached() {
        let harness = Harness::new(HarnessConfig {
            soft_odometry_limit_m: 0.001,
            ..HarnessConfig::default()
        })
        .unwrap();

        let status = harness
            .set_planner_goal(PlannerGoal {
                x_m: 0.25,
                y_m: 0.0,
                ..PlannerGoal::default()
            })
            .unwrap();
        assert!(status.active);

        time::sleep(Duration::from_millis(150)).await;

        let status = harness.planner_status();
        let telemetry = harness.telemetry();
        assert!(!status.ok);
        assert!(!status.active);
        assert_eq!(status.status, "limited");
        assert!(telemetry.soft_odometry_limited);
        assert_eq!(telemetry.left_cmd, 0.0);
        assert_eq!(telemetry.right_cmd, 0.0);
    }

    #[tokio::test]
    async fn patrol_strategy_selection_sets_safe_goals() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();

        let coverage = harness
            .start_patrol(PatrolStrategy::Coverage, SpeedMode::Low)
            .unwrap();
        assert!(coverage.ok);
        assert!(coverage.active);
        assert_eq!(coverage.strategy, Some(PatrolStrategy::Coverage));
        assert_eq!(coverage.visited_cells, vec!["2,2".to_string()]);
        assert_eq!(
            coverage.goal.as_ref().map(|goal| (goal.x_m, goal.y_m)),
            Some((-0.375, -0.375))
        );
        assert!(coverage.path.poses.len() >= 2);

        harness.stop_patrol().unwrap();

        let frontier = harness
            .start_patrol(PatrolStrategy::Frontier, SpeedMode::Low)
            .unwrap();
        assert!(frontier.ok);
        assert!(frontier.active);
        assert_eq!(frontier.strategy, Some(PatrolStrategy::Frontier));
        assert_eq!(
            frontier.goal.as_ref().map(|goal| (goal.x_m, goal.y_m)),
            Some((0.125, -0.125))
        );
    }

    #[tokio::test]
    async fn stop_patrol_cancels_planner_motion() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();
        let status = harness
            .start_patrol(PatrolStrategy::Coverage, SpeedMode::Low)
            .unwrap();
        assert!(status.active);
        assert!(harness.planner_status().active);
        assert!(harness.telemetry().left_cmd != 0.0 || harness.telemetry().right_cmd != 0.0);

        let status = harness.stop_patrol().unwrap();

        assert!(!status.active);
        assert_eq!(status.status, "stopped");
        assert!(!harness.planner_status().active);
        assert_eq!(harness.telemetry().left_cmd, 0.0);
        assert_eq!(harness.telemetry().right_cmd, 0.0);
    }

    #[tokio::test]
    async fn saved_patrol_zone_executes_in_sim_and_replay_without_physical_actuation() {
        for profile in [Profile::Sim, Profile::Replay] {
            let harness = Harness::new(HarnessConfig {
                profile,
                replay_source: (profile == Profile::Replay)
                    .then(|| std::path::PathBuf::from("examples/replay/sim-basic.jsonl")),
                ..HarnessConfig::default()
            })
            .unwrap();
            harness
                .create_waypoint(WaypointSpec {
                    id: "entry".to_string(),
                    name: "Entry".to_string(),
                    frame_id: "map".to_string(),
                    x_m: 0.25,
                    y_m: 0.0,
                    tolerance_m: 0.1,
                })
                .unwrap();
            harness
                .create_patrol_zone(PatrolZoneSpec {
                    id: "front".to_string(),
                    name: "Front".to_string(),
                    frame_id: "map".to_string(),
                    waypoint_ids: vec!["entry".to_string()],
                    boundary: vec![
                        crate::types::ZoneBoundaryPoint { x_m: 0.0, y_m: 0.0 },
                        crate::types::ZoneBoundaryPoint { x_m: 0.5, y_m: 0.0 },
                        crate::types::ZoneBoundaryPoint { x_m: 0.5, y_m: 0.5 },
                    ],
                })
                .unwrap();

            let status = harness.start_patrol_zone("front", SpeedMode::Low).unwrap();
            assert!(status.ok);
            assert!(status.active);
            assert_eq!(status.zone_id.as_deref(), Some("front"));
            assert_eq!(status.waypoint_index, Some(0));
            assert!(!harness.health().physical_actuation_enabled);
            if profile == Profile::Replay {
                assert_eq!(status.status, "replay");
                assert_eq!(harness.telemetry().left_cmd, 0.0);
            }
        }
    }

    #[cfg(feature = "manipulator")]
    #[tokio::test]
    async fn saved_patrol_zone_is_rejected_by_a_physical_profile() {
        let harness = Harness::new(HarnessConfig {
            profile: Profile::Manipulator,
            allow_physical_actuation: true,
            ..HarnessConfig::default()
        })
        .unwrap();

        let error = harness
            .start_patrol_zone("anything", SpeedMode::Low)
            .unwrap_err()
            .to_string();
        assert!(error.contains("only available for sim and replay"));
    }

    #[tokio::test]
    async fn sim_and_replay_telemetry_emit_observe_only_motion_events() {
        let sim = Harness::new(HarnessConfig::default()).unwrap();
        sim.drive(None, 0.1, 0.1, Some(SpeedMode::Low)).unwrap();
        let sim_frame = sim.telemetry();
        assert_eq!(sim_frame.motion_events.len(), 1);
        assert_eq!(sim_frame.motion_events[0].source, "simulated-motion");

        let replay = Harness::new(HarnessConfig {
            profile: Profile::Replay,
            replay_source: Some(std::path::PathBuf::from("examples/replay/sim-basic.jsonl")),
            ..HarnessConfig::default()
        })
        .unwrap();
        time::sleep(Duration::from_millis(80)).await;
        let replay_frame = replay.telemetry();
        assert_eq!(replay_frame.motion_events.len(), 1);
        assert_eq!(replay_frame.motion_events[0].source, "replay-motion");
        assert_eq!(replay_frame.left_cmd, 0.2);
    }

    #[tokio::test]
    async fn patrol_reports_no_goal_when_all_clear_cells_are_visited() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();
        harness.mark_all_patrol_cells_visited_for_test();

        let status = harness
            .start_patrol(PatrolStrategy::Coverage, SpeedMode::Low)
            .unwrap();

        assert!(!status.ok);
        assert!(!status.active);
        assert_eq!(status.status, "no-goal");
        assert!(status.goal.is_none());
        assert!(status.path.poses.is_empty());
    }

    #[tokio::test]
    async fn telemetry_frame_includes_patrol_overlay() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();
        harness
            .start_patrol(PatrolStrategy::Random, SpeedMode::Low)
            .unwrap();

        let frame = harness.telemetry_stream_frame();

        assert!(frame.visualization.autonomy.active);
        assert_eq!(frame.visualization.autonomy.mode, "patrol");
        assert_eq!(
            frame.visualization.autonomy.strategy,
            Some(PatrolStrategy::Random)
        );
        assert!(frame.visualization.autonomy.goal.is_some());
        assert_eq!(
            frame.visualization.autonomy.visited_cells,
            vec!["2,2".to_string()]
        );
    }

    #[tokio::test]
    async fn agent_messages_are_recorded_and_published_without_provider() {
        let harness = Harness::new(HarnessConfig {
            stream_transport: crate::transport::StreamTransportBackend::Memory,
            ..HarnessConfig::default()
        })
        .unwrap();
        let mut receiver = harness.subscribe_stream("agent").unwrap();

        let message = harness
            .submit_agent_message("test", "inspect the battery")
            .unwrap();
        let received = receiver.recv().await.unwrap();

        assert_eq!(message.id, 1);
        assert_eq!(message.source, "test");
        assert_eq!(message.text, "inspect the battery");
        assert_eq!(harness.agent_messages(), vec![message]);
        assert_eq!(received.stream, "agent");
        assert_eq!(received.payload["text"], "inspect the battery");
    }

    #[tokio::test]
    async fn sim_runtime_uses_deterministic_agent_provider_without_network() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();

        let response = harness
            .agent_model_response(" inspect   the battery ")
            .unwrap()
            .unwrap();

        assert_eq!(response.provider, "deterministic-test");
        assert_eq!(response.text, "deterministic-agent: inspect the battery");
    }

    #[tokio::test]
    async fn agent_messages_reject_empty_text() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();

        let err = harness.submit_agent_message("test", "   ").unwrap_err();

        assert!(err.to_string().contains("text cannot be empty"));
    }

    #[tokio::test]
    async fn replay_profile_observes_fixture_as_non_physical() {
        let harness = Harness::new(HarnessConfig {
            profile: Profile::Replay,
            replay_source: Some(std::path::PathBuf::from("examples/replay/sim-basic.jsonl")),
            ..HarnessConfig::default()
        })
        .unwrap();

        let health = harness.health();
        assert_eq!(health.mode, "replay");
        assert!(health.replay);
        assert_eq!(health.profile, "replay");
        assert!(!health.physical_actuation_enabled);

        let capabilities = harness.capabilities();
        assert_eq!(capabilities.mode, "replay");
        assert!(capabilities.replay);
        assert!(!capabilities.physical);

        let telemetry = harness.telemetry();
        assert_eq!(telemetry.profile, "replay");
        assert_eq!(telemetry.source, "replay");
        assert_eq!(telemetry.vision.status, "ok");
        assert_eq!(telemetry.vision.detections[0].label, "replay-fixture");
    }
}
