use std::{
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
    bounded_lane, latest_slot, safety_mailbox, BoundedReceiver, BoundedSender, LatestPublisher,
    LatestReader, OverflowPolicy, SafetyKind, SafetyReceiver, SafetyRequestError, SafetySender,
    SendError,
};

pub trait ActuationAcknowledgement: Send + 'static {
    fn applied(&self) -> bool;
    fn verified_zero(&self) -> bool;
}

pub trait ActuationPort: Send + 'static {
    type Acknowledgement: ActuationAcknowledgement;
    type Error: fmt::Display + Send + 'static;

    fn submit_drive(&mut self, command: Authorized<DifferentialDrive>) -> Result<(), Self::Error>;

    fn request_safety(&mut self, kind: SafetyKind) -> Result<u64, Self::Error>;

    fn try_acknowledgement(&mut self) -> Result<Option<Self::Acknowledgement>, Self::Error>;
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
}

struct SharedState {
    shutdown: AtomicBool,
    faulted: AtomicBool,
    closed: AtomicBool,
    next_proposal: AtomicU64,
    last_fault: Mutex<Option<Box<str>>>,
    worker_thread: OnceLock<thread::Thread>,
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

#[derive(Clone)]
pub struct SupervisorHandle {
    proposals: BoundedSender<Proposal>,
    safety: SafetySender,
    shared: Arc<SharedState>,
}

impl SupervisorHandle {
    pub fn submit(&self, input: ControlInput) -> Result<TransitionTicket, SupervisorSubmitError> {
        if matches!(input, ControlInput::Stop { .. } | ControlInput::EStop) {
            return Err(SupervisorSubmitError::SafetyUsesPriorityPath);
        }
        if self.shared.faulted.load(Ordering::Acquire) {
            self.shared
                .metrics
                .proposals_rejected
                .fetch_add(1, Ordering::Relaxed);
            return Err(SupervisorSubmitError::Faulted);
        }
        let sequence = self
            .shared
            .next_proposal
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(1)
            })
            .map_err(|_| SupervisorSubmitError::SequenceExhausted)?;
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
                self.shared.wake();
                Ok(TransitionTicket { receiver })
            }
            Err(SendError::Full(_)) => {
                self.shared
                    .metrics
                    .proposals_rejected
                    .fetch_add(1, Ordering::Relaxed);
                Err(SupervisorSubmitError::Full)
            }
            Err(SendError::Closed(_)) => Err(SupervisorSubmitError::Closed),
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
            metrics: MetricAtoms::default(),
        });
        let handle = SupervisorHandle {
            proposals: proposal_sender,
            safety: safety_sender,
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
    shared: Arc<SharedState>,
) where
    P: ActuationPort,
{
    let mut tick_sequence = Sequence::new(1).expect("one is non-zero");
    let mut event_sequence = Sequence::new(1).expect("one is non-zero");
    let mut internal_estop = false;
    let mut next_periodic_tick = Instant::now() + config.tick_period;
    while !shared.shutdown.load(Ordering::Acquire) {
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
                for effect in effects.iter() {
                    if let ControlEffect::Actuate { command, .. } = effect {
                        let actuation = if command.command().is_stop() {
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
                                &events,
                                &mut event_sequence,
                                at,
                                &shared,
                            )
                        } else {
                            submit_drive(
                                &mut port,
                                command,
                                &events,
                                &mut event_sequence,
                                at,
                                &shared,
                            )
                        };
                        if let Err(error) = actuation {
                            set_fault(&shared, error);
                            internal_estop = true;
                        }
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
        let now = Instant::now();
        while next_periodic_tick <= now {
            next_periodic_tick += config.tick_period;
        }
        thread::park_timeout(next_periodic_tick.saturating_duration_since(now));
    }
    while let Some(proposal) = proposals.try_recv() {
        let _ = proposal
            .reply
            .send(Err("CPU safety supervisor stopped".into()));
    }
    shared.closed.store(true, Ordering::Release);
}

fn request_safety<P: ActuationPort>(
    port: &mut P,
    kind: SafetyKind,
    command: &Authorized<DifferentialDrive>,
    events: &LatestPublisher<SupervisorEvent<P::Acknowledgement>>,
    event_sequence: &mut Sequence,
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
    Ok(())
}

fn submit_drive<P: ActuationPort>(
    port: &mut P,
    command: &Authorized<DifferentialDrive>,
    events: &LatestPublisher<SupervisorEvent<P::Acknowledgement>>,
    event_sequence: &mut Sequence,
    at: MonotonicNanos,
    shared: &SharedState,
) -> Result<(), Box<str>> {
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
    use std::time::Instant;

    use leash_core::{
        ControlKernelConfig, NormalizedDrive, OperatorId, ProducerEpoch, SafetyState,
    };

    use super::*;

    #[derive(Clone)]
    struct AtomicClock(Arc<AtomicU64>);

    impl Clock for AtomicClock {
        fn now(&mut self) -> MonotonicNanos {
            MonotonicNanos::new(self.0.fetch_add(1_000_000, Ordering::Relaxed))
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
}
