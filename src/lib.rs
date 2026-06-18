//! Reusable harness runtime for local LLM tools, simulations, and robot adapters.
//!
//! The crate defaults to a simulation-safe runtime. Physical adapters are opt-in
//! features and also require an explicit runtime gate before any motor command is
//! allowed to reach hardware.

pub mod accelerator;
pub mod capability;
pub mod config;
pub mod daemon;
#[cfg(feature = "http")]
pub mod http;
#[cfg(feature = "mcp")]
pub mod mcp;
pub mod module;
pub mod replay;
pub mod runtime;
pub mod stack;
pub mod stream_processing;
pub mod transport;
pub mod types;

pub use accelerator::{AcceleratorProbe, AcceleratorProvider, AcceleratorStatus};
pub use capability::{CapabilityDescriptor, CapabilityRegistry, SafetyClass};
pub use config::{AcceleratorBackend, HarnessConfig, Profile};
pub use daemon::{RunRecord, RunRegistry};
pub use module::{ModuleCoordinator, ModuleGraph, ModuleInfo, ModuleState};
pub use replay::{
    scaled_delay, validate_replay_speed, ReplayEvent, ReplayEventKind, ReplayPlayback,
    ReplayRecording, REPLAY_FORMAT_VERSION,
};
pub use runtime::Harness;
pub use stack::{Stack, StackModule, StackTransport, TransportBinding};
pub use stream_processing::{
    pair_by_timestamp, select_best_frame, FrameQuality, LatestValue, QualityDecision,
    QualityFilter, RateLimiter, TimestampPair, Timestamped,
};
pub use transport::{
    new_stream_transport, StreamMessage, StreamRecvError, StreamSubscriber, StreamTransport,
    StreamTransportBackend,
};
pub use types::{
    Capabilities, CaptureResult, Health, SpeedMode, TelemetryFrame, TelemetryStreamFrame,
};
