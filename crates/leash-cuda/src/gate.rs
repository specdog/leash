use std::{
    fmt,
    sync::Mutex,
    time::{Duration, Instant},
};

use crate::{
    BackendKind, ComputeExecutor, ComputeJob, ComputeResult, ExecutorConfig, FaultInjection,
    JobPriority, StartError, SubmitError, WorkError,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParityTolerance {
    pub absolute: f32,
    pub relative: f32,
}

impl Default for ParityTolerance {
    fn default() -> Self {
        Self {
            absolute: 1e-5,
            relative: 1e-5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateMode {
    Cpu,
    Shadow,
    Cuda,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkloadClass {
    Voxel,
    LidarSmall,
    LidarLarge,
    CollisionAdvisory,
    SpatialCombinedLarge,
    CameraSmall,
    CameraLarge,
    Cognition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkloadDecision {
    pub cuda_eligible: bool,
    pub authority: BackendKind,
    pub reason: &'static str,
}

pub const fn orin_nx_workload_decision(workload: WorkloadClass) -> WorkloadDecision {
    match workload {
        WorkloadClass::LidarLarge | WorkloadClass::SpatialCombinedLarge => WorkloadDecision {
            cuda_eligible: true,
            authority: BackendKind::Cuda,
            reason: "CUDA won end-to-end p50 and tail latency on the Orin NX",
        },
        WorkloadClass::CameraLarge => WorkloadDecision {
            cuda_eligible: true,
            authority: BackendKind::Cuda,
            reason: "CUDA won end-to-end p50 and p95 when a GPU provider consumes the tensor",
        },
        WorkloadClass::CollisionAdvisory => WorkloadDecision {
            cuda_eligible: false,
            authority: BackendKind::Cpu,
            reason: "CUDA collision output remains advisory and CPU owns final stop authority",
        },
        WorkloadClass::Voxel => WorkloadDecision {
            cuda_eligible: false,
            authority: BackendKind::Cpu,
            reason: "voxel projection was slower on CUDA end-to-end",
        },
        WorkloadClass::LidarSmall => WorkloadDecision {
            cuda_eligible: false,
            authority: BackendKind::Cpu,
            reason: "small lidar transform was slower on CUDA end-to-end",
        },
        WorkloadClass::CameraSmall => WorkloadDecision {
            cuda_eligible: false,
            authority: BackendKind::Cpu,
            reason: "small camera tail latency was not consistently better on CUDA",
        },
        WorkloadClass::Cognition => WorkloadDecision {
            cuda_eligible: false,
            authority: BackendKind::Cpu,
            reason: "resident cognition was slower on CUDA at both measured sizes",
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComputeGateConfig {
    pub deadline: Duration,
    pub fallback_reserve: Duration,
    pub shadow_samples_required: u32,
    pub tolerance: ParityTolerance,
    pub cuda_eligible: bool,
}

impl Default for ComputeGateConfig {
    fn default() -> Self {
        Self {
            deadline: Duration::from_millis(100),
            fallback_reserve: Duration::from_millis(25),
            shadow_samples_required: 16,
            tolerance: ParityTolerance::default(),
            cuda_eligible: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateStartError {
    InvalidConfig,
    Cpu(StartError),
    Cuda(StartError),
}

impl fmt::Display for GateStartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig => formatter.write_str("compute gate configuration is invalid"),
            Self::Cpu(error) => write!(formatter, "start CPU fallback: {error}"),
            Self::Cuda(error) => write!(formatter, "start CUDA primary: {error}"),
        }
    }
}

impl std::error::Error for GateStartError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputeGateStatus {
    pub mode: GateMode,
    pub selected: BackendKind,
    pub active: BackendKind,
    pub degraded: bool,
    pub reason: Option<String>,
    pub shadow_completed: u32,
    pub shadow_mismatches: u64,
    pub fallbacks: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowComparison {
    pub matched: bool,
    pub max_absolute_error: f32,
    pub max_relative_error: f32,
    pub cpu_elapsed: Duration,
    pub cuda_elapsed: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComputeGateOutcome {
    pub result: ComputeResult,
    pub authority: BackendKind,
    pub elapsed: Duration,
    pub shadow: Option<ShadowComparison>,
    pub fallback_reason: Option<String>,
}

pub struct ComputeGate {
    cpu: ComputeExecutor,
    cuda: ComputeExecutor,
    config: ComputeGateConfig,
    status: Mutex<ComputeGateStatus>,
}

impl ComputeGate {
    #[cfg(feature = "cuda")]
    pub fn start_cuda(
        executor_config: ExecutorConfig,
        config: ComputeGateConfig,
    ) -> Result<Self, GateStartError> {
        validate_config(config)?;
        let cpu = ComputeExecutor::start_cpu(executor_config).map_err(GateStartError::Cpu)?;
        let cuda = ComputeExecutor::start_cuda(executor_config).map_err(GateStartError::Cuda)?;
        Ok(Self::with_executors(cpu, cuda, config))
    }

    fn with_executors(
        cpu: ComputeExecutor,
        cuda: ComputeExecutor,
        config: ComputeGateConfig,
    ) -> Self {
        let mode = if config.cuda_eligible {
            GateMode::Shadow
        } else {
            GateMode::Cpu
        };
        Self {
            cpu,
            cuda,
            config,
            status: Mutex::new(ComputeGateStatus {
                mode,
                selected: if config.cuda_eligible {
                    BackendKind::Cuda
                } else {
                    BackendKind::Cpu
                },
                active: BackendKind::Cpu,
                degraded: false,
                reason: (!config.cuda_eligible)
                    .then(|| "CUDA is ineligible for this measured workload".to_string()),
                shadow_completed: 0,
                shadow_mismatches: 0,
                fallbacks: 0,
            }),
        }
    }

    pub fn execute(&self, job: ComputeJob) -> Result<ComputeGateOutcome, WorkError> {
        match self.status().mode {
            GateMode::Cpu => self.execute_cpu(job),
            GateMode::Shadow => self.execute_shadow(job),
            GateMode::Cuda => self.execute_cuda_with_fallback(job),
        }
    }

    pub fn inject_next_cuda_fault(&self, fault: FaultInjection) -> Result<(), SubmitError> {
        self.cuda.inject_next_fault(fault)
    }

    pub fn status(&self) -> ComputeGateStatus {
        self.status
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn execute_cpu(&self, job: ComputeJob) -> Result<ComputeGateOutcome, WorkError> {
        let started = Instant::now();
        let result = execute_before(&self.cpu, job, started + self.config.deadline)?;
        Ok(ComputeGateOutcome {
            result,
            authority: BackendKind::Cpu,
            elapsed: started.elapsed(),
            shadow: None,
            fallback_reason: None,
        })
    }

    fn execute_shadow(&self, job: ComputeJob) -> Result<ComputeGateOutcome, WorkError> {
        let started = Instant::now();
        let cpu_started = Instant::now();
        let authoritative =
            execute_before(&self.cpu, job.clone(), cpu_started + self.config.deadline)?;
        let cpu_elapsed = cpu_started.elapsed();
        let cuda_started = Instant::now();
        let cuda = execute_before(&self.cuda, job, cuda_started + self.config.deadline);
        let cuda_elapsed = cuda_started.elapsed();
        let comparison = match cuda {
            Ok(cuda) => compare_results(
                &authoritative,
                &cuda,
                self.config.tolerance,
                cpu_elapsed,
                cuda_elapsed,
            ),
            Err(error) => {
                self.disqualify_cuda(format!("shadow CUDA failure: {error}"), false);
                return Ok(ComputeGateOutcome {
                    result: authoritative,
                    authority: BackendKind::Cpu,
                    elapsed: started.elapsed(),
                    shadow: None,
                    fallback_reason: Some(error.to_string()),
                });
            }
        };
        {
            let mut status = self
                .status
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if comparison.matched {
                status.shadow_completed = status.shadow_completed.saturating_add(1);
                if status.shadow_completed >= self.config.shadow_samples_required {
                    status.mode = GateMode::Cuda;
                    status.active = BackendKind::Cuda;
                    status.reason = Some("startup probe and shadow parity passed".to_string());
                }
            } else {
                status.shadow_mismatches = status.shadow_mismatches.saturating_add(1);
                status.mode = GateMode::Cpu;
                status.active = BackendKind::Cpu;
                status.degraded = true;
                status.reason = Some("CPU/CUDA shadow parity mismatch".to_string());
            }
        }
        Ok(ComputeGateOutcome {
            result: authoritative,
            authority: BackendKind::Cpu,
            elapsed: started.elapsed(),
            shadow: Some(comparison),
            fallback_reason: None,
        })
    }

    fn execute_cuda_with_fallback(&self, job: ComputeJob) -> Result<ComputeGateOutcome, WorkError> {
        let started = Instant::now();
        let deadline = started + self.config.deadline;
        let wait_for = self.config.deadline - self.config.fallback_reserve;
        let primary = match self
            .cuda
            .submit(JobPriority::Interactive, Some(deadline), job.clone())
        {
            Ok(ticket) => match ticket.wait_timeout(wait_for) {
                Ok(Some(result)) => result,
                Ok(None) => {
                    ticket.cancel();
                    Err(WorkError::DeadlineExpired)
                }
                Err(error) => Err(error),
            },
            Err(error) => Err(submit_error(error)),
        };
        match primary {
            Ok(result) => Ok(ComputeGateOutcome {
                result,
                authority: BackendKind::Cuda,
                elapsed: started.elapsed(),
                shadow: None,
                fallback_reason: None,
            }),
            Err(error) => {
                let reason = error.to_string();
                let result = execute_before(&self.cpu, job, deadline)?;
                self.disqualify_cuda(reason.clone(), true);
                Ok(ComputeGateOutcome {
                    result,
                    authority: BackendKind::Cpu,
                    elapsed: started.elapsed(),
                    shadow: None,
                    fallback_reason: Some(reason),
                })
            }
        }
    }

    fn disqualify_cuda(&self, reason: String, fallback: bool) {
        let mut status = self
            .status
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        status.mode = GateMode::Cpu;
        status.active = BackendKind::Cpu;
        status.degraded = true;
        status.reason = Some(reason);
        if fallback {
            status.fallbacks = status.fallbacks.saturating_add(1);
        }
    }
}

fn validate_config(config: ComputeGateConfig) -> Result<(), GateStartError> {
    if config.deadline.is_zero()
        || config.fallback_reserve.is_zero()
        || config.fallback_reserve >= config.deadline
        || config.shadow_samples_required == 0
        || !config.tolerance.absolute.is_finite()
        || config.tolerance.absolute < 0.0
        || !config.tolerance.relative.is_finite()
        || config.tolerance.relative < 0.0
    {
        return Err(GateStartError::InvalidConfig);
    }
    Ok(())
}

fn execute_before(
    executor: &ComputeExecutor,
    job: ComputeJob,
    deadline: Instant,
) -> Result<ComputeResult, WorkError> {
    executor
        .submit(JobPriority::Interactive, Some(deadline), job)
        .map_err(submit_error)?
        .wait()
}

fn submit_error(error: SubmitError) -> WorkError {
    match error {
        SubmitError::CircuitOpen => WorkError::CircuitOpen,
        SubmitError::WorkerStopped => WorkError::WorkerStopped,
        SubmitError::QueueFull => WorkError::Backend("compute queue is full".to_string()),
    }
}

fn compare_results(
    cpu: &ComputeResult,
    cuda: &ComputeResult,
    tolerance: ParityTolerance,
    cpu_elapsed: Duration,
    cuda_elapsed: Duration,
) -> ShadowComparison {
    let mut errors = ErrorBounds::new(tolerance);
    let variants_match = compare_result_values(cpu, cuda, &mut errors);
    ShadowComparison {
        matched: variants_match && errors.within_tolerance,
        max_absolute_error: errors.max_absolute,
        max_relative_error: errors.max_relative,
        cpu_elapsed,
        cuda_elapsed,
    }
}

struct ErrorBounds {
    tolerance: ParityTolerance,
    max_absolute: f32,
    max_relative: f32,
    within_tolerance: bool,
}

impl ErrorBounds {
    fn new(tolerance: ParityTolerance) -> Self {
        Self {
            tolerance,
            max_absolute: 0.0,
            max_relative: 0.0,
            within_tolerance: true,
        }
    }

    fn observe(&mut self, left: f32, right: f32) -> bool {
        if !left.is_finite() || !right.is_finite() {
            return left.to_bits() == right.to_bits();
        }
        let absolute = (left - right).abs();
        let relative = absolute / left.abs().max(right.abs()).max(f32::EPSILON);
        self.max_absolute = self.max_absolute.max(absolute);
        self.max_relative = self.max_relative.max(relative);
        if absolute > self.tolerance.absolute && relative > self.tolerance.relative {
            self.within_tolerance = false;
        }
        true
    }

    fn slices(&mut self, left: &[f32], right: &[f32]) -> bool {
        left.len() == right.len()
            && left
                .iter()
                .zip(right)
                .all(|(left, right)| self.observe(*left, *right))
    }
}

#[allow(clippy::too_many_lines)]
fn compare_result_values(
    cpu: &ComputeResult,
    cuda: &ComputeResult,
    errors: &mut ErrorBounds,
) -> bool {
    match (cpu, cuda) {
        (ComputeResult::Occupancy(cpu), ComputeResult::Occupancy(cuda)) => cpu == cuda,
        (ComputeResult::Lidar(cpu), ComputeResult::Lidar(cuda)) => {
            cpu.len() == cuda.len()
                && cpu.iter().zip(cuda).all(|(cpu, cuda)| {
                    cpu.valid == cuda.valid
                        && (!cpu.valid
                            || errors.observe(cpu.x_m, cuda.x_m)
                                && errors.observe(cpu.y_m, cuda.y_m))
                })
        }
        (ComputeResult::CollisionSector(cpu), ComputeResult::CollisionSector(cuda)) => {
            cpu.sample_count == cuda.sample_count
                && match (cpu.min_range_m, cuda.min_range_m) {
                    (Some(cpu), Some(cuda)) => errors.observe(cpu, cuda),
                    (None, None) => true,
                    _ => false,
                }
        }
        (
            ComputeResult::Spatial {
                lidar: cpu_lidar,
                collision: cpu_collision,
            },
            ComputeResult::Spatial {
                lidar: cuda_lidar,
                collision: cuda_collision,
            },
        ) => {
            compare_result_values(
                &ComputeResult::Lidar(cpu_lidar.clone()),
                &ComputeResult::Lidar(cuda_lidar.clone()),
                errors,
            ) && compare_result_values(
                &ComputeResult::CollisionSector(*cpu_collision),
                &ComputeResult::CollisionSector(*cuda_collision),
                errors,
            )
        }
        (ComputeResult::NormalizedRgb(cpu), ComputeResult::NormalizedRgb(cuda)) => {
            errors.slices(cpu, cuda)
        }
        (ComputeResult::Predictive(cpu), ComputeResult::Predictive(cuda)) => {
            errors.slices(&cpu.state, &cuda.state)
                && errors.slices(&cpu.weights, &cuda.weights)
                && errors.slices(&cpu.bias, &cuda.bias)
        }
        (ComputeResult::CognitionLoaded, ComputeResult::CognitionLoaded) => true,
        (ComputeResult::CognitionAdvanced(cpu), ComputeResult::CognitionAdvanced(cuda)) => {
            cpu.layers.len() == cuda.layers.len()
                && cpu.layers.iter().zip(&cuda.layers).all(|(cpu, cuda)| {
                    cpu.sequence == cuda.sequence
                        && errors.observe(cpu.precision, cuda.precision)
                        && errors.observe(cpu.prediction_error_l2, cuda.prediction_error_l2)
                        && errors.observe(cpu.activation_mean, cuda.activation_mean)
                        && errors.observe(cpu.activation_rms, cuda.activation_rms)
                })
        }
        (ComputeResult::CognitionCheckpoint(cpu), ComputeResult::CognitionCheckpoint(cuda)) => {
            cpu.schema_version == cuda.schema_version
                && errors.slices(&cpu.sensor, &cuda.sensor)
                && errors.slices(&cpu.top_down, &cuda.top_down)
                && cpu.layers.len() == cuda.layers.len()
                && cpu.layers.iter().zip(&cuda.layers).all(|(cpu, cuda)| {
                    cpu.sequence == cuda.sequence
                        && errors.observe(cpu.precision, cuda.precision)
                        && errors.observe(cpu.prediction_error_l2, cuda.prediction_error_l2)
                        && errors.slices(&cpu.activation, &cuda.activation)
                        && errors.slices(&cpu.weights, &cuda.weights)
                        && errors.slices(&cpu.bias, &cuda.bias)
                })
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> ComputeJob {
        ComputeJob::ProjectOccupancy {
            cells: vec![-1, 0, 100],
            depth: 2,
        }
    }

    fn test_gate(config: ComputeGateConfig) -> ComputeGate {
        ComputeGate::with_executors(
            ComputeExecutor::start_cpu(ExecutorConfig::default()).unwrap(),
            ComputeExecutor::start_cpu(ExecutorConfig::default()).unwrap(),
            config,
        )
    }

    #[test]
    fn measured_cpu_workloads_never_enter_shadow_or_cuda_mode() {
        for workload in [
            WorkloadClass::Voxel,
            WorkloadClass::LidarSmall,
            WorkloadClass::CollisionAdvisory,
            WorkloadClass::CameraSmall,
            WorkloadClass::Cognition,
        ] {
            let decision = orin_nx_workload_decision(workload);
            assert!(!decision.cuda_eligible);
            assert_eq!(decision.authority, BackendKind::Cpu);
        }
    }

    #[test]
    fn shadow_keeps_cpu_authority_until_required_parity_samples_pass() {
        let gate = test_gate(ComputeGateConfig {
            cuda_eligible: true,
            shadow_samples_required: 2,
            ..ComputeGateConfig::default()
        });
        let first = gate.execute(job()).unwrap();
        assert_eq!(first.authority, BackendKind::Cpu);
        assert!(first.shadow.unwrap().matched);
        assert_eq!(gate.status().mode, GateMode::Shadow);
        let second = gate.execute(job()).unwrap();
        assert_eq!(second.authority, BackendKind::Cpu);
        assert_eq!(gate.status().mode, GateMode::Cuda);
        let third = gate.execute(job()).unwrap();
        assert_eq!(third.authority, BackendKind::Cuda);
    }

    #[test]
    fn injected_primary_failure_falls_back_inside_one_deadline() {
        let gate = test_gate(ComputeGateConfig {
            deadline: Duration::from_millis(100),
            fallback_reserve: Duration::from_millis(40),
            shadow_samples_required: 1,
            cuda_eligible: true,
            ..ComputeGateConfig::default()
        });
        gate.execute(job()).unwrap();
        gate.inject_next_cuda_fault(FaultInjection::Stall(Duration::from_millis(200)))
            .unwrap();
        let outcome = gate.execute(job()).unwrap();
        assert_eq!(outcome.authority, BackendKind::Cpu);
        assert!(outcome.elapsed < Duration::from_millis(100));
        assert!(outcome.fallback_reason.is_some());
        assert_eq!(gate.status().fallbacks, 1);
        assert!(gate.status().degraded);
    }

    #[test]
    fn invalid_gate_configuration_is_rejected() {
        assert_eq!(
            validate_config(ComputeGateConfig {
                fallback_reserve: Duration::from_millis(100),
                ..ComputeGateConfig::default()
            }),
            Err(GateStartError::InvalidConfig)
        );
    }
}
