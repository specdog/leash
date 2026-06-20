//! Reusable harness runtime for local LLM tools, simulations, and robot adapters.
//!
//! The crate defaults to a simulation-safe runtime. Physical adapters are opt-in
//! features and also require an explicit runtime gate before any motor command is
//! allowed to reach hardware.

pub mod accelerator;
pub mod agent;
pub mod capability;
pub mod config;
pub mod daemon;
#[cfg(feature = "http")]
pub mod http;
#[cfg(feature = "mcp")]
pub mod mcp;
pub mod memory;
pub mod module;
pub mod perception;
pub mod replay;
pub mod runtime;
pub mod stack;
pub mod stream_processing;
pub mod transport;
pub mod types;

pub use accelerator::{AcceleratorProbe, AcceleratorProvider, AcceleratorStatus};
pub use agent::complete as complete_agent_prompt;
pub use capability::{CapabilityDescriptor, CapabilityRegistry, SafetyClass};
pub use config::{AcceleratorBackend, AgentProvider, HarnessConfig, Profile};
pub use daemon::{RunRecord, RunRegistry};
pub use memory::{
    default_spatial_memory_path, SpatialMemoryQuery, SpatialMemoryStore, SpatialMemoryTag,
    SPATIAL_MEMORY_FORMAT, SPATIAL_MEMORY_STALE_AFTER_MS,
};
pub use module::{ModuleCoordinator, ModuleGraph, ModuleInfo, ModuleState};
pub use perception::{FakePerceptionAdapter, PerceptionAdapter, PerceptionRuntime};
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
    new_stream_transport, StreamMessage, StreamRecvError, StreamSubscriber, StreamTransport,
    StreamTransportBackend,
};
pub use types::{
    AgentModelResponse, AutonomyOverlay, Capabilities, CaptureResult, CommandOverlay, CostmapFrame,
    DetectionFrame, Health, ImageObservation, MapMetadata, OccupancyGridFrame, PatrolStatus,
    PatrolStrategy, PlannerGoal, PlannerStatus, PointCloudMetadata, Pose2d, ResourceSample,
    RunLogEntry, SpatialMemoryEntry, SpatialMemoryKind, SpatialMemoryStatus, SpeedMode,
    TelemetryFrame, TelemetryStreamFrame, Twist2d, VisionResult, VisualizationFrame,
    VisualizationPath, COST_FREE, COST_LETHAL, COST_UNKNOWN, OCCUPANCY_FREE, OCCUPANCY_OCCUPIED,
    OCCUPANCY_UNKNOWN, VISUALIZATION_FRAME_VERSION,
};
