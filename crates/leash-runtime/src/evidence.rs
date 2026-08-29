use std::{
    collections::VecDeque,
    fmt,
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Condvar, Mutex, MutexGuard,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use leash_core::{CommandId, EvidenceId, MonotonicNanos, ProducerEpoch, Sequence};

const FILE_MAGIC: [u8; 8] = *b"LEASHEV1";
const FILE_HEADER_LEN: usize = 16;
const FRAME_MAGIC: [u8; 4] = *b"EVR1";
const PAYLOAD_LEN: usize = 104;
const FRAME_LEN: usize = 8 + PAYLOAD_LEN + 8;
const FORMAT_VERSION: u32 = 1;

const HAS_PROPOSAL: u16 = 1 << 0;
const HAS_COMMAND: u16 = 1 << 1;
const HAS_EVIDENCE: u16 = 1 << 2;
const HAS_ACKNOWLEDGEMENT: u16 = 1 << 3;
const HAS_ACK_FIRST_REQUEST: u16 = 1 << 4;
const HAS_ACK_THROUGH_REQUEST: u16 = 1 << 5;
const HAS_ACK_APPLIED_SEQUENCE: u16 = 1 << 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceSource {
    ProposalIngress,
    CpuSafetySupervisor,
    Actuator,
    PersistenceOwner,
}

impl EvidenceSource {
    const fn tag(self) -> u8 {
        match self {
            Self::ProposalIngress => 1,
            Self::CpuSafetySupervisor => 2,
            Self::Actuator => 3,
            Self::PersistenceOwner => 4,
        }
    }

    fn from_tag(tag: u8) -> Result<Self, EvidenceOpenError> {
        match tag {
            1 => Ok(Self::ProposalIngress),
            2 => Ok(Self::CpuSafetySupervisor),
            3 => Ok(Self::Actuator),
            4 => Ok(Self::PersistenceOwner),
            _ => Err(EvidenceOpenError::CorruptRecord("unknown evidence source")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceDecision {
    ProposalAccepted,
    ProposalRejected,
    CommandAccepted,
    CommandRejected,
    CommandSuperseded,
    ZeroRequested,
    AcknowledgementApplied,
    AcknowledgementFailed,
    ZeroVerified,
    JournalSaturated,
    StorageFull,
    TornTailRecovered,
}

impl EvidenceDecision {
    const fn tag(self) -> u8 {
        match self {
            Self::ProposalAccepted => 1,
            Self::ProposalRejected => 2,
            Self::CommandAccepted => 3,
            Self::CommandRejected => 4,
            Self::CommandSuperseded => 5,
            Self::ZeroRequested => 6,
            Self::AcknowledgementApplied => 7,
            Self::AcknowledgementFailed => 8,
            Self::ZeroVerified => 9,
            Self::JournalSaturated => 10,
            Self::StorageFull => 11,
            Self::TornTailRecovered => 12,
        }
    }

    fn from_tag(tag: u8) -> Result<Self, EvidenceOpenError> {
        match tag {
            1 => Ok(Self::ProposalAccepted),
            2 => Ok(Self::ProposalRejected),
            3 => Ok(Self::CommandAccepted),
            4 => Ok(Self::CommandRejected),
            5 => Ok(Self::CommandSuperseded),
            6 => Ok(Self::ZeroRequested),
            7 => Ok(Self::AcknowledgementApplied),
            8 => Ok(Self::AcknowledgementFailed),
            9 => Ok(Self::ZeroVerified),
            10 => Ok(Self::JournalSaturated),
            11 => Ok(Self::StorageFull),
            12 => Ok(Self::TornTailRecovered),
            _ => Err(EvidenceOpenError::CorruptRecord(
                "unknown evidence decision",
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcknowledgementKind {
    Command,
    Safety,
}

impl AcknowledgementKind {
    const fn tag(self) -> u8 {
        match self {
            Self::Command => 1,
            Self::Safety => 2,
        }
    }

    fn from_tag(tag: u8) -> Result<Self, EvidenceOpenError> {
        match tag {
            1 => Ok(Self::Command),
            2 => Ok(Self::Safety),
            _ => Err(EvidenceOpenError::CorruptRecord(
                "unknown acknowledgement kind",
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcknowledgementIdentity {
    pub kind: AcknowledgementKind,
    pub first_request_sequence: Option<u64>,
    pub through_request_sequence: Option<u64>,
    pub applied_sequence: Option<u64>,
    pub at: MonotonicNanos,
}

impl AcknowledgementIdentity {
    pub const fn command(applied_sequence: Option<u64>, at: MonotonicNanos) -> Self {
        Self {
            kind: AcknowledgementKind::Command,
            first_request_sequence: None,
            through_request_sequence: None,
            applied_sequence,
            at,
        }
    }

    pub const fn safety(
        first_request_sequence: u64,
        through_request_sequence: u64,
        applied_sequence: Option<u64>,
        at: MonotonicNanos,
    ) -> Self {
        Self {
            kind: AcknowledgementKind::Safety,
            first_request_sequence: Some(first_request_sequence),
            through_request_sequence: Some(through_request_sequence),
            applied_sequence,
            at,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvidenceRecord {
    ordinal: u64,
    pub proposal_sequence: Option<u64>,
    pub command_id: Option<CommandId>,
    pub evidence_id: Option<EvidenceId>,
    pub source: EvidenceSource,
    pub at: MonotonicNanos,
    pub decision: EvidenceDecision,
    pub acknowledgement: Option<AcknowledgementIdentity>,
}

impl EvidenceRecord {
    pub const fn new(
        proposal_sequence: Option<u64>,
        command_id: Option<CommandId>,
        evidence_id: Option<EvidenceId>,
        source: EvidenceSource,
        at: MonotonicNanos,
        decision: EvidenceDecision,
        acknowledgement: Option<AcknowledgementIdentity>,
    ) -> Self {
        Self {
            ordinal: 0,
            proposal_sequence,
            command_id,
            evidence_id,
            source,
            at,
            decision,
            acknowledgement,
        }
    }

    pub const fn ordinal(self) -> u64 {
        self.ordinal
    }

    fn with_ordinal(mut self, ordinal: u64) -> Self {
        self.ordinal = ordinal;
        self
    }

    fn persistence(ordinal: u64, at: MonotonicNanos, decision: EvidenceDecision) -> Self {
        Self::new(
            None,
            None,
            None,
            EvidenceSource::PersistenceOwner,
            at,
            decision,
            None,
        )
        .with_ordinal(ordinal)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceJournalConfig {
    pub path: PathBuf,
    pub normal_capacity: usize,
    pub priority_capacity: usize,
    pub maximum_records: Option<u64>,
}

impl EvidenceJournalConfig {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            normal_capacity: 1_024,
            priority_capacity: 64,
            maximum_records: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvidenceRecoveryState {
    pub last_complete_record: Option<EvidenceRecord>,
    pub torn_tail_detected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceJournalStatus {
    pub normal_capacity: usize,
    pub normal_depth: usize,
    pub normal_high_watermark: usize,
    pub priority_capacity: usize,
    pub priority_depth: usize,
    pub priority_high_watermark: usize,
    pub durable_records: u64,
    pub last_durable_ordinal: u64,
    pub saturated: bool,
    pub storage_full: bool,
    pub closed: bool,
    pub writer_fault: Option<Box<str>>,
    pub recovery: EvidenceRecoveryState,
}

impl EvidenceJournalStatus {
    pub fn healthy(&self) -> bool {
        !self.saturated && !self.storage_full && !self.closed && self.writer_fault.is_none()
    }
}

#[derive(Debug)]
pub enum EvidenceOpenError {
    ZeroNormalCapacity,
    ZeroPriorityCapacity,
    MaximumRecordsTooSmall,
    InvalidHeader,
    CorruptRecord(&'static str),
    SequenceExhausted,
    Io(io::Error),
    Thread(String),
}

impl fmt::Display for EvidenceOpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroNormalCapacity => {
                formatter.write_str("evidence normal capacity must be positive")
            }
            Self::ZeroPriorityCapacity => {
                formatter.write_str("evidence priority capacity must be positive")
            }
            Self::MaximumRecordsTooSmall => {
                formatter.write_str("evidence maximum records must reserve a terminal record")
            }
            Self::InvalidHeader => formatter.write_str("invalid evidence journal header"),
            Self::CorruptRecord(message) => write!(formatter, "corrupt evidence record: {message}"),
            Self::SequenceExhausted => formatter.write_str("evidence record sequence exhausted"),
            Self::Io(error) => write!(formatter, "evidence journal I/O: {error}"),
            Self::Thread(error) => write!(formatter, "start evidence persistence owner: {error}"),
        }
    }
}

impl std::error::Error for EvidenceOpenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for EvidenceOpenError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceEnqueueError {
    Closed,
    Saturated,
    Full,
    SequenceExhausted,
}

impl fmt::Display for EvidenceEnqueueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => formatter.write_str("evidence persistence owner is closed"),
            Self::Saturated => formatter.write_str("evidence normal ingress is saturated"),
            Self::Full => formatter.write_str("evidence priority ingress is full"),
            Self::SequenceExhausted => formatter.write_str("evidence record sequence exhausted"),
        }
    }
}

impl std::error::Error for EvidenceEnqueueError {}

#[derive(Debug)]
struct IngressState {
    next_ordinal: u64,
    normal: VecDeque<EvidenceRecord>,
    priority: VecDeque<EvidenceRecord>,
    terminal: Option<EvidenceRecord>,
    normal_high_watermark: usize,
    priority_high_watermark: usize,
    saturated: bool,
    shutdown: bool,
}

#[derive(Debug)]
struct SharedState {
    normal_capacity: usize,
    priority_capacity: usize,
    ingress: Mutex<IngressState>,
    ready: Condvar,
    durable_ready: Condvar,
    durable_records: AtomicU64,
    last_durable_ordinal: AtomicU64,
    storage_full: AtomicBool,
    closed: AtomicBool,
    writer_fault: Mutex<Option<Box<str>>>,
    recovery: EvidenceRecoveryState,
    #[cfg(test)]
    writer_paused: AtomicBool,
}

#[derive(Clone, Debug)]
pub struct EvidenceProducer {
    shared: Arc<SharedState>,
}

impl EvidenceProducer {
    pub fn try_record(&self, record: EvidenceRecord) -> Result<u64, EvidenceEnqueueError> {
        self.enqueue(record, false)
    }

    pub fn try_record_priority(&self, record: EvidenceRecord) -> Result<u64, EvidenceEnqueueError> {
        self.enqueue(record, true)
    }

    fn enqueue(&self, record: EvidenceRecord, priority: bool) -> Result<u64, EvidenceEnqueueError> {
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(EvidenceEnqueueError::Closed);
        }
        let mut ingress = lock(&self.shared.ingress);
        if ingress.shutdown || self.shared.closed.load(Ordering::Acquire) {
            return Err(EvidenceEnqueueError::Closed);
        }
        if !priority && ingress.saturated {
            return Err(EvidenceEnqueueError::Saturated);
        }
        let ordinal = ingress.next_ordinal;
        ingress.next_ordinal = ordinal
            .checked_add(1)
            .ok_or(EvidenceEnqueueError::SequenceExhausted)?;
        if priority {
            if ingress.priority.len() == self.shared.priority_capacity {
                if !ingress.saturated {
                    ingress.saturated = true;
                    ingress.terminal = Some(EvidenceRecord::persistence(
                        ordinal,
                        record.at,
                        EvidenceDecision::JournalSaturated,
                    ));
                    self.shared.ready.notify_one();
                }
                return Err(EvidenceEnqueueError::Full);
            }
            ingress.priority.push_back(record.with_ordinal(ordinal));
            ingress.priority_high_watermark =
                ingress.priority_high_watermark.max(ingress.priority.len());
        } else if ingress.normal.len() == self.shared.normal_capacity {
            ingress.saturated = true;
            ingress.terminal = Some(EvidenceRecord::persistence(
                ordinal,
                record.at,
                EvidenceDecision::JournalSaturated,
            ));
            self.shared.ready.notify_one();
            return Err(EvidenceEnqueueError::Full);
        } else {
            ingress.normal.push_back(record.with_ordinal(ordinal));
            ingress.normal_high_watermark = ingress.normal_high_watermark.max(ingress.normal.len());
        }
        self.shared.ready.notify_one();
        Ok(ordinal)
    }

    pub fn status(&self) -> EvidenceJournalStatus {
        status(&self.shared)
    }

    pub fn healthy(&self) -> bool {
        let ingress = lock(&self.shared.ingress);
        !ingress.saturated
            && !self.shared.storage_full.load(Ordering::Acquire)
            && !self.shared.closed.load(Ordering::Acquire)
            && lock(&self.shared.writer_fault).is_none()
    }

    pub fn wait_durable(&self, ordinal: u64, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut ingress = lock(&self.shared.ingress);
        loop {
            if self.shared.last_durable_ordinal.load(Ordering::Acquire) >= ordinal {
                return true;
            }
            if self.shared.closed.load(Ordering::Acquire)
                || self.shared.storage_full.load(Ordering::Acquire)
                || lock(&self.shared.writer_fault).is_some()
            {
                return false;
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            let (next, timeout_result) = self
                .shared
                .durable_ready
                .wait_timeout(ingress, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            ingress = next;
            if timeout_result.timed_out() {
                return self.shared.last_durable_ordinal.load(Ordering::Acquire) >= ordinal;
            }
        }
    }
}

pub struct EvidenceJournal {
    producer: EvidenceProducer,
    worker: Option<JoinHandle<()>>,
}

impl EvidenceJournal {
    pub fn open(config: EvidenceJournalConfig) -> Result<Self, EvidenceOpenError> {
        Self::open_inner(config, false)
    }

    fn open_inner(
        config: EvidenceJournalConfig,
        #[cfg_attr(not(test), allow(unused_variables))] paused: bool,
    ) -> Result<Self, EvidenceOpenError> {
        if config.normal_capacity == 0 {
            return Err(EvidenceOpenError::ZeroNormalCapacity);
        }
        if config.priority_capacity == 0 {
            return Err(EvidenceOpenError::ZeroPriorityCapacity);
        }
        if config.maximum_records.is_some_and(|maximum| maximum < 2) {
            return Err(EvidenceOpenError::MaximumRecordsTooSmall);
        }
        let (mut journal_file, recovery) = JournalFile::open(&config.path)?;
        if let Some(maximum) = config.maximum_records {
            if journal_file.record_count >= maximum {
                return Err(EvidenceOpenError::MaximumRecordsTooSmall);
            }
        }
        if recovery.torn_tail_detected {
            let ordinal = journal_file
                .last_ordinal
                .checked_add(1)
                .ok_or(EvidenceOpenError::SequenceExhausted)?;
            journal_file.append(EvidenceRecord::persistence(
                ordinal,
                recovery
                    .last_complete_record
                    .map_or(MonotonicNanos::ZERO, |record| record.at),
                EvidenceDecision::TornTailRecovered,
            ))?;
            journal_file.sync()?;
        }
        let next_ordinal = journal_file
            .last_ordinal
            .checked_add(1)
            .ok_or(EvidenceOpenError::SequenceExhausted)?;
        let durable_records = journal_file.record_count;
        let last_durable_ordinal = journal_file.last_ordinal;
        let shared = Arc::new(SharedState {
            normal_capacity: config.normal_capacity,
            priority_capacity: config.priority_capacity,
            ingress: Mutex::new(IngressState {
                next_ordinal,
                normal: VecDeque::with_capacity(config.normal_capacity),
                priority: VecDeque::with_capacity(config.priority_capacity),
                terminal: None,
                normal_high_watermark: 0,
                priority_high_watermark: 0,
                saturated: false,
                shutdown: false,
            }),
            ready: Condvar::new(),
            durable_ready: Condvar::new(),
            durable_records: AtomicU64::new(durable_records),
            last_durable_ordinal: AtomicU64::new(last_durable_ordinal),
            storage_full: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            writer_fault: Mutex::new(None),
            recovery,
            #[cfg(test)]
            writer_paused: AtomicBool::new(paused),
        });
        let thread_shared = Arc::clone(&shared);
        let panic_shared = Arc::clone(&shared);
        let maximum_records = config.maximum_records;
        let worker = thread::Builder::new()
            .name("leash-evidence-persistence".to_string())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_writer(journal_file, maximum_records, thread_shared);
                }));
                if result.is_err() {
                    set_writer_fault(&panic_shared, "evidence persistence owner panicked");
                    panic_shared.closed.store(true, Ordering::Release);
                    panic_shared.durable_ready.notify_all();
                }
            })
            .map_err(|error| EvidenceOpenError::Thread(error.to_string()))?;
        Ok(Self {
            producer: EvidenceProducer { shared },
            worker: Some(worker),
        })
    }

    #[cfg(test)]
    pub(crate) fn open_paused(config: EvidenceJournalConfig) -> Result<Self, EvidenceOpenError> {
        Self::open_inner(config, true)
    }

    #[cfg(test)]
    pub(crate) fn resume(&self) {
        self.producer
            .shared
            .writer_paused
            .store(false, Ordering::Release);
        self.producer.shared.ready.notify_one();
    }

    pub fn producer(&self) -> EvidenceProducer {
        self.producer.clone()
    }

    pub fn status(&self) -> EvidenceJournalStatus {
        self.producer.status()
    }

    pub fn shutdown(mut self) -> EvidenceJournalStatus {
        self.stop_and_join();
        self.status()
    }

    fn stop_and_join(&mut self) {
        {
            let mut ingress = lock(&self.producer.shared.ingress);
            ingress.shutdown = true;
        }
        #[cfg(test)]
        self.producer
            .shared
            .writer_paused
            .store(false, Ordering::Release);
        self.producer.shared.ready.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for EvidenceJournal {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

pub fn read_evidence_records(
    path: impl AsRef<Path>,
) -> Result<Vec<EvidenceRecord>, EvidenceOpenError> {
    let mut file = OpenOptions::new().read(true).open(path)?;
    let length = file.metadata()?.len();
    if length < FILE_HEADER_LEN as u64 {
        return Err(EvidenceOpenError::InvalidHeader);
    }
    let mut header = [0_u8; FILE_HEADER_LEN];
    file.read_exact(&mut header)?;
    validate_header(&header)?;
    let remaining = length - FILE_HEADER_LEN as u64;
    if !remaining.is_multiple_of(FRAME_LEN as u64) {
        return Err(EvidenceOpenError::CorruptRecord("incomplete frame"));
    }
    let mut records = Vec::new();
    let mut last_ordinal = 0;
    for _ in 0..(remaining / FRAME_LEN as u64) {
        let mut frame = [0_u8; FRAME_LEN];
        file.read_exact(&mut frame)?;
        let record = decode_record(&frame)?;
        if record.ordinal <= last_ordinal {
            return Err(EvidenceOpenError::CorruptRecord(
                "record ordinals are not increasing",
            ));
        }
        last_ordinal = record.ordinal;
        records.push(record);
    }
    Ok(records)
}

fn status(shared: &SharedState) -> EvidenceJournalStatus {
    let ingress = lock(&shared.ingress);
    EvidenceJournalStatus {
        normal_capacity: shared.normal_capacity,
        normal_depth: ingress.normal.len(),
        normal_high_watermark: ingress.normal_high_watermark,
        priority_capacity: shared.priority_capacity,
        priority_depth: ingress.priority.len(),
        priority_high_watermark: ingress.priority_high_watermark,
        durable_records: shared.durable_records.load(Ordering::Acquire),
        last_durable_ordinal: shared.last_durable_ordinal.load(Ordering::Acquire),
        saturated: ingress.saturated,
        storage_full: shared.storage_full.load(Ordering::Acquire),
        closed: shared.closed.load(Ordering::Acquire),
        writer_fault: lock(&shared.writer_fault).clone(),
        recovery: shared.recovery,
    }
}

fn run_writer(mut journal: JournalFile, maximum_records: Option<u64>, shared: Arc<SharedState>) {
    loop {
        #[cfg(test)]
        if shared.writer_paused.load(Ordering::Acquire) {
            let ingress = lock(&shared.ingress);
            let _ = shared
                .ready
                .wait_timeout(ingress, Duration::from_millis(10))
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            continue;
        }

        let (mut batch, shutdown) = {
            let mut ingress = lock(&shared.ingress);
            while ingress.normal.is_empty()
                && ingress.priority.is_empty()
                && ingress.terminal.is_none()
                && !ingress.shutdown
            {
                ingress = shared
                    .ready
                    .wait(ingress)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            let mut batch = Vec::with_capacity(
                ingress.normal.len()
                    + ingress.priority.len()
                    + usize::from(ingress.terminal.is_some()),
            );
            batch.extend(ingress.normal.drain(..));
            batch.extend(ingress.priority.drain(..));
            if let Some(terminal) = ingress.terminal.take() {
                batch.push(terminal);
            }
            (batch, ingress.shutdown)
        };
        batch.sort_unstable_by_key(|record| record.ordinal);
        if batch.is_empty() && shutdown {
            break;
        }
        if batch.is_empty() {
            continue;
        }

        let mut wrote = false;
        for record in batch {
            if maximum_records
                .is_some_and(|maximum| journal.record_count >= maximum.saturating_sub(1))
            {
                let full = EvidenceRecord::persistence(
                    record.ordinal,
                    record.at,
                    EvidenceDecision::StorageFull,
                );
                match journal.append(full).and_then(|()| journal.sync()) {
                    Ok(()) => {
                        publish_durable(&shared, &journal);
                        shared.storage_full.store(true, Ordering::Release);
                    }
                    Err(error) => set_writer_fault(&shared, error.to_string()),
                }
                shared.durable_ready.notify_all();
                shared.closed.store(true, Ordering::Release);
                return;
            }
            if let Err(error) = journal.append(record) {
                set_writer_fault(&shared, error.to_string());
                shared.durable_ready.notify_all();
                shared.closed.store(true, Ordering::Release);
                return;
            }
            wrote = true;
        }
        if wrote {
            if let Err(error) = journal.sync() {
                set_writer_fault(&shared, error.to_string());
                shared.durable_ready.notify_all();
                shared.closed.store(true, Ordering::Release);
                return;
            }
            publish_durable(&shared, &journal);
            shared.durable_ready.notify_all();
        }
    }
    shared.closed.store(true, Ordering::Release);
    shared.durable_ready.notify_all();
}

fn publish_durable(shared: &SharedState, journal: &JournalFile) {
    shared
        .durable_records
        .store(journal.record_count, Ordering::Release);
    shared
        .last_durable_ordinal
        .store(journal.last_ordinal, Ordering::Release);
}

fn set_writer_fault(shared: &SharedState, message: impl Into<Box<str>>) {
    let mut fault = lock(&shared.writer_fault);
    if fault.is_none() {
        *fault = Some(message.into());
    }
}

struct JournalFile {
    file: File,
    record_count: u64,
    last_ordinal: u64,
}

impl JournalFile {
    fn open(path: &Path) -> Result<(Self, EvidenceRecoveryState), EvidenceOpenError> {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        let mut length = file.metadata()?.len();
        let mut torn_tail_detected = false;
        if length == 0 {
            file.write_all(&file_header())?;
            file.sync_data()?;
            length = FILE_HEADER_LEN as u64;
        } else if length < FILE_HEADER_LEN as u64 {
            torn_tail_detected = true;
            file.set_len(0)?;
            file.seek(SeekFrom::Start(0))?;
            file.write_all(&file_header())?;
            file.sync_data()?;
            length = FILE_HEADER_LEN as u64;
        }
        file.seek(SeekFrom::Start(0))?;
        let mut header = [0_u8; FILE_HEADER_LEN];
        file.read_exact(&mut header)?;
        validate_header(&header)?;

        let mut offset = FILE_HEADER_LEN as u64;
        let mut record_count = 0_u64;
        let mut last_ordinal = 0_u64;
        let mut last_complete_record = None;
        while offset < length {
            let remaining = length - offset;
            if remaining < FRAME_LEN as u64 {
                torn_tail_detected = true;
                break;
            }
            file.seek(SeekFrom::Start(offset))?;
            let mut frame = [0_u8; FRAME_LEN];
            file.read_exact(&mut frame)?;
            match decode_record(&frame) {
                Ok(record) if record.ordinal > last_ordinal => {
                    last_ordinal = record.ordinal;
                    last_complete_record = Some(record);
                    record_count = record_count.saturating_add(1);
                    offset += FRAME_LEN as u64;
                }
                Ok(_) => {
                    if remaining == FRAME_LEN as u64 {
                        torn_tail_detected = true;
                        break;
                    }
                    return Err(EvidenceOpenError::CorruptRecord(
                        "record ordinals are not increasing",
                    ));
                }
                Err(_) if remaining == FRAME_LEN as u64 => {
                    torn_tail_detected = true;
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        if torn_tail_detected {
            file.set_len(offset)?;
            file.sync_data()?;
        }
        file.seek(SeekFrom::End(0))?;
        Ok((
            Self {
                file,
                record_count,
                last_ordinal,
            },
            EvidenceRecoveryState {
                last_complete_record,
                torn_tail_detected,
            },
        ))
    }

    fn append(&mut self, record: EvidenceRecord) -> Result<(), EvidenceOpenError> {
        if record.ordinal <= self.last_ordinal {
            return Err(EvidenceOpenError::CorruptRecord(
                "writer received an out-of-order record",
            ));
        }
        self.file.write_all(&encode_record(record))?;
        self.last_ordinal = record.ordinal;
        self.record_count = self.record_count.saturating_add(1);
        Ok(())
    }

    fn sync(&mut self) -> Result<(), EvidenceOpenError> {
        self.file.sync_data()?;
        Ok(())
    }
}

fn file_header() -> [u8; FILE_HEADER_LEN] {
    let mut header = [0_u8; FILE_HEADER_LEN];
    header[..8].copy_from_slice(&FILE_MAGIC);
    header[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    header[12..16].copy_from_slice(&(FRAME_LEN as u32).to_le_bytes());
    header
}

fn validate_header(header: &[u8; FILE_HEADER_LEN]) -> Result<(), EvidenceOpenError> {
    let version = u32::from_le_bytes(header[8..12].try_into().expect("fixed header slice"));
    let frame_len = u32::from_le_bytes(header[12..16].try_into().expect("fixed header slice"));
    if header[..8] != FILE_MAGIC || version != FORMAT_VERSION || frame_len != FRAME_LEN as u32 {
        return Err(EvidenceOpenError::InvalidHeader);
    }
    Ok(())
}

fn encode_record(record: EvidenceRecord) -> [u8; FRAME_LEN] {
    let mut frame = [0_u8; FRAME_LEN];
    frame[..4].copy_from_slice(&FRAME_MAGIC);
    frame[4..8].copy_from_slice(&(PAYLOAD_LEN as u32).to_le_bytes());
    put_u64(&mut frame, 8, record.ordinal);
    put_u64(&mut frame, 16, record.at.get());
    frame[24] = record.source.tag();
    frame[25] = record.decision.tag();
    let mut flags = 0_u16;
    if let Some(proposal_sequence) = record.proposal_sequence {
        flags |= HAS_PROPOSAL;
        put_u64(&mut frame, 28, proposal_sequence);
    }
    if let Some(command_id) = record.command_id {
        flags |= HAS_COMMAND;
        put_u64(&mut frame, 36, command_id.producer_epoch.get());
        put_u64(&mut frame, 44, command_id.sequence.get());
    }
    if let Some(evidence_id) = record.evidence_id {
        flags |= HAS_EVIDENCE;
        put_u64(&mut frame, 52, evidence_id.producer_epoch.get());
        put_u64(&mut frame, 60, evidence_id.sequence.get());
    }
    if let Some(acknowledgement) = record.acknowledgement {
        flags |= HAS_ACKNOWLEDGEMENT;
        frame[68] = acknowledgement.kind.tag();
        if let Some(first) = acknowledgement.first_request_sequence {
            flags |= HAS_ACK_FIRST_REQUEST;
            put_u64(&mut frame, 76, first);
        }
        if let Some(through) = acknowledgement.through_request_sequence {
            flags |= HAS_ACK_THROUGH_REQUEST;
            put_u64(&mut frame, 84, through);
        }
        if let Some(applied) = acknowledgement.applied_sequence {
            flags |= HAS_ACK_APPLIED_SEQUENCE;
            put_u64(&mut frame, 92, applied);
        }
        put_u64(&mut frame, 100, acknowledgement.at.get());
    }
    frame[26..28].copy_from_slice(&flags.to_le_bytes());
    let checksum = checksum(&frame[..FRAME_LEN - 8]);
    put_u64(&mut frame, FRAME_LEN - 8, checksum);
    frame
}

fn decode_record(frame: &[u8; FRAME_LEN]) -> Result<EvidenceRecord, EvidenceOpenError> {
    if frame[..4] != FRAME_MAGIC
        || get_u32(frame, 4) != PAYLOAD_LEN as u32
        || get_u64(frame, FRAME_LEN - 8) != checksum(&frame[..FRAME_LEN - 8])
    {
        return Err(EvidenceOpenError::CorruptRecord(
            "frame marker, length, or checksum",
        ));
    }
    let flags = u16::from_le_bytes(frame[26..28].try_into().expect("fixed frame slice"));
    let command_id = if flags & HAS_COMMAND != 0 {
        Some(CommandId::new(
            ProducerEpoch::new(get_u64(frame, 36))
                .map_err(|_| EvidenceOpenError::CorruptRecord("zero command epoch"))?,
            Sequence::new(get_u64(frame, 44))
                .map_err(|_| EvidenceOpenError::CorruptRecord("zero command sequence"))?,
        ))
    } else {
        None
    };
    let evidence_id = if flags & HAS_EVIDENCE != 0 {
        Some(EvidenceId {
            producer_epoch: ProducerEpoch::new(get_u64(frame, 52))
                .map_err(|_| EvidenceOpenError::CorruptRecord("zero evidence epoch"))?,
            sequence: Sequence::new(get_u64(frame, 60))
                .map_err(|_| EvidenceOpenError::CorruptRecord("zero evidence sequence"))?,
        })
    } else {
        None
    };
    let acknowledgement = if flags & HAS_ACKNOWLEDGEMENT != 0 {
        Some(AcknowledgementIdentity {
            kind: AcknowledgementKind::from_tag(frame[68])?,
            first_request_sequence: (flags & HAS_ACK_FIRST_REQUEST != 0)
                .then(|| get_u64(frame, 76)),
            through_request_sequence: (flags & HAS_ACK_THROUGH_REQUEST != 0)
                .then(|| get_u64(frame, 84)),
            applied_sequence: (flags & HAS_ACK_APPLIED_SEQUENCE != 0).then(|| get_u64(frame, 92)),
            at: MonotonicNanos::new(get_u64(frame, 100)),
        })
    } else {
        None
    };
    Ok(EvidenceRecord {
        ordinal: get_u64(frame, 8),
        proposal_sequence: (flags & HAS_PROPOSAL != 0).then(|| get_u64(frame, 28)),
        command_id,
        evidence_id,
        source: EvidenceSource::from_tag(frame[24])?,
        at: MonotonicNanos::new(get_u64(frame, 16)),
        decision: EvidenceDecision::from_tag(frame[25])?,
        acknowledgement,
    })
}

fn checksum(bytes: &[u8]) -> u64 {
    let mut value = 0xcbf29ce484222325_u64;
    for byte in bytes {
        value ^= u64::from(*byte);
        value = value.wrapping_mul(0x00000100000001b3);
    }
    value
}

fn put_u64(buffer: &mut [u8], offset: usize, value: u64) {
    buffer[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn get_u64(buffer: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(
        buffer[offset..offset + 8]
            .try_into()
            .expect("fixed record slice"),
    )
}

fn get_u32(buffer: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        buffer[offset..offset + 4]
            .try_into()
            .expect("fixed record slice"),
    )
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::atomic::AtomicU64};

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    fn temp_path(name: &str) -> PathBuf {
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "leash-evidence-{name}-{}-{unique}.journal",
            std::process::id()
        ))
    }

    fn config(path: &Path) -> EvidenceJournalConfig {
        EvidenceJournalConfig {
            path: path.to_path_buf(),
            normal_capacity: 4,
            priority_capacity: 2,
            maximum_records: None,
        }
    }

    fn record(decision: EvidenceDecision, at: u64) -> EvidenceRecord {
        EvidenceRecord::new(
            Some(at),
            None,
            None,
            EvidenceSource::CpuSafetySupervisor,
            MonotonicNanos::new(at),
            decision,
            None,
        )
    }

    #[test]
    fn records_round_trip_and_restart_continues_ordering() {
        let path = temp_path("restart");
        let journal = EvidenceJournal::open(config(&path)).unwrap();
        let producer = journal.producer();
        let first = producer
            .try_record(record(EvidenceDecision::ProposalAccepted, 1))
            .unwrap();
        assert!(producer.wait_durable(first, Duration::from_secs(1)));
        let status = journal.shutdown();
        assert_eq!(status.last_durable_ordinal, 1);

        let journal = EvidenceJournal::open(config(&path)).unwrap();
        assert_eq!(
            journal
                .status()
                .recovery
                .last_complete_record
                .unwrap()
                .ordinal(),
            1
        );
        let second = journal
            .producer()
            .try_record(record(EvidenceDecision::CommandAccepted, 2))
            .unwrap();
        assert_eq!(second, 2);
        journal.shutdown();
        let records = read_evidence_records(&path).unwrap();
        assert_eq!(
            records
                .iter()
                .map(|item| item.ordinal())
                .collect::<Vec<_>>(),
            [1, 2]
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn restart_truncates_a_torn_tail_and_records_recovery() {
        let path = temp_path("torn");
        let journal = EvidenceJournal::open(config(&path)).unwrap();
        let ordinal = journal
            .producer()
            .try_record(record(EvidenceDecision::CommandAccepted, 7))
            .unwrap();
        assert!(journal
            .producer()
            .wait_durable(ordinal, Duration::from_secs(1)));
        journal.shutdown();
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&[1, 2, 3, 4, 5]).unwrap();
        file.sync_data().unwrap();

        let journal = EvidenceJournal::open(config(&path)).unwrap();
        let recovery = journal.status().recovery;
        assert!(recovery.torn_tail_detected);
        assert_eq!(recovery.last_complete_record.unwrap().ordinal(), 1);
        journal.shutdown();
        let records = read_evidence_records(&path).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].decision, EvidenceDecision::TornTailRecovered);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn writer_stall_saturates_normally_but_retains_priority_zero_evidence() {
        let path = temp_path("stall");
        let mut journal_config = config(&path);
        journal_config.normal_capacity = 1;
        let journal = EvidenceJournal::open_paused(journal_config).unwrap();
        let producer = journal.producer();
        producer
            .try_record(record(EvidenceDecision::ProposalAccepted, 1))
            .unwrap();
        assert_eq!(
            producer.try_record(record(EvidenceDecision::ProposalAccepted, 2)),
            Err(EvidenceEnqueueError::Full)
        );
        producer
            .try_record_priority(record(EvidenceDecision::ZeroRequested, 3))
            .unwrap();
        assert!(!producer.healthy());
        journal.resume();
        let status = journal.shutdown();
        assert!(status.saturated);
        let records = read_evidence_records(&path).unwrap();
        assert_eq!(
            records.iter().map(|item| item.decision).collect::<Vec<_>>(),
            [
                EvidenceDecision::ProposalAccepted,
                EvidenceDecision::JournalSaturated,
                EvidenceDecision::ZeroRequested,
            ]
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn priority_saturation_is_terminally_observable_without_blocking_the_caller() {
        let path = temp_path("priority-saturation");
        let mut journal_config = config(&path);
        journal_config.priority_capacity = 1;
        let journal = EvidenceJournal::open_paused(journal_config).unwrap();
        let producer = journal.producer();
        producer
            .try_record_priority(record(EvidenceDecision::ZeroRequested, 1))
            .unwrap();
        let started = Instant::now();
        assert_eq!(
            producer.try_record_priority(record(EvidenceDecision::ZeroRequested, 2)),
            Err(EvidenceEnqueueError::Full)
        );
        assert!(started.elapsed() < Duration::from_millis(10));
        journal.resume();
        let status = journal.shutdown();
        assert!(status.saturated);
        let records = read_evidence_records(&path).unwrap();
        assert_eq!(
            records.iter().map(|item| item.decision).collect::<Vec<_>>(),
            [
                EvidenceDecision::ZeroRequested,
                EvidenceDecision::JournalSaturated,
            ]
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn configured_full_storage_uses_the_reserved_terminal_record() {
        let path = temp_path("full");
        let mut journal_config = config(&path);
        journal_config.maximum_records = Some(3);
        let journal = EvidenceJournal::open(journal_config).unwrap();
        let producer = journal.producer();
        let first = producer
            .try_record(record(EvidenceDecision::ProposalAccepted, 1))
            .unwrap();
        let second = producer
            .try_record(record(EvidenceDecision::CommandAccepted, 2))
            .unwrap();
        let third = producer
            .try_record(record(EvidenceDecision::AcknowledgementApplied, 3))
            .unwrap();
        assert!(producer.wait_durable(first, Duration::from_secs(1)));
        assert!(producer.wait_durable(third, Duration::from_secs(1)));
        let status = journal.shutdown();
        assert!(status.storage_full);
        assert!(status.last_durable_ordinal >= second);
        let records = read_evidence_records(&path).unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[2].decision, EvidenceDecision::StorageFull);
        fs::remove_file(path).unwrap();
    }
}
