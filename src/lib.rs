//! Reusable harness runtime for local LLM tools, simulations, and robot adapters.
//!
//! The crate defaults to a simulation-safe runtime. Physical adapters are opt-in
//! features and also require an explicit runtime gate before any motor command is
//! allowed to reach hardware.

pub mod accelerator;
pub mod adapter;
pub mod agent;
pub mod agent_runtime;
pub mod capability;
pub mod config;
#[cfg(feature = "cuda")]
mod cuda_voxel;
pub mod daemon;
#[cfg(feature = "http")]
pub mod http;
pub mod localization;
#[cfg(feature = "mcp")]
pub mod mcp;
#[cfg(feature = "mcp")]
pub mod mcp_bridge;
pub mod memory;
pub mod module;
pub mod navigation;
pub mod operator_session;
pub mod perception;
pub mod replay;
pub mod runtime;
pub mod stack;
pub mod stream_processing;
pub mod transport;
pub mod types;
#[cfg(all(feature = "v4l2-camera", target_os = "linux"))]
pub mod v4l2_camera;
#[cfg(feature = "waveshare-ugv")]
#[path = "../implementations/waveshare-ugv/adapter.rs"]
mod waveshare_ugv;
#[cfg(feature = "webrtc")]
pub mod webrtc_camera;
pub mod worker;

pub use accelerator::{AcceleratorProbe, AcceleratorProvider, AcceleratorStatus};
pub use adapter::{
    simulated_imu_sample, simulated_range_scan, CameraAdapter, CameraCommandPlan,
    CameraInputConfig, CameraStreamCodec, FfmpegV4l2CameraAdapter, GimbalAdapter, ImuAdapter,
    MobileBaseAdapter, RangeScanAdapter,
};
pub use agent::complete as complete_agent_prompt;
pub use agent_runtime::{
    AgentConsoleCapability, AgentConsoleHealth, AgentRunOutput, AgentRuntime, AgentRuntimeSnapshot,
    AgentSession, AgentSessionStore, AgentSessionSummary, AgentTaskRecord, AgentTaskSnapshot,
    AgentTaskState, AgentTaskStopOutput, AgentTaskStore, AgentTurn, CapabilityPermissions,
    AGENT_SESSION_FORMAT, AGENT_TASK_FORMAT,
};
pub use capability::{CapabilityDescriptor, CapabilityRegistry, SafetyClass};
pub use config::{AcceleratorBackend, AgentProvider, HarnessConfig, Profile};
pub use daemon::{RunRecord, RunRegistry};
pub use localization::{
    ExternalLocalizationProvider, InProcessLocalizationProvider, LocalizationApplyOutcome,
    LocalizationProvider, LocalizationProviderError, LocalizationProviderSnapshot,
    LocalizationProviderState, LocalizationProviderStatus, LocalizationProviderUpdate,
    ReplayLocalizationProvider, SimulationLocalizationProvider,
    DEFAULT_LOCALIZATION_STALE_AFTER_MS, LOCALIZATION_PROVIDER_UPDATE_VERSION,
};
pub use memory::{
    default_spatial_memory_path, SpatialMemoryQuery, SpatialMemoryStore, SpatialMemoryTag,
    SPATIAL_MEMORY_FORMAT, SPATIAL_MEMORY_STALE_AFTER_MS,
};
pub use module::{ModuleCoordinator, ModuleGraph, ModuleInfo, ModuleState, StackBlueprintMetadata};
pub use navigation::{
    default_navigation_path, NavigationStore, PatrolZoneSpec, WaypointSpec, NAVIGATION_FORMAT,
};
pub use operator_session::{validate_operator_session, OPERATOR_SESSION_FORMAT};
pub use perception::{
    FakePerceptionAdapter, PerceptionAdapter, PerceptionRuntime, SimulatedPerceptionWorker,
};
pub use replay::{
    scaled_delay, validate_replay_speed, ReplayEvent, ReplayEventKind, ReplayPlayback,
    ReplayRecording, REPLAY_FORMAT_VERSION,
};
pub use runtime::Harness;
pub use stack::{
    adapter_profile_for_profile, AdapterCategory, AdapterMaturity, AdapterProfile, Stack,
    StackModule, StackTransport, TransportBinding,
};
pub use stream_processing::{
    pair_by_timestamp, select_best_frame, FrameQuality, LatestValue, QualityDecision,
    QualityFilter, RateLimiter, TimestampPair, Timestamped,
};
pub use transport::{
    accept_tcp_jsonl_stream_message, new_stream_transport, read_network_stream_frame,
    read_network_stream_message, send_tcp_jsonl_stream_message, spawn_tcp_jsonl_stream_hub,
    write_network_stream_frame, write_network_stream_message, NetworkStreamFrame, StreamMessage,
    StreamRecvError, StreamSubscriber, StreamTransport, StreamTransportBackend, TcpJsonlStreamHub,
    TcpJsonlStreamHubStatus, NETWORK_STREAM_FRAME_VERSION,
};
pub use types::{
    AgentModelResponse, AutonomyOverlay, CameraRecoveryResponse, CameraStreamFailure,
    CameraStreamHealth, Capabilities, CaptureResult, CommandOverlay, CostmapFrame, DetectionFrame,
    Health, ImageObservation, ImuSample, ImuStatus, LocalizationContractError, LocalizationFrame,
    LocalizationHealth, LocalizationStatus, MapIdentity, MapMetadata, MotionEvent, MotionEventKind,
    OccupancyGridFrame, OperatorSessionEvent, OperatorSessionEventKind, OperatorSessionRecording,
    OperatorSessionRobot, PatrolStatus, PatrolStrategy, PatrolZone, PatrolZoneList,
    PlanarRangeScan, PlannerGoal, PlannerStatus, PointCloudMetadata, Pose2d, PoseWithCovariance2d,
    Quaternion, RangeScanStatus, ResourceSample, RunLogEntry, SavedWaypoint, SavedWaypointList,
    SensorContractError, SensorDataStatus, SpatialMemoryEntry, SpatialMemoryKind,
    SpatialMemoryStatus, SpeedMode, TelemetryContractError, TelemetryFrame, TelemetryStreamFrame,
    Twist2d, Vector3Si, VisionResult, VisualizationFrame, VisualizationPath, ZoneBoundaryPoint,
    COST_FREE, COST_LETHAL, COST_UNKNOWN, LOCALIZATION_FRAME_VERSION,
    MAX_IMU_ANGULAR_VELOCITY_RADPS, MAX_IMU_LINEAR_ACCELERATION_MPS2, OCCUPANCY_FREE,
    OCCUPANCY_OCCUPIED, OCCUPANCY_UNKNOWN, SENSOR_CONTRACT_VERSION, VISUALIZATION_FRAME_VERSION,
};
pub use worker::{
    simulated_perception_worker_status, ExternalWorkerSpec, ExternalWorkerState,
    ExternalWorkerStatus, WorkerHealthCheck, WorkerInputFrame, WorkerInputPayload,
    WorkerOutputFrame, WorkerOutputPayload, WorkerRestartPolicy, WorkerSupervisor,
    WORKER_FRAME_VERSION,
};
