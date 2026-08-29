//! Bounded orchestration primitives for Leash runtime v2.

#![forbid(unsafe_code)]

mod evidence;
mod lane;
mod latest;
mod safety;
mod supervisor;

pub use evidence::{
    read_evidence_records, AcknowledgementIdentity, AcknowledgementKind, EvidenceDecision,
    EvidenceEnqueueError, EvidenceJournal, EvidenceJournalConfig, EvidenceJournalStatus,
    EvidenceOpenError, EvidenceProducer, EvidenceRecord, EvidenceRecoveryState, EvidenceSource,
};
pub use lane::{
    bounded_lane, BoundedReceiver, BoundedSender, LaneCreateError, LaneSnapshot, OverflowPolicy,
    SendError, SendOutcome,
};
pub use latest::{latest_slot, LatestPublisher, LatestReader, LatestSnapshot, PublishError};
pub use safety::{
    safety_mailbox, SafetyKind, SafetyReceiveError, SafetyReceiver, SafetyRequestError,
    SafetySender, SafetySignal,
};
pub use supervisor::{
    ActuationAcknowledgement, ActuationAcknowledgementOutcome, ActuationPort, CpuSafetySupervisor,
    SafetyAcknowledgement, SupervisorConfig, SupervisorEvent, SupervisorHandle, SupervisorMetrics,
    SupervisorStartError, SupervisorStatus, SupervisorSubmitError, TransitionReceipt,
    TransitionTicket,
};
