use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Result};
use parking_lot::{Mutex, RwLock};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::{sync::broadcast, time};
use tracing::{debug, warn};

#[cfg(feature = "waveshare-ugv")]
use crate::waveshare_ugv::{
    imu_with_freshness, read_base_telemetry_loop, scan_blocks_motion, spawn_ld06_reader,
    with_freshness, BaseTelemetryUpdate, WaveshareSensorConfig, WaveshareUgvDriver,
};

#[cfg(feature = "mavlink-drone")]
use crate::types::DroneCommandStatus;
#[cfg(feature = "manipulator")]
use crate::types::{
    ManipulatorCommandStatus, ManipulatorJoint, ManipulatorJointState, MANIPULATOR_SCHEMA_VERSION,
};
use crate::{
    accelerator::{resolve_accelerator, AcceleratorStatus},
    adapter::{simulated_imu_sample, simulated_range_scan, GimbalAdapter, MobileBaseAdapter},
    capability::{default_capability_descriptors, CapabilityRegistry},
    config::{AcceleratorBackend, HarnessConfig, Profile},
    localization::{
        ExternalLocalizationProvider, InProcessLocalizationProvider, LocalizationProvider,
        LocalizationProviderStatus, LocalizationProviderUpdate,
        DEFAULT_LOCALIZATION_STALE_AFTER_MS,
    },
    memory::{
        default_spatial_memory_path, SpatialMemoryQuery, SpatialMemoryStore, SpatialMemoryTag,
    },
    module::{default_module_graph, ModuleCoordinator, ModuleGraph},
    navigation::{navigation_path_for_memory, NavigationStore, PatrolZoneSpec, WaypointSpec},
    perception::PerceptionRuntime,
    replay::{replay_telemetry_source, ReplayPlayback},
    transport::{new_stream_transport, StreamSubscriber, StreamTransport},
    types::{
        AgentMessage, AgentModelResponse, AppliedActionEvidence, AppliedActionEvidencePage,
        BatteryStatus, CameraAimOutcome, CameraStatus, Capabilities, CaptureResult, CommandOverlay,
        CommandStreamState, CostmapFrame, DriveOutcome, Health, ImageObservation, ImuStatus,
        LocalizationFrame, LocalizationHealth, LocalizationStatus, MapIdentity, MapMetadata,
        MotionEvent, MotionEventKind, OccupancyGridFrame, OdometryStatus, OperatorTokenStatus,
        PatrolStatus, PatrolStrategy, PatrolZoneList, PlannerGoal, PlannerStatus,
        PointCloudMetadata, Pose2d, PoseWithCovariance2d, RangeScanStatus, RawFrameStatus,
        ResourceSample, SafetyStreamState, SavedWaypointList, SensorSnapshot, SpatialMemoryStatus,
        SpeedMode, TelemetryFrame, TelemetryStreamFrame, Twist2d, VisionResult, VisualizationFrame,
        VisualizationPath, VoxelCell, VoxelGridFrame, APPLIED_ACTION_PAGE_SCHEMA_VERSION,
        APPLIED_ACTION_SCHEMA_VERSION, COST_FREE, COST_LETHAL, LOCALIZATION_FRAME_VERSION,
        OCCUPANCY_FREE, OCCUPANCY_OCCUPIED, SENSOR_CONTRACT_VERSION, VISUALIZATION_FRAME_VERSION,
        VOXEL_GRID_VERSION,
    },
};

const AGENT_MESSAGE_LIMIT: usize = 128;
const DASHBOARD_EVENT_LIMIT: usize = 64;
const ACTION_EVIDENCE_HISTORY_CAPACITY: usize = 4096;
const ACTION_EVIDENCE_HEARTBEAT_MS: u64 = 100;
const ACTION_SAFETY_COLLISION_CLAMP: u32 = 1 << 0;
const ACTION_SAFETY_SOFT_ODOMETRY_LIMIT: u32 = 1 << 1;
const ACTION_SAFETY_ESTOP: u32 = 1 << 2;
const ACTION_SAFETY_DEADMAN: u32 = 1 << 3;
const PLANNER_GRID_WIDTH: usize = 4;
const PLANNER_GRID_HEIGHT: usize = 4;
const PLANNER_GRID_CELLS: usize = PLANNER_GRID_WIDTH * PLANNER_GRID_HEIGHT;
const PLANNER_RESOLUTION_M: f64 = 0.25;
const PLANNER_ORIGIN_X_M: f64 = -0.5;
const PLANNER_ORIGIN_Y_M: f64 = -0.5;
const PLANNER_BLOCKED_CELLS: &[(usize, usize)] = &[(1, 1)];
const PLANNER_STEP_CMD: f64 = 0.2;
#[cfg(feature = "physical-navigation")]
const PHYSICAL_NAV_COMMAND_INTERVAL_MS: u64 = 100;
#[cfg(feature = "physical-navigation")]
const PHYSICAL_NAV_SENSOR_FRESH_MS: u128 = 500;
#[cfg(feature = "physical-navigation")]
const PHYSICAL_NAV_MIN_CLEARANCE_M: f64 = 0.15;
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

pub(crate) trait RobotDriver: MobileBaseAdapter + GimbalAdapter {
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

#[derive(Debug, Clone, Copy)]
struct ActionCommandEvidence {
    requested_left: f64,
    requested_right: f64,
    clamped_left: f64,
    clamped_right: f64,
    applied_left: f64,
    applied_right: f64,
    speed_scale: f64,
    safety_flags: u32,
    valid: bool,
    armed: bool,
    deadman_active: bool,
    collision_clamped: bool,
}

impl ActionCommandEvidence {
    fn stopped(speed_scale: f64, safety_flags: u32) -> Self {
        Self {
            requested_left: 0.0,
            requested_right: 0.0,
            clamped_left: 0.0,
            clamped_right: 0.0,
            applied_left: 0.0,
            applied_right: 0.0,
            speed_scale,
            safety_flags,
            valid: true,
            armed: false,
            deadman_active: safety_flags & ACTION_SAFETY_DEADMAN != 0,
            collision_clamped: safety_flags & ACTION_SAFETY_COLLISION_CLAMP != 0,
        }
    }
}

#[derive(Debug)]
struct ActionEvidenceState {
    producer_epoch: u64,
    next_sequence: u64,
    interval_start_ns: u64,
    current: ActionCommandEvidence,
    history: VecDeque<AppliedActionEvidence>,
}

impl ActionEvidenceState {
    fn new(producer_epoch: u64, speed_scale: f64) -> Self {
        Self {
            producer_epoch: producer_epoch.max(1),
            next_sequence: 1,
            interval_start_ns: now_ns().max(1),
            current: ActionCommandEvidence::stopped(speed_scale, 0),
            history: VecDeque::with_capacity(ACTION_EVIDENCE_HISTORY_CAPACITY),
        }
    }

    fn seal(&mut self, interval_end_ns: u64) -> Option<AppliedActionEvidence> {
        if interval_end_ns <= self.interval_start_ns {
            return None;
        }
        let action_sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        let current = self.current;
        let evidence = AppliedActionEvidence {
            schema_version: APPLIED_ACTION_SCHEMA_VERSION.to_string(),
            authority: "leash".to_string(),
            producer_epoch: self.producer_epoch,
            action_sequence,
            interval_start_ns: self.interval_start_ns,
            interval_end_ns,
            requested_left: current.requested_left,
            requested_right: current.requested_right,
            clamped_left: current.clamped_left,
            clamped_right: current.clamped_right,
            applied_left: current.applied_left,
            applied_right: current.applied_right,
            speed_scale: current.speed_scale,
            safety_flags: current.safety_flags,
            valid: current.valid,
            armed: current.armed,
            deadman_active: current.deadman_active,
            collision_clamped: current.collision_clamped,
        };
        if self.history.len() == ACTION_EVIDENCE_HISTORY_CAPACITY {
            self.history.pop_front();
        }
        self.history.push_back(evidence.clone());
        self.interval_start_ns = interval_end_ns;
        Some(evidence)
    }

    fn transition(&mut self, at_ns: u64, next: ActionCommandEvidence) {
        self.seal(at_ns);
        self.current = next;
        self.interval_start_ns = self.interval_start_ns.max(at_ns);
    }

    fn set_speed_scale(&mut self, at_ns: u64, speed_scale: f64) {
        self.seal(at_ns);
        self.current.speed_scale = speed_scale;
        self.interval_start_ns = self.interval_start_ns.max(at_ns);
    }

    fn page(&self, after_sequence: u64, limit: usize) -> Result<AppliedActionEvidencePage> {
        let oldest_sequence = self
            .history
            .front()
            .map(|entry| entry.action_sequence)
            .unwrap_or(0);
        let latest_sequence = self
            .history
            .back()
            .map(|entry| entry.action_sequence)
            .unwrap_or(0);
        if after_sequence != 0
            && oldest_sequence != 0
            && after_sequence.saturating_add(1) < oldest_sequence
        {
            return Err(anyhow!(
                "applied-action history overrun: requested after {after_sequence}, oldest is {oldest_sequence}"
            ));
        }
        let entries = self
            .history
            .iter()
            .filter(|entry| entry.action_sequence > after_sequence)
            .take(limit.clamp(1, 512))
            .cloned()
            .collect();
        Ok(AppliedActionEvidencePage {
            schema_version: APPLIED_ACTION_PAGE_SCHEMA_VERSION.to_string(),
            producer_epoch: self.producer_epoch,
            oldest_sequence,
            latest_sequence,
            entries,
        })
    }
}

#[cfg(feature = "physical-navigation")]
#[derive(Debug, Clone)]
struct PhysicalNavigationLease {
    token: String,
    approval: bool,
    last_command_at: Option<Instant>,
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
    range_scan: RangeScanStatus,
    imu: ImuStatus,
}

impl RawTelemetry {
    fn sim() -> Self {
        let ts_ms = now_ms();
        Self {
            battery_v: Some(12.3),
            battery_pct: battery_percent_from_voltage(12.3),
            odometry_left: Some(0.0),
            odometry_right: Some(0.0),
            source: "sim".to_string(),
            last_raw_frame_ms: Some(ts_ms),
            last_raw_payload: None,
            range_scan: simulated_range_scan(ts_ms),
            imu: simulated_imu_sample(ts_ms),
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
            range_scan: RangeScanStatus {
                source: source.to_string(),
                ..RangeScanStatus::default()
            },
            imu: ImuStatus {
                source: source.to_string(),
                ..ImuStatus::default()
            },
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
            range_scan: RangeScanStatus {
                source: "replay".to_string(),
                ..RangeScanStatus::default()
            },
            imu: ImuStatus {
                source: "replay".to_string(),
                ..ImuStatus::default()
            },
        }
    }
}

#[derive(Clone)]
pub struct Harness {
    config: HarnessConfig,
    started_at: Instant,
    driver: Arc<dyn RobotDriver>,
    command: Arc<Mutex<CommandState>>,
    action_evidence: Arc<Mutex<ActionEvidenceState>>,
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
    localization_provider: InProcessLocalizationProvider,
    localization_input: ExternalLocalizationProvider,
    localization_seq: Arc<AtomicU64>,
    #[cfg(feature = "waveshare-ugv")]
    waveshare_sensors: Option<Arc<WaveshareSensorConfig>>,
    #[cfg(feature = "physical-navigation")]
    physical_navigation_lease: Arc<Mutex<Option<PhysicalNavigationLease>>>,
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

    #[cfg(all(test, feature = "physical-navigation"))]
    pub(crate) fn new_with_test_driver(config: HarnessConfig) -> Result<Self> {
        Self::new_inner_with_driver(config, None, Some(Arc::new(SimDriver)))
    }

    fn new_inner(config: HarnessConfig, memory_path: Option<PathBuf>) -> Result<Self> {
        Self::new_inner_with_driver(config, memory_path, None)
    }

    fn new_inner_with_driver(
        config: HarnessConfig,
        memory_path: Option<PathBuf>,
        driver_override: Option<Arc<dyn RobotDriver>>,
    ) -> Result<Self> {
        config.validate()?;
        let accelerator = resolve_accelerator(config.accelerator, config.require_accelerator)?;
        #[cfg(feature = "waveshare-ugv")]
        let waveshare_sensors = (config.profile == Profile::WaveshareUgv)
            .then(WaveshareSensorConfig::from_env)
            .transpose()?
            .map(Arc::new);
        let instance_id = HARNESS_INSTANCE_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
        let memory_path =
            memory_path.unwrap_or_else(|| default_spatial_memory_path(&config, instance_id));
        let navigation_path = navigation_path_for_memory(&memory_path);
        let spatial_memory = Arc::new(SpatialMemoryStore::open(memory_path)?);
        let navigation = Arc::new(NavigationStore::open(navigation_path)?);

        let driver: Arc<dyn RobotDriver> = if let Some(driver) = driver_override {
            driver
        } else {
            match config.profile {
                Profile::Sim => Arc::new(SimDriver),
                Profile::Replay => Arc::new(ReplayDriver),
                Profile::WaveshareUgv => open_physical_driver(&config)?,
                Profile::MavlinkDrone => open_physical_driver(&config)?,
                Profile::Manipulator => open_physical_driver(&config)?,
            }
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
        let localization_provider = InProcessLocalizationProvider::new(
            format!("{}-localization", config.profile.as_str()),
            DEFAULT_LOCALIZATION_STALE_AFTER_MS,
        );
        let localization_sequence = if config.profile == Profile::Sim {
            let ts_ms = now_ms();
            let localization = simulated_localization_frame(ts_ms, &raw);
            let (map, occupancy_grid, costmap) = simulated_map_frames(ts_ms);
            let voxel_grid = projected_voxel_grid(&occupancy_grid, 0.25);
            localization_provider.apply_at(
                LocalizationProviderUpdate {
                    version: crate::localization::LOCALIZATION_PROVIDER_UPDATE_VERSION.to_string(),
                    sequence: 1,
                    localization,
                    map,
                    occupancy_grid,
                    costmap,
                    path: VisualizationPath::default(),
                    voxel_grid,
                },
                ts_ms,
            )?;
            1
        } else {
            0
        };
        let localization_input =
            ExternalLocalizationProvider::from_provider(localization_provider.clone(), 64);

        let capabilities = default_capability_descriptors()
            .into_iter()
            .map(|descriptor| descriptor.name)
            .collect();
        let mut coordinator = ModuleCoordinator::new(default_module_graph(&config, capabilities));
        coordinator.start()?;

        let (telemetry_tx, _) = broadcast::channel(128);
        let stream_transport = new_stream_transport(config.stream_transport);
        let action_producer_epoch = now_ns().saturating_add(instance_id).max(1);
        let harness = Self {
            config,
            started_at: Instant::now(),
            driver,
            command: Arc::new(Mutex::new(CommandState::default())),
            action_evidence: Arc::new(Mutex::new(ActionEvidenceState::new(
                action_producer_epoch,
                SpeedMode::default().cap(),
            ))),
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
            localization_provider,
            localization_input,
            localization_seq: Arc::new(AtomicU64::new(localization_sequence)),
            #[cfg(feature = "waveshare-ugv")]
            waveshare_sensors,
            #[cfg(feature = "physical-navigation")]
            physical_navigation_lease: Arc::new(Mutex::new(None)),
            coordinator: Arc::new(RwLock::new(coordinator)),
            accelerator,
        };
        harness.spawn_deadman();
        harness.spawn_action_evidence_loop();
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

    pub fn applied_action_evidence(
        &self,
        after_sequence: u64,
        limit: usize,
    ) -> Result<AppliedActionEvidencePage> {
        self.action_evidence.lock().page(after_sequence, limit)
    }

    fn seal_action_evidence(&self, at_ns: u64) {
        self.action_evidence.lock().seal(at_ns);
    }

    fn spawn_action_evidence_loop(&self) {
        let harness = self.clone();
        tokio::spawn(async move {
            loop {
                time::sleep(Duration::from_millis(ACTION_EVIDENCE_HEARTBEAT_MS)).await;
                harness.seal_action_evidence(now_ns());
            }
        });
    }

    pub fn submit_localization_update(&self, update: LocalizationProviderUpdate) -> Result<()> {
        self.localization_input.submit(update).map_err(Into::into)
    }

    pub fn disconnect_localization_provider(&self, error: impl Into<String>) -> Result<()> {
        self.localization_input
            .disconnect(error)
            .map_err(Into::into)
    }

    pub fn fail_localization_provider(&self, error: impl Into<String>) -> Result<()> {
        self.localization_input.fail(error).map_err(Into::into)
    }

    pub fn localization_provider_status(&self) -> LocalizationProviderStatus {
        self.localization_provider.snapshot(now_ms()).status
    }

    pub fn update_range_scan_status(&self, status: RangeScanStatus) -> Result<()> {
        status.validate()?;
        self.raw.write().range_scan = status;
        Ok(())
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

    pub fn set_planner_goal_authorized(
        &self,
        goal: PlannerGoal,
        token: Option<&str>,
        approval: bool,
    ) -> Result<PlannerStatus> {
        if !self.config.profile.is_physical() {
            return self.set_planner_goal(goal);
        }
        #[cfg(feature = "physical-navigation")]
        {
            self.set_physical_planner_goal(goal, token, approval)
        }
        #[cfg(not(feature = "physical-navigation"))]
        {
            let _ = (goal, token, approval);
            Err(anyhow!(
                "physical planner requires the 'physical-navigation' compile-time feature"
            ))
        }
    }

    pub fn cancel_planner_goal(&self) -> Result<PlannerStatus> {
        self.clear_physical_navigation_lease();
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
        let map = self.map_scope_for_frame(&tag.frame_id);
        self.spatial_memory.tag_scoped(tag, map)
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
        let map = self.map_scope_for_frame(&spec.frame_id);
        self.navigation.create_waypoint_scoped(spec, map)
    }

    pub fn update_waypoint(&self, spec: WaypointSpec) -> Result<SavedWaypointList> {
        let map = self.map_scope_for_frame(&spec.frame_id);
        self.navigation.update_waypoint_scoped(spec, map)
    }

    pub fn delete_waypoint(&self, id: &str) -> Result<SavedWaypointList> {
        self.navigation.delete_waypoint(id)
    }

    fn map_scope_for_frame(&self, frame_id: &str) -> Option<MapIdentity> {
        let map = self
            .localization_provider
            .snapshot(now_ms())
            .localization
            .map;
        (map.frame_id == frame_id
            && !map.map_id.trim().is_empty()
            && !map.map_revision.trim().is_empty())
        .then_some(map)
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

    pub fn start_patrol_zone_authorized(
        &self,
        zone_id: &str,
        speed_mode: SpeedMode,
        token: Option<&str>,
        approval: bool,
    ) -> Result<PatrolStatus> {
        if !self.config.profile.is_physical() {
            return self.start_patrol_zone(zone_id, speed_mode);
        }
        #[cfg(feature = "physical-navigation")]
        {
            self.start_physical_patrol_zone(zone_id, token, approval)
        }
        #[cfg(not(feature = "physical-navigation"))]
        {
            let _ = (zone_id, speed_mode, token, approval);
            Err(anyhow!(
                "physical patrol requires the 'physical-navigation' compile-time feature"
            ))
        }
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

    pub fn start_patrol_authorized(
        &self,
        strategy: PatrolStrategy,
        speed_mode: SpeedMode,
        token: Option<&str>,
        approval: bool,
    ) -> Result<PatrolStatus> {
        if !self.config.profile.is_physical() {
            return self.start_patrol(strategy, speed_mode);
        }
        #[cfg(feature = "physical-navigation")]
        {
            let _ = speed_mode;
            self.start_physical_patrol(strategy, token, approval)
        }
        #[cfg(not(feature = "physical-navigation"))]
        {
            let _ = (strategy, speed_mode, token, approval);
            Err(anyhow!(
                "physical patrol requires the 'physical-navigation' compile-time feature"
            ))
        }
    }

    #[cfg(feature = "physical-navigation")]
    fn set_physical_planner_goal(
        &self,
        mut goal: PlannerGoal,
        token: Option<&str>,
        approval: bool,
    ) -> Result<PlannerStatus> {
        let token = self.validate_physical_navigation_authorization(token, approval)?;
        let snapshot = self.physical_navigation_snapshot()?;
        let current = snapshot
            .localization
            .pose
            .as_ref()
            .map(|localized| localized.pose.clone())
            .ok_or_else(|| anyhow!("physical navigation requires a localized pose"))?;
        if goal.frame_id != current.frame_id {
            return Err(anyhow!(
                "physical planner goal frame '{}' does not match localization frame '{}'",
                goal.frame_id,
                current.frame_id
            ));
        }
        if !goal.x_m.is_finite() || !goal.y_m.is_finite() {
            return Err(anyhow!("physical planner goal coordinates must be finite"));
        }
        if !goal.tolerance_m.is_finite() || goal.tolerance_m <= 0.0 {
            return Err(anyhow!(
                "physical planner goal tolerance_m must be positive and finite"
            ));
        }
        goal.speed_mode = SpeedMode::Low;
        let ts_ms = now_ms();
        let path = VisualizationPath {
            ts_ms,
            frame_id: goal.frame_id.clone(),
            poses: vec![
                current,
                Pose2d {
                    ts_ms,
                    frame_id: goal.frame_id.clone(),
                    x_m: goal.x_m,
                    y_m: goal.y_m,
                    yaw_rad: 0.0,
                },
            ],
        };
        *self.physical_navigation_lease.lock() = Some(PhysicalNavigationLease {
            token,
            approval,
            last_command_at: None,
        });
        {
            let mut command = self.command.lock();
            command.stopped_by_deadman = false;
            command.speed_mode = SpeedMode::Low;
        }
        *self.planner.lock() = PlannerStatus {
            ok: true,
            active: true,
            status: "active".to_string(),
            message: "physical planner goal accepted through safety gate".to_string(),
            goal: Some(goal),
            path,
            last_drive: None,
        };
        self.planner_step();
        Ok(self.planner_status())
    }

    #[cfg(feature = "physical-navigation")]
    fn start_physical_patrol(
        &self,
        strategy: PatrolStrategy,
        token: Option<&str>,
        approval: bool,
    ) -> Result<PatrolStatus> {
        let token = self.validate_physical_navigation_authorization(token, approval)?;
        let snapshot = self.physical_navigation_snapshot()?;
        let goal = physical_patrol_goal(strategy, &snapshot)?;
        let planner = self.set_physical_planner_goal(goal.clone(), Some(&token), approval)?;
        let mut patrol = self.patrol.lock();
        patrol.status = patrol_status(PatrolStatusUpdate {
            ok: planner.ok,
            active: planner.active,
            status: &planner.status,
            message: "physical patrol goal accepted through safety gate",
            strategy: Some(strategy),
            speed_mode: SpeedMode::Low,
            goal: Some(goal),
            path: planner.path,
        });
        Ok(patrol.status_with_visited())
    }

    #[cfg(feature = "physical-navigation")]
    fn start_physical_patrol_zone(
        &self,
        zone_id: &str,
        token: Option<&str>,
        approval: bool,
    ) -> Result<PatrolStatus> {
        let token = self.validate_physical_navigation_authorization(token, approval)?;
        let snapshot = self.physical_navigation_snapshot()?;
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
        if waypoint.map.as_ref() != Some(&snapshot.localization.map) {
            return Err(anyhow!(
                "physical patrol waypoint map identity does not match the active provider map"
            ));
        }
        let goal = PlannerGoal {
            frame_id: waypoint.frame_id,
            x_m: waypoint.x_m,
            y_m: waypoint.y_m,
            tolerance_m: waypoint.tolerance_m,
            speed_mode: SpeedMode::Low,
        };
        let planner = self.set_physical_planner_goal(goal.clone(), Some(&token), approval)?;
        let mut patrol = self.patrol.lock();
        patrol.status = patrol_status(PatrolStatusUpdate {
            ok: planner.ok,
            active: planner.active,
            status: &planner.status,
            message: "physical patrol zone accepted through safety gate",
            strategy: Some(PatrolStrategy::Coverage),
            speed_mode: SpeedMode::Low,
            goal: Some(goal),
            path: planner.path,
        });
        patrol.status.zone_id = Some(zone.id);
        patrol.status.waypoint_index = Some(0);
        Ok(patrol.status_with_visited())
    }

    #[cfg(feature = "physical-navigation")]
    fn validate_physical_navigation_authorization(
        &self,
        token: Option<&str>,
        approval: bool,
    ) -> Result<String> {
        if !self.config.allow_physical_navigation {
            return Err(anyhow!(
                "physical navigation requires --allow-physical-navigation or LEASH_ALLOW_PHYSICAL_NAVIGATION=1"
            ));
        }
        if crate::stack::adapter_profile_for_profile(self.config.profile).category
            != crate::stack::AdapterCategory::MobileBase
        {
            return Err(anyhow!(
                "physical navigation requires a mobile-base adapter profile"
            ));
        }
        if !self.physical_actuation_enabled() {
            return Err(anyhow!(
                "physical navigation requires the physical actuation gate"
            ));
        }
        if matches!(
            self.config.policy_mode,
            crate::config::PolicyMode::DryRun | crate::config::PolicyMode::Deny
        ) {
            return Err(anyhow!(
                "physical navigation requires an executing token or approval policy"
            ));
        }
        if !approval {
            return Err(anyhow!("physical navigation requires approval=true"));
        }
        let token = token.ok_or_else(|| anyhow!("physical navigation requires a pilot token"))?;
        self.validate_required_session(token)?;
        Ok(token.to_string())
    }

    #[cfg(feature = "physical-navigation")]
    fn physical_navigation_snapshot(
        &self,
    ) -> Result<crate::localization::LocalizationProviderSnapshot> {
        let command = self.command.lock();
        if command.estop {
            return Err(anyhow!("physical navigation blocked by latched estop"));
        }
        if command.stopped_by_deadman {
            return Err(anyhow!("physical navigation blocked by deadman stop"));
        }
        if command.soft_odometry_limited {
            return Err(anyhow!(
                "physical navigation blocked by soft odometry limit"
            ));
        }
        drop(command);

        let now = now_ms();
        let snapshot = self.localization_provider.snapshot(now);
        if snapshot.status.state != crate::localization::LocalizationProviderState::Tracking
            || snapshot.localization.health.status != LocalizationStatus::Tracking
            || snapshot.localization.pose.is_none()
        {
            return Err(anyhow!(
                "physical navigation requires fresh tracking localization"
            ));
        }
        let range_scan = self.raw.read().range_scan.clone();
        if range_scan.status != crate::types::SensorDataStatus::Available {
            return Err(anyhow!("physical navigation requires available lidar"));
        }
        let last_ms = range_scan
            .last_ms
            .ok_or_else(|| anyhow!("physical navigation lidar timestamp is missing"))?;
        if now.saturating_sub(last_ms) > PHYSICAL_NAV_SENSOR_FRESH_MS {
            return Err(anyhow!("physical navigation lidar is stale"));
        }
        let scan = range_scan
            .sample
            .as_ref()
            .ok_or_else(|| anyhow!("physical navigation lidar sample is missing"))?;
        if scan
            .ranges_m
            .iter()
            .flatten()
            .any(|range| *range <= PHYSICAL_NAV_MIN_CLEARANCE_M)
        {
            return Err(anyhow!("physical navigation path is blocked by lidar"));
        }
        Ok(snapshot)
    }

    pub fn revoke_physical_navigation_approval(&self) {
        #[cfg(feature = "physical-navigation")]
        if let Some(lease) = self.physical_navigation_lease.lock().as_mut() {
            lease.approval = false;
        }
    }

    fn clear_physical_navigation_lease(&self) {
        #[cfg(feature = "physical-navigation")]
        {
            *self.physical_navigation_lease.lock() = None;
        }
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
        #[cfg(feature = "waveshare-ugv")]
        self.refresh_waveshare_sensor_freshness();
        let command = self.command.lock().clone();
        let coordinator = self.coordinator.read();
        #[cfg(feature = "waveshare-ugv")]
        let sensors_ok = self.waveshare_sensor_health_ok();
        #[cfg(not(feature = "waveshare-ugv"))]
        let sensors_ok = true;
        Health {
            ok: coordinator.is_healthy() && sensors_ok,
            mode: self.runtime_mode().to_string(),
            replay: self.replay.is_some(),
            role: self.config.role.clone(),
            profile: self.config.profile.as_str().to_string(),
            uptime_ms: self.started_at.elapsed().as_millis(),
            estop: command.estop,
            deadman_ok: !command.stopped_by_deadman,
            physical_actuation_enabled: self.physical_actuation_enabled(),
            physical_navigation_enabled: self.physical_navigation_enabled(),
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
            physical_navigation_enabled: self.physical_navigation_enabled(),
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
                "GET /action-evidence".to_string(),
                "GET /evidence/action/applied".to_string(),
                "GET /localization".to_string(),
                "POST /localization/update".to_string(),
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
            let now = now_ms();
            let mut telemetry = replay_telemetry_source(frame.telemetry);
            let sequence = self.localization_seq.fetch_add(1, Ordering::Relaxed) + 1;
            let _ = self.localization_provider.apply_at(
                LocalizationProviderUpdate::from_telemetry(sequence, &telemetry),
                now,
            );
            let snapshot = self.localization_provider.snapshot(now);
            apply_localization_snapshot(&mut telemetry, snapshot);
            telemetry.voxel_grid = self.voxel_grid_for(&telemetry.occupancy_grid);
            return self.telemetry_with_vision(telemetry);
        }

        let now = now_ms();
        #[cfg(feature = "waveshare-ugv")]
        self.refresh_waveshare_sensor_freshness();
        let command = self.command.lock().clone();
        let raw = self.raw.read().clone();
        let sensors = sensor_snapshot(&raw);
        if self.config.profile == Profile::Sim {
            let localization = simulated_localization_frame(now, &raw);
            let (map, occupancy_grid, costmap) = simulated_map_frames(now);
            let voxel_grid = projected_voxel_grid(&occupancy_grid, 0.25);
            let sequence = self.localization_seq.fetch_add(1, Ordering::Relaxed) + 1;
            let _ = self.localization_provider.apply_at(
                LocalizationProviderUpdate {
                    version: crate::localization::LOCALIZATION_PROVIDER_UPDATE_VERSION.to_string(),
                    sequence,
                    localization,
                    map,
                    occupancy_grid,
                    costmap,
                    path: VisualizationPath::default(),
                    voxel_grid,
                },
                now,
            );
        }
        let localization_snapshot = self.localization_provider.snapshot(now);
        let voxel_grid = self.voxel_grid_for(&localization_snapshot.occupancy_grid);
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
            localization: localization_snapshot.localization,
            localization_provider: localization_snapshot.status,
            map: localization_snapshot.map,
            occupancy_grid: localization_snapshot.occupancy_grid,
            costmap: localization_snapshot.costmap,
            path: localization_snapshot.path,
            voxel_grid,
            vision: VisionResult::default(),
            workers,
            motion_events: Vec::new(),
            resource,
            source: raw.source,
        };
        self.telemetry_with_vision(telemetry)
    }

    #[cfg(feature = "waveshare-ugv")]
    fn refresh_waveshare_sensor_freshness(&self) {
        if self.config.profile != Profile::WaveshareUgv {
            return;
        }
        let Some(config) = self.waveshare_sensors.as_ref() else {
            return;
        };
        let now = now_ms();
        let mut raw = self.raw.write();
        raw.imu = imu_with_freshness(raw.imu.clone(), now, config.imu.stale_after_ms);
        if let Some(stale_after_ms) = config.lidar_stale_after_ms() {
            raw.range_scan = with_freshness(raw.range_scan.clone(), now, stale_after_ms);
        }
    }

    fn voxel_grid_for(&self, grid: &OccupancyGridFrame) -> VoxelGridFrame {
        if self.accelerator.active == AcceleratorBackend::Cuda {
            #[cfg(feature = "cuda")]
            match cuda_projected_voxel_grid(grid, 0.25) {
                Ok(voxels) => return voxels,
                Err(error) => {
                    tracing::error!(?error, "CUDA voxel projection failed after startup probe");
                    return VoxelGridFrame {
                        source: "cuda-error".to_string(),
                        ..VoxelGridFrame::default()
                    };
                }
            }
        }
        projected_voxel_grid(grid, 0.25)
    }

    #[cfg(feature = "waveshare-ugv")]
    fn waveshare_sensor_health_ok(&self) -> bool {
        if self.config.profile != Profile::WaveshareUgv {
            return true;
        }
        let Some(config) = self.waveshare_sensors.as_ref() else {
            return false;
        };
        let raw = self.raw.read();
        raw.imu.status == crate::types::SensorDataStatus::Available
            && (!config.lidar_is_configured()
                || raw.range_scan.status == crate::types::SensorDataStatus::Available)
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
                physical_navigation_enabled: self.physical_navigation_enabled(),
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
        let map = telemetry.map.clone();
        let planner_path = self.planner.lock().path.clone();
        let pose = telemetry
            .localization
            .pose
            .as_ref()
            .map(|localized| localized.pose.clone())
            .unwrap_or(Pose2d {
                ts_ms: telemetry.ts_ms,
                frame_id: "map".to_string(),
                x_m,
                y_m: 0.0,
                yaw_rad,
            });
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
                telemetry.path.clone()
            } else {
                planner_path
            },
            occupancy_grid: telemetry.occupancy_grid.clone(),
            costmap: telemetry.costmap.clone(),
            voxel_grid: telemetry.voxel_grid.clone(),
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
            range_scan: telemetry.sensors.range_scan.clone(),
            imu: telemetry.sensors.imu.clone(),
            localization: telemetry.localization.clone(),
            localization_provider: telemetry.localization_provider.clone(),
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

    pub fn physical_navigation_enabled(&self) -> bool {
        cfg!(feature = "physical-navigation")
            && self.config.profile.is_physical()
            && self.config.allow_physical_navigation
            && self.physical_actuation_enabled()
    }

    fn write_drive_with_evidence(
        &self,
        left: f64,
        right: f64,
        next: ActionCommandEvidence,
    ) -> Result<()> {
        let mut evidence = self.action_evidence.lock();
        self.driver.drive(left, right)?;
        evidence.transition(now_ns(), next);
        Ok(())
    }

    fn write_stop_with_evidence(&self, next: ActionCommandEvidence) -> Result<()> {
        let mut evidence = self.action_evidence.lock();
        self.driver.stop()?;
        evidence.transition(now_ns(), next);
        Ok(())
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
            let mut command = self.command.lock();
            self.write_stop_with_evidence(ActionCommandEvidence::stopped(
                command.speed_mode.cap(),
                0,
            ))?;
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
        let mut command = self.command.lock();
        self.action_evidence
            .lock()
            .set_speed_scale(now_ns(), speed_mode.cap());
        command.speed_mode = speed_mode;
        Ok(())
    }

    pub fn drive(
        &self,
        token: Option<&str>,
        left: f64,
        right: f64,
        speed_mode: Option<SpeedMode>,
    ) -> Result<DriveOutcome> {
        let requested_left = left;
        let requested_right = right;
        let session = self.validate_session(token)?;
        let speed_mode = speed_mode.or(session.map(|session| session.speed_mode));
        if let Some(speed_mode) = speed_mode {
            self.command.lock().speed_mode = speed_mode;
        }

        #[cfg(feature = "waveshare-ugv")]
        if (left.abs() > f64::EPSILON || right.abs() > f64::EPSILON)
            && self.obstacle_blocks_motion()
        {
            self.stop_for_obstacle(requested_left, requested_right)?;
            return Err(anyhow!(
                "drive blocked by the configured lidar collision threshold"
            ));
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

        let safety_flags = if command.soft_odometry_limited {
            ACTION_SAFETY_SOFT_ODOMETRY_LIMIT
        } else {
            0
        };
        self.write_drive_with_evidence(
            left,
            right,
            ActionCommandEvidence {
                requested_left,
                requested_right,
                clamped_left: left,
                clamped_right: right,
                applied_left: left,
                applied_right: right,
                speed_scale: max_speed,
                safety_flags,
                valid: true,
                armed: token.is_some(),
                deadman_active: false,
                collision_clamped: false,
            },
        )?;
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

    #[cfg(feature = "waveshare-ugv")]
    fn obstacle_blocks_motion(&self) -> bool {
        if self.config.profile != Profile::WaveshareUgv {
            return false;
        }
        let Some(config) = self.waveshare_sensors.as_ref() else {
            return false;
        };
        let Some(threshold_m) = config.collision_threshold_m() else {
            return false;
        };
        let Some(stale_after_ms) = config.lidar_stale_after_ms() else {
            return false;
        };
        let status = with_freshness(self.raw.read().range_scan.clone(), now_ms(), stale_after_ms);
        scan_blocks_motion(&status, threshold_m)
    }

    #[cfg(feature = "waveshare-ugv")]
    fn enforce_obstacle_stop(&self) {
        if !self.obstacle_blocks_motion() {
            return;
        }
        let moving = {
            let command = self.command.lock();
            command.left_cmd.abs() > f64::EPSILON || command.right_cmd.abs() > f64::EPSILON
        };
        if moving {
            if let Err(error) = self.stop_for_obstacle(0.0, 0.0) {
                warn!(?error, "lidar collision stop failed");
            }
        }
    }

    #[cfg(feature = "waveshare-ugv")]
    fn stop_for_obstacle(&self, requested_left: f64, requested_right: f64) -> Result<()> {
        {
            let mut command = self.command.lock();
            self.write_stop_with_evidence(ActionCommandEvidence {
                requested_left,
                requested_right,
                clamped_left: 0.0,
                clamped_right: 0.0,
                applied_left: 0.0,
                applied_right: 0.0,
                speed_scale: command.speed_mode.cap(),
                safety_flags: ACTION_SAFETY_COLLISION_CLAMP,
                valid: true,
                armed: command.active_session_id.is_some(),
                deadman_active: false,
                collision_clamped: true,
            })?;
            command.left_cmd = 0.0;
            command.right_cmd = 0.0;
            command.last_cmd_at = Some(Instant::now());
        }
        self.clear_physical_navigation_lease();
        self.cancel_planner_state(
            "collision-stop",
            "planner movement cancelled by lidar collision threshold",
        );
        self.cancel_patrol_state(
            "collision-stop",
            "patrol movement cancelled by lidar collision threshold",
        );
        Ok(())
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
        self.clear_physical_navigation_lease();
        self.cancel_patrol_state("stopped", "patrol movement stopped");
        self.cancel_planner_state("stopped", "planner movement stopped");
        self.stop_without_planner_cancel()
    }

    fn stop_without_planner_cancel(&self) -> Result<DriveOutcome> {
        let mut command = self.command.lock();
        self.write_stop_with_evidence(ActionCommandEvidence::stopped(command.speed_mode.cap(), 0))?;
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
        self.clear_physical_navigation_lease();
        self.cancel_patrol_state("estop", "patrol movement cancelled by estop");
        self.cancel_planner_state("estop", "planner movement cancelled by estop");
        let mut command = self.command.lock();
        self.write_stop_with_evidence(ActionCommandEvidence::stopped(
            command.speed_mode.cap(),
            ACTION_SAFETY_ESTOP,
        ))?;
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

        if self.config.profile.is_physical() {
            #[cfg(feature = "physical-navigation")]
            self.physical_planner_step(goal, path);
            #[cfg(not(feature = "physical-navigation"))]
            self.fail_physical_navigation(
                "gate-disabled",
                "physical planner stopped because the compile-time gate is disabled",
            );
            return;
        }

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

    #[cfg(feature = "physical-navigation")]
    fn physical_planner_step(&self, goal: PlannerGoal, path: VisualizationPath) {
        let current_lease = { self.physical_navigation_lease.lock().clone() };
        let lease = match current_lease {
            Some(lease) if lease.approval => lease,
            Some(_) => {
                self.fail_physical_navigation(
                    "approval-lost",
                    "physical navigation stopped because approval was revoked",
                );
                return;
            }
            None => {
                self.fail_physical_navigation(
                    "authorization-lost",
                    "physical navigation stopped because its authorization lease is missing",
                );
                return;
            }
        };
        if let Err(error) = self.validate_required_session(&lease.token) {
            self.fail_physical_navigation(
                "token-expired",
                &format!("physical navigation stopped: {error}"),
            );
            return;
        }
        let snapshot = match self.physical_navigation_snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.fail_physical_navigation(
                    "safety-stop",
                    &format!("physical navigation stopped: {error}"),
                );
                return;
            }
        };
        let current = snapshot
            .localization
            .pose
            .as_ref()
            .expect("readiness requires a pose")
            .pose
            .clone();
        if distance2d(current.x_m, current.y_m, goal.x_m, goal.y_m) <= goal.tolerance_m {
            self.cancel_planner_state("reached", "physical planner goal reached");
            let _ = self.stop_without_planner_cancel();
            return;
        }
        {
            let mut lease = self.physical_navigation_lease.lock();
            let Some(lease) = lease.as_mut() else {
                return;
            };
            if lease.last_command_at.is_some_and(|last| {
                last.elapsed() < Duration::from_millis(PHYSICAL_NAV_COMMAND_INTERVAL_MS)
            }) {
                return;
            }
            lease.last_command_at = Some(Instant::now());
        }
        let next = path
            .poses
            .iter()
            .find(|pose| distance2d(current.x_m, current.y_m, pose.x_m, pose.y_m) > 0.05)
            .cloned()
            .unwrap_or_else(|| Pose2d {
                ts_ms: now_ms(),
                frame_id: goal.frame_id.clone(),
                x_m: goal.x_m,
                y_m: goal.y_m,
                yaw_rad: 0.0,
            });
        let (left, right) = planner_drive_command(&current, &next);
        match self.drive(Some(&lease.token), left, right, Some(SpeedMode::Low)) {
            Ok(outcome) if outcome.soft_odometry_limited => {
                self.fail_physical_navigation(
                    "limited",
                    "physical navigation stopped by soft odometry limit",
                );
            }
            Ok(outcome) => {
                let mut planner = self.planner.lock();
                planner.last_drive = Some(outcome);
                planner.ok = true;
                planner.active = true;
                planner.status = "active".to_string();
                planner.message = "physical planner driving through safety gate".to_string();
            }
            Err(error) => {
                self.fail_physical_navigation(
                    "safety-stop",
                    &format!("physical navigation drive stopped: {error}"),
                );
            }
        }
    }

    fn fail_physical_navigation(&self, status: &str, message: &str) {
        let stop_result = {
            let mut command = self.command.lock();
            self.write_stop_with_evidence(ActionCommandEvidence::stopped(
                command.speed_mode.cap(),
                0,
            ))
            .map(|()| {
                command.left_cmd = 0.0;
                command.right_cmd = 0.0;
                command.last_cmd_at = Some(Instant::now());
            })
        };
        if let Err(error) = stop_result {
            warn!(?error, "physical navigation stop failed");
        }
        self.clear_physical_navigation_lease();
        self.cancel_planner_state(status, message);
        self.cancel_patrol_state(status, message);
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
        if self.config.profile.is_physical() {
            #[cfg(feature = "physical-navigation")]
            self.physical_patrol_step();
            return;
        }
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

    #[cfg(feature = "physical-navigation")]
    fn physical_patrol_step(&self) {
        if !self.patrol.lock().status.active {
            return;
        }
        let planner = self.planner_status();
        {
            let mut patrol = self.patrol.lock();
            patrol.status.path = planner.path.clone();
            patrol.status.goal = planner.goal.clone();
            if planner.active {
                return;
            }
            if planner.status != "reached" {
                patrol.status.ok = false;
                patrol.status.active = false;
                patrol.status.status = planner.status;
                patrol.status.message =
                    "physical patrol stopped by planner safety state".to_string();
                return;
            }
        }
        let current_lease = { self.physical_navigation_lease.lock().clone() };
        let Some(lease) = current_lease else {
            self.fail_physical_navigation(
                "authorization-lost",
                "physical patrol authorization lease is missing",
            );
            return;
        };
        let strategy = self.patrol.lock().status.strategy.unwrap_or_default();
        let snapshot = match self.physical_navigation_snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.fail_physical_navigation(
                    "safety-stop",
                    &format!("physical patrol stopped: {error}"),
                );
                return;
            }
        };
        let goal = match physical_patrol_goal(strategy, &snapshot) {
            Ok(goal) => goal,
            Err(error) => {
                self.fail_physical_navigation(
                    "no-goal",
                    &format!("physical patrol stopped: {error}"),
                );
                return;
            }
        };
        match self.set_physical_planner_goal(goal.clone(), Some(&lease.token), lease.approval) {
            Ok(planner) => {
                let mut patrol = self.patrol.lock();
                patrol.status.goal = Some(goal);
                patrol.status.path = planner.path;
                patrol.status.status = planner.status;
                patrol.status.message = "physical patrol selected next safe goal".to_string();
            }
            Err(error) => self.fail_physical_navigation(
                "safety-stop",
                &format!("physical patrol stopped: {error}"),
            ),
        }
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
        self.action_evidence.lock().transition(
            now_ns(),
            ActionCommandEvidence::stopped(command.speed_mode.cap(), 0),
        );
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

    #[cfg(feature = "physical-navigation")]
    fn validate_required_session(&self, token: &str) -> Result<PilotSession> {
        let mut sessions = self.sessions.lock();
        let Some(session) = sessions.get(token).cloned() else {
            return Err(anyhow!("invalid pilot token"));
        };
        if Instant::now() > session.expires_at {
            sessions.remove(token);
            return Err(anyhow!("expired pilot token"));
        }
        Ok(session)
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
                    let mut command = harness.command.lock();
                    let stop_result =
                        harness.write_stop_with_evidence(ActionCommandEvidence::stopped(
                            command.speed_mode.cap(),
                            ACTION_SAFETY_DEADMAN,
                        ));
                    if stop_result.is_ok() {
                        command.left_cmd = 0.0;
                        command.right_cmd = 0.0;
                        command.stopped_by_deadman = true;
                    }
                    drop(command);
                    if let Err(err) = stop_result {
                        warn!(?err, "deadman stop failed");
                        continue;
                    }
                    harness.clear_physical_navigation_lease();
                    harness
                        .cancel_planner_state("deadman", "planner movement cancelled by deadman");
                    harness.cancel_patrol_state("deadman", "patrol movement cancelled by deadman");
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
        let Some(sensor_config) = self.waveshare_sensors.as_ref().cloned() else {
            return;
        };

        match self.driver.telemetry_reader() {
            Ok(Some(port)) => {
                let raw = self.raw.clone();
                let publish = Arc::new(move |update: BaseTelemetryUpdate| {
                    apply_waveshare_update(&raw, update);
                });
                let raw = self.raw.clone();
                let publish_status = Arc::new(move |status: ImuStatus| {
                    raw.write().imu = status;
                });
                let imu = sensor_config.imu.clone();
                std::thread::spawn(move || {
                    read_base_telemetry_loop(port, imu, publish, publish_status)
                });
            }
            Ok(None) => {}
            Err(err) => warn!(?err, "waveshare telemetry reader unavailable"),
        }

        if let Some(lidar) = sensor_config.lidar.clone() {
            let raw = self.raw.clone();
            let publish = Arc::new(move |status: RangeScanStatus| {
                raw.write().range_scan = status;
            });
            spawn_ld06_reader(lidar, publish);
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
                #[cfg(feature = "waveshare-ugv")]
                harness.enforce_obstacle_stop();
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
fn apply_waveshare_update(raw: &Arc<RwLock<RawTelemetry>>, update: BaseTelemetryUpdate) {
    let mut next = raw.write();
    if let Some(voltage) = update.battery_v {
        next.battery_v = Some(round3(voltage));
        next.battery_pct = battery_percent_from_voltage(voltage);
    }
    if let Some(left_m) = update.odometry_left_m {
        next.odometry_left = Some(round3(left_m));
    }
    if let Some(right_m) = update.odometry_right_m {
        next.odometry_right = Some(round3(right_m));
    }
    if let Some(imu) = update.imu {
        next.imu = imu;
    }
    next.last_raw_frame_ms = Some(now_ms());
    next.last_raw_payload = Some(update.raw);
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
        version: SENSOR_CONTRACT_VERSION.to_string(),
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
        range_scan: raw.range_scan.clone(),
        imu: raw.imu.clone(),
    }
}

fn simulated_localization_frame(ts_ms: u128, raw: &RawTelemetry) -> LocalizationFrame {
    let left_m = raw.odometry_left.unwrap_or_default();
    let right_m = raw.odometry_right.unwrap_or_default();
    let pose = Pose2d {
        ts_ms,
        frame_id: "map".to_string(),
        x_m: round3((left_m + right_m) / 2.0),
        y_m: 0.0,
        yaw_rad: round3((right_m - left_m) * 0.25),
    };
    LocalizationFrame {
        version: LOCALIZATION_FRAME_VERSION.to_string(),
        ts_ms,
        map: MapIdentity {
            map_id: "sim-local".to_string(),
            map_revision: "sim-grid-v1".to_string(),
            frame_id: "map".to_string(),
        },
        pose: Some(PoseWithCovariance2d {
            pose,
            covariance: vec![0.01, 0.0, 0.0, 0.0, 0.01, 0.0, 0.0, 0.0, 0.02],
        }),
        health: LocalizationHealth {
            status: LocalizationStatus::Tracking,
            last_update_ms: Some(ts_ms),
            message: "sim localization healthy".to_string(),
            error: None,
        },
    }
}

fn simulated_map_frames(ts_ms: u128) -> (MapMetadata, OccupancyGridFrame, CostmapFrame) {
    let origin = Pose2d {
        ts_ms,
        frame_id: "map".to_string(),
        x_m: -0.5,
        y_m: -0.5,
        yaw_rad: 0.0,
    };
    let map = MapMetadata {
        ts_ms,
        map_id: "sim-local".to_string(),
        frame_id: "map".to_string(),
        width: 4,
        height: 4,
        resolution_m: 0.25,
        origin: origin.clone(),
        cell_order: "row-major".to_string(),
    };
    let occupancy_grid = OccupancyGridFrame {
        ts_ms,
        frame_id: "map".to_string(),
        width: 4,
        height: 4,
        resolution_m: 0.25,
        origin: origin.clone(),
        metadata: map.clone(),
        cells: planner_occupancy_cells(),
    };
    let costmap = CostmapFrame {
        ts_ms,
        frame_id: "map".to_string(),
        width: 4,
        height: 4,
        resolution_m: 0.25,
        origin,
        metadata: map.clone(),
        costs: planner_costs(),
    };
    (map, occupancy_grid, costmap)
}

fn apply_localization_snapshot(
    telemetry: &mut TelemetryFrame,
    snapshot: crate::localization::LocalizationProviderSnapshot,
) {
    telemetry.localization = snapshot.localization;
    telemetry.localization_provider = snapshot.status;
    telemetry.map = snapshot.map;
    telemetry.occupancy_grid = snapshot.occupancy_grid;
    telemetry.costmap = snapshot.costmap;
    telemetry.path = snapshot.path;
    telemetry.voxel_grid = snapshot.voxel_grid;
}

fn projected_voxel_grid(grid: &OccupancyGridFrame, obstacle_height_m: f64) -> VoxelGridFrame {
    if grid.width == 0 || grid.height == 0 || grid.resolution_m <= 0.0 {
        return VoxelGridFrame::default();
    }
    let depth = (obstacle_height_m / grid.resolution_m).ceil().max(1.0) as u32;
    let voxels = grid
        .cells
        .iter()
        .enumerate()
        .filter(|(_, occupancy)| **occupancy > OCCUPANCY_FREE)
        .flat_map(|(index, occupancy)| {
            let x = index as u32 % grid.width;
            let y = index as u32 / grid.width;
            (0..depth).map(move |z| VoxelCell {
                x,
                y,
                z,
                occupancy: *occupancy,
            })
        })
        .collect();
    VoxelGridFrame {
        version: VOXEL_GRID_VERSION.to_string(),
        ts_ms: grid.ts_ms,
        frame_id: grid.frame_id.clone(),
        width: grid.width,
        height: grid.height,
        depth,
        resolution_m: grid.resolution_m,
        origin: grid.origin.clone(),
        origin_z_m: 0.0,
        source: "projected-occupancy".to_string(),
        observed_3d: false,
        voxels,
    }
}

#[cfg(feature = "cuda")]
fn cuda_projected_voxel_grid(
    grid: &OccupancyGridFrame,
    obstacle_height_m: f64,
) -> Result<VoxelGridFrame> {
    if grid.width == 0 || grid.height == 0 || grid.resolution_m <= 0.0 {
        return Ok(VoxelGridFrame::default());
    }
    let depth = (obstacle_height_m / grid.resolution_m).ceil().max(1.0) as u32;
    let projected = crate::cuda_voxel::project_occupancy(&grid.cells, depth)?;
    let voxels = projected
        .into_iter()
        .enumerate()
        .filter(|(_, occupancy)| *occupancy > 0)
        .map(|(index, occupancy)| {
            let cell_index = index / depth as usize;
            VoxelCell {
                x: cell_index as u32 % grid.width,
                y: cell_index as u32 / grid.width,
                z: index as u32 % depth,
                occupancy: occupancy as i8,
            }
        })
        .collect();
    Ok(VoxelGridFrame {
        version: VOXEL_GRID_VERSION.to_string(),
        ts_ms: grid.ts_ms,
        frame_id: grid.frame_id.clone(),
        width: grid.width,
        height: grid.height,
        depth,
        resolution_m: grid.resolution_m,
        origin: grid.origin.clone(),
        origin_z_m: 0.0,
        source: "cuda-projected-occupancy".to_string(),
        observed_3d: false,
        voxels,
    })
}

#[cfg(all(test, feature = "cuda"))]
#[test]
fn cuda_voxel_projection_matches_cpu_when_device_is_available() {
    if crate::cuda_voxel::probe().is_err() {
        return;
    }
    let (_, grid, _) = simulated_map_frames(42);
    let cpu = projected_voxel_grid(&grid, 0.25);
    let cuda = cuda_projected_voxel_grid(&grid, 0.25).unwrap();
    assert_eq!(cuda.voxels, cpu.voxels);
    assert_eq!(cuda.source, "cuda-projected-occupancy");
}
#[cfg(feature = "physical-navigation")]
fn physical_patrol_goal(
    strategy: PatrolStrategy,
    snapshot: &crate::localization::LocalizationProviderSnapshot,
) -> Result<PlannerGoal> {
    let current = snapshot
        .localization
        .pose
        .as_ref()
        .ok_or_else(|| anyhow!("physical patrol requires a localized pose"))?;
    let grid = &snapshot.occupancy_grid;
    if grid.width == 0 || grid.height == 0 || grid.resolution_m <= 0.0 {
        return Err(anyhow!(
            "physical patrol requires a non-empty occupancy grid"
        ));
    }
    let mut candidates = Vec::new();
    for row in 0..grid.height {
        for column in 0..grid.width {
            let index = row as usize * grid.width as usize + column as usize;
            if grid.cells.get(index) != Some(&OCCUPANCY_FREE)
                || snapshot
                    .costmap
                    .costs
                    .get(index)
                    .is_none_or(|cost| *cost >= COST_LETHAL)
            {
                continue;
            }
            let local_x = (column as f64 + 0.5) * grid.resolution_m;
            let local_y = (row as f64 + 0.5) * grid.resolution_m;
            let cos_yaw = grid.origin.yaw_rad.cos();
            let sin_yaw = grid.origin.yaw_rad.sin();
            let x_m = grid.origin.x_m + local_x * cos_yaw - local_y * sin_yaw;
            let y_m = grid.origin.y_m + local_x * sin_yaw + local_y * cos_yaw;
            if distance2d(current.pose.x_m, current.pose.y_m, x_m, y_m) > grid.resolution_m.max(0.1)
            {
                candidates.push((x_m, y_m));
            }
        }
    }
    let selected = match strategy {
        PatrolStrategy::Coverage => candidates.first(),
        PatrolStrategy::Frontier => candidates.last(),
        PatrolStrategy::Random => snapshot
            .status
            .sequence
            .and_then(|sequence| candidates.get(sequence as usize % candidates.len().max(1))),
    }
    .copied()
    .ok_or_else(|| anyhow!("physical patrol could not find a clear reachable grid cell"))?;
    Ok(PlannerGoal {
        frame_id: snapshot.localization.map.frame_id.clone(),
        x_m: selected.0,
        y_m: selected.1,
        tolerance_m: (grid.resolution_m * 0.5).max(0.05),
        speed_mode: SpeedMode::Low,
    })
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

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after unix epoch")
        .as_nanos()
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "waveshare-ugv")]
    type RecordedCommands = Arc<Mutex<Vec<(f64, f64)>>>;

    #[cfg(feature = "waveshare-ugv")]
    struct RecordingUgvDriver {
        commands: RecordedCommands,
    }

    #[cfg(feature = "waveshare-ugv")]
    impl RobotDriver for RecordingUgvDriver {}

    #[cfg(feature = "waveshare-ugv")]
    impl MobileBaseAdapter for RecordingUgvDriver {
        fn drive(&self, left: f64, right: f64) -> Result<()> {
            self.commands.lock().push((left, right));
            Ok(())
        }
    }

    #[cfg(feature = "waveshare-ugv")]
    impl GimbalAdapter for RecordingUgvDriver {}

    #[cfg(feature = "waveshare-ugv")]
    fn waveshare_sensor_harness() -> (Harness, RecordedCommands) {
        let commands = Arc::new(Mutex::new(Vec::new()));
        let driver = Arc::new(RecordingUgvDriver {
            commands: commands.clone(),
        });
        let mut harness = Harness::new_inner_with_driver(
            HarnessConfig {
                profile: Profile::WaveshareUgv,
                allow_physical_actuation: true,
                deadman_ms: 5_000,
                ..HarnessConfig::default()
            },
            None,
            Some(driver),
        )
        .unwrap();
        harness.waveshare_sensors = Some(Arc::new(
            WaveshareSensorConfig::from_lookup(|key| {
                (key == "LEASH_UGV_LIDAR_DEVICE").then(|| "/dev/test-lidar".to_string())
            })
            .unwrap(),
        ));
        (harness, commands)
    }

    #[cfg(feature = "waveshare-ugv")]
    #[tokio::test]
    async fn waveshare_collision_gate_stops_active_motion_and_blocks_restart() {
        let (harness, commands) = waveshare_sensor_harness();
        harness
            .update_range_scan_status(simulated_range_scan(now_ms()))
            .unwrap();
        harness.drive(None, 0.1, 0.1, Some(SpeedMode::Low)).unwrap();

        let mut blocked = simulated_range_scan(now_ms());
        blocked.sample.as_mut().unwrap().ranges_m[0] = Some(0.2);
        harness.update_range_scan_status(blocked).unwrap();
        harness.enforce_obstacle_stop();

        let command = harness.command.lock().clone();
        assert_eq!((command.left_cmd, command.right_cmd), (0.0, 0.0));
        assert_eq!(commands.lock().last(), Some(&(0.0, 0.0)));
        let error = harness
            .drive(None, 0.1, 0.1, Some(SpeedMode::Low))
            .unwrap_err()
            .to_string();
        assert!(error.contains("lidar collision threshold"));
        assert_eq!(commands.lock().last(), Some(&(0.0, 0.0)));
    }

    #[cfg(feature = "waveshare-ugv")]
    #[tokio::test]
    async fn waveshare_sensor_health_tracks_imu_lidar_disconnect_and_staleness() {
        let (harness, _) = waveshare_sensor_harness();
        let ts_ms = now_ms();
        {
            let mut raw = harness.raw.write();
            raw.imu = simulated_imu_sample(ts_ms);
            raw.range_scan = RangeScanStatus {
                status: crate::types::SensorDataStatus::Disconnected,
                source: "test-lidar".to_string(),
                error: Some("disconnected".to_string()),
                ..RangeScanStatus::default()
            };
        }
        assert!(!harness.health().ok);

        harness
            .update_range_scan_status(simulated_range_scan(ts_ms))
            .unwrap();
        assert!(harness.health().ok);

        let stale_ts = now_ms().saturating_sub(501);
        harness.raw.write().imu = simulated_imu_sample(stale_ts);
        assert!(!harness.health().ok);
        assert_eq!(
            harness.telemetry().sensors.imu.status,
            crate::types::SensorDataStatus::Stale
        );
    }

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

        harness.seal_action_evidence(now_ns());
        let page = harness.applied_action_evidence(0, 64).unwrap();
        assert!(page.entries.iter().any(|entry| entry.requested_left == 1.0
            && entry.requested_right == 1.0
            && entry.applied_left == SpeedMode::Low.cap()
            && entry.applied_right == SpeedMode::Low.cap()));
        assert!(page.entries.iter().any(|entry| entry.deadman_active
            && entry.safety_flags & ACTION_SAFETY_DEADMAN != 0
            && entry.applied_left == 0.0
            && entry.applied_right == 0.0));
    }

    #[tokio::test]
    async fn applied_action_heartbeats_continuously_cover_zero_motion() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();

        time::sleep(Duration::from_millis(260)).await;
        let page = harness.applied_action_evidence(0, 64).unwrap();

        assert!(page.entries.len() >= 2);
        assert!(page.entries.iter().all(|entry| {
            entry.schema_version == APPLIED_ACTION_SCHEMA_VERSION
                && entry.authority == "leash"
                && entry.producer_epoch == page.producer_epoch
                && entry.interval_end_ns > entry.interval_start_ns
                && entry.applied_left == 0.0
                && entry.applied_right == 0.0
                && entry.valid
        }));
        assert!(page.entries.windows(2).all(|pair| {
            pair[0].action_sequence + 1 == pair[1].action_sequence
                && pair[0].interval_end_ns == pair[1].interval_start_ns
        }));
    }

    #[test]
    fn applied_action_history_reports_overrun_instead_of_skipping() {
        let mut state = ActionEvidenceState::new(7, SpeedMode::Low.cap());
        let start = state.interval_start_ns;
        for offset in 1..=(ACTION_EVIDENCE_HISTORY_CAPACITY as u64 + 2) {
            state.seal(start + offset);
        }

        let error = state.page(1, 64).unwrap_err().to_string();
        assert!(error.contains("history overrun"));
        let page = state.page(0, 64).unwrap();
        assert_eq!(page.entries.len(), 64);
        assert_eq!(page.oldest_sequence, 3);
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

    #[cfg(feature = "physical-navigation")]
    fn physical_navigation_harness() -> Harness {
        let harness = Harness::new_with_test_driver(HarnessConfig {
            profile: Profile::WaveshareUgv,
            allow_untokened_drive: false,
            allow_physical_actuation: true,
            allow_physical_navigation: true,
            deadman_ms: 5_000,
            soft_odometry_limit_m: 1.0,
            policy_mode: crate::config::PolicyMode::RequireApproval,
            ..HarnessConfig::default()
        })
        .unwrap();
        let ts_ms = now_ms();
        let raw = RawTelemetry::sim();
        let localization = simulated_localization_frame(ts_ms, &raw);
        let (map, occupancy_grid, costmap) = simulated_map_frames(ts_ms);
        let voxel_grid = projected_voxel_grid(&occupancy_grid, 0.25);
        harness
            .localization_provider
            .apply_at(
                LocalizationProviderUpdate {
                    version: crate::localization::LOCALIZATION_PROVIDER_UPDATE_VERSION.to_string(),
                    sequence: 1,
                    localization,
                    map,
                    occupancy_grid,
                    costmap,
                    path: VisualizationPath::default(),
                    voxel_grid,
                },
                ts_ms,
            )
            .unwrap();
        harness
            .update_range_scan_status(simulated_range_scan(ts_ms))
            .unwrap();
        harness
            .authorize("nav-token".to_string(), 30, SpeedMode::High)
            .unwrap();
        harness
    }

    #[cfg(feature = "physical-navigation")]
    fn physical_goal() -> PlannerGoal {
        PlannerGoal {
            frame_id: "map".to_string(),
            x_m: 0.25,
            y_m: 0.0,
            tolerance_m: 0.05,
            speed_mode: SpeedMode::High,
        }
    }

    #[cfg(feature = "physical-navigation")]
    fn assert_physical_navigation_stopped(harness: &Harness, expected: &str) {
        harness.planner_step();
        let planner = harness.planner_status();
        let command = harness.command.lock();
        assert!(!planner.active, "planner remained active: {planner:?}");
        assert!(
            planner.message.contains(expected) || planner.status.contains(expected),
            "unexpected planner stop: {planner:?}"
        );
        assert_eq!(command.left_cmd, 0.0);
        assert_eq!(command.right_cmd, 0.0);
    }

    #[cfg(feature = "physical-navigation")]
    #[tokio::test]
    async fn physical_navigation_requires_runtime_gate_token_approval_and_fresh_inputs() {
        let gated = Harness::new_with_test_driver(HarnessConfig {
            profile: Profile::WaveshareUgv,
            allow_physical_actuation: true,
            allow_untokened_drive: false,
            ..HarnessConfig::default()
        })
        .unwrap();
        assert!(gated
            .set_planner_goal_authorized(physical_goal(), Some("token"), true)
            .unwrap_err()
            .to_string()
            .contains("allow-physical-navigation"));

        let harness = physical_navigation_harness();
        assert!(harness
            .set_planner_goal_authorized(physical_goal(), None, true)
            .unwrap_err()
            .to_string()
            .contains("pilot token"));
        assert!(harness
            .set_planner_goal_authorized(physical_goal(), Some("nav-token"), false)
            .unwrap_err()
            .to_string()
            .contains("approval=true"));

        harness
            .localization_provider
            .mark_disconnected("provider disconnected");
        assert!(harness
            .set_planner_goal_authorized(physical_goal(), Some("nav-token"), true)
            .unwrap_err()
            .to_string()
            .contains("tracking localization"));

        let harness = physical_navigation_harness();
        let telemetry = harness.telemetry();
        harness
            .localization_provider
            .apply_at(
                LocalizationProviderUpdate::from_telemetry(2, &telemetry),
                now_ms().saturating_sub(DEFAULT_LOCALIZATION_STALE_AFTER_MS as u128 + 1),
            )
            .unwrap();
        assert!(harness
            .set_planner_goal_authorized(physical_goal(), Some("nav-token"), true)
            .unwrap_err()
            .to_string()
            .contains("tracking localization"));

        let harness = physical_navigation_harness();
        harness
            .update_range_scan_status(RangeScanStatus {
                status: crate::types::SensorDataStatus::Disconnected,
                source: "test".to_string(),
                error: Some("lidar disconnected".to_string()),
                ..RangeScanStatus::default()
            })
            .unwrap();
        assert!(harness
            .set_planner_goal_authorized(physical_goal(), Some("nav-token"), true)
            .unwrap_err()
            .to_string()
            .contains("available lidar"));

        let mut malformed = simulated_range_scan(now_ms());
        malformed.sample = None;
        assert!(harness
            .update_range_scan_status(malformed)
            .unwrap_err()
            .to_string()
            .contains("requires a sample"));

        let harness = physical_navigation_harness();
        let stale_ts = now_ms().saturating_sub(PHYSICAL_NAV_SENSOR_FRESH_MS + 1);
        harness
            .update_range_scan_status(simulated_range_scan(stale_ts))
            .unwrap();
        assert!(harness
            .set_planner_goal_authorized(physical_goal(), Some("nav-token"), true)
            .unwrap_err()
            .to_string()
            .contains("lidar is stale"));

        let harness = physical_navigation_harness();
        let mut blocked = simulated_range_scan(now_ms());
        blocked.sample.as_mut().unwrap().ranges_m[0] = Some(0.1);
        harness.update_range_scan_status(blocked).unwrap();
        assert!(harness
            .set_planner_goal_authorized(physical_goal(), Some("nav-token"), true)
            .unwrap_err()
            .to_string()
            .contains("blocked by lidar"));
    }

    #[cfg(feature = "physical-navigation")]
    #[tokio::test]
    async fn physical_navigation_caps_and_rate_limits_planner_commands() {
        let harness = physical_navigation_harness();
        let status = harness
            .set_planner_goal_authorized(physical_goal(), Some("nav-token"), true)
            .unwrap();
        let first = status.last_drive.unwrap();
        assert!(first.left.abs() <= SpeedMode::Low.cap());
        assert!(first.right.abs() <= SpeedMode::Low.cap());
        assert_eq!(first.speed_mode, SpeedMode::Low);
        let first_command_at = harness.command.lock().last_cmd_at;
        harness.planner_step();
        assert_eq!(harness.command.lock().last_cmd_at, first_command_at);
    }

    #[cfg(feature = "physical-navigation")]
    #[tokio::test]
    async fn physical_navigation_cancels_every_continuation_safety_path() {
        let harness = physical_navigation_harness();
        harness
            .set_planner_goal_authorized(physical_goal(), Some("nav-token"), true)
            .unwrap();
        harness.revoke_physical_navigation_approval();
        assert_physical_navigation_stopped(&harness, "approval was revoked");

        let harness = physical_navigation_harness();
        harness
            .set_planner_goal_authorized(physical_goal(), Some("nav-token"), true)
            .unwrap();
        harness
            .sessions
            .lock()
            .get_mut("nav-token")
            .unwrap()
            .expires_at = Instant::now() - Duration::from_millis(1);
        assert_physical_navigation_stopped(&harness, "expired pilot token");

        let harness = physical_navigation_harness();
        harness
            .set_planner_goal_authorized(physical_goal(), Some("nav-token"), true)
            .unwrap();
        harness.command.lock().stopped_by_deadman = true;
        assert_physical_navigation_stopped(&harness, "deadman stop");

        let harness = physical_navigation_harness();
        harness
            .set_planner_goal_authorized(physical_goal(), Some("nav-token"), true)
            .unwrap();
        harness.command.lock().soft_odometry_limited = true;
        assert_physical_navigation_stopped(&harness, "soft odometry limit");

        let harness = physical_navigation_harness();
        harness
            .set_planner_goal_authorized(physical_goal(), Some("nav-token"), true)
            .unwrap();
        harness
            .localization_provider
            .mark_disconnected("provider disconnected");
        assert_physical_navigation_stopped(&harness, "tracking localization");

        let harness = physical_navigation_harness();
        harness
            .set_planner_goal_authorized(physical_goal(), Some("nav-token"), true)
            .unwrap();
        let stale_ts = now_ms().saturating_sub(PHYSICAL_NAV_SENSOR_FRESH_MS + 1);
        harness
            .update_range_scan_status(simulated_range_scan(stale_ts))
            .unwrap();
        assert_physical_navigation_stopped(&harness, "lidar is stale");

        let harness = physical_navigation_harness();
        harness
            .set_planner_goal_authorized(physical_goal(), Some("nav-token"), true)
            .unwrap();
        harness.stop().unwrap();
        assert!(!harness.planner_status().active);
        assert!(harness.physical_navigation_lease.lock().is_none());

        let harness = physical_navigation_harness();
        harness
            .set_planner_goal_authorized(physical_goal(), Some("nav-token"), true)
            .unwrap();
        harness.estop().unwrap();
        assert!(!harness.planner_status().active);
        assert!(harness.command.lock().estop);
        assert!(harness.physical_navigation_lease.lock().is_none());
    }

    #[cfg(feature = "physical-navigation")]
    #[tokio::test]
    async fn physical_patrol_uses_provider_grid_and_map_scoped_waypoints() {
        let harness = physical_navigation_harness();
        let patrol = harness
            .start_patrol_authorized(
                PatrolStrategy::Coverage,
                SpeedMode::High,
                Some("nav-token"),
                true,
            )
            .unwrap();
        assert!(patrol.active);
        assert_eq!(patrol.speed_mode, SpeedMode::Low);

        harness.stop().unwrap();
        harness
            .create_waypoint(WaypointSpec {
                id: "dock".to_string(),
                name: "Dock".to_string(),
                frame_id: "map".to_string(),
                x_m: 0.25,
                y_m: 0.0,
                tolerance_m: 0.05,
            })
            .unwrap();
        harness
            .create_patrol_zone(PatrolZoneSpec {
                id: "dock-zone".to_string(),
                name: "Dock zone".to_string(),
                frame_id: "map".to_string(),
                waypoint_ids: vec!["dock".to_string()],
                boundary: Vec::new(),
            })
            .unwrap();
        let zone = harness
            .start_patrol_zone_authorized("dock-zone", SpeedMode::High, Some("nav-token"), true)
            .unwrap();
        assert!(zone.active);
        assert_eq!(zone.zone_id.as_deref(), Some("dock-zone"));
        assert_eq!(zone.speed_mode, SpeedMode::Low);

        harness.stop().unwrap();
        let telemetry = harness.telemetry();
        let mut replacement = LocalizationProviderUpdate::from_telemetry(2, &telemetry);
        replacement.localization.map.map_revision = "replacement-map".to_string();
        harness
            .localization_provider
            .apply_at(replacement, now_ms())
            .unwrap();
        harness
            .update_range_scan_status(simulated_range_scan(now_ms()))
            .unwrap();
        assert!(harness
            .start_patrol_zone_authorized("dock-zone", SpeedMode::Low, Some("nav-token"), true,)
            .unwrap_err()
            .to_string()
            .contains("map identity does not match"));
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
        assert_eq!(
            message.payload["telemetry"]["sensors"]["version"],
            SENSOR_CONTRACT_VERSION
        );
        assert_eq!(
            message.payload["telemetry"]["localization"]["health"]["status"],
            "tracking"
        );
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
        assert!(frame.visualization.path.poses.is_empty());
        assert_eq!(frame.visualization.occupancy_grid.cells.len(), 16);
        assert_eq!(
            frame.visualization.occupancy_grid.metadata,
            frame.visualization.map
        );
        assert_eq!(frame.visualization.costmap.costs.len(), 16);
        assert_eq!(frame.visualization.voxel_grid.source, "projected-occupancy");
        assert!(!frame.visualization.voxel_grid.observed_3d);
        assert_eq!(frame.visualization.voxel_grid.voxels.len(), 1);
        assert_eq!(
            frame.visualization.localization.health.status,
            LocalizationStatus::Tracking
        );
        assert_eq!(
            frame.visualization.localization.map.map_id,
            frame.visualization.map.map_id
        );
        assert_eq!(frame.telemetry.map, frame.visualization.map);
        assert_eq!(
            frame.telemetry.occupancy_grid,
            frame.visualization.occupancy_grid
        );
        assert_eq!(frame.telemetry.costmap, frame.visualization.costmap);
        assert_eq!(
            frame.visualization.range_scan,
            frame.telemetry.sensors.range_scan
        );
        assert_eq!(frame.visualization.imu, frame.telemetry.sensors.imu);
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
    async fn provider_map_scopes_memory_and_waypoints_without_issuing_motion() {
        let memory_path = std::env::temp_dir().join(format!(
            "leash-provider-scope-{}-{}.json",
            std::process::id(),
            now_ms()
        ));
        let navigation_path = navigation_path_for_memory(&memory_path);
        let harness =
            Harness::new_with_memory_path(HarnessConfig::default(), memory_path.clone()).unwrap();

        let memory = harness
            .tag_spatial_memory(SpatialMemoryTag {
                name: "dock".to_string(),
                kind: crate::types::SpatialMemoryKind::Location,
                frame_id: "map".to_string(),
                x_m: 0.25,
                y_m: 0.0,
                confidence: 0.9,
            })
            .unwrap();
        let waypoints = harness
            .create_waypoint(WaypointSpec {
                id: "dock".to_string(),
                name: "Dock".to_string(),
                frame_id: "map".to_string(),
                x_m: 0.25,
                y_m: 0.0,
                tolerance_m: 0.1,
            })
            .unwrap();

        let memory_map = memory.entries[0].map.as_ref().unwrap();
        let waypoint_map = waypoints.waypoints[0].map.as_ref().unwrap();
        assert_eq!(memory_map, waypoint_map);
        assert_eq!(memory_map.map_id, "sim-local");
        assert_eq!(memory_map.map_revision, "sim-grid-v1");
        let telemetry = harness.telemetry();
        assert_eq!(telemetry.left_cmd, 0.0);
        assert_eq!(telemetry.right_cmd, 0.0);

        let _ = std::fs::remove_file(memory_path);
        let _ = std::fs::remove_file(navigation_path);
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
                assert!(harness.planner_status().last_drive.is_none());
                let command = harness.command.lock();
                assert_eq!(command.left_cmd, 0.0);
                assert_eq!(command.right_cmd, 0.0);
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
    async fn sim_and_legacy_replay_expose_backward_compatible_sensor_contracts() {
        let sim = Harness::new(HarnessConfig::default()).unwrap();
        let sim_sensors = sim.telemetry().sensors;
        assert_eq!(
            sim_sensors.range_scan.status,
            crate::types::SensorDataStatus::Available
        );
        assert_eq!(
            sim_sensors.imu.status,
            crate::types::SensorDataStatus::Available
        );
        sim_sensors.range_scan.validate().unwrap();
        sim_sensors.imu.validate().unwrap();
        let sim_localization = sim.telemetry().localization;
        sim_localization.validate().unwrap();
        assert_eq!(sim_localization.health.status, LocalizationStatus::Tracking);

        let replay = Harness::new(HarnessConfig {
            profile: Profile::Replay,
            replay_source: Some(std::path::PathBuf::from("examples/replay/sim-basic.jsonl")),
            ..HarnessConfig::default()
        })
        .unwrap();
        let replay_sensors = replay.telemetry().sensors;
        assert_eq!(
            replay_sensors.range_scan.status,
            crate::types::SensorDataStatus::Unavailable
        );
        assert_eq!(
            replay_sensors.imu.status,
            crate::types::SensorDataStatus::Unavailable
        );
        assert_eq!(
            replay.telemetry().localization.health.status,
            LocalizationStatus::Unavailable
        );
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
