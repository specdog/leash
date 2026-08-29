use std::{
    error::Error,
    fmt, thread,
    time::{Duration, Instant},
};

use leash_core::{
    Authorized, Clock, ControlInput, ControlKernel, ControlKernelConfig, DifferentialDrive,
    DurationNanos, MonotonicNanos, NormalizedDrive, OperatorId, ProducerEpoch,
};
use leash_runtime::{
    ActuationAcknowledgement, ActuationPort, CpuSafetySupervisor, SafetyKind, SupervisorConfig,
};

const PERIOD: Duration = Duration::from_millis(10);
const DEFAULT_TICKS: usize = 250;

#[derive(Debug, Clone, Copy)]
struct BenchmarkAck;

impl ActuationAcknowledgement for BenchmarkAck {
    fn applied(&self) -> bool {
        true
    }

    fn verified_zero(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, Copy)]
struct BenchmarkPortError;

impl fmt::Display for BenchmarkPortError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("benchmark port error")
    }
}

struct BenchmarkPort;

impl ActuationPort for BenchmarkPort {
    type Acknowledgement = BenchmarkAck;
    type Error = BenchmarkPortError;

    fn submit_drive(&mut self, _command: Authorized<DifferentialDrive>) -> Result<(), Self::Error> {
        Ok(())
    }

    fn request_safety(&mut self, _kind: SafetyKind) -> Result<u64, Self::Error> {
        Ok(1)
    }

    fn try_acknowledgement(&mut self) -> Result<Option<Self::Acknowledgement>, Self::Error> {
        Ok(None)
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
    let ticks = parse_ticks()?;
    let origin = Instant::now();
    let supervisor = CpuSafetySupervisor::spawn(
        ControlKernel::new(ControlKernelConfig {
            command_epoch: ProducerEpoch::new(71)?,
            evidence_epoch: ProducerEpoch::new(72)?,
            deadman: DurationNanos::from_millis(100)?,
        }),
        BenchmarkPort,
        Box::new(SteadyClock { origin }),
        SupervisorConfig {
            proposal_capacity: 32,
            tick_period: PERIOD,
        },
    )?;
    let handle = supervisor.handle();
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
            operator: OperatorId::new("host-benchmark")?,
            expires_at: MonotonicNanos::new(u64::MAX),
        })?
        .wait()
        .map_err(boxed_message)?;

    let speed = NormalizedDrive::new(0.1)?;
    let drive = DifferentialDrive::new(speed, speed);
    let run_origin = Instant::now();
    let mut transition_latency = Vec::with_capacity(ticks);
    let mut completion_intervals = Vec::with_capacity(ticks.saturating_sub(1));
    let mut previous_completion = None;
    let mut missed_deadlines = 0_u64;

    for index in 0..ticks {
        let target = run_origin + mul_duration(PERIOD, index)?;
        if let Some(remaining) = target.checked_duration_since(Instant::now()) {
            thread::sleep(remaining);
        }
        let submitted = Instant::now();
        let deadline_ns = elapsed_ns(origin.elapsed())
            .checked_add(100_000_000)
            .ok_or("benchmark deadline overflow")?;
        let ticket = handle.submit(ControlInput::Drive {
            command: drive,
            deadline: MonotonicNanos::new(deadline_ns),
        })?;
        let receipt = ticket
            .wait_timeout(Duration::from_millis(100))?
            .ok_or("control transition exceeded 100 ms")?
            .map_err(boxed_message)?;
        if receipt.effects.is_empty() {
            return Err("control transition returned no effect".into());
        }
        let completed = Instant::now();
        let latency = completed.duration_since(submitted);
        if latency > PERIOD {
            missed_deadlines = missed_deadlines.saturating_add(1);
        }
        transition_latency.push(elapsed_ns(latency));
        if let Some(previous) = previous_completion {
            completion_intervals.push(elapsed_ns(completed.duration_since(previous)));
        }
        previous_completion = Some(completed);
    }

    let status = handle.status();
    let jitter = completion_intervals
        .into_iter()
        .map(|interval| interval.abs_diff(elapsed_ns(PERIOD)))
        .collect::<Vec<_>>();
    let jitter = Distribution::from_samples(jitter)?;
    let latency = Distribution::from_samples(transition_latency)?;
    println!(
        concat!(
            "{{\"schema_version\":\"leash.control-loop-benchmark.v1\",",
            "\"frequency_hz\":100,\"ticks\":{},\"deadline_ns\":{},",
            "\"missed_deadlines\":{},",
            "\"jitter_ns\":{{\"p50\":{},\"p95\":{},\"p99\":{},\"max\":{}}},",
            "\"transition_latency_ns\":{{\"p50\":{},\"p95\":{},\"p99\":{},\"max\":{}}},",
            "\"proposal_queue\":{{\"capacity\":{},\"high_watermark\":{},",
            "\"rejected\":{},\"depth_at_end\":{}}}}}"
        ),
        ticks,
        elapsed_ns(PERIOD),
        missed_deadlines,
        jitter.p50,
        jitter.p95,
        jitter.p99,
        jitter.maximum,
        latency.p50,
        latency.p95,
        latency.p99,
        latency.maximum,
        status.proposal_lane.capacity,
        status.proposal_lane.high_watermark,
        status.proposal_lane.rejected,
        status.proposal_lane.depth,
    );
    supervisor.shutdown();
    Ok(())
}

fn parse_ticks() -> Result<usize, Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let Some(flag) = args.next() else {
        return Ok(DEFAULT_TICKS);
    };
    if flag != "--ticks" {
        return Err(format!("unknown argument {flag}; expected --ticks N").into());
    }
    let ticks = args
        .next()
        .ok_or("--ticks requires a value")?
        .parse::<usize>()?;
    if ticks < 2 {
        return Err("--ticks must be at least 2".into());
    }
    if args.next().is_some() {
        return Err("unexpected trailing benchmark argument".into());
    }
    Ok(ticks)
}

fn mul_duration(duration: Duration, multiplier: usize) -> Result<Duration, Box<dyn Error>> {
    let multiplier = u32::try_from(multiplier).map_err(|_| "tick count exceeds u32")?;
    duration
        .checked_mul(multiplier)
        .ok_or_else(|| "benchmark schedule overflow".into())
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
