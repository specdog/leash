use std::{
    error::Error,
    fmt,
    sync::{mpsc, Arc, Condvar, Mutex},
    thread,
    time::{Duration, Instant},
};

use leash_core::{
    Authorized, Clock, ControlKernel, ControlKernelConfig, DifferentialDrive, DurationNanos,
    MonotonicNanos, ProducerEpoch,
};
use leash_cuda::{
    BackendKind, ComputeGate, ComputeGateConfig, ComputeJob, ExecutorConfig, FaultInjection,
    GateMode,
};
use leash_runtime::{
    ActuationAcknowledgement, ActuationPort, CpuSafetySupervisor, SafetyKind, SupervisorConfig,
};

const COMPUTE_DEADLINE: Duration = Duration::from_millis(100);
const FALLBACK_RESERVE: Duration = Duration::from_millis(40);
const SAFETY_DEADLINE: Duration = Duration::from_millis(10);
const SHADOW_SAMPLES: u32 = 16;

fn main() -> Result<(), Box<dyn Error>> {
    let parity = [
        run_parity("lidar_large", lidar_job)?,
        run_parity("spatial_combined_large", spatial_job)?,
        run_parity("camera_large", camera_job)?,
    ];
    let faults = [
        run_fault("context_loss", FaultInjection::ContextLoss)?,
        run_fault("launch_error", FaultInjection::LaunchError)?,
        run_fault("timeout", FaultInjection::Stall(Duration::from_millis(200)))?,
        run_fault("executor_panic", FaultInjection::WorkerPanic)?,
    ];

    print!(
        concat!(
            "{{\"schema_version\":\"leash.cuda-gate-probe.v1\",",
            "\"compute_deadline_ns\":{},\"fallback_reserve_ns\":{},",
            "\"safety_deadline_ns\":{},\"shadow_samples_required\":{},",
            "\"parity\":["
        ),
        nanos(COMPUTE_DEADLINE),
        nanos(FALLBACK_RESERVE),
        nanos(SAFETY_DEADLINE),
        SHADOW_SAMPLES,
    );
    for (index, result) in parity.iter().enumerate() {
        if index != 0 {
            print!(",");
        }
        print!(
            concat!(
                "{{\"workload\":\"{}\",\"samples\":{},",
                "\"max_absolute_error\":{},\"max_relative_error\":{},",
                "\"authority_after_shadow\":\"cuda\"}}"
            ),
            result.name, result.samples, result.max_absolute_error, result.max_relative_error,
        );
    }
    print!("],\"faults\":[");
    for (index, result) in faults.iter().enumerate() {
        if index != 0 {
            print!(",");
        }
        print!(
            concat!(
                "{{\"fault\":\"{}\",\"fallback_authority\":\"cpu\",",
                "\"fallback_ns\":{},\"safety_estop_ns\":{},",
                "\"gate_degraded\":true}}"
            ),
            result.name, result.fallback_ns, result.safety_ns,
        );
    }
    println!(concat!(
        "],\"safety\":{{\"actuator\":\"in_process_fake\",",
        "\"serial_opened\":false,\"motor_commands_sent\":false}}}}"
    ));
    Ok(())
}

struct ParityResult {
    name: &'static str,
    samples: u32,
    max_absolute_error: f32,
    max_relative_error: f32,
}

fn run_parity(
    name: &'static str,
    make_job: fn(u32) -> ComputeJob,
) -> Result<ParityResult, Box<dyn Error>> {
    let gate = ComputeGate::start_cuda(
        ExecutorConfig { queue_capacity: 2 },
        gate_config(SHADOW_SAMPLES),
    )?;
    if gate.status().active != BackendKind::Cpu || gate.status().mode != GateMode::Shadow {
        return Err(format!("{name}: CPU was not authoritative during shadow startup").into());
    }
    let mut max_absolute_error = 0.0_f32;
    let mut max_relative_error = 0.0_f32;
    for round in 0..SHADOW_SAMPLES {
        let outcome = gate.execute(make_job(round))?;
        if outcome.authority != BackendKind::Cpu {
            return Err(
                format!("{name}: CUDA became authoritative before shadow completed").into(),
            );
        }
        let comparison = outcome
            .shadow
            .ok_or_else(|| format!("{name}: shadow comparison missing"))?;
        if !comparison.matched {
            return Err(format!("{name}: CPU/CUDA parity mismatch in round {round}").into());
        }
        max_absolute_error = max_absolute_error.max(comparison.max_absolute_error);
        max_relative_error = max_relative_error.max(comparison.max_relative_error);
    }
    let status = gate.status();
    if status.mode != GateMode::Cuda || status.active != BackendKind::Cuda || status.degraded {
        return Err(format!("{name}: CUDA did not activate after the shadow gate").into());
    }
    let authoritative = gate.execute(make_job(SHADOW_SAMPLES))?;
    if authoritative.authority != BackendKind::Cuda {
        return Err(format!("{name}: CUDA did not own the post-shadow result").into());
    }
    Ok(ParityResult {
        name,
        samples: SHADOW_SAMPLES,
        max_absolute_error,
        max_relative_error,
    })
}

struct FaultResult {
    name: &'static str,
    fallback_ns: u64,
    safety_ns: u64,
}

fn run_fault(name: &'static str, fault: FaultInjection) -> Result<FaultResult, Box<dyn Error>> {
    let gate = Arc::new(ComputeGate::start_cuda(
        ExecutorConfig { queue_capacity: 2 },
        gate_config(1),
    )?);
    let shadow = gate.execute(lidar_job(0))?;
    if shadow.authority != BackendKind::Cpu || gate.status().mode != GateMode::Cuda {
        return Err(format!("{name}: setup shadow gate failed").into());
    }

    let safety_observed = Arc::new((Mutex::new(None), Condvar::new()));
    let supervisor = CpuSafetySupervisor::spawn(
        ControlKernel::new(ControlKernelConfig {
            command_epoch: ProducerEpoch::new(81)?,
            evidence_epoch: ProducerEpoch::new(82)?,
            deadman: DurationNanos::from_millis(100)?,
        }),
        ProbePort {
            safety_observed: Arc::clone(&safety_observed),
        },
        Box::new(SteadyClock {
            origin: Instant::now(),
        }),
        SupervisorConfig::default(),
    )?;

    gate.inject_next_cuda_fault(fault)?;
    let worker_gate = Arc::clone(&gate);
    let (started_tx, started_rx) = mpsc::sync_channel(0);
    let worker = thread::spawn(move || {
        started_tx.send(()).expect("probe receiver remains alive");
        worker_gate.execute(lidar_job(1))
    });
    started_rx.recv()?;
    if matches!(fault, FaultInjection::Stall(_)) {
        thread::sleep(Duration::from_millis(2));
    }
    let safety_started = Instant::now();
    supervisor.handle().estop()?;
    let observed = wait_for_safety(&safety_observed, SAFETY_DEADLINE)?;
    let safety_elapsed = observed.saturating_duration_since(safety_started);
    if safety_elapsed >= SAFETY_DEADLINE {
        return Err(format!("{name}: CPU safety missed its deadline").into());
    }

    let outcome = worker
        .join()
        .map_err(|_| format!("{name}: compute probe thread panicked"))??;
    if outcome.authority != BackendKind::Cpu || outcome.elapsed >= COMPUTE_DEADLINE {
        return Err(format!("{name}: CPU fallback missed its compute deadline").into());
    }
    let status = gate.status();
    if status.mode != GateMode::Cpu
        || status.active != BackendKind::Cpu
        || !status.degraded
        || status.fallbacks != 1
    {
        return Err(format!("{name}: fallback health status was not fail-closed").into());
    }
    supervisor.shutdown();
    Ok(FaultResult {
        name,
        fallback_ns: nanos(outcome.elapsed),
        safety_ns: nanos(safety_elapsed),
    })
}

fn gate_config(shadow_samples_required: u32) -> ComputeGateConfig {
    ComputeGateConfig {
        deadline: COMPUTE_DEADLINE,
        fallback_reserve: FALLBACK_RESERVE,
        shadow_samples_required,
        cuda_eligible: true,
        ..ComputeGateConfig::default()
    }
}

fn lidar_job(round: u32) -> ComputeJob {
    ComputeJob::LidarTransform {
        ranges_m: ranges(round),
        angle_min_rad: -core::f32::consts::PI,
        angle_increment_rad: core::f32::consts::TAU / 10_000.0,
        range_min_m: 0.05,
        range_max_m: 12.0,
        yaw_offset_rad: 0.1,
        clockwise: round.is_multiple_of(2),
    }
}

fn spatial_job(round: u32) -> ComputeJob {
    ComputeJob::LidarTransformAndCollision {
        ranges_m: ranges(round),
        angle_min_rad: -core::f32::consts::PI,
        angle_increment_rad: core::f32::consts::TAU / 10_000.0,
        range_min_m: 0.05,
        range_max_m: 12.0,
        yaw_offset_rad: 0.1,
        clockwise: round.is_multiple_of(2),
        sector_center_rad: 0.0,
        sector_half_width_rad: core::f32::consts::FRAC_PI_4,
    }
}

fn camera_job(round: u32) -> ComputeJob {
    let mut state = u64::from(round) + 0x9e37_79b9_7f4a_7c15;
    let input = (0..640 * 480 * 3)
        .map(|_| next_random(&mut state).to_le_bytes()[0])
        .collect();
    ComputeJob::NormalizeRgbU8 {
        input,
        mean: [0.485, 0.456, 0.406],
        inverse_std: [4.366_812, 4.464_286, 4.444_444],
    }
}

fn ranges(round: u32) -> Vec<f32> {
    let mut state = u64::from(round) + 0xd1b5_4a32_d192_ed03;
    (0..10_000)
        .map(|index| match index % 257 {
            0 => f32::NAN,
            1 => f32::INFINITY,
            _ => 0.05 + (next_random(&mut state) % 1_195) as f32 * 0.01,
        })
        .collect()
}

fn next_random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn wait_for_safety(
    observed: &(Mutex<Option<Instant>>, Condvar),
    timeout: Duration,
) -> Result<Instant, Box<dyn Error>> {
    let deadline = Instant::now() + timeout;
    let mut guard = observed.0.lock().unwrap_or_else(|error| error.into_inner());
    loop {
        if let Some(at) = *guard {
            return Ok(at);
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Err("CPU safety acknowledgement deadline expired".into());
        };
        let (next, timed_out) = observed
            .1
            .wait_timeout(guard, remaining)
            .unwrap_or_else(|error| error.into_inner());
        guard = next;
        if timed_out.timed_out() && guard.is_none() {
            return Err("CPU safety acknowledgement deadline expired".into());
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ProbeAck;

impl ActuationAcknowledgement for ProbeAck {
    fn applied(&self) -> bool {
        true
    }

    fn verified_zero(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy)]
struct ProbePortError;

impl fmt::Display for ProbePortError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("probe port error")
    }
}

struct ProbePort {
    safety_observed: Arc<(Mutex<Option<Instant>>, Condvar)>,
}

impl ActuationPort for ProbePort {
    type Acknowledgement = ProbeAck;
    type Error = ProbePortError;

    fn submit_drive(&mut self, _command: Authorized<DifferentialDrive>) -> Result<(), Self::Error> {
        Ok(())
    }

    fn request_safety(&mut self, _kind: SafetyKind) -> Result<u64, Self::Error> {
        *self
            .safety_observed
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(Instant::now());
        self.safety_observed.1.notify_all();
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
        MonotonicNanos::new(nanos(self.origin.elapsed()))
    }
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}
