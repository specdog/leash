//! Bounded orchestration primitives for Leash runtime v2.

#![forbid(unsafe_code)]

mod lane;
mod latest;
mod safety;

pub use lane::{
    bounded_lane, BoundedReceiver, BoundedSender, LaneCreateError, LaneSnapshot, OverflowPolicy,
    SendError, SendOutcome,
};
pub use latest::{latest_slot, LatestPublisher, LatestReader, LatestSnapshot, PublishError};
pub use safety::{
    safety_mailbox, SafetyKind, SafetyReceiveError, SafetyReceiver, SafetyRequestError,
    SafetySender, SafetySignal,
};
