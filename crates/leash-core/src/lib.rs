//! Deterministic domain contracts for Leash.
//!
//! This crate intentionally contains no async runtime, wire format, middleware,
//! hardware, filesystem, or accelerator dependency. Orchestration supplies
//! owned inputs and explicit monotonic time; the core returns owned effects.

#![forbid(unsafe_code)]

mod control;
mod drive;
mod error;
mod frame;
mod kernel;
mod time;
mod units;

pub use control::{
    ActuatorSink, Clock, ComputeBackend, ComputeCompletion, ComputeRequest, Controller, Effects,
    SensorSource, Tick,
};
pub use drive::{
    Authorized, Candidate, CommandId, DifferentialDrive, EvidenceId, NormalizedDrive, SafetyDenial,
    SafetyGate, SafetyState,
};
pub use error::DomainError;
pub use frame::{Base, Frame, FrameName, Map, Odom, Pose2, Sensor};
pub use kernel::{
    ActuationReason, ControlDenial, ControlEffect, ControlInput, ControlKernel,
    ControlKernelConfig, KernelError, OperatorId, StopReason,
};
pub use time::{DurationNanos, MonotonicNanos, ProducerEpoch, Sequence, Stamped};
pub use units::{Meters, MetersPerSecond, Radians, RadiansPerSecond};
