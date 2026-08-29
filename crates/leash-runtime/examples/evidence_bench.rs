use std::{
    collections::VecDeque,
    error::Error,
    fmt,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use leash_core::{
    Authorized, Clock, CommandId, ControlInput, ControlKernel, ControlKernelConfig,
    DifferentialDrive, DurationNanos, EvidenceId, MonotonicNanos, NormalizedDrive, OperatorId,
    ProducerEpoch,
};
use leash_runtime::{
    ActuationAcknowledgement, ActuationPort, CpuSafetySupervisor, EvidenceJournal,
    EvidenceJournalConfig, EvidenceProducer, SafetyAcknowledgement, SafetyKind, SupervisorConfig,
};

const DEFAULT_SAMPLES: usize = 1_000;
const STOP_SAMPLES: usize = 100;

#[derive(Debug, Clone, Copy)]
struct BenchError;

impl fmt::Display for BenchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("evidence benchmark port error")
    }
}

impl Error for BenchError {}

#[derive(Debug, Clone, Copy)]
struct BenchAck {
    command_id: CommandId,
    evidence_id: EvidenceId,
    applied_sequence: u64,
    at: MonotonicNanos,
}

impl ActuationAcknowledgement for BenchAck {
    fn applied(&self) -> bool {
        true
    }

    fn verified_zero(&self) -> bool {
        false
    }

    fn command_id(&self) -> Option<CommandId> {
        Some(self.command_id)
    }

    fn evidence_id(&self) -> Option<EvidenceId> {
        Some(self.evidence_id)
    }

    fn applied_sequence(&self) -> Option<u64> {
        Some(self.applied_sequence)
    }

    fn acknowledged_at(&self) -> Option<MonotonicNanos> {
        Some(self.at)
    }
}

#[derive(Default)]
struct BenchState {
    safety_requests: AtomicU64,
}

struct BenchPort {
    state: Arc<BenchState>,
    acknowledgements: VecDeque<BenchAck>,
    safety_acknowledgements: VecDeque<SafetyAcknowledgement>,
    applied_sequence: u64,
    safety_sequence: u64,
}

impl BenchPort {
    fn new(state: Arc<BenchState>) -> Self {
        Self {
            state,
            acknowledgements: VecDeque::new(),
            safety_acknowledgements: VecDeque::new(),
            applied_sequence: 0,
            safety_sequence: 0,
        }
    }
}

impl ActuationPort for BenchPort {
    type Acknowledgement = BenchAck;
    type Error = BenchError;

    fn submit_drive(&mut self, command: Authorized<DifferentialDrive>) -> Result<(), Self::Error> {
        self.applied_sequence = self.applied_sequence.saturating_add(1);
        self.acknowledgements.push_back(BenchAck {
            command_id: command.command_id(),
            evidence_id: command.evidence_id(),
            applied_sequence: self.applied_sequence,
            at: command.authorized_at(),
        });
        Ok(())
    }

    fn request_safety(&mut self, kind: SafetyKind) -> Result<u64, Self::Error> {
        self.safety_sequence = self.safety_sequence.saturating_add(1);
        self.applied_sequence = self.applied_sequence.saturating_add(1);
        self.state.safety_requests.fetch_add(1, Ordering::Release);
        self.safety_acknowledgements
            .push_back(SafetyAcknowledgement {
                kind,
                first_request_sequence: self.safety_sequence,
                through_request_sequence: self.safety_sequence,
                applied_sequence: Some(self.applied_sequence),
                at: MonotonicNanos::new(self.applied_sequence),
                verified_zero: true,
            });
        Ok(self.safety_sequence)
    }

    fn try_acknowledgement(&mut self) -> Result<Option<Self::Acknowledgement>, Self::Error> {
        Ok(self.acknowledgements.pop_front())
    }

    fn try_safety_acknowledgement(&mut self) -> Result<Option<SafetyAcknowledgement>, Self::Error> {
        Ok(self.safety_acknowledgements.pop_front())
    }
}

struct SteadyClock {
    origin: Instant,
}

impl Clock for SteadyClock {
    fn now(&mut self) -> MonotonicNanos {
        MonotonicNanos::new(elapsed_ns(self.origin.elapsed()))
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = parse_arguments()?;
    let journal = EvidenceJournal::open(EvidenceJournalConfig {
        path: arguments.journal_path.clone(),
        normal_capacity: arguments.samples.saturating_mul(4).max(1_024),
        priority_capacity: STOP_SAMPLES.saturating_mul(3).max(64),
        maximum_records: None,
    })?;
    let baseline_state = Arc::new(BenchState::default());
    let baseline_supervisor = spawn_supervisor(Arc::clone(&baseline_state), None)?;
    let state = Arc::new(BenchState::default());
    let supervisor = spawn_supervisor(Arc::clone(&state), Some(journal.producer()))?;
    let handle = supervisor.handle();
    configure_motion(&handle)?;
    let speed = NormalizedDrive::new(0.1)?;
    let drive = DifferentialDrive::new(speed, speed);
    let persisted_started = Instant::now();
    for _ in 0..arguments.samples {
        handle
            .submit(ControlInput::Drive {
                command: drive,
                deadline: MonotonicNanos::new(u64::MAX),
            })?
            .wait()
            .map_err(boxed_message)?;
    }
    let (baseline, with_evidence) = measure_alternating_stops(
        &baseline_supervisor.handle(),
        &baseline_state,
        &handle,
        &state,
        STOP_SAMPLES,
    )?;
    let supervisor_status = handle.status();
    baseline_supervisor.shutdown();
    supervisor.shutdown();
    let journal_status = journal.shutdown();
    let persisted_elapsed_ns = elapsed_ns(persisted_started.elapsed()).max(1);
    let records_per_second =
        journal_status.durable_records.saturating_mul(1_000_000_000) / persisted_elapsed_ns;
    let impact_p99_ns = i128::from(with_evidence.p99) - i128::from(baseline.p99);

    println!(
        concat!(
            "{{\"schema_version\":\"leash.evidence-benchmark.v1\",",
            "\"sampling\":\"alternating_order\",",
            "\"decision_samples\":{},\"stop_samples\":{},",
            "\"journal\":{{\"durable_records\":{},\"records_per_second\":{},",
            "\"normal_high_watermark\":{},\"priority_high_watermark\":{},",
            "\"saturated\":{},\"storage_full\":{}}},",
            "\"supervisor_evidence_records\":{},\"supervisor_evidence_failures\":{},",
            "\"stop_latency_without_evidence_ns\":{{\"p50\":{},\"p95\":{},",
            "\"p99\":{},\"max\":{}}},",
            "\"stop_latency_with_evidence_ns\":{{\"p50\":{},\"p95\":{},",
            "\"p99\":{},\"max\":{}}},",
            "\"stop_latency_impact_p99_ns\":{}}}"
        ),
        arguments.samples,
        STOP_SAMPLES,
        journal_status.durable_records,
        records_per_second,
        journal_status.normal_high_watermark,
        journal_status.priority_high_watermark,
        journal_status.saturated,
        journal_status.storage_full,
        supervisor_status.metrics.evidence_records,
        supervisor_status.metrics.evidence_failures,
        baseline.p50,
        baseline.p95,
        baseline.p99,
        baseline.maximum,
        with_evidence.p50,
        with_evidence.p95,
        with_evidence.p99,
        with_evidence.maximum,
        impact_p99_ns,
    );
    if arguments.remove_after_run {
        std::fs::remove_file(arguments.journal_path)?;
    }
    Ok(())
}

fn spawn_supervisor(
    state: Arc<BenchState>,
    evidence: Option<EvidenceProducer>,
) -> Result<CpuSafetySupervisor<BenchAck>, Box<dyn Error>> {
    let kernel = ControlKernel::new(ControlKernelConfig {
        command_epoch: ProducerEpoch::new(81)?,
        evidence_epoch: ProducerEpoch::new(82)?,
        deadman: DurationNanos::from_millis(100)?,
    });
    let origin = Instant::now();
    let config = SupervisorConfig {
        proposal_capacity: 32,
        tick_period: Duration::from_millis(10),
    };
    match evidence {
        Some(evidence) => Ok(CpuSafetySupervisor::spawn_with_evidence(
            kernel,
            BenchPort::new(state),
            Box::new(SteadyClock { origin }),
            config,
            evidence,
        )?),
        None => Ok(CpuSafetySupervisor::spawn(
            kernel,
            BenchPort::new(state),
            Box::new(SteadyClock { origin }),
            config,
        )?),
    }
}

fn configure_motion(handle: &leash_runtime::SupervisorHandle) -> Result<(), Box<dyn Error>> {
    handle
        .submit(ControlInput::UpdateEvidence {
            obstacle_blocked: false,
            lidar_fresh: true,
            localization_fresh: true,
        })?
        .wait()
        .map_err(boxed_message)?;
    handle
        .submit(ControlInput::Authorize {
            operator: OperatorId::new("evidence-benchmark")?,
            expires_at: MonotonicNanos::new(u64::MAX),
        })?
        .wait()
        .map_err(boxed_message)?;
    Ok(())
}

fn measure_alternating_stops(
    baseline_handle: &leash_runtime::SupervisorHandle,
    baseline_state: &BenchState,
    evidence_handle: &leash_runtime::SupervisorHandle,
    evidence_state: &BenchState,
    samples: usize,
) -> Result<(Distribution, Distribution), Box<dyn Error>> {
    let mut baseline_latencies = Vec::with_capacity(samples);
    let mut evidence_latencies = Vec::with_capacity(samples);
    for expected in 1..=samples as u64 {
        if expected % 2 == 0 {
            evidence_latencies.push(measure_one_stop(evidence_handle, evidence_state, expected)?);
            baseline_latencies.push(measure_one_stop(baseline_handle, baseline_state, expected)?);
        } else {
            baseline_latencies.push(measure_one_stop(baseline_handle, baseline_state, expected)?);
            evidence_latencies.push(measure_one_stop(evidence_handle, evidence_state, expected)?);
        }
    }
    Ok((
        Distribution::from_samples(baseline_latencies)?,
        Distribution::from_samples(evidence_latencies)?,
    ))
}

fn measure_one_stop(
    handle: &leash_runtime::SupervisorHandle,
    state: &BenchState,
    expected: u64,
) -> Result<u64, Box<dyn Error>> {
    let started = Instant::now();
    handle.stop()?;
    while state.safety_requests.load(Ordering::Acquire) < expected {
        if started.elapsed() > Duration::from_millis(100) {
            return Err("stop request exceeded 100 ms".into());
        }
        thread::yield_now();
    }
    Ok(elapsed_ns(started.elapsed()))
}

struct Arguments {
    samples: usize,
    journal_path: PathBuf,
    remove_after_run: bool,
}

fn parse_arguments() -> Result<Arguments, Box<dyn Error>> {
    let mut samples = DEFAULT_SAMPLES;
    let mut journal_path = None;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--samples" => {
                samples = args.next().ok_or("--samples requires a value")?.parse()?;
                if samples == 0 {
                    return Err("--samples must be positive".into());
                }
            }
            "--journal" => {
                journal_path = Some(PathBuf::from(
                    args.next().ok_or("--journal requires a path")?,
                ));
            }
            _ => return Err(format!("unknown argument {flag}").into()),
        }
    }
    let remove_after_run = journal_path.is_none();
    let journal_path = journal_path.unwrap_or_else(|| {
        std::env::temp_dir().join(format!(
            "leash-evidence-bench-{}.journal",
            std::process::id()
        ))
    });
    Ok(Arguments {
        samples,
        journal_path,
        remove_after_run,
    })
}

fn elapsed_ns(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn boxed_message(message: Box<str>) -> Box<dyn Error> {
    message.to_string().into()
}

struct Distribution {
    p50: u64,
    p95: u64,
    p99: u64,
    maximum: u64,
}

impl Distribution {
    fn from_samples(mut samples: Vec<u64>) -> Result<Self, Box<dyn Error>> {
        if samples.is_empty() {
            return Err("benchmark distribution has no samples".into());
        }
        samples.sort_unstable();
        Ok(Self {
            p50: percentile(&samples, 50),
            p95: percentile(&samples, 95),
            p99: percentile(&samples, 99),
            maximum: *samples.last().expect("non-empty samples"),
        })
    }
}

fn percentile(sorted: &[u64], percentile: usize) -> u64 {
    let rank = sorted.len().saturating_mul(percentile).saturating_add(99) / 100;
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}
