//! Single-owner Waveshare UGV controller boundary.

#![forbid(unsafe_code)]

use std::{
    collections::VecDeque,
    fmt,
    io::{self, Read, Write},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver},
        Arc, Mutex, MutexGuard,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use leash_core::{
    Authorized, Clock, CommandId, DifferentialDrive, EvidenceId, MonotonicNanos, Sequence, Stamped,
};
use leash_runtime::{
    bounded_lane, latest_slot, safety_mailbox, ActuationAcknowledgement,
    ActuationAcknowledgementOutcome, ActuationPort, BoundedReceiver, BoundedSender, LaneSnapshot,
    LatestPublisher, LatestReader, OverflowPolicy, SafetyAcknowledgement, SafetyKind,
    SafetyReceiver, SafetyRequestError, SafetySender, SafetySignal, SendError,
};
use serde_json::json;

#[cfg(feature = "serial")]
mod serial;

#[cfg(feature = "serial")]
pub use serial::{SerialFactoryError, SerialPortFactory};

pub trait ControllerIo: Read + Write + Send {}

impl<T> ControllerIo for T where T: Read + Write + Send {}

pub trait ControllerIoFactory: Send {
    fn open(&mut self) -> io::Result<Box<dyn ControllerIo>>;
}

impl<F> ControllerIoFactory for F
where
    F: FnMut() -> io::Result<Box<dyn ControllerIo>> + Send,
{
    fn open(&mut self) -> io::Result<Box<dyn ControllerIo>> {
        self()
    }
}

#[derive(Debug)]
pub struct SystemMonotonicClock {
    origin: Instant,
}

impl SystemMonotonicClock {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemMonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemMonotonicClock {
    fn now(&mut self) -> MonotonicNanos {
        MonotonicNanos::new(u64::try_from(self.origin.elapsed().as_nanos()).unwrap_or(u64::MAX))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnerConfig {
    pub command_capacity: usize,
    pub poll_interval: Duration,
    pub reconnect_interval: Duration,
    pub maximum_telemetry_line_bytes: usize,
    pub drive_invert: bool,
    pub drive_swap: bool,
}

impl Default for OwnerConfig {
    fn default() -> Self {
        Self {
            command_capacity: 16,
            poll_interval: Duration::from_millis(2),
            reconnect_interval: Duration::from_millis(250),
            maximum_telemetry_line_bytes: 8 * 1024,
            drive_invert: false,
            drive_swap: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigError {
    ZeroCommandCapacity,
    ZeroPollInterval,
    ZeroReconnectInterval,
    TelemetryLineTooSmall,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroCommandCapacity => formatter.write_str("command capacity must be positive"),
            Self::ZeroPollInterval => formatter.write_str("poll interval must be positive"),
            Self::ZeroReconnectInterval => {
                formatter.write_str("reconnect interval must be positive")
            }
            Self::TelemetryLineTooSmall => {
                formatter.write_str("maximum telemetry line must be at least 64 bytes")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    Config(ConfigError),
    Thread(String),
}

impl fmt::Display for StartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => write!(formatter, "invalid owner configuration: {error}"),
            Self::Thread(error) => write!(formatter, "start controller owner thread: {error}"),
        }
    }
}

impl std::error::Error for StartError {}

impl From<ConfigError> for StartError {
    fn from(value: ConfigError) -> Self {
        Self::Config(value)
    }
}

impl OwnerConfig {
    fn validate(self) -> Result<Self, ConfigError> {
        if self.command_capacity == 0 {
            return Err(ConfigError::ZeroCommandCapacity);
        }
        if self.poll_interval.is_zero() {
            return Err(ConfigError::ZeroPollInterval);
        }
        if self.reconnect_interval.is_zero() {
            return Err(ConfigError::ZeroReconnectInterval);
        }
        if self.maximum_telemetry_line_bytes < 64 {
            return Err(ConfigError::TelemetryLineTooSmall);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GimbalCommand {
    pub id: CommandId,
    pub pan_degrees: f64,
    pub tilt_degrees: f64,
    pub speed: u32,
    pub acceleration: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GimbalError {
    NonFinite,
    PanOutOfRange,
    TiltOutOfRange,
    ZeroSpeed,
    ZeroAcceleration,
}

impl fmt::Display for GimbalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFinite => formatter.write_str("gimbal angle is non-finite"),
            Self::PanOutOfRange => formatter.write_str("gimbal pan must be within -180..=180"),
            Self::TiltOutOfRange => formatter.write_str("gimbal tilt must be within -30..=90"),
            Self::ZeroSpeed => formatter.write_str("gimbal speed must be positive"),
            Self::ZeroAcceleration => formatter.write_str("gimbal acceleration must be positive"),
        }
    }
}

impl std::error::Error for GimbalError {}

impl GimbalCommand {
    pub fn new(
        id: CommandId,
        pan_degrees: f64,
        tilt_degrees: f64,
        speed: u32,
        acceleration: u32,
    ) -> Result<Self, GimbalError> {
        if !pan_degrees.is_finite() || !tilt_degrees.is_finite() {
            return Err(GimbalError::NonFinite);
        }
        if !(-180.0..=180.0).contains(&pan_degrees) {
            return Err(GimbalError::PanOutOfRange);
        }
        if !(-30.0..=90.0).contains(&tilt_degrees) {
            return Err(GimbalError::TiltOutOfRange);
        }
        if speed == 0 {
            return Err(GimbalError::ZeroSpeed);
        }
        if acceleration == 0 {
            return Err(GimbalError::ZeroAcceleration);
        }
        Ok(Self {
            id,
            pan_degrees,
            tilt_degrees,
            speed,
            acceleration,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckOutcome {
    Applied,
    SupersededBySafety,
    EStopped,
    Disconnected,
    IoFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandAck {
    pub command_id: CommandId,
    pub evidence_id: Option<EvidenceId>,
    pub outcome: AckOutcome,
    pub applied_sequence: Option<u64>,
    pub at: MonotonicNanos,
    pub verified_zero: bool,
    pub detail: Option<Box<str>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SafetyReceipt {
    pub kind: SafetyKind,
    pub first_request_sequence: u64,
    pub through_request_sequence: u64,
    pub coalesced: u64,
    pub applied_sequence: u64,
    pub applied_at: MonotonicNanos,
    pub verified_zero: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawTelemetry {
    pub json: Box<str>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OwnerMetrics {
    pub accepted: u64,
    pub rejected_full: u64,
    pub rejected_estopped: u64,
    pub superseded_by_safety: u64,
    pub writes: u64,
    pub write_failures: u64,
    pub telemetry_frames: u64,
    pub malformed_telemetry: u64,
    pub disconnects: u64,
    pub reconnects: u64,
    pub worker_panics: u64,
}

#[derive(Default)]
struct MetricAtoms {
    accepted: AtomicU64,
    rejected_full: AtomicU64,
    rejected_estopped: AtomicU64,
    superseded_by_safety: AtomicU64,
    writes: AtomicU64,
    write_failures: AtomicU64,
    telemetry_frames: AtomicU64,
    malformed_telemetry: AtomicU64,
    disconnects: AtomicU64,
    reconnects: AtomicU64,
    worker_panics: AtomicU64,
}

impl MetricAtoms {
    fn snapshot(&self) -> OwnerMetrics {
        OwnerMetrics {
            accepted: self.accepted.load(Ordering::Relaxed),
            rejected_full: self.rejected_full.load(Ordering::Relaxed),
            rejected_estopped: self.rejected_estopped.load(Ordering::Relaxed),
            superseded_by_safety: self.superseded_by_safety.load(Ordering::Relaxed),
            writes: self.writes.load(Ordering::Relaxed),
            write_failures: self.write_failures.load(Ordering::Relaxed),
            telemetry_frames: self.telemetry_frames.load(Ordering::Relaxed),
            malformed_telemetry: self.malformed_telemetry.load(Ordering::Relaxed),
            disconnects: self.disconnects.load(Ordering::Relaxed),
            reconnects: self.reconnects.load(Ordering::Relaxed),
            worker_panics: self.worker_panics.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerStatus {
    pub connected: bool,
    pub estopped: bool,
    pub last_error: Option<Box<str>>,
    pub last_safety_receipt: Option<SafetyReceipt>,
    pub last_stop_receipt: Option<SafetyReceipt>,
    pub last_estop_receipt: Option<SafetyReceipt>,
    pub metrics: OwnerMetrics,
    pub command_lane: LaneSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitError {
    Full,
    Closed,
    EStopped,
}

impl fmt::Display for SubmitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full => formatter.write_str("controller command lane is full"),
            Self::Closed => formatter.write_str("controller owner is closed"),
            Self::EStopped => formatter.write_str("controller owner is e-stopped"),
        }
    }
}

impl std::error::Error for SubmitError {}

pub struct AckTicket {
    receiver: Receiver<CommandAck>,
}

impl AckTicket {
    pub fn wait(self) -> Result<CommandAck, SubmitError> {
        self.receiver.recv().map_err(|_| SubmitError::Closed)
    }

    pub fn wait_timeout(&self, timeout: Duration) -> Result<Option<CommandAck>, SubmitError> {
        match self.receiver.recv_timeout(timeout) {
            Ok(ack) => Ok(Some(ack)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(SubmitError::Closed),
        }
    }

    pub fn try_take(&self) -> Result<Option<CommandAck>, SubmitError> {
        match self.receiver.try_recv() {
            Ok(ack) => Ok(Some(ack)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(SubmitError::Closed),
        }
    }
}

enum ControllerCommand {
    Drive {
        command: Authorized<DifferentialDrive>,
        reply: mpsc::Sender<CommandAck>,
    },
    Gimbal {
        command: GimbalCommand,
        reply: mpsc::Sender<CommandAck>,
    },
    RequestTelemetry {
        id: CommandId,
        reply: mpsc::Sender<CommandAck>,
    },
    EnableTelemetry {
        id: CommandId,
        reply: mpsc::Sender<CommandAck>,
    },
}

struct SharedState {
    connected: AtomicBool,
    estopped: AtomicBool,
    shutdown: AtomicBool,
    last_error: Mutex<Option<Box<str>>>,
    last_stop_receipt: Mutex<Option<SafetyReceipt>>,
    last_estop_receipt: Mutex<Option<SafetyReceipt>>,
    metrics: MetricAtoms,
}

#[derive(Clone)]
pub struct ControllerHandle {
    commands: BoundedSender<ControllerCommand>,
    safety: SafetySender,
    shared: Arc<SharedState>,
}

impl ControllerHandle {
    pub fn submit_drive(
        &self,
        command: Authorized<DifferentialDrive>,
    ) -> Result<AckTicket, SubmitError> {
        if self.shared.estopped.load(Ordering::Acquire) && !command.command().is_stop() {
            self.shared
                .metrics
                .rejected_estopped
                .fetch_add(1, Ordering::Relaxed);
            return Err(SubmitError::EStopped);
        }
        let (reply, receiver) = mpsc::channel();
        self.enqueue(ControllerCommand::Drive { command, reply })?;
        Ok(AckTicket { receiver })
    }

    pub fn submit_gimbal(&self, command: GimbalCommand) -> Result<AckTicket, SubmitError> {
        let (reply, receiver) = mpsc::channel();
        self.enqueue(ControllerCommand::Gimbal { command, reply })?;
        Ok(AckTicket { receiver })
    }

    pub fn request_telemetry(&self, id: CommandId) -> Result<AckTicket, SubmitError> {
        let (reply, receiver) = mpsc::channel();
        self.enqueue(ControllerCommand::RequestTelemetry { id, reply })?;
        Ok(AckTicket { receiver })
    }

    pub fn enable_telemetry(&self, id: CommandId) -> Result<AckTicket, SubmitError> {
        let (reply, receiver) = mpsc::channel();
        self.enqueue(ControllerCommand::EnableTelemetry { id, reply })?;
        Ok(AckTicket { receiver })
    }

    pub fn safety(&self) -> SafetySender {
        self.safety.clone()
    }

    /// Clears the controller-owner e-stop latch after the CPU safety authority
    /// has approved the corresponding reset transition.
    ///
    /// This does not reset the safety kernel and cannot authorize motion by
    /// itself. Callers must reset the kernel first and then establish a fresh
    /// operator lease before submitting another non-zero command.
    pub fn reset_estop_latch(&self, approved: bool) -> bool {
        if !approved || self.shared.shutdown.load(Ordering::Acquire) {
            return false;
        }
        self.shared.estopped.store(false, Ordering::Release);
        true
    }

    pub fn status(&self) -> OwnerStatus {
        let last_stop_receipt = *lock(&self.shared.last_stop_receipt);
        let last_estop_receipt = *lock(&self.shared.last_estop_receipt);
        let last_safety_receipt = match (last_stop_receipt, last_estop_receipt) {
            (Some(stop), Some(estop)) => Some(if stop.applied_sequence > estop.applied_sequence {
                stop
            } else {
                estop
            }),
            (Some(stop), None) => Some(stop),
            (None, Some(estop)) => Some(estop),
            (None, None) => None,
        };
        OwnerStatus {
            connected: self.shared.connected.load(Ordering::Acquire),
            estopped: self.shared.estopped.load(Ordering::Acquire),
            last_error: lock(&self.shared.last_error).clone(),
            last_safety_receipt,
            last_stop_receipt,
            last_estop_receipt,
            metrics: self.shared.metrics.snapshot(),
            command_lane: self.commands.snapshot(),
        }
    }

    fn enqueue(&self, command: ControllerCommand) -> Result<(), SubmitError> {
        match self.commands.try_send(command) {
            Ok(_) => {
                self.shared.metrics.accepted.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(SendError::Full(_)) => {
                self.shared
                    .metrics
                    .rejected_full
                    .fetch_add(1, Ordering::Relaxed);
                Err(SubmitError::Full)
            }
            Err(SendError::Closed(_)) => Err(SubmitError::Closed),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortError {
    Submit(SubmitError),
    Safety(SafetyRequestError),
}

impl fmt::Display for PortError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Submit(error) => write!(formatter, "submit controller command: {error}"),
            Self::Safety(error) => write!(formatter, "request controller safety action: {error}"),
        }
    }
}

impl std::error::Error for PortError {}

impl ActuationAcknowledgement for CommandAck {
    fn applied(&self) -> bool {
        self.outcome == AckOutcome::Applied
    }

    fn verified_zero(&self) -> bool {
        self.verified_zero
    }

    fn outcome(&self) -> ActuationAcknowledgementOutcome {
        match self.outcome {
            AckOutcome::Applied => ActuationAcknowledgementOutcome::Applied,
            AckOutcome::SupersededBySafety => ActuationAcknowledgementOutcome::Superseded,
            AckOutcome::EStopped | AckOutcome::Disconnected | AckOutcome::IoFailed => {
                ActuationAcknowledgementOutcome::Failed
            }
        }
    }

    fn command_id(&self) -> Option<CommandId> {
        Some(self.command_id)
    }

    fn evidence_id(&self) -> Option<EvidenceId> {
        self.evidence_id
    }

    fn applied_sequence(&self) -> Option<u64> {
        self.applied_sequence
    }

    fn acknowledged_at(&self) -> Option<MonotonicNanos> {
        Some(self.at)
    }
}

pub struct WaveshareActuationPort {
    handle: ControllerHandle,
    pending: VecDeque<AckTicket>,
    seen_stop_request: u64,
    seen_estop_request: u64,
}

impl WaveshareActuationPort {
    pub fn new(handle: ControllerHandle) -> Self {
        Self {
            handle,
            pending: VecDeque::new(),
            seen_stop_request: 0,
            seen_estop_request: 0,
        }
    }

    pub fn pending_acknowledgements(&self) -> usize {
        self.pending.len()
    }
}

impl ActuationPort for WaveshareActuationPort {
    type Acknowledgement = CommandAck;
    type Error = PortError;

    fn submit_drive(&mut self, command: Authorized<DifferentialDrive>) -> Result<(), Self::Error> {
        let ticket = self
            .handle
            .submit_drive(command)
            .map_err(PortError::Submit)?;
        self.pending.push_back(ticket);
        Ok(())
    }

    fn request_safety(&mut self, kind: SafetyKind) -> Result<u64, Self::Error> {
        self.handle
            .safety()
            .request(kind)
            .map_err(PortError::Safety)
    }

    fn try_acknowledgement(&mut self) -> Result<Option<Self::Acknowledgement>, Self::Error> {
        let Some(ticket) = self.pending.front() else {
            return Ok(None);
        };
        match ticket.try_take().map_err(PortError::Submit)? {
            Some(ack) => {
                self.pending.pop_front();
                Ok(Some(ack))
            }
            None => Ok(None),
        }
    }

    fn try_safety_acknowledgement(&mut self) -> Result<Option<SafetyAcknowledgement>, Self::Error> {
        let status = self.handle.status();
        if let Some(receipt) = status
            .last_estop_receipt
            .filter(|receipt| receipt.through_request_sequence > self.seen_estop_request)
        {
            let first_request_sequence = receipt
                .first_request_sequence
                .max(self.seen_estop_request.saturating_add(1));
            self.seen_estop_request = receipt.through_request_sequence;
            return Ok(Some(safety_acknowledgement(
                receipt,
                first_request_sequence,
            )));
        }
        if let Some(receipt) = status
            .last_stop_receipt
            .filter(|receipt| receipt.through_request_sequence > self.seen_stop_request)
        {
            let first_request_sequence = receipt
                .first_request_sequence
                .max(self.seen_stop_request.saturating_add(1));
            self.seen_stop_request = receipt.through_request_sequence;
            return Ok(Some(safety_acknowledgement(
                receipt,
                first_request_sequence,
            )));
        }
        Ok(None)
    }
}

fn safety_acknowledgement(
    receipt: SafetyReceipt,
    first_request_sequence: u64,
) -> SafetyAcknowledgement {
    SafetyAcknowledgement {
        kind: receipt.kind,
        first_request_sequence,
        through_request_sequence: receipt.through_request_sequence,
        applied_sequence: Some(receipt.applied_sequence),
        at: receipt.applied_at,
        verified_zero: receipt.verified_zero,
    }
}

pub struct ControllerOwner {
    handle: ControllerHandle,
    telemetry: Option<LatestReader<RawTelemetry>>,
    worker: Option<JoinHandle<()>>,
}

impl ControllerOwner {
    pub fn spawn(
        factory: Box<dyn ControllerIoFactory>,
        clock: Box<dyn Clock + Send>,
        config: OwnerConfig,
    ) -> Result<Self, StartError> {
        let config = config.validate()?;
        let (commands, command_receiver) =
            bounded_lane(config.command_capacity, OverflowPolicy::RejectNewest)
                .expect("validated non-zero command capacity");
        let (safety, safety_receiver) = safety_mailbox();
        let (telemetry_publisher, telemetry) = latest_slot();
        let shared = Arc::new(SharedState {
            connected: AtomicBool::new(false),
            estopped: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            last_error: Mutex::new(None),
            last_stop_receipt: Mutex::new(None),
            last_estop_receipt: Mutex::new(None),
            metrics: MetricAtoms::default(),
        });
        let handle = ControllerHandle {
            commands,
            safety,
            shared: Arc::clone(&shared),
        };
        let panic_shared = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("leash-waveshare-owner".to_string())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_owner(
                        factory,
                        clock,
                        config,
                        command_receiver,
                        safety_receiver,
                        telemetry_publisher,
                        shared,
                    );
                }));
                if result.is_err() {
                    panic_shared.connected.store(false, Ordering::Release);
                    panic_shared.shutdown.store(true, Ordering::Release);
                    panic_shared
                        .metrics
                        .worker_panics
                        .fetch_add(1, Ordering::Relaxed);
                    *lock(&panic_shared.last_error) =
                        Some("controller owner thread panicked".into());
                }
            })
            .map_err(|error| StartError::Thread(error.to_string()))?;
        Ok(Self {
            handle,
            telemetry: Some(telemetry),
            worker: Some(worker),
        })
    }

    pub fn handle(&self) -> ControllerHandle {
        self.handle.clone()
    }

    pub fn take_telemetry(&mut self) -> Option<Stamped<RawTelemetry>> {
        self.telemetry.as_mut().and_then(LatestReader::take)
    }

    pub fn shutdown(mut self) {
        self.stop_and_join();
    }

    fn stop_and_join(&mut self) {
        self.handle.shared.shutdown.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for ControllerOwner {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

#[allow(clippy::too_many_arguments)]
fn run_owner(
    mut factory: Box<dyn ControllerIoFactory>,
    mut clock: Box<dyn Clock + Send>,
    config: OwnerConfig,
    mut commands: BoundedReceiver<ControllerCommand>,
    mut safety: SafetyReceiver,
    telemetry: LatestPublisher<RawTelemetry>,
    shared: Arc<SharedState>,
) {
    let mut io = None;
    let mut next_reconnect = Instant::now();
    let mut force_zero_on_connect = true;
    let mut pending_stop = None;
    let mut pending_estop = None;
    let mut applied_sequence = 0_u64;
    let mut telemetry_sequence = Sequence::new(1).expect("one is non-zero");
    let mut telemetry_buffer = Vec::new();

    while !shared.shutdown.load(Ordering::Acquire) {
        if let Ok(Some(signal)) = safety.try_recv() {
            match signal.kind {
                SafetyKind::EStop => {
                    shared.estopped.store(true, Ordering::Release);
                    pending_estop = Some(signal);
                }
                SafetyKind::Stop => {
                    if shared.estopped.load(Ordering::Acquire) {
                        if let Some(estop) = *lock(&shared.last_estop_receipt) {
                            record_safety_receipt(
                                shared.as_ref(),
                                receipt_covered_by(signal, estop),
                            );
                        } else {
                            pending_stop = Some(signal);
                        }
                    } else {
                        pending_stop = Some(signal);
                    }
                }
            }
            supersede_pending(&mut commands, &mut *clock, &shared);
        }

        ensure_connected(
            &mut *factory,
            &mut io,
            &mut next_reconnect,
            config.reconnect_interval,
            &shared,
        );

        if io.is_some() && force_zero_on_connect {
            match write_frame(io.as_mut().unwrap(), &encode_stop()) {
                Ok(()) => {
                    applied_sequence = applied_sequence.saturating_add(1);
                    shared.metrics.writes.fetch_add(1, Ordering::Relaxed);
                    force_zero_on_connect = false;
                }
                Err(error) => {
                    write_failed(&mut io, &error, &shared);
                    next_reconnect = Instant::now() + config.reconnect_interval;
                }
            }
            thread::sleep(config.poll_interval);
            continue;
        }

        if let Some(signal) = pending_estop.or(pending_stop) {
            let Some(stream) = io.as_mut() else {
                thread::sleep(config.poll_interval);
                continue;
            };
            match write_frame(stream, &encode_stop()) {
                Ok(()) => {
                    applied_sequence = applied_sequence.saturating_add(1);
                    shared.metrics.writes.fetch_add(1, Ordering::Relaxed);
                    let receipt = SafetyReceipt {
                        kind: signal.kind,
                        first_request_sequence: signal.first_sequence,
                        through_request_sequence: signal.through_sequence,
                        coalesced: signal.coalesced,
                        applied_sequence,
                        applied_at: clock.now(),
                        verified_zero: true,
                    };
                    record_safety_receipt(&shared, receipt);
                    match signal.kind {
                        SafetyKind::EStop => pending_estop = None,
                        SafetyKind::Stop => pending_stop = None,
                    }
                }
                Err(error) => {
                    write_failed(&mut io, &error, &shared);
                    force_zero_on_connect = true;
                    next_reconnect = Instant::now() + config.reconnect_interval;
                }
            }
            thread::sleep(config.poll_interval);
            continue;
        }

        if let Some(command) = commands.try_recv() {
            process_command(
                command,
                &mut io,
                &mut *clock,
                config,
                &shared,
                &mut applied_sequence,
                &mut force_zero_on_connect,
            );
        }

        if let Some(stream) = io.as_mut() {
            if let Err(error) = read_telemetry(
                stream,
                &mut *clock,
                &telemetry,
                &mut telemetry_sequence,
                &mut telemetry_buffer,
                config.maximum_telemetry_line_bytes,
                &shared,
            ) {
                disconnect(&mut io, &error, &shared);
                force_zero_on_connect = true;
                next_reconnect = Instant::now() + config.reconnect_interval;
            }
        }
        thread::sleep(config.poll_interval);
    }
    supersede_pending(&mut commands, &mut *clock, &shared);
    shared.connected.store(false, Ordering::Release);
}

fn record_safety_receipt(shared: &SharedState, receipt: SafetyReceipt) {
    let target = match receipt.kind {
        SafetyKind::Stop => &shared.last_stop_receipt,
        SafetyKind::EStop => &shared.last_estop_receipt,
    };
    let mut stored = lock(target);
    *stored = Some(match *stored {
        Some(previous) => SafetyReceipt {
            kind: receipt.kind,
            first_request_sequence: previous
                .first_request_sequence
                .min(receipt.first_request_sequence),
            through_request_sequence: previous
                .through_request_sequence
                .max(receipt.through_request_sequence),
            coalesced: previous
                .through_request_sequence
                .max(receipt.through_request_sequence)
                .saturating_sub(
                    previous
                        .first_request_sequence
                        .min(receipt.first_request_sequence),
                ),
            applied_sequence: receipt.applied_sequence,
            applied_at: receipt.applied_at,
            verified_zero: previous.verified_zero && receipt.verified_zero,
        },
        None => receipt,
    });
}

fn receipt_covered_by(signal: SafetySignal, applied: SafetyReceipt) -> SafetyReceipt {
    SafetyReceipt {
        kind: signal.kind,
        first_request_sequence: signal.first_sequence,
        through_request_sequence: signal.through_sequence,
        coalesced: signal.coalesced,
        applied_sequence: applied.applied_sequence,
        applied_at: applied.applied_at,
        verified_zero: applied.verified_zero,
    }
}

fn supersede_pending(
    commands: &mut BoundedReceiver<ControllerCommand>,
    clock: &mut dyn Clock,
    shared: &SharedState,
) {
    while let Some(command) = commands.try_recv() {
        shared
            .metrics
            .superseded_by_safety
            .fetch_add(1, Ordering::Relaxed);
        reply_rejected(command, AckOutcome::SupersededBySafety, clock.now(), None);
    }
}

fn ensure_connected(
    factory: &mut dyn ControllerIoFactory,
    io: &mut Option<Box<dyn ControllerIo>>,
    next_reconnect: &mut Instant,
    reconnect_interval: Duration,
    shared: &SharedState,
) {
    if io.is_some() || Instant::now() < *next_reconnect {
        return;
    }
    match factory.open() {
        Ok(stream) => {
            *io = Some(stream);
            shared.connected.store(true, Ordering::Release);
            shared.metrics.reconnects.fetch_add(1, Ordering::Relaxed);
            *lock(&shared.last_error) = None;
        }
        Err(error) => {
            *next_reconnect = Instant::now() + reconnect_interval;
            *lock(&shared.last_error) = Some(error.to_string().into_boxed_str());
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn process_command(
    command: ControllerCommand,
    io: &mut Option<Box<dyn ControllerIo>>,
    clock: &mut dyn Clock,
    config: OwnerConfig,
    shared: &SharedState,
    applied_sequence: &mut u64,
    force_zero_on_connect: &mut bool,
) {
    let Some(stream) = io.as_mut() else {
        reply_rejected(command, AckOutcome::Disconnected, clock.now(), None);
        return;
    };
    let (frame, command_id, evidence_id, is_zero, reply) = match command {
        ControllerCommand::Drive { command, reply } => {
            if shared.estopped.load(Ordering::Acquire) && !command.command().is_stop() {
                let ack = CommandAck {
                    command_id: command.command_id(),
                    evidence_id: Some(command.evidence_id()),
                    outcome: AckOutcome::EStopped,
                    applied_sequence: None,
                    at: clock.now(),
                    verified_zero: false,
                    detail: None,
                };
                let _ = reply.send(ack);
                return;
            }
            (
                encode_drive(*command.command(), config.drive_invert, config.drive_swap),
                command.command_id(),
                Some(command.evidence_id()),
                command.command().is_stop(),
                reply,
            )
        }
        ControllerCommand::Gimbal { command, reply } => {
            (encode_gimbal(command), command.id, None, false, reply)
        }
        ControllerCommand::RequestTelemetry { id, reply } => {
            (encode_telemetry_request(), id, None, false, reply)
        }
        ControllerCommand::EnableTelemetry { id, reply } => {
            (encode_telemetry_enable(), id, None, false, reply)
        }
    };
    match write_frame(stream, &frame) {
        Ok(()) => {
            *applied_sequence = applied_sequence.saturating_add(1);
            shared.metrics.writes.fetch_add(1, Ordering::Relaxed);
            let _ = reply.send(CommandAck {
                command_id,
                evidence_id,
                outcome: AckOutcome::Applied,
                applied_sequence: Some(*applied_sequence),
                at: clock.now(),
                verified_zero: is_zero,
                detail: None,
            });
        }
        Err(error) => {
            let at = clock.now();
            let detail = error.to_string().into_boxed_str();
            let _ = reply.send(CommandAck {
                command_id,
                evidence_id,
                outcome: AckOutcome::IoFailed,
                applied_sequence: None,
                at,
                verified_zero: false,
                detail: Some(detail),
            });
            write_failed(io, &error, shared);
            *force_zero_on_connect = true;
        }
    }
}

fn reply_rejected(
    command: ControllerCommand,
    outcome: AckOutcome,
    at: MonotonicNanos,
    detail: Option<Box<str>>,
) {
    let (command_id, evidence_id, reply) = match command {
        ControllerCommand::Drive { command, reply } => {
            (command.command_id(), Some(command.evidence_id()), reply)
        }
        ControllerCommand::Gimbal { command, reply } => (command.id, None, reply),
        ControllerCommand::RequestTelemetry { id, reply } => (id, None, reply),
        ControllerCommand::EnableTelemetry { id, reply } => (id, None, reply),
    };
    let _ = reply.send(CommandAck {
        command_id,
        evidence_id,
        outcome,
        applied_sequence: None,
        at,
        verified_zero: false,
        detail,
    });
}

fn disconnect(io: &mut Option<Box<dyn ControllerIo>>, error: &io::Error, shared: &SharedState) {
    *io = None;
    shared.connected.store(false, Ordering::Release);
    shared.metrics.disconnects.fetch_add(1, Ordering::Relaxed);
    *lock(&shared.last_error) = Some(error.to_string().into_boxed_str());
}

fn write_failed(io: &mut Option<Box<dyn ControllerIo>>, error: &io::Error, shared: &SharedState) {
    shared
        .metrics
        .write_failures
        .fetch_add(1, Ordering::Relaxed);
    disconnect(io, error, shared);
}

fn write_frame(stream: &mut Box<dyn ControllerIo>, frame: &[u8]) -> io::Result<()> {
    stream.write_all(frame)?;
    stream.flush()
}

#[allow(clippy::too_many_arguments)]
fn read_telemetry(
    stream: &mut Box<dyn ControllerIo>,
    clock: &mut dyn Clock,
    publisher: &LatestPublisher<RawTelemetry>,
    sequence: &mut Sequence,
    buffer: &mut Vec<u8>,
    maximum_line_bytes: usize,
    shared: &SharedState,
) -> io::Result<()> {
    let mut chunk = [0_u8; 512];
    match stream.read(&mut chunk) {
        Ok(0) => {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "controller EOF",
            ))
        }
        Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            return Ok(())
        }
        Err(error) => return Err(error),
    }
    while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
        let mut bytes = buffer.drain(..=newline).collect::<Vec<_>>();
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        if bytes.is_empty() {
            continue;
        }
        let valid = bytes.len() <= maximum_line_bytes
            && serde_json::from_slice::<serde_json::Value>(&bytes)
                .is_ok_and(|value| value.is_object());
        if !valid {
            shared
                .metrics
                .malformed_telemetry
                .fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let json = String::from_utf8(bytes)
            .expect("valid JSON is UTF-8")
            .into_boxed_str();
        let sample = Stamped::new(clock.now(), *sequence, RawTelemetry { json });
        let _ = publisher.publish(sample);
        shared
            .metrics
            .telemetry_frames
            .fetch_add(1, Ordering::Relaxed);
        *sequence = match sequence.next() {
            Ok(next) => next,
            Err(_) => return Err(io::Error::other("telemetry sequence exhausted")),
        };
    }
    if buffer.len() > maximum_line_bytes {
        buffer.clear();
        shared
            .metrics
            .malformed_telemetry
            .fetch_add(1, Ordering::Relaxed);
    }
    Ok(())
}

pub fn encode_drive(command: DifferentialDrive, invert: bool, swap: bool) -> Vec<u8> {
    let (mut left, mut right) = if swap {
        (command.right.get(), command.left.get())
    } else {
        (command.left.get(), command.right.get())
    };
    if invert {
        left = -left;
        right = -right;
    }
    if left == 0.0 {
        left = 0.0;
    }
    if right == 0.0 {
        right = 0.0;
    }
    json_line(json!({"T": 1, "L": left, "R": right}))
}

pub fn encode_stop() -> Vec<u8> {
    encode_drive(DifferentialDrive::STOP, false, false)
}

pub fn encode_gimbal(command: GimbalCommand) -> Vec<u8> {
    json_line(json!({
        "T": 133,
        "X": command.pan_degrees,
        "Y": command.tilt_degrees,
        "SPD": command.speed,
        "ACC": command.acceleration,
    }))
}

pub fn encode_telemetry_request() -> Vec<u8> {
    json_line(json!({"T": 130}))
}

pub fn encode_telemetry_enable() -> Vec<u8> {
    let mut bytes = json_line(json!({"T": 142, "cmd": 100}));
    bytes.extend(json_line(json!({"T": 131, "cmd": 1})));
    bytes
}

fn json_line(value: serde_json::Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(&value).expect("finite checked values serialize");
    bytes.push(b'\n');
    bytes
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use leash_core::{
        ActivityId, BeliefId, Candidate, ControlInput, ControlKernel, ControlKernelConfig,
        DurationNanos, Meters, MetersPerSecond, NormalizedDrive, OperatorId, ProducerEpoch,
        ProposalId, SafetyGate, SafetyState,
    };
    use leash_ros2::{
        cmd_vel_to_proposal, Nav2DispatchAcceptance, Nav2Kinematics, Nav2ProposalDispatcher,
        Nav2SourceState, Nav2StopReason, Nav2Unavailable, Twist, Vector3,
    };
    use leash_runtime::{
        read_evidence_records, CpuSafetySupervisor, EvidenceDecision, EvidenceJournal,
        EvidenceJournalConfig, SupervisorConfig,
    };

    use super::*;

    #[derive(Clone)]
    struct TestClock(Arc<AtomicU64>);

    impl Clock for TestClock {
        fn now(&mut self) -> MonotonicNanos {
            MonotonicNanos::new(self.0.fetch_add(1, Ordering::Relaxed))
        }
    }

    #[derive(Default)]
    struct Transcript {
        writes: Vec<u8>,
        reads: VecDeque<io::Result<Vec<u8>>>,
        maximum_write: usize,
        fail_writes: bool,
    }

    struct FakeIo(Arc<Mutex<Transcript>>);

    impl Read for FakeIo {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            let mut transcript = lock(&self.0);
            match transcript.reads.pop_front() {
                Some(Ok(bytes)) => {
                    let count = bytes.len().min(output.len());
                    output[..count].copy_from_slice(&bytes[..count]);
                    if count < bytes.len() {
                        transcript.reads.push_front(Ok(bytes[count..].to_vec()));
                    }
                    Ok(count)
                }
                Some(Err(error)) => Err(error),
                None => Err(io::Error::from(io::ErrorKind::WouldBlock)),
            }
        }
    }

    impl Write for FakeIo {
        fn write(&mut self, input: &[u8]) -> io::Result<usize> {
            let mut transcript = lock(&self.0);
            if transcript.fail_writes {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "injected write failure",
                ));
            }
            let count = if transcript.maximum_write == 0 {
                input.len()
            } else {
                input.len().min(transcript.maximum_write)
            };
            transcript.writes.extend_from_slice(&input[..count]);
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn config() -> OwnerConfig {
        OwnerConfig {
            command_capacity: 4,
            poll_interval: Duration::from_millis(1),
            reconnect_interval: Duration::from_millis(2),
            maximum_telemetry_line_bytes: 256,
            drive_invert: false,
            drive_swap: false,
        }
    }

    fn command_id(sequence: u64) -> CommandId {
        CommandId::new(
            ProducerEpoch::new(7).unwrap(),
            Sequence::new(sequence).unwrap(),
        )
    }

    fn authorized(sequence: u64, left: f64, right: f64) -> Authorized<DifferentialDrive> {
        let mut gate = SafetyGate::new(ProducerEpoch::new(8).unwrap());
        gate.set_state(SafetyState::Ready);
        gate.authorize(
            Candidate::new(
                command_id(sequence),
                MonotonicNanos::new(1),
                MonotonicNanos::new(100),
                DifferentialDrive::new(
                    NormalizedDrive::new(left).unwrap(),
                    NormalizedDrive::new(right).unwrap(),
                ),
            ),
            MonotonicNanos::new(2),
        )
        .unwrap()
    }

    fn owner(transcript: Arc<Mutex<Transcript>>) -> ControllerOwner {
        ControllerOwner::spawn(
            Box::new(
                move || Ok(Box::new(FakeIo(Arc::clone(&transcript))) as Box<dyn ControllerIo>),
            ),
            Box::new(TestClock(Arc::new(AtomicU64::new(10)))),
            config(),
        )
        .unwrap()
    }

    fn wait_connected(handle: &ControllerHandle) {
        for _ in 0..100 {
            if handle.status().connected && handle.status().metrics.writes >= 1 {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("owner did not connect")
    }

    #[test]
    fn zero_encoding_survives_every_wiring_transform() {
        for invert in [false, true] {
            for swap in [false, true] {
                let encoded = encode_drive(DifferentialDrive::STOP, invert, swap);
                let value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
                assert_eq!(value["T"], 1);
                assert_eq!(value["L"], 0.0);
                assert_eq!(value["R"], 0.0);
                assert_eq!(encoded.last(), Some(&b'\n'));
            }
        }
    }

    #[test]
    fn telemetry_enable_preserves_the_two_legacy_owner_frames() {
        let encoded = String::from_utf8(encode_telemetry_enable()).unwrap();
        let frames = encoded
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            frames,
            vec![json!({"T": 142, "cmd": 100}), json!({"T": 131, "cmd": 1})]
        );
    }

    #[test]
    fn one_owner_serializes_partial_writes_and_acknowledges_identity() {
        let transcript = Arc::new(Mutex::new(Transcript {
            maximum_write: 3,
            ..Transcript::default()
        }));
        let owner = owner(Arc::clone(&transcript));
        let handle = owner.handle();
        wait_connected(&handle);
        let drive = handle.submit_drive(authorized(1, 0.5, -0.25)).unwrap();
        let gimbal = handle
            .submit_gimbal(GimbalCommand::new(command_id(2), 10.0, 5.0, 100, 10).unwrap())
            .unwrap();
        let drive_ack = drive.wait().unwrap();
        let gimbal_ack = gimbal.wait().unwrap();
        assert_eq!(drive_ack.command_id, command_id(1));
        assert!(drive_ack.evidence_id.is_some());
        assert_eq!(drive_ack.outcome, AckOutcome::Applied);
        assert_eq!(gimbal_ack.command_id, command_id(2));
        let writes = String::from_utf8(lock(&transcript).writes.clone()).unwrap();
        for line in writes.lines() {
            serde_json::from_str::<serde_json::Value>(line).unwrap();
        }
        assert_eq!(writes.lines().count(), 3);
    }

    #[test]
    fn estop_preempts_and_latches_without_dropping_receipt_counts() {
        let transcript = Arc::new(Mutex::new(Transcript::default()));
        let owner = owner(Arc::clone(&transcript));
        let handle = owner.handle();
        wait_connected(&handle);
        let safety = handle.safety();
        safety.stop().unwrap();
        safety.estop().unwrap();
        for _ in 0..100 {
            let status = handle.status();
            if status.last_stop_receipt.is_some() && status.last_estop_receipt.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        let status = handle.status();
        let receipt = status.last_safety_receipt.unwrap();
        assert_eq!(receipt.kind, SafetyKind::EStop);
        assert!(receipt.verified_zero);
        assert!(status.last_stop_receipt.unwrap().verified_zero);
        assert!(status.estopped);
        assert!(matches!(
            handle.submit_drive(authorized(3, 0.2, 0.2)),
            Err(SubmitError::EStopped)
        ));
        assert!(!handle.reset_estop_latch(false));
        assert!(handle.status().estopped);
        assert!(handle.reset_estop_latch(true));
        assert!(!handle.status().estopped);
    }

    #[test]
    fn stop_after_estop_reset_writes_a_fresh_verified_zero() {
        let transcript = Arc::new(Mutex::new(Transcript::default()));
        let owner = owner(Arc::clone(&transcript));
        let handle = owner.handle();
        wait_connected(&handle);
        let safety = handle.safety();

        safety.estop().unwrap();
        for _ in 0..100 {
            if handle.status().last_estop_receipt.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        let estop = handle.status().last_estop_receipt.unwrap();
        assert!(estop.verified_zero);
        assert!(handle.reset_estop_latch(true));

        let drive = handle.submit_drive(authorized(3, 0.2, -0.2)).unwrap();
        let drive_ack = drive.wait().unwrap();
        assert_eq!(drive_ack.outcome, AckOutcome::Applied);
        let drive_sequence = drive_ack.applied_sequence.unwrap();
        assert!(drive_sequence > estop.applied_sequence);

        safety.stop().unwrap();
        for _ in 0..100 {
            if handle.status().last_stop_receipt.is_some_and(|receipt| {
                receipt.applied_sequence > drive_sequence && receipt.verified_zero
            }) {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        let status = handle.status();
        let stop = status.last_stop_receipt.unwrap();
        assert!(stop.applied_sequence > drive_sequence);
        assert!(stop.verified_zero);
        assert_eq!(status.last_safety_receipt, Some(stop));

        let writes = String::from_utf8(lock(&transcript).writes.clone()).unwrap();
        let last =
            serde_json::from_str::<serde_json::Value>(writes.lines().last().unwrap()).unwrap();
        assert_eq!(last["L"], 0.0);
        assert_eq!(last["R"], 0.0);
    }

    #[test]
    fn actuation_port_observes_cumulative_safety_receipt_ranges() {
        let transcript = Arc::new(Mutex::new(Transcript::default()));
        let owner = owner(transcript);
        let handle = owner.handle();
        wait_connected(&handle);
        let mut port = WaveshareActuationPort::new(handle);
        assert_eq!(port.request_safety(SafetyKind::Stop).unwrap(), 1);
        for _ in 0..100 {
            if port.handle.status().last_stop_receipt.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(port.request_safety(SafetyKind::Stop).unwrap(), 2);
        for _ in 0..100 {
            if port
                .handle
                .status()
                .last_stop_receipt
                .is_some_and(|receipt| receipt.through_request_sequence == 2)
            {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        let acknowledgement = port.try_safety_acknowledgement().unwrap().unwrap();
        assert_eq!(acknowledgement.kind, SafetyKind::Stop);
        assert_eq!(acknowledgement.first_request_sequence, 1);
        assert_eq!(acknowledgement.through_request_sequence, 2);
        assert!(acknowledgement.verified_zero);
        assert_eq!(port.try_safety_acknowledgement().unwrap(), None);
    }

    #[test]
    fn corrupt_telemetry_is_counted_and_latest_valid_frame_wins() {
        let transcript = Arc::new(Mutex::new(Transcript {
            reads: VecDeque::from([Ok(b"not-json\n{\"T\":1001,\"v\":1}\n".to_vec())]),
            ..Transcript::default()
        }));
        let mut owner = owner(transcript);
        let handle = owner.handle();
        wait_connected(&handle);
        let mut sample = None;
        for _ in 0..100 {
            sample = owner.take_telemetry();
            if sample.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(sample.unwrap().value.json.as_ref(), "{\"T\":1001,\"v\":1}");
        assert_eq!(handle.status().metrics.malformed_telemetry, 1);
    }

    #[test]
    fn write_failure_disconnects_and_never_claims_zero_verification() {
        let transcript = Arc::new(Mutex::new(Transcript::default()));
        let owner = owner(Arc::clone(&transcript));
        let handle = owner.handle();
        wait_connected(&handle);
        lock(&transcript).fail_writes = true;
        let ack = handle
            .submit_drive(authorized(4, 0.0, 0.0))
            .unwrap()
            .wait()
            .unwrap();
        assert_eq!(ack.outcome, AckOutcome::IoFailed);
        assert!(!ack.verified_zero);
        assert!(ack.applied_sequence.is_none());
    }

    #[test]
    fn invalid_configuration_and_gimbal_values_fail_closed() {
        assert_eq!(
            config()
                .validate()
                .map(|mut config| {
                    config.command_capacity = 0;
                    config
                })
                .and_then(OwnerConfig::validate),
            Err(ConfigError::ZeroCommandCapacity)
        );
        assert_eq!(
            GimbalCommand::new(command_id(1), f64::NAN, 0.0, 1, 1),
            Err(GimbalError::NonFinite)
        );
    }

    #[test]
    fn queue_saturation_rejects_newest_without_blocking_the_caller() {
        let (started_tx, started_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::sync_channel(0);
        let transcript = Arc::new(Mutex::new(Transcript::default()));
        let factory_transcript = Arc::clone(&transcript);
        let owner = ControllerOwner::spawn(
            Box::new(move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(Box::new(FakeIo(Arc::clone(&factory_transcript))) as Box<dyn ControllerIo>)
            }),
            Box::new(TestClock(Arc::new(AtomicU64::new(10)))),
            OwnerConfig {
                command_capacity: 1,
                ..config()
            },
        )
        .unwrap();
        let handle = owner.handle();
        started_rx.recv().unwrap();
        let first = handle.submit_drive(authorized(1, 0.1, 0.1)).unwrap();
        assert!(matches!(
            handle.submit_drive(authorized(2, 0.2, 0.2)),
            Err(SubmitError::Full)
        ));
        release_tx.send(()).unwrap();
        assert_eq!(first.wait().unwrap().outcome, AckOutcome::Applied);
        assert_eq!(handle.status().metrics.rejected_full, 1);
    }

    #[test]
    fn disconnect_reconnects_and_applies_zero_before_normal_work() {
        let transcript = Arc::new(Mutex::new(Transcript {
            fail_writes: true,
            ..Transcript::default()
        }));
        let owner = owner(Arc::clone(&transcript));
        let handle = owner.handle();
        for _ in 0..100 {
            if handle.status().metrics.disconnects >= 1 {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        lock(&transcript).fail_writes = false;
        wait_connected(&handle);
        let status = handle.status();
        assert!(status.metrics.reconnects >= 2);
        let first_line = String::from_utf8(lock(&transcript).writes.clone()).unwrap();
        let first: serde_json::Value =
            serde_json::from_str(first_line.lines().next().unwrap()).unwrap();
        assert_eq!(first["L"], 0.0);
        assert_eq!(first["R"], 0.0);
    }

    #[test]
    fn owner_thread_panic_is_contained_and_observable() {
        let owner = ControllerOwner::spawn(
            Box::new(|| -> io::Result<Box<dyn ControllerIo>> { panic!("injected factory panic") }),
            Box::new(TestClock(Arc::new(AtomicU64::new(10)))),
            config(),
        )
        .unwrap();
        let handle = owner.handle();
        for _ in 0..100 {
            if handle.status().metrics.worker_panics == 1 {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        let status = handle.status();
        assert_eq!(status.metrics.worker_panics, 1);
        assert_eq!(
            status.last_error.as_deref(),
            Some("controller owner thread panicked")
        );
        assert!(matches!(
            handle.submit_drive(authorized(1, 0.1, 0.1)),
            Err(SubmitError::Closed)
        ));
    }

    #[test]
    fn cpu_supervisor_is_the_only_motion_path_into_the_owner() {
        let evidence_path = std::env::temp_dir().join(format!(
            "leash-nav2-evidence-{}-{}.journal",
            std::process::id(),
            command_id(1).sequence.get()
        ));
        let journal = EvidenceJournal::open(EvidenceJournalConfig {
            path: evidence_path.clone(),
            normal_capacity: 64,
            priority_capacity: 16,
            maximum_records: None,
        })
        .unwrap();
        let transcript = Arc::new(Mutex::new(Transcript::default()));
        let owner = owner(Arc::clone(&transcript));
        let controller = owner.handle();
        wait_connected(&controller);
        let supervisor = CpuSafetySupervisor::spawn_with_evidence(
            ControlKernel::new(ControlKernelConfig {
                command_epoch: ProducerEpoch::new(41).unwrap(),
                evidence_epoch: ProducerEpoch::new(42).unwrap(),
                deadman: DurationNanos::from_millis(50).unwrap(),
            }),
            WaveshareActuationPort::new(controller.clone()),
            Box::new(TestClock(Arc::new(AtomicU64::new(100)))),
            SupervisorConfig {
                proposal_capacity: 4,
                tick_period: Duration::from_millis(1),
            },
            journal.producer(),
        )
        .unwrap();
        let safety = supervisor.handle();
        safety
            .submit(ControlInput::UpdateEvidence {
                obstacle_blocked: false,
                lidar_fresh: true,
                localization_fresh: true,
            })
            .unwrap()
            .wait()
            .unwrap();
        safety
            .submit(ControlInput::Authorize {
                operator: OperatorId::new("integration-test").unwrap(),
                expires_at: MonotonicNanos::new(1_000_000),
            })
            .unwrap()
            .wait()
            .unwrap();
        let ros_proposal = cmd_vel_to_proposal(
            Twist {
                linear: Vector3 {
                    x: 0.2,
                    ..Vector3::default()
                },
                ..Twist::default()
            },
            Nav2Kinematics::new(
                Meters::new(0.4).unwrap(),
                MetersPerSecond::new(1.0).unwrap(),
            )
            .unwrap(),
            ProposalId::new(ProducerEpoch::new(51).unwrap(), Sequence::new(1).unwrap()),
            ActivityId::new(ProducerEpoch::new(52).unwrap(), Sequence::new(1).unwrap()),
            MonotonicNanos::new(100),
            MonotonicNanos::new(1_000_000),
            10,
            Box::new([BeliefId::new(
                ProducerEpoch::new(53).unwrap(),
                Sequence::new(1).unwrap(),
            )]) as Box<[BeliefId]>,
        )
        .unwrap();
        let dispatcher = Nav2ProposalDispatcher::new(safety.clone());
        let dispatch = dispatcher
            .dispatch(
                ros_proposal,
                Nav2SourceState {
                    connected: true,
                    goal_active: true,
                    last_localization: MonotonicNanos::new(100),
                    last_scan: MonotonicNanos::new(100),
                    maximum_age: DurationNanos::from_millis(100).unwrap(),
                },
                MonotonicNanos::new(100),
            )
            .unwrap();
        let Nav2DispatchAcceptance::Transition(ticket) = dispatch else {
            panic!("fresh ROS velocity must enter the CPU safety transition lane")
        };
        ticket.wait().unwrap();
        for _ in 0..100 {
            if controller.status().metrics.writes >= 2 {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        let disconnected_proposal = cmd_vel_to_proposal(
            Twist {
                linear: Vector3 {
                    x: 0.4,
                    ..Vector3::default()
                },
                ..Twist::default()
            },
            Nav2Kinematics::new(
                Meters::new(0.4).unwrap(),
                MetersPerSecond::new(1.0).unwrap(),
            )
            .unwrap(),
            ProposalId::new(ProducerEpoch::new(51).unwrap(), Sequence::new(2).unwrap()),
            ActivityId::new(ProducerEpoch::new(52).unwrap(), Sequence::new(1).unwrap()),
            MonotonicNanos::new(101),
            MonotonicNanos::new(1_000_000),
            10,
            Box::new([BeliefId::new(
                ProducerEpoch::new(53).unwrap(),
                Sequence::new(2).unwrap(),
            )]) as Box<[BeliefId]>,
        )
        .unwrap();
        let disconnected = dispatcher
            .dispatch(
                disconnected_proposal,
                Nav2SourceState {
                    connected: false,
                    goal_active: true,
                    last_localization: MonotonicNanos::new(101),
                    last_scan: MonotonicNanos::new(101),
                    maximum_age: DurationNanos::from_millis(100).unwrap(),
                },
                MonotonicNanos::new(101),
            )
            .unwrap();
        assert!(matches!(
            disconnected,
            Nav2DispatchAcceptance::SafetyStop {
                reason: Nav2StopReason::SourceUnavailable(Nav2Unavailable::Disconnected),
                ..
            }
        ));
        for _ in 0..100 {
            if controller.status().last_stop_receipt.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(controller.status().last_stop_receipt.unwrap().verified_zero);
        safety.estop().unwrap();
        for _ in 0..100 {
            if controller.status().last_estop_receipt.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            controller
                .status()
                .last_estop_receipt
                .unwrap()
                .verified_zero
        );
        let frames = String::from_utf8(lock(&transcript).writes.clone()).unwrap();
        assert!(frames.lines().any(|line| {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            value["L"] == 0.2 && value["R"] == 0.2
        }));
        supervisor.shutdown();
        let journal_status = journal.shutdown();
        assert!(journal_status.writer_fault.is_none());
        let records = read_evidence_records(&evidence_path).unwrap();
        let decisions = records
            .iter()
            .map(|record| record.decision)
            .collect::<Vec<_>>();
        for required in [
            EvidenceDecision::ProposalAccepted,
            EvidenceDecision::CommandAccepted,
            EvidenceDecision::AcknowledgementApplied,
            EvidenceDecision::ZeroRequested,
            EvidenceDecision::ZeroVerified,
        ] {
            assert!(
                decisions.contains(&required),
                "durable Nav2 chain is missing {required:?}: {decisions:?}"
            );
        }
        assert!(records.iter().any(|record| {
            record.decision == EvidenceDecision::CommandAccepted
                && record.proposal_sequence.is_some()
                && record.command_id.is_some()
                && record.evidence_id.is_some()
        }));
        assert!(records.iter().any(|record| {
            record.decision == EvidenceDecision::ZeroVerified
                && record.acknowledgement.is_some_and(|acknowledgement| {
                    acknowledgement.through_request_sequence.is_some()
                })
        }));
        std::fs::remove_file(evidence_path).unwrap();
    }
}
