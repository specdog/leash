//! Deterministic domain contracts for Leash.
//!
//! This crate intentionally contains no async runtime, wire format, middleware,
//! hardware, filesystem, or accelerator dependency. Orchestration supplies
//! owned inputs and explicit monotonic time; the core returns owned effects.

#![forbid(unsafe_code)]

mod activity;
mod control;
mod drive;
mod error;
mod frame;
mod kernel;
mod time;
mod units;

pub use activity::{
    resolve_competing, Activity, ActivityEvent, ActivityFailure, ActivityId, ActivityKind,
    ActivityState, ActivityTransitionError, Arbitration, Belief, BeliefError, BeliefId,
    BeliefSource, ComputeIntent, Effect, Intent, Lineage, LineageError, Observation, Outcome,
    Precision, Proposal, ProposalError, ProposalId, ProposalRejection, ACTIVITY_SCHEMA_VERSION,
    BELIEF_SCHEMA_VERSION, PROPOSAL_SCHEMA_VERSION,
};
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
