use std::{
    collections::VecDeque,
    fmt,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver},
        Arc, Mutex, MutexGuard, OnceLock,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use leash_core::{
    Authorized, Clock, CommandId, ControlEffect, ControlInput, ControlKernel, Controller,
    DifferentialDrive, EvidenceId, MonotonicNanos, Sequence, Stamped, StopReason, Tick,
};

use crate::{
    bounded_lane, latest_slot, safety_mailbox, AcknowledgementIdentity, BoundedReceiver,
    BoundedSender, EvidenceDecision, EvidenceEnqueueError, EvidenceJournalStatus, EvidenceProducer,
    EvidenceRecord, EvidenceSource, LatestPublisher, LatestReader, OverflowPolicy, SafetyKind,
    SafetyReceiver, SafetyRequestError, SafetySender, SendError,
};

const MAX_PENDING_SAFETY_EVIDENCE: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActuationAcknowledgementOutcome {
    Applied,
    Superseded,
    Failed,
}

pub trait ActuationAcknowledgement: Send + 'static {
    fn applied(&self) -> bool;
    fn verified_zero(&self) -> bool;

    fn outcome(&self) -> ActuationAcknowledgementOutcome {
        if self.applied() {
            ActuationAcknowledgementOutcome::Applied
        } else {
            ActuationAcknowledgementOutcome::Failed
        }
    }

    fn command_id(&self) -> Option<CommandId> {
        None
    }

    fn evidence_id(&self) -> Option<EvidenceId> {
        None
    }

    fn applied_sequence(&self) -> Option<u64> {
        None
    }

    fn acknowledged_at(&self) -> Option<MonotonicNanos> {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SafetyAcknowledgement {
    pub kind: SafetyKind,
    pub first_request_sequence: u64,
    pub through_request_sequence: u64,
    pub applied_sequence: Option<u64>,
    pub at: MonotonicNanos,
    pub verified_zero: bool,
}

pub trait ActuationPort: Send + 'static {
    type Acknowledgement: ActuationAcknowledgement;
    type Error: fmt::Display + Send + 'static;

    fn submit_drive(&mut self, command: Authorized<DifferentialDrive>) -> Result<(), Self::Error>;

    fn request_safety(&mut self, kind: SafetyKind) -> Result<u64, Self::Error>;

    fn try_acknowledgement(&mut self) -> Result<Option<Self::Acknowledgement>, Self::Error>;

    fn try_safety_acknowledgement(&mut self) -> Result<Option<SafetyAcknowledgement>, Self::Error> {
        Ok(None)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupervisorConfig {
    pub proposal_capacity: usize,
    pub tick_period: Duration,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            proposal_capacity: 32,
            tick_period: Duration::from_millis(10),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupervisorStartError {
    ZeroProposalCapacity,
    ZeroTickPeriod,
    Thread(String),
}

impl fmt::Display for SupervisorStartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroProposalCapacity => {
                formatter.write_str("safety proposal capacity must be positive")
            }
            Self::ZeroTickPeriod => formatter.write_str("safety tick period must be positive"),
            Self::Thread(error) => write!(formatter, "start CPU safety supervisor: {error}"),
        }
    }
}

impl std::error::Error for SupervisorStartError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisorSubmitError {
    Full,
    Closed,
    Faulted,
    SafetyUsesPriorityPath,
    SequenceExhausted,
}

impl fmt::Display for SupervisorSubmitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full => formatter.write_str("safety proposal lane is full"),
            Self::Closed => formatter.write_str("CPU safety supervisor is closed"),
            Self::Faulted => formatter.write_str("CPU safety supervisor is faulted"),
            Self::SafetyUsesPriorityPath => {
                formatter.write_str("stop and e-stop must use the priority safety API")
            }
            Self::SequenceExhausted => formatter.write_str("proposal sequence exhausted"),
        }
    }
}

impl std::error::Error for SupervisorSubmitError {}

#[derive(Debug, Clone, PartialEq)]
pub struct TransitionReceipt {
    pub proposal_sequence: u64,
    pub processed_at: MonotonicNanos,
    pub effects: Vec<ControlEffect>,
}

pub struct TransitionTicket {
    receiver: Receiver<Result<TransitionReceipt, Box<str>>>,
}

impl TransitionTicket {
    pub fn wait(self) -> Result<TransitionReceipt, Box<str>> {
        self.receiver
            .recv()
            .unwrap_or_else(|_| Err("CPU safety supervisor stopped".into()))
    }

    pub fn wait_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Option<Result<TransitionReceipt, Box<str>>>, SupervisorSubmitError> {
        match self.receiver.recv_timeout(timeout) {
            Ok(receipt) => Ok(Some(receipt)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(SupervisorSubmitError::Closed),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SupervisorEvent<A> {
    SafetyRequested {
        kind: SafetyKind,
        request_sequence: u64,
        command_id: CommandId,
        evidence_id: EvidenceId,
    },
    DriveSubmitted {
        command_id: CommandId,
        evidence_id: EvidenceId,
    },
    Acknowledged(A),
    Faulted(Box<str>),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SupervisorMetrics {
    pub proposals_accepted: u64,
    pub proposals_rejected: u64,
    pub transitions: u64,
    pub stop_requests: u64,
    pub estop_requests: u64,
    pub drives_submitted: u64,
    pub acknowledgements: u64,
    pub acknowledgement_failures: u64,
    pub faults: u64,
    pub worker_panics: u64,
    pub evidence_records: u64,
    pub evidence_failures: u64,
}

#[derive(Default)]
struct MetricAtoms {
    proposals_accepted: AtomicU64,
    proposals_rejected: AtomicU64,
    transitions: AtomicU64,
    stop_requests: AtomicU64,
    estop_requests: AtomicU64,
    drives_submitted: AtomicU64,
    acknowledgements: AtomicU64,
    acknowledgement_failures: AtomicU64,
    faults: AtomicU64,
    worker_panics: AtomicU64,
    evidence_records: AtomicU64,
    evidence_failures: AtomicU64,
}

impl MetricAtoms {
    fn snapshot(&self) -> SupervisorMetrics {
        SupervisorMetrics {
            proposals_accepted: self.proposals_accepted.load(Ordering::Relaxed),
            proposals_rejected: self.proposals_rejected.load(Ordering::Relaxed),
            transitions: self.transitions.load(Ordering::Relaxed),
            stop_requests: self.stop_requests.load(Ordering::Relaxed),
            estop_requests: self.estop_requests.load(Ordering::Relaxed),
            drives_submitted: self.drives_submitted.load(Ordering::Relaxed),
            acknowledgements: self.acknowledgements.load(Ordering::Relaxed),
            acknowledgement_failures: self.acknowledgement_failures.load(Ordering::Relaxed),
            faults: self.faults.load(Ordering::Relaxed),
            worker_panics: self.worker_panics.load(Ordering::Relaxed),
            evidence_records: self.evidence_records.load(Ordering::Relaxed),
            evidence_failures: self.evidence_failures.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorStatus {
    pub faulted: bool,
    pub closed: bool,
    pub last_fault: Option<Box<str>>,
    pub metrics: SupervisorMetrics,
    pub proposal_lane: crate::LaneSnapshot,
    pub evidence: Option<EvidenceJournalStatus>,
}

struct SharedState {
    shutdown: AtomicBool,
    faulted: AtomicBool,
    closed: AtomicBool,
    next_proposal: AtomicU64,
    last_fault: Mutex<Option<Box<str>>>,
    worker_thread: OnceLock<thread::Thread>,
    last_clock: AtomicU64,
    evidence_fault_latched: AtomicBool,
    metrics: MetricAtoms,
}

impl SharedState {
    fn wake(&self) {
        if let Some(worker) = self.worker_thread.get() {
            worker.unpark();
        }
    }
}

struct Proposal {
    sequence: u64,
    input: ControlInput,
    reply: mpsc::Sender<Result<TransitionReceipt, Box<str>>>,
}

#[derive(Debug, Clone, Copy)]
struct PendingSafetyEvidence {
    kind: SafetyKind,
    request_sequence: u64,
    proposal_sequence: Option<u64>,
    command_id: CommandId,
    evidence_id: EvidenceId,
}

#[derive(Clone)]
pub struct SupervisorHandle {
    proposals: BoundedSender<Proposal>,
    safety: SafetySender,
    evidence: Option<EvidenceProducer>,
    shared: Arc<SharedState>,
}

impl SupervisorHandle {
    pub fn submit(&self, input: ControlInput) -> Result<TransitionTicket, SupervisorSubmitError> {
        if matches!(input, ControlInput::Stop { .. } | ControlInput::EStop) {
            return Err(SupervisorSubmitError::SafetyUsesPriorityPath);
        }
        let sequence = self
            .shared
            .next_proposal
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(1)
            })
            .map_err(|_| SupervisorSubmitError::SequenceExhausted)?;
        if self.shared.faulted.load(Ordering::Acquire) {
            self.shared
                .metrics
                .proposals_rejected
                .fetch_add(1, Ordering::Relaxed);
            if let Err(error) = record_evidence(
                self.evidence.as_ref(),
                &self.shared,
                false,
                EvidenceRecord::new(
                    Some(sequence),
                    None,
                    None,
                    EvidenceSource::ProposalIngress,
                    MonotonicNanos::new(self.shared.last_clock.load(Ordering::Acquire)),
                    EvidenceDecision::ProposalRejected,
                    None,
                ),
            ) {
                evidence_failed(&self.shared, error);
            }
            return Err(SupervisorSubmitError::Faulted);
        }
        let (reply, receiver) = mpsc::channel();
        match self.proposals.try_send(Proposal {
            sequence,
            input,
            reply,
        }) {
            Ok(_) => {
                self.shared
                    .metrics
                    .proposals_accepted
                    .fetch_add(1, Ordering::Relaxed);
                if let Err(error) = record_evidence(
                    self.evidence.as_ref(),
                    &self.shared,
                    false,
                    EvidenceRecord::new(
                        Some(sequence),
                        None,
                        None,
                        EvidenceSource::ProposalIngress,
                        MonotonicNanos::new(self.shared.last_clock.load(Ordering::Acquire)),
                        EvidenceDecision::ProposalAccepted,
                        None,
                    ),
                ) {
                    evidence_failed(&self.shared, error);
                    let _ = self.safety.estop();
                    self.shared.wake();
                    return Err(SupervisorSubmitError::Faulted);
                }
                self.shared.wake();
                Ok(TransitionTicket { receiver })
            }
            Err(SendError::Full(_)) => {
                self.shared
                    .metrics
                    .proposals_rejected
                    .fetch_add(1, Ordering::Relaxed);
                if let Err(error) = record_evidence(
                    self.evidence.as_ref(),
                    &self.shared,
                    false,
                    EvidenceRecord::new(
                        Some(sequence),
                        None,
                        None,
                        EvidenceSource::ProposalIngress,
                        MonotonicNanos::new(self.shared.last_clock.load(Ordering::Acquire)),
                        EvidenceDecision::ProposalRejected,
                        None,
                    ),
                ) {
                    evidence_failed(&self.shared, error);
                    let _ = self.safety.estop();
                    self.shared.wake();
                }
                Err(SupervisorSubmitError::Full)
            }
            Err(SendError::Closed(_)) => {
                self.shared
                    .metrics
                    .proposals_rejected
                    .fetch_add(1, Ordering::Relaxed);
                if let Err(error) = record_evidence(
                    self.evidence.as_ref(),
                    &self.shared,
                    false,
                    EvidenceRecord::new(
                        Some(sequence),
                        None,
                        None,
                        EvidenceSource::ProposalIngress,
                        MonotonicNanos::new(self.shared.last_clock.load(Ordering::Acquire)),
                        EvidenceDecision::ProposalRejected,
                        None,
                    ),
                ) {
                    evidence_failed(&self.shared, error);
                }
                Err(SupervisorSubmitError::Closed)
            }
        }
    }

    pub fn stop(&self) -> Result<u64, SafetyRequestError> {
        let sequence = self.safety.stop()?;
        self.shared.wake();
        Ok(sequence)
    }

    pub fn estop(&self) -> Result<u64, SafetyRequestError> {
        let sequence = self.safety.estop()?;
        self.shared.wake();
        Ok(sequence)
    }

    pub fn status(&self) -> SupervisorStatus {
        SupervisorStatus {
            faulted: self.shared.faulted.load(Ordering::Acquire),
            closed: self.shared.closed.load(Ordering::Acquire),
            last_fault: lock(&self.shared.last_fault).clone(),
            metrics: self.shared.metrics.snapshot(),
            proposal_lane: self.proposals.snapshot(),
            evidence: self.evidence.as_ref().map(EvidenceProducer::status),
        }
    }
}

pub struct CpuSafetySupervisor<A> {
    handle: SupervisorHandle,
    events: Option<LatestReader<SupervisorEvent<A>>>,
    worker: Option<JoinHandle<()>>,
}

impl<A> CpuSafetySupervisor<A>
where
    A: ActuationAcknowledgement,
{
    pub fn spawn<P>(
        kernel: ControlKernel,
        port: P,
        clock: Box<dyn Clock + Send>,
        config: SupervisorConfig,
    ) -> Result<Self, SupervisorStartError>
    where
        P: ActuationPort<Acknowledgement = A>,
    {
        Self::spawn_inner(kernel, port, clock, config, None)
    }

    pub fn spawn_with_evidence<P>(
        kernel: ControlKernel,
        port: P,
        clock: Box<dyn Clock + Send>,
        config: SupervisorConfig,
        evidence: EvidenceProducer,
    ) -> Result<Self, SupervisorStartError>
    where
        P: ActuationPort<Acknowledgement = A>,
    {
        Self::spawn_inner(kernel, port, clock, config, Some(evidence))
    }

    fn spawn_inner<P>(
        kernel: ControlKernel,
        port: P,
        clock: Box<dyn Clock + Send>,
        config: SupervisorConfig,
        evidence: Option<EvidenceProducer>,
    ) -> Result<Self, SupervisorStartError>
    where
        P: ActuationPort<Acknowledgement = A>,
    {
        if config.proposal_capacity == 0 {
            return Err(SupervisorStartError::ZeroProposalCapacity);
        }
        if config.tick_period.is_zero() {
            return Err(SupervisorStartError::ZeroTickPeriod);
        }
        let (proposal_sender, proposal_receiver) =
            bounded_lane(config.proposal_capacity, OverflowPolicy::RejectNewest)
                .expect("validated non-zero proposal capacity");
        let (safety_sender, safety_receiver) = safety_mailbox();
        let (event_publisher, events) = latest_slot();
        let shared = Arc::new(SharedState {
            shutdown: AtomicBool::new(false),
            faulted: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            next_proposal: AtomicU64::new(1),
            last_fault: Mutex::new(None),
            worker_thread: OnceLock::new(),
            last_clock: AtomicU64::new(0),
            evidence_fault_latched: AtomicBool::new(false),
            metrics: MetricAtoms::default(),
        });
        let handle = SupervisorHandle {
            proposals: proposal_sender,
            safety: safety_sender,
            evidence: evidence.clone(),
            shared: Arc::clone(&shared),
        };
        let panic_shared = Arc::clone(&shared);
        let thread_shared = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("leash-cpu-safety".to_string())
            .spawn(move || {
                let _ = thread_shared.worker_thread.set(thread::current());
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_supervisor(
                        kernel,
                        port,
                        clock,
                        config,
                        proposal_receiver,
                        safety_receiver,
                        event_publisher,
                        evidence,
                        shared,
                    );
                }));
                if result.is_err() {
                    panic_shared.faulted.store(true, Ordering::Release);
                    panic_shared.closed.store(true, Ordering::Release);
                    panic_shared
                        .metrics
                        .worker_panics
                        .fetch_add(1, Ordering::Relaxed);
                    *lock(&panic_shared.last_fault) = Some("CPU safety worker panicked".into());
                }
            })
            .map_err(|error| SupervisorStartError::Thread(error.to_string()))?;
        Ok(Self {
            handle,
            events: Some(events),
            worker: Some(worker),
        })
    }

    pub fn handle(&self) -> SupervisorHandle {
        self.handle.clone()
    }

    pub fn take_event(&mut self) -> Option<Stamped<SupervisorEvent<A>>> {
        self.events.as_mut().and_then(LatestReader::take)
    }

    pub fn shutdown(mut self) {
        self.stop_and_join();
    }

    fn stop_and_join(&mut self) {
        self.handle.shared.shutdown.store(true, Ordering::Release);
        self.handle.shared.wake();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl<A> Drop for CpuSafetySupervisor<A> {
    fn drop(&mut self) {
        self.handle.shared.shutdown.store(true, Ordering::Release);
        self.handle.shared.wake();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_supervisor<P>(
    mut kernel: ControlKernel,
    mut port: P,
    mut clock: Box<dyn Clock + Send>,
    config: SupervisorConfig,
    mut proposals: BoundedReceiver<Proposal>,
    mut safety: SafetyReceiver,
    events: LatestPublisher<SupervisorEvent<P::Acknowledgement>>,
    evidence: Option<EvidenceProducer>,
    shared: Arc<SharedState>,
) where
    P: ActuationPort,
{
    let mut tick_sequence = Sequence::new(1).expect("one is non-zero");
    let mut event_sequence = Sequence::new(1).expect("one is non-zero");
    let mut internal_estop = false;
    let mut pending_safety = VecDeque::new();
    let mut next_periodic_tick = Instant::now() + config.tick_period;
    while !shared.shutdown.load(Ordering::Acquire) {
        if evidence
            .as_ref()
            .is_some_and(|producer| !producer.healthy())
            && !shared.evidence_fault_latched.load(Ordering::Acquire)
        {
            evidence_unhealthy(&shared);
            internal_estop = true;
        }
        let safety_signal = safety.try_recv().ok().flatten();
        let input = if internal_estop {
            internal_estop = false;
            Some((None, ControlInput::EStop))
        } else if let Some(signal) = safety_signal {
            let input = match signal.kind {
                SafetyKind::Stop => ControlInput::Stop {
                    reason: StopReason::Operator,
                },
                SafetyKind::EStop => ControlInput::EStop,
            };
            Some((None, input))
        } else if shared.evidence_fault_latched.load(Ordering::Acquire) {
            while let Some(proposal) = proposals.try_recv() {
                let _ = proposal
                    .reply
                    .send(Err("evidence persistence is unavailable".into()));
            }
            None
        } else {
            proposals
                .try_recv()
                .map(|proposal| (Some(proposal), ControlInput::Idle))
        };

        let (proposal, input) = match input {
            Some((Some(proposal), _)) => {
                let input = proposal.input.clone();
                (Some(proposal), input)
            }
            Some((None, input)) => (None, input),
            None => (None, ControlInput::Idle),
        };
        let at = clock.now();
        shared.last_clock.store(at.get(), Ordering::Release);
        let result = kernel.step(Tick::new(at, tick_sequence, input));
        tick_sequence = match tick_sequence.next() {
            Ok(sequence) => sequence,
            Err(error) => {
                set_fault(&shared, error.to_string());
                break;
            }
        };
        match result {
            Ok(effects) => {
                shared.metrics.transitions.fetch_add(1, Ordering::Relaxed);
                let owned_effects = effects.iter().cloned().collect::<Vec<_>>();
                let proposal_sequence = proposal.as_ref().map(|proposal| proposal.sequence);
                for effect in effects.iter() {
                    let decision = match effect {
                        ControlEffect::Denied { command_id, .. } => record_evidence(
                            evidence.as_ref(),
                            &shared,
                            false,
                            EvidenceRecord::new(
                                proposal_sequence,
                                Some(*command_id),
                                None,
                                EvidenceSource::CpuSafetySupervisor,
                                at,
                                EvidenceDecision::CommandRejected,
                                None,
                            ),
                        )
                        .map_err(|error| {
                            format!("persist command denial: {error}").into_boxed_str()
                        }),
                        ControlEffect::Actuate { command, .. } if command.command().is_stop() => {
                            let kind = if safety_signal
                                .is_some_and(|signal| signal.kind == SafetyKind::EStop)
                                || matches!(
                                    effect,
                                    ControlEffect::Actuate {
                                        reason: leash_core::ActuationReason::EStop,
                                        ..
                                    }
                                ) {
                                SafetyKind::EStop
                            } else {
                                SafetyKind::Stop
                            };
                            request_safety(
                                &mut port,
                                kind,
                                command,
                                proposal_sequence,
                                &mut pending_safety,
                                &events,
                                &mut event_sequence,
                                evidence.as_ref(),
                                at,
                                &shared,
                            )
                        }
                        ControlEffect::Actuate { command, .. } => submit_drive(
                            &mut port,
                            command,
                            proposal_sequence,
                            &events,
                            &mut event_sequence,
                            evidence.as_ref(),
                            at,
                            &shared,
                        ),
                        _ => Ok(()),
                    };
                    if let Err(error) = decision {
                        if let Some(source) = error.strip_prefix("persist ") {
                            evidence_unhealthy_with_message(&shared, source.to_string());
                        } else {
                            set_fault(&shared, error);
                        }
                        internal_estop = true;
                    }
                }
                if let Some(proposal) = proposal {
                    let _ = proposal.reply.send(Ok(TransitionReceipt {
                        proposal_sequence: proposal.sequence,
                        processed_at: at,
                        effects: owned_effects,
                    }));
                }
            }
            Err(error) => {
                let message = error.to_string().into_boxed_str();
                if let Some(proposal) = proposal {
                    let _ = proposal.reply.send(Err(message.clone()));
                }
                set_fault(&shared, message);
                internal_estop = true;
            }
        }

        loop {
            match port.try_acknowledgement() {
                Ok(Some(ack)) => {
                    shared
                        .metrics
                        .acknowledgements
                        .fetch_add(1, Ordering::Relaxed);
                    let applied = ack.applied();
                    let outcome = ack.outcome();
                    let verified_zero = ack.verified_zero();
                    let command_id = ack.command_id();
                    let evidence_id = ack.evidence_id();
                    let acknowledgement = AcknowledgementIdentity::command(
                        ack.applied_sequence(),
                        ack.acknowledged_at().unwrap_or(at),
                    );
                    if !applied {
                        shared
                            .metrics
                            .acknowledgement_failures
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    publish_event(
                        &events,
                        &mut event_sequence,
                        at,
                        SupervisorEvent::Acknowledged(ack),
                    );
                    let decision = if verified_zero && applied {
                        EvidenceDecision::ZeroVerified
                    } else {
                        match outcome {
                            ActuationAcknowledgementOutcome::Applied => {
                                EvidenceDecision::AcknowledgementApplied
                            }
                            ActuationAcknowledgementOutcome::Superseded => {
                                EvidenceDecision::CommandSuperseded
                            }
                            ActuationAcknowledgementOutcome::Failed => {
                                EvidenceDecision::AcknowledgementFailed
                            }
                        }
                    };
                    if let Err(error) = record_evidence(
                        evidence.as_ref(),
                        &shared,
                        verified_zero,
                        EvidenceRecord::new(
                            None,
                            command_id,
                            evidence_id,
                            EvidenceSource::Actuator,
                            acknowledgement.at,
                            decision,
                            Some(acknowledgement),
                        ),
                    ) {
                        evidence_failed(&shared, error);
                        if !verified_zero {
                            internal_estop = true;
                        }
                    }
                    if !applied {
                        set_fault(&shared, "actuator rejected or failed a command");
                        internal_estop = true;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    set_fault(&shared, format!("poll actuator acknowledgement: {error}"));
                    internal_estop = true;
                    break;
                }
            }
        }
        loop {
            match port.try_safety_acknowledgement() {
                Ok(Some(acknowledgement)) => {
                    let identity = AcknowledgementIdentity::safety(
                        acknowledgement.first_request_sequence,
                        acknowledgement.through_request_sequence,
                        acknowledgement.applied_sequence,
                        acknowledgement.at,
                    );
                    let mut matched = false;
                    let mut unmatched = VecDeque::with_capacity(pending_safety.len());
                    while let Some(pending) = pending_safety.pop_front() {
                        let covered = pending.kind == acknowledgement.kind
                            && pending.request_sequence >= acknowledgement.first_request_sequence
                            && pending.request_sequence <= acknowledgement.through_request_sequence;
                        if !covered {
                            unmatched.push_back(pending);
                            continue;
                        }
                        matched = true;
                        let decision = if acknowledgement.verified_zero {
                            EvidenceDecision::ZeroVerified
                        } else {
                            EvidenceDecision::AcknowledgementFailed
                        };
                        if let Err(error) = record_evidence(
                            evidence.as_ref(),
                            &shared,
                            true,
                            EvidenceRecord::new(
                                pending.proposal_sequence,
                                Some(pending.command_id),
                                Some(pending.evidence_id),
                                EvidenceSource::Actuator,
                                acknowledgement.at,
                                decision,
                                Some(identity),
                            ),
                        ) {
                            evidence_failed(&shared, error);
                        }
                    }
                    pending_safety = unmatched;
                    if !matched {
                        let decision = if acknowledgement.verified_zero {
                            EvidenceDecision::ZeroVerified
                        } else {
                            EvidenceDecision::AcknowledgementFailed
                        };
                        if let Err(error) = record_evidence(
                            evidence.as_ref(),
                            &shared,
                            true,
                            EvidenceRecord::new(
                                None,
                                None,
                                None,
                                EvidenceSource::Actuator,
                                acknowledgement.at,
                                decision,
                                Some(identity),
                            ),
                        ) {
                            evidence_failed(&shared, error);
                        }
                    }
                    if !acknowledgement.verified_zero {
                        set_fault(&shared, "safety acknowledgement did not verify zero");
                        internal_estop = true;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    set_fault(
                        &shared,
                        format!("poll actuator safety acknowledgement: {error}"),
                    );
                    internal_estop = true;
                    break;
                }
            }
        }
        let now = Instant::now();
        while next_periodic_tick <= now {
            next_periodic_tick += config.tick_period;
        }
        thread::park_timeout(next_periodic_tick.saturating_duration_since(now));
    }
    while let Some(proposal) = proposals.try_recv() {
        if let Err(error) = record_evidence(
            evidence.as_ref(),
            &shared,
            false,
            EvidenceRecord::new(
                Some(proposal.sequence),
                None,
                None,
                EvidenceSource::CpuSafetySupervisor,
                MonotonicNanos::new(shared.last_clock.load(Ordering::Acquire)),
                EvidenceDecision::ProposalRejected,
                None,
            ),
        ) {
            evidence_failed(&shared, error);
        }
        let _ = proposal
            .reply
            .send(Err("CPU safety supervisor stopped".into()));
    }
    shared.closed.store(true, Ordering::Release);
}

#[allow(clippy::too_many_arguments)]
fn request_safety<P: ActuationPort>(
    port: &mut P,
    kind: SafetyKind,
    command: &Authorized<DifferentialDrive>,
    proposal_sequence: Option<u64>,
    pending_safety: &mut VecDeque<PendingSafetyEvidence>,
    events: &LatestPublisher<SupervisorEvent<P::Acknowledgement>>,
    event_sequence: &mut Sequence,
    evidence: Option<&EvidenceProducer>,
    at: MonotonicNanos,
    shared: &SharedState,
) -> Result<(), Box<str>> {
    let request_sequence = port
        .request_safety(kind)
        .map_err(|error| format!("request {kind:?}: {error}").into_boxed_str())?;
    match kind {
        SafetyKind::Stop => shared.metrics.stop_requests.fetch_add(1, Ordering::Relaxed),
        SafetyKind::EStop => shared
            .metrics
            .estop_requests
            .fetch_add(1, Ordering::Relaxed),
    };
    if pending_safety.len() == MAX_PENDING_SAFETY_EVIDENCE {
        // The durable zero-request record below retains the evicted identity and
        // request sequence, so a later cumulative acknowledgement remains joinable.
        pending_safety.pop_front();
    }
    pending_safety.push_back(PendingSafetyEvidence {
        kind,
        request_sequence,
        proposal_sequence,
        command_id: command.command_id(),
        evidence_id: command.evidence_id(),
    });
    publish_event(
        events,
        event_sequence,
        at,
        SupervisorEvent::SafetyRequested {
            kind,
            request_sequence,
            command_id: command.command_id(),
            evidence_id: command.evidence_id(),
        },
    );
    if let Err(error) = record_evidence(
        evidence,
        shared,
        true,
        EvidenceRecord::new(
            proposal_sequence,
            Some(command.command_id()),
            Some(command.evidence_id()),
            EvidenceSource::CpuSafetySupervisor,
            at,
            EvidenceDecision::ZeroRequested,
            Some(AcknowledgementIdentity::safety(
                request_sequence,
                request_sequence,
                None,
                at,
            )),
        ),
    ) {
        evidence_failed(shared, error);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn submit_drive<P: ActuationPort>(
    port: &mut P,
    command: &Authorized<DifferentialDrive>,
    proposal_sequence: Option<u64>,
    events: &LatestPublisher<SupervisorEvent<P::Acknowledgement>>,
    event_sequence: &mut Sequence,
    evidence: Option<&EvidenceProducer>,
    at: MonotonicNanos,
    shared: &SharedState,
) -> Result<(), Box<str>> {
    record_evidence(
        evidence,
        shared,
        false,
        EvidenceRecord::new(
            proposal_sequence,
            Some(command.command_id()),
            Some(command.evidence_id()),
            EvidenceSource::CpuSafetySupervisor,
            at,
            EvidenceDecision::CommandAccepted,
            None,
        ),
    )
    .map_err(|error| format!("persist accepted command: {error}").into_boxed_str())?;
    port.submit_drive(command.clone())
        .map_err(|error| format!("submit drive: {error}").into_boxed_str())?;
    shared
        .metrics
        .drives_submitted
        .fetch_add(1, Ordering::Relaxed);
    publish_event(
        events,
        event_sequence,
        at,
        SupervisorEvent::DriveSubmitted {
            command_id: command.command_id(),
            evidence_id: command.evidence_id(),
        },
    );
    Ok(())
}

fn record_evidence(
    producer: Option<&EvidenceProducer>,
    shared: &SharedState,
    priority: bool,
    record: EvidenceRecord,
) -> Result<(), EvidenceEnqueueError> {
    let Some(producer) = producer else {
        return Ok(());
    };
    let result = if priority {
        producer.try_record_priority(record)
    } else {
        producer.try_record(record)
    };
    if result.is_ok() {
        shared
            .metrics
            .evidence_records
            .fetch_add(1, Ordering::Relaxed);
    }
    result.map(|_| ())
}

fn evidence_failed(shared: &SharedState, error: EvidenceEnqueueError) {
    evidence_unhealthy_with_message(shared, error.to_string());
}

fn evidence_unhealthy(shared: &SharedState) {
    evidence_unhealthy_with_message(shared, "persistence owner is unhealthy");
}

fn evidence_unhealthy_with_message(shared: &SharedState, message: impl fmt::Display) {
    if !shared.evidence_fault_latched.swap(true, Ordering::AcqRel) {
        shared
            .metrics
            .evidence_failures
            .fetch_add(1, Ordering::Relaxed);
    }
    set_fault(shared, format!("evidence persistence: {message}"));
}

fn publish_event<A>(
    publisher: &LatestPublisher<SupervisorEvent<A>>,
    sequence: &mut Sequence,
    at: MonotonicNanos,
    event: SupervisorEvent<A>,
) {
    let _ = publisher.publish(Stamped::new(at, *sequence, event));
    if let Ok(next) = sequence.next() {
        *sequence = next;
    }
}

fn set_fault(shared: &SharedState, message: impl Into<Box<str>>) {
    let message = message.into();
    if !shared.faulted.swap(true, Ordering::AcqRel) {
        shared.metrics.faults.fetch_add(1, Ordering::Relaxed);
    }
    *lock(&shared.last_fault) = Some(message);
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::{atomic::AtomicU64, Condvar},
        time::Instant,
    };

    use leash_core::{
        ControlKernelConfig, NormalizedDrive, OperatorId, ProducerEpoch, SafetyState,
    };

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    fn temp_path(name: &str) -> PathBuf {
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "leash-supervisor-{name}-{}-{unique}.journal",
            std::process::id()
        ))
    }

    fn journal_config(path: &Path) -> crate::EvidenceJournalConfig {
        crate::EvidenceJournalConfig {
            path: path.to_path_buf(),
            normal_capacity: 64,
            priority_capacity: 16,
            maximum_records: None,
        }
    }

    #[derive(Clone)]
    struct AtomicClock(Arc<AtomicU64>);

    impl Clock for AtomicClock {
        fn now(&mut self) -> MonotonicNanos {
            MonotonicNanos::new(self.0.fetch_add(1_000_000, Ordering::Relaxed))
        }
    }

    struct PanicClock;

    impl Clock for PanicClock {
        fn now(&mut self) -> MonotonicNanos {
            panic!("injected safety worker panic")
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestAck(bool);

    impl ActuationAcknowledgement for TestAck {
        fn applied(&self) -> bool {
            self.0
        }

        fn verified_zero(&self) -> bool {
            false
        }
    }

    #[derive(Clone, Default)]
    struct PortState {
        drives: Arc<Mutex<Vec<CommandId>>>,
        safety: Arc<Mutex<Vec<SafetyKind>>>,
    }

    struct TestPort {
        state: PortState,
        acknowledgements: Vec<TestAck>,
    }

    #[derive(Debug, Clone, Copy)]
    struct DetailedAck {
        outcome: ActuationAcknowledgementOutcome,
        command_id: CommandId,
        evidence_id: EvidenceId,
        at: MonotonicNanos,
    }

    impl ActuationAcknowledgement for DetailedAck {
        fn applied(&self) -> bool {
            self.outcome == ActuationAcknowledgementOutcome::Applied
        }

        fn verified_zero(&self) -> bool {
            false
        }

        fn outcome(&self) -> ActuationAcknowledgementOutcome {
            self.outcome
        }

        fn command_id(&self) -> Option<CommandId> {
            Some(self.command_id)
        }

        fn evidence_id(&self) -> Option<EvidenceId> {
            Some(self.evidence_id)
        }

        fn applied_sequence(&self) -> Option<u64> {
            Some(self.command_id.sequence.get())
        }

        fn acknowledged_at(&self) -> Option<MonotonicNanos> {
            Some(self.at)
        }
    }

    struct EvidencePort {
        state: PortState,
        outcome: ActuationAcknowledgementOutcome,
        acknowledgements: VecDeque<DetailedAck>,
        safety_acknowledgements: VecDeque<SafetyAcknowledgement>,
        next_safety_sequence: u64,
    }

    impl EvidencePort {
        fn new(state: PortState, outcome: ActuationAcknowledgementOutcome) -> Self {
            Self {
                state,
                outcome,
                acknowledgements: VecDeque::new(),
                safety_acknowledgements: VecDeque::new(),
                next_safety_sequence: 0,
            }
        }
    }

    impl ActuationPort for EvidencePort {
        type Acknowledgement = DetailedAck;
        type Error = &'static str;

        fn submit_drive(
            &mut self,
            command: Authorized<DifferentialDrive>,
        ) -> Result<(), Self::Error> {
            lock(&self.state.drives).push(command.command_id());
            self.acknowledgements.push_back(DetailedAck {
                outcome: self.outcome,
                command_id: command.command_id(),
                evidence_id: command.evidence_id(),
                at: command.authorized_at(),
            });
            Ok(())
        }

        fn request_safety(&mut self, kind: SafetyKind) -> Result<u64, Self::Error> {
            lock(&self.state.safety).push(kind);
            self.next_safety_sequence += 1;
            self.safety_acknowledgements
                .push_back(SafetyAcknowledgement {
                    kind,
                    first_request_sequence: self.next_safety_sequence,
                    through_request_sequence: self.next_safety_sequence,
                    applied_sequence: Some(self.next_safety_sequence),
                    at: MonotonicNanos::new(self.next_safety_sequence),
                    verified_zero: true,
                });
            Ok(self.next_safety_sequence)
        }

        fn try_acknowledgement(&mut self) -> Result<Option<Self::Acknowledgement>, Self::Error> {
            Ok(self.acknowledgements.pop_front())
        }

        fn try_safety_acknowledgement(
            &mut self,
        ) -> Result<Option<SafetyAcknowledgement>, Self::Error> {
            Ok(self.safety_acknowledgements.pop_front())
        }
    }

    impl ActuationPort for TestPort {
        type Acknowledgement = TestAck;
        type Error = &'static str;

        fn submit_drive(
            &mut self,
            command: Authorized<DifferentialDrive>,
        ) -> Result<(), Self::Error> {
            lock(&self.state.drives).push(command.command_id());
            Ok(())
        }

        fn request_safety(&mut self, kind: SafetyKind) -> Result<u64, Self::Error> {
            let mut safety = lock(&self.state.safety);
            safety.push(kind);
            Ok(safety.len() as u64)
        }

        fn try_acknowledgement(&mut self) -> Result<Option<Self::Acknowledgement>, Self::Error> {
            Ok(self.acknowledgements.pop())
        }
    }

    fn kernel() -> ControlKernel {
        ControlKernel::new(ControlKernelConfig {
            command_epoch: ProducerEpoch::new(31).unwrap(),
            evidence_epoch: ProducerEpoch::new(32).unwrap(),
            deadman: leash_core::DurationNanos::from_millis(50).unwrap(),
        })
    }

    fn supervisor(state: PortState) -> CpuSafetySupervisor<TestAck> {
        CpuSafetySupervisor::spawn(
            kernel(),
            TestPort {
                state,
                acknowledgements: Vec::new(),
            },
            Box::new(AtomicClock(Arc::new(AtomicU64::new(0)))),
            SupervisorConfig {
                proposal_capacity: 4,
                tick_period: Duration::from_millis(1),
            },
        )
        .unwrap()
    }

    fn configure_motion(handle: &SupervisorHandle) {
        handle
            .submit(ControlInput::UpdateEvidence {
                obstacle_blocked: false,
                lidar_fresh: true,
                localization_fresh: true,
            })
            .unwrap()
            .wait()
            .unwrap();
        handle
            .submit(ControlInput::Authorize {
                operator: OperatorId::new("operator").unwrap(),
                expires_at: MonotonicNanos::new(1_000_000_000),
            })
            .unwrap()
            .wait()
            .unwrap();
    }

    #[test]
    fn normal_motion_is_authorized_by_the_kernel_before_submission() {
        let state = PortState::default();
        let supervisor = supervisor(state.clone());
        let handle = supervisor.handle();
        handle
            .submit(ControlInput::UpdateEvidence {
                obstacle_blocked: false,
                lidar_fresh: true,
                localization_fresh: true,
            })
            .unwrap()
            .wait()
            .unwrap();
        handle
            .submit(ControlInput::Authorize {
                operator: OperatorId::new("operator").unwrap(),
                expires_at: MonotonicNanos::new(1_000_000_000),
            })
            .unwrap()
            .wait()
            .unwrap();
        let value = NormalizedDrive::new(0.2).unwrap();
        handle
            .submit(ControlInput::Drive {
                command: DifferentialDrive::new(value, value),
                deadline: MonotonicNanos::new(1_000_000_000),
            })
            .unwrap()
            .wait()
            .unwrap();
        assert_eq!(lock(&state.drives).len(), 1);
        assert_eq!(handle.status().metrics.drives_submitted, 1);
    }

    #[test]
    fn stop_bypasses_a_saturated_proposal_lane_with_bounded_latency() {
        let state = PortState::default();
        let supervisor = supervisor(state.clone());
        let handle = supervisor.handle();
        for _ in 0..100 {
            let _ = handle.submit(ControlInput::Idle);
        }
        let started = Instant::now();
        handle.stop().unwrap();
        for _ in 0..100 {
            if lock(&state.safety).contains(&SafetyKind::Stop) {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(lock(&state.safety).contains(&SafetyKind::Stop));
        assert!(started.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn proposals_and_safety_wake_a_parked_long_period_supervisor() {
        let state = PortState::default();
        let supervisor = CpuSafetySupervisor::spawn(
            kernel(),
            TestPort {
                state: state.clone(),
                acknowledgements: Vec::new(),
            },
            Box::new(AtomicClock(Arc::new(AtomicU64::new(0)))),
            SupervisorConfig {
                proposal_capacity: 4,
                tick_period: Duration::from_secs(1),
            },
        )
        .unwrap();
        let handle = supervisor.handle();
        thread::sleep(Duration::from_millis(10));

        let proposal_started = Instant::now();
        let result = handle
            .submit(ControlInput::UpdateEvidence {
                obstacle_blocked: false,
                lidar_fresh: true,
                localization_fresh: true,
            })
            .unwrap()
            .wait_timeout(Duration::from_millis(100))
            .unwrap();
        assert!(result.is_some());
        assert!(proposal_started.elapsed() < Duration::from_millis(100));

        let stop_started = Instant::now();
        handle.stop().unwrap();
        for _ in 0..100 {
            if lock(&state.safety).contains(&SafetyKind::Stop) {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(lock(&state.safety).contains(&SafetyKind::Stop));
        assert!(stop_started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn estop_is_not_accepted_through_the_normal_proposal_lane() {
        let state = PortState::default();
        let supervisor = supervisor(state);
        assert!(matches!(
            supervisor.handle().submit(ControlInput::EStop),
            Err(SupervisorSubmitError::SafetyUsesPriorityPath)
        ));
    }

    #[test]
    fn failed_actuator_ack_faults_and_requests_estop() {
        let state = PortState::default();
        let supervisor = CpuSafetySupervisor::spawn(
            kernel(),
            TestPort {
                state: state.clone(),
                acknowledgements: vec![TestAck(false)],
            },
            Box::new(AtomicClock(Arc::new(AtomicU64::new(0)))),
            SupervisorConfig {
                proposal_capacity: 1,
                tick_period: Duration::from_millis(1),
            },
        )
        .unwrap();
        for _ in 0..100 {
            if supervisor.handle().status().faulted
                && lock(&state.safety).contains(&SafetyKind::EStop)
            {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(supervisor.handle().status().faulted);
        assert!(lock(&state.safety).contains(&SafetyKind::EStop));
    }

    #[test]
    fn supervisor_does_not_depend_on_gpu_or_gateway_progress() {
        let state = PortState::default();
        let supervisor = supervisor(state.clone());
        let blocked = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let worker_blocked = Arc::clone(&blocked);
        let unrelated = thread::spawn(move || {
            let (mutex, ready) = &*worker_blocked;
            let mut release = lock(mutex);
            while !*release {
                release = ready.wait(release).unwrap();
            }
        });
        supervisor.handle().estop().unwrap();
        for _ in 0..100 {
            if lock(&state.safety).contains(&SafetyKind::EStop) {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(lock(&state.safety).contains(&SafetyKind::EStop));
        let (mutex, ready) = &*blocked;
        *lock(mutex) = true;
        ready.notify_one();
        unrelated.join().unwrap();
    }

    #[test]
    fn worker_panic_is_contained_closed_and_observable() {
        let supervisor = CpuSafetySupervisor::spawn(
            kernel(),
            TestPort {
                state: PortState::default(),
                acknowledgements: Vec::new(),
            },
            Box::new(PanicClock),
            SupervisorConfig {
                proposal_capacity: 4,
                tick_period: Duration::from_millis(1),
            },
        )
        .unwrap();
        let handle = supervisor.handle();
        for _ in 0..100 {
            if handle.status().closed {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        let status = handle.status();
        assert!(status.closed);
        assert!(status.faulted);
        assert_eq!(status.metrics.worker_panics, 1);
        assert_eq!(
            status.last_fault.as_deref(),
            Some("CPU safety worker panicked")
        );
        assert_eq!(
            handle.submit(ControlInput::Idle).err(),
            Some(SupervisorSubmitError::Faulted)
        );
    }

    #[test]
    fn shutdown_wakes_a_parked_worker_and_closes_its_lane() {
        let supervisor = CpuSafetySupervisor::spawn(
            kernel(),
            TestPort {
                state: PortState::default(),
                acknowledgements: Vec::new(),
            },
            Box::new(AtomicClock(Arc::new(AtomicU64::new(0)))),
            SupervisorConfig {
                proposal_capacity: 4,
                tick_period: Duration::from_secs(60),
            },
        )
        .unwrap();
        let handle = supervisor.handle();
        let started = Instant::now();
        supervisor.shutdown();
        assert!(started.elapsed() < Duration::from_millis(50));
        assert!(handle.status().closed);
        assert_eq!(
            handle.submit(ControlInput::Idle).err(),
            Some(SupervisorSubmitError::Closed)
        );
    }

    #[test]
    fn configuration_is_fail_closed() {
        let result = CpuSafetySupervisor::spawn(
            kernel(),
            TestPort {
                state: PortState::default(),
                acknowledgements: Vec::new(),
            },
            Box::new(AtomicClock(Arc::new(AtomicU64::new(0)))),
            SupervisorConfig {
                proposal_capacity: 0,
                tick_period: Duration::from_millis(1),
            },
        );
        assert!(matches!(
            result,
            Err(SupervisorStartError::ZeroProposalCapacity)
        ));
        let _ = SafetyState::Disarmed;
    }

    #[test]
    fn durable_evidence_covers_acceptance_denial_and_verified_zero() {
        let path = temp_path("decisions");
        let journal = crate::EvidenceJournal::open(journal_config(&path)).unwrap();
        let state = PortState::default();
        let supervisor = CpuSafetySupervisor::spawn_with_evidence(
            kernel(),
            EvidencePort::new(state.clone(), ActuationAcknowledgementOutcome::Applied),
            Box::new(AtomicClock(Arc::new(AtomicU64::new(0)))),
            SupervisorConfig {
                proposal_capacity: 8,
                tick_period: Duration::from_millis(1),
            },
            journal.producer(),
        )
        .unwrap();
        let handle = supervisor.handle();
        let speed = NormalizedDrive::new(0.2).unwrap();
        handle
            .submit(ControlInput::Drive {
                command: DifferentialDrive::new(speed, speed),
                deadline: MonotonicNanos::new(1_000_000_000),
            })
            .unwrap()
            .wait()
            .unwrap();
        configure_motion(&handle);
        handle
            .submit(ControlInput::Drive {
                command: DifferentialDrive::new(speed, speed),
                deadline: MonotonicNanos::new(1_000_000_000),
            })
            .unwrap()
            .wait()
            .unwrap();
        handle.stop().unwrap();
        for _ in 0..100 {
            if handle.status().metrics.acknowledgements > 0
                && lock(&state.safety).contains(&SafetyKind::Stop)
            {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        supervisor.shutdown();
        journal.shutdown();
        let records = crate::read_evidence_records(&path).unwrap();
        let decisions = records
            .iter()
            .map(|record| record.decision)
            .collect::<Vec<_>>();
        assert!(decisions.contains(&EvidenceDecision::ProposalAccepted));
        assert!(decisions.contains(&EvidenceDecision::CommandRejected));
        assert!(decisions.contains(&EvidenceDecision::CommandAccepted));
        assert!(decisions.contains(&EvidenceDecision::AcknowledgementApplied));
        assert!(decisions.contains(&EvidenceDecision::ZeroRequested));
        assert!(decisions.contains(&EvidenceDecision::ZeroVerified));
        assert!(records
            .iter()
            .filter(|record| matches!(record.decision, EvidenceDecision::CommandAccepted))
            .all(|record| record.proposal_sequence.is_some()
                && record.command_id.is_some()
                && record.evidence_id.is_some()));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn durable_evidence_distinguishes_superseded_and_failed_acknowledgements() {
        for (name, outcome, expected) in [
            (
                "superseded",
                ActuationAcknowledgementOutcome::Superseded,
                EvidenceDecision::CommandSuperseded,
            ),
            (
                "failed",
                ActuationAcknowledgementOutcome::Failed,
                EvidenceDecision::AcknowledgementFailed,
            ),
        ] {
            let path = temp_path(name);
            let journal = crate::EvidenceJournal::open(journal_config(&path)).unwrap();
            let state = PortState::default();
            let supervisor = CpuSafetySupervisor::spawn_with_evidence(
                kernel(),
                EvidencePort::new(state.clone(), outcome),
                Box::new(AtomicClock(Arc::new(AtomicU64::new(0)))),
                SupervisorConfig {
                    proposal_capacity: 8,
                    tick_period: Duration::from_millis(1),
                },
                journal.producer(),
            )
            .unwrap();
            let handle = supervisor.handle();
            configure_motion(&handle);
            let speed = NormalizedDrive::new(0.2).unwrap();
            handle
                .submit(ControlInput::Drive {
                    command: DifferentialDrive::new(speed, speed),
                    deadline: MonotonicNanos::new(1_000_000_000),
                })
                .unwrap()
                .wait()
                .unwrap();
            for _ in 0..100 {
                if handle.status().faulted && lock(&state.safety).contains(&SafetyKind::EStop) {
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            }
            assert!(handle.status().faulted);
            assert!(handle.stop().is_ok());
            supervisor.shutdown();
            journal.shutdown();
            let records = crate::read_evidence_records(&path).unwrap();
            assert!(records.iter().any(|record| {
                record.decision == expected
                    && record.command_id.is_some()
                    && record.evidence_id.is_some()
                    && record.acknowledgement.is_some()
            }));
            fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn proposal_queue_rejection_is_persisted() {
        struct GatedClock {
            gate: Arc<(Mutex<bool>, Condvar)>,
        }

        impl Clock for GatedClock {
            fn now(&mut self) -> MonotonicNanos {
                let (mutex, ready) = &*self.gate;
                let mut released = lock(mutex);
                while !*released {
                    released = ready.wait(released).unwrap();
                }
                MonotonicNanos::new(1)
            }
        }

        let path = temp_path("queue-rejection");
        let journal = crate::EvidenceJournal::open(journal_config(&path)).unwrap();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let supervisor = CpuSafetySupervisor::spawn_with_evidence(
            kernel(),
            TestPort {
                state: PortState::default(),
                acknowledgements: Vec::new(),
            },
            Box::new(GatedClock {
                gate: Arc::clone(&gate),
            }),
            SupervisorConfig {
                proposal_capacity: 1,
                tick_period: Duration::from_millis(1),
            },
            journal.producer(),
        )
        .unwrap();
        let handle = supervisor.handle();
        let first = handle.submit(ControlInput::Idle).unwrap();
        assert_eq!(
            handle.submit(ControlInput::Idle).err(),
            Some(SupervisorSubmitError::Full)
        );
        let (mutex, ready) = &*gate;
        *lock(mutex) = true;
        ready.notify_one();
        first.wait().unwrap();
        supervisor.shutdown();
        journal.shutdown();
        let records = crate::read_evidence_records(&path).unwrap();
        assert!(records
            .iter()
            .any(|record| record.decision == EvidenceDecision::ProposalRejected));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn stalled_evidence_writer_fails_closed_without_delaying_estop() {
        let path = temp_path("writer-stall");
        let mut config = journal_config(&path);
        config.normal_capacity = 1;
        let journal = crate::EvidenceJournal::open_paused(config).unwrap();
        let state = PortState::default();
        let supervisor = CpuSafetySupervisor::spawn_with_evidence(
            kernel(),
            EvidencePort::new(state.clone(), ActuationAcknowledgementOutcome::Applied),
            Box::new(AtomicClock(Arc::new(AtomicU64::new(0)))),
            SupervisorConfig {
                proposal_capacity: 4,
                tick_period: Duration::from_secs(1),
            },
            journal.producer(),
        )
        .unwrap();
        let handle = supervisor.handle();
        let first = handle.submit(ControlInput::Idle).unwrap();
        first.wait().unwrap();
        let started = Instant::now();
        assert_eq!(
            handle.submit(ControlInput::Idle).err(),
            Some(SupervisorSubmitError::Faulted)
        );
        for _ in 0..100 {
            if lock(&state.safety).contains(&SafetyKind::EStop) {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(lock(&state.safety).contains(&SafetyKind::EStop));
        assert!(started.elapsed() < Duration::from_millis(50));
        assert!(handle.stop().is_ok());
        journal.resume();
        supervisor.shutdown();
        let status = journal.shutdown();
        assert!(status.saturated);
        let records = crate::read_evidence_records(&path).unwrap();
        assert!(records
            .iter()
            .any(|record| record.decision == EvidenceDecision::JournalSaturated));
        assert!(records
            .iter()
            .any(|record| record.decision == EvidenceDecision::ZeroRequested));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn full_storage_records_terminal_state_and_fails_closed() {
        let path = temp_path("storage-full");
        let mut config = journal_config(&path);
        config.maximum_records = Some(2);
        let journal = crate::EvidenceJournal::open(config).unwrap();
        let state = PortState::default();
        let supervisor = CpuSafetySupervisor::spawn_with_evidence(
            kernel(),
            EvidencePort::new(state.clone(), ActuationAcknowledgementOutcome::Applied),
            Box::new(AtomicClock(Arc::new(AtomicU64::new(0)))),
            SupervisorConfig {
                proposal_capacity: 4,
                tick_period: Duration::from_millis(1),
            },
            journal.producer(),
        )
        .unwrap();
        let handle = supervisor.handle();
        handle.submit(ControlInput::Idle).unwrap().wait().unwrap();
        let _ = handle.submit(ControlInput::Idle);
        for _ in 0..100 {
            if handle.status().faulted {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(handle.status().faulted);
        assert!(handle.stop().is_ok());
        supervisor.shutdown();
        let status = journal.shutdown();
        assert!(status.storage_full);
        let records = crate::read_evidence_records(&path).unwrap();
        assert_eq!(
            records.last().unwrap().decision,
            EvidenceDecision::StorageFull
        );
        fs::remove_file(path).unwrap();
    }
}
