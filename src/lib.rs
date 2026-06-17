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
pub mod runtime;
pub mod types;

pub use accelerator::AcceleratorStatus;
pub use capability::{CapabilityDescriptor, CapabilityRegistry, SafetyClass};
pub use config::{AcceleratorBackend, HarnessConfig, Profile};
pub use daemon::{RunRecord, RunRegistry};
pub use module::{ModuleCoordinator, ModuleGraph, ModuleInfo, ModuleState};
pub use runtime::Harness;
pub use types::{Capabilities, CaptureResult, Health, SpeedMode, TelemetryFrame};
