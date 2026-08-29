use std::{
    collections::VecDeque,
    fmt,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::{
    collision_sector_reduce_cpu, lidar_transform_cpu, normalize_rgb_u8_cpu, predictive_step_cpu,
    project_occupancy_cpu, CollisionSector, ComputeInputError, LidarPoint,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct JobId(u64);

impl JobId {
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobPriority {
    Interactive,
    Bulk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Cpu,
    Cuda,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PredictiveState {
    pub state: Vec<f32>,
    pub weights: Vec<f32>,
    pub bias: Vec<f32>,
}

pub const RESIDENT_COGNITION_SCHEMA_VERSION: &str = "leash.cuda-cognition.v1";
const MAX_COGNITION_LAYERS: usize = 16;
const MAX_COGNITION_STATE_DIM: usize = 1_048_576;

#[derive(Debug, Clone, PartialEq)]
pub struct ResidentCognitionLayer {
    pub activation: Vec<f32>,
    pub weights: Vec<f32>,
    pub bias: Vec<f32>,
    pub sequence: u64,
    pub precision: f32,
    pub prediction_error_l2: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResidentCognitionCheckpoint {
    pub schema_version: String,
    pub sensor: Vec<f32>,
    pub top_down: Vec<f32>,
    pub layers: Vec<ResidentCognitionLayer>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CognitionLayerMetrics {
    pub sequence: u64,
    pub precision: f32,
    pub prediction_error_l2: f32,
    pub activation_mean: f32,
    pub activation_rms: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CognitionLayerSnapshot {
    pub layer: usize,
    pub activation: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CognitionStep {
    pub layers: Vec<CognitionLayerMetrics>,
    pub snapshot: Option<CognitionLayerSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ComputeJob {
    ProjectOccupancy {
        cells: Vec<i8>,
        depth: u32,
    },
    LidarTransform {
        ranges_m: Vec<f32>,
        angle_min_rad: f32,
        angle_increment_rad: f32,
        range_min_m: f32,
        range_max_m: f32,
        yaw_offset_rad: f32,
        clockwise: bool,
    },
    CollisionSectorReduce {
        ranges_m: Vec<f32>,
        angle_min_rad: f32,
        angle_increment_rad: f32,
        range_min_m: f32,
        range_max_m: f32,
        sector_center_rad: f32,
        sector_half_width_rad: f32,
    },
    LidarTransformAndCollision {
        ranges_m: Vec<f32>,
        angle_min_rad: f32,
        angle_increment_rad: f32,
        range_min_m: f32,
        range_max_m: f32,
        yaw_offset_rad: f32,
        clockwise: bool,
        sector_center_rad: f32,
        sector_half_width_rad: f32,
    },
    NormalizeRgbU8 {
        input: Vec<u8>,
        mean: [f32; 3],
        inverse_std: [f32; 3],
    },
    PredictiveStep {
        lower: Vec<f32>,
        state: PredictiveState,
        top_down: Vec<f32>,
        source_precision: f32,
        top_precision: f32,
    },
    CognitionLoad {
        checkpoint: ResidentCognitionCheckpoint,
    },
    CognitionAdvance {
        sensor: Vec<f32>,
        sensor_precision: f32,
        top_down: Vec<f32>,
        top_precision: f32,
        due_layers: Vec<bool>,
        snapshot_layer: Option<usize>,
    },
    CognitionCheckpoint,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ComputeResult {
    Occupancy(Vec<i32>),
    Lidar(Vec<LidarPoint>),
    CollisionSector(CollisionSector),
    Spatial {
        lidar: Vec<LidarPoint>,
        collision: CollisionSector,
    },
    NormalizedRgb(Vec<f32>),
    Predictive(PredictiveState),
    CognitionLoaded,
    CognitionAdvanced(CognitionStep),
    CognitionCheckpoint(ResidentCognitionCheckpoint),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkError {
    Cancelled,
    DeadlineExpired,
    InvalidInput(ComputeInputError),
    Backend(String),
    CircuitOpen,
    WorkerPanicked,
    WorkerStopped,
}

impl fmt::Display for WorkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("compute job was cancelled"),
            Self::DeadlineExpired => formatter.write_str("compute deadline expired"),
            Self::InvalidInput(error) => write!(formatter, "invalid compute input: {error}"),
            Self::Backend(error) => write!(formatter, "compute backend failed: {error}"),
            Self::CircuitOpen => formatter.write_str("compute circuit is open"),
            Self::WorkerPanicked => formatter.write_str("compute worker panicked"),
            Self::WorkerStopped => formatter.write_str("compute worker stopped"),
        }
    }
}

impl std::error::Error for WorkError {}

impl From<ComputeInputError> for WorkError {
    fn from(value: ComputeInputError) -> Self {
        Self::InvalidInput(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutorConfig {
    pub queue_capacity: usize,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self { queue_capacity: 8 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    ZeroCapacity,
    Thread(String),
    Backend(String),
}

impl fmt::Display for StartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroCapacity => formatter.write_str("compute queue capacity must be positive"),
            Self::Thread(error) => write!(formatter, "start compute thread: {error}"),
            Self::Backend(error) => write!(formatter, "start compute backend: {error}"),
        }
    }
}

impl std::error::Error for StartError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitError {
    QueueFull,
    CircuitOpen,
    WorkerStopped,
}

impl fmt::Display for SubmitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QueueFull => formatter.write_str("compute queue is full"),
            Self::CircuitOpen => formatter.write_str("compute circuit is open"),
            Self::WorkerStopped => formatter.write_str("compute worker is stopped"),
        }
    }
}

impl std::error::Error for SubmitError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendStatus {
    pub selected: BackendKind,
    pub active: BackendKind,
    pub degraded: bool,
    pub fallback_reason: Option<String>,
    pub circuit_open: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExecutorMetrics {
    pub submitted: u64,
    pub rejected: u64,
    pub completed: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub deadline_expired: u64,
    pub depth: usize,
    pub high_watermark: usize,
}

#[derive(Default)]
struct MetricAtoms {
    submitted: AtomicU64,
    rejected: AtomicU64,
    completed: AtomicU64,
    failed: AtomicU64,
    cancelled: AtomicU64,
    deadline_expired: AtomicU64,
    depth: AtomicUsize,
    high_watermark: AtomicUsize,
}

impl MetricAtoms {
    fn snapshot(&self) -> ExecutorMetrics {
        ExecutorMetrics {
            submitted: self.submitted.load(Ordering::Relaxed),
            rejected: self.rejected.load(Ordering::Relaxed),
            completed: self.completed.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            cancelled: self.cancelled.load(Ordering::Relaxed),
            deadline_expired: self.deadline_expired.load(Ordering::Relaxed),
            depth: self.depth.load(Ordering::Relaxed),
            high_watermark: self.high_watermark.load(Ordering::Relaxed),
        }
    }
}

struct WorkItem {
    id: JobId,
    job: ComputeJob,
    deadline: Option<Instant>,
    cancelled: Arc<AtomicBool>,
    reply: mpsc::Sender<Result<ComputeResult, WorkError>>,
}

struct QueueState {
    interactive: VecDeque<WorkItem>,
    bulk: VecDeque<WorkItem>,
    stopped: bool,
}

struct WorkQueue {
    capacity: usize,
    state: Mutex<QueueState>,
    ready: Condvar,
    metrics: Arc<MetricAtoms>,
}

impl WorkQueue {
    fn push(&self, priority: JobPriority, item: WorkItem) -> Result<(), SubmitError> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.stopped {
            return Err(SubmitError::WorkerStopped);
        }
        let depth = state.interactive.len() + state.bulk.len();
        if depth == self.capacity {
            self.metrics.rejected.fetch_add(1, Ordering::Relaxed);
            return Err(SubmitError::QueueFull);
        }
        match priority {
            JobPriority::Interactive => state.interactive.push_back(item),
            JobPriority::Bulk => state.bulk.push_back(item),
        }
        let depth = depth + 1;
        self.metrics.depth.store(depth, Ordering::Relaxed);
        self.metrics
            .high_watermark
            .fetch_max(depth, Ordering::Relaxed);
        self.metrics.submitted.fetch_add(1, Ordering::Relaxed);
        self.ready.notify_one();
        Ok(())
    }

    fn pop(&self) -> Option<WorkItem> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        while !state.stopped && state.interactive.is_empty() && state.bulk.is_empty() {
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
        if state.stopped {
            return None;
        }
        let item = state
            .interactive
            .pop_front()
            .or_else(|| state.bulk.pop_front());
        self.metrics.depth.fetch_sub(1, Ordering::Relaxed);
        item
    }

    fn stop(&self) {
        let drained = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if state.stopped {
                return;
            }
            state.stopped = true;
            self.metrics.depth.store(0, Ordering::Relaxed);
            let mut drained = state.interactive.drain(..).collect::<Vec<_>>();
            drained.extend(state.bulk.drain(..));
            drained
        };
        for item in drained {
            let _ = item.reply.send(Err(WorkError::WorkerStopped));
        }
        self.ready.notify_all();
    }
}

pub struct JobTicket {
    id: JobId,
    cancelled: Arc<AtomicBool>,
    result: Receiver<Result<ComputeResult, WorkError>>,
}

impl JobTicket {
    pub const fn id(&self) -> JobId {
        self.id
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn wait(self) -> Result<ComputeResult, WorkError> {
        self.result.recv().unwrap_or(Err(WorkError::WorkerStopped))
    }

    pub fn wait_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Option<Result<ComputeResult, WorkError>>, WorkError> {
        match self.result.recv_timeout(timeout) {
            Ok(result) => Ok(Some(result)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(WorkError::WorkerStopped),
        }
    }
}

pub(crate) trait Backend {
    fn execute(&mut self, job: ComputeJob) -> Result<ComputeResult, WorkError>;
}

#[derive(Default)]
struct CpuBackend {
    cognition: Option<ResidentCognitionCheckpoint>,
}

impl Backend for CpuBackend {
    fn execute(&mut self, job: ComputeJob) -> Result<ComputeResult, WorkError> {
        match job {
            ComputeJob::ProjectOccupancy { cells, depth } => Ok(ComputeResult::Occupancy(
                project_occupancy_cpu(&cells, depth)?,
            )),
            ComputeJob::LidarTransform {
                ranges_m,
                angle_min_rad,
                angle_increment_rad,
                range_min_m,
                range_max_m,
                yaw_offset_rad,
                clockwise,
            } => Ok(ComputeResult::Lidar(lidar_transform_cpu(
                &ranges_m,
                angle_min_rad,
                angle_increment_rad,
                range_min_m,
                range_max_m,
                yaw_offset_rad,
                clockwise,
            )?)),
            ComputeJob::CollisionSectorReduce {
                ranges_m,
                angle_min_rad,
                angle_increment_rad,
                range_min_m,
                range_max_m,
                sector_center_rad,
                sector_half_width_rad,
            } => Ok(ComputeResult::CollisionSector(collision_sector_reduce_cpu(
                &ranges_m,
                angle_min_rad,
                angle_increment_rad,
                range_min_m,
                range_max_m,
                sector_center_rad,
                sector_half_width_rad,
            )?)),
            ComputeJob::LidarTransformAndCollision {
                ranges_m,
                angle_min_rad,
                angle_increment_rad,
                range_min_m,
                range_max_m,
                yaw_offset_rad,
                clockwise,
                sector_center_rad,
                sector_half_width_rad,
            } => Ok(ComputeResult::Spatial {
                lidar: lidar_transform_cpu(
                    &ranges_m,
                    angle_min_rad,
                    angle_increment_rad,
                    range_min_m,
                    range_max_m,
                    yaw_offset_rad,
                    clockwise,
                )?,
                collision: collision_sector_reduce_cpu(
                    &ranges_m,
                    angle_min_rad,
                    angle_increment_rad,
                    range_min_m,
                    range_max_m,
                    sector_center_rad,
                    sector_half_width_rad,
                )?,
            }),
            ComputeJob::NormalizeRgbU8 {
                input,
                mean,
                inverse_std,
            } => Ok(ComputeResult::NormalizedRgb(normalize_rgb_u8_cpu(
                &input,
                mean,
                inverse_std,
            )?)),
            ComputeJob::PredictiveStep {
                lower,
                mut state,
                top_down,
                source_precision,
                top_precision,
            } => {
                predictive_step_cpu(
                    &lower,
                    &mut state.state,
                    &top_down,
                    &mut state.weights,
                    &mut state.bias,
                    source_precision,
                    top_precision,
                )?;
                Ok(ComputeResult::Predictive(state))
            }
            ComputeJob::CognitionLoad { checkpoint } => {
                validate_cognition_checkpoint(&checkpoint)?;
                self.cognition = Some(checkpoint);
                Ok(ComputeResult::CognitionLoaded)
            }
            ComputeJob::CognitionAdvance {
                sensor,
                sensor_precision,
                top_down,
                top_precision,
                due_layers,
                snapshot_layer,
            } => {
                let cognition = self.cognition.as_mut().ok_or(WorkError::InvalidInput(
                    ComputeInputError::InvalidCognitionState,
                ))?;
                advance_resident_cognition_cpu(
                    cognition,
                    sensor,
                    sensor_precision,
                    top_down,
                    top_precision,
                    &due_layers,
                )?;
                Ok(ComputeResult::CognitionAdvanced(cognition_step(
                    cognition,
                    snapshot_layer,
                )?))
            }
            ComputeJob::CognitionCheckpoint => self
                .cognition
                .clone()
                .map(ComputeResult::CognitionCheckpoint)
                .ok_or(WorkError::InvalidInput(
                    ComputeInputError::InvalidCognitionState,
                )),
        }
    }
}

pub(crate) fn validate_cognition_checkpoint(
    checkpoint: &ResidentCognitionCheckpoint,
) -> Result<(), ComputeInputError> {
    let dimension = checkpoint.sensor.len();
    if checkpoint.schema_version != RESIDENT_COGNITION_SCHEMA_VERSION
        || dimension == 0
        || dimension > MAX_COGNITION_STATE_DIM
        || checkpoint.top_down.len() != dimension
        || checkpoint.layers.is_empty()
        || checkpoint.layers.len() > MAX_COGNITION_LAYERS
        || checkpoint
            .sensor
            .iter()
            .chain(&checkpoint.top_down)
            .any(|value| !value.is_finite())
    {
        return Err(ComputeInputError::InvalidCognitionState);
    }
    for layer in &checkpoint.layers {
        if layer.activation.len() != dimension
            || layer.weights.len() != dimension
            || layer.bias.len() != dimension
            || !layer.precision.is_finite()
            || !(0.0..=1.0).contains(&layer.precision)
            || !layer.prediction_error_l2.is_finite()
            || layer.prediction_error_l2 < 0.0
            || layer
                .activation
                .iter()
                .chain(&layer.weights)
                .chain(&layer.bias)
                .any(|value| !value.is_finite())
        {
            return Err(ComputeInputError::InvalidCognitionState);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn advance_resident_cognition_cpu(
    checkpoint: &mut ResidentCognitionCheckpoint,
    sensor: Vec<f32>,
    sensor_precision: f32,
    top_down: Vec<f32>,
    top_precision: f32,
    due_layers: &[bool],
) -> Result<(), ComputeInputError> {
    validate_cognition_checkpoint(checkpoint)?;
    let dimension = checkpoint.sensor.len();
    if sensor.len() != dimension
        || top_down.len() != dimension
        || due_layers.len() != checkpoint.layers.len()
        || sensor
            .iter()
            .chain(&top_down)
            .any(|value| !value.is_finite())
        || !valid_precision(sensor_precision)
        || !valid_precision(top_precision)
    {
        return Err(ComputeInputError::InvalidCognitionSchedule);
    }
    checkpoint.sensor = sensor;
    checkpoint.top_down = top_down;
    for (layer_index, due) in due_layers.iter().copied().enumerate() {
        if !due {
            continue;
        }
        let lower = if layer_index == 0 {
            checkpoint.sensor.clone()
        } else {
            checkpoint.layers[layer_index - 1].activation.clone()
        };
        let source_precision = if layer_index == 0 {
            sensor_precision
        } else {
            checkpoint.layers[layer_index - 1].precision
        };
        let (upper, upper_precision) = if layer_index + 1 < checkpoint.layers.len() {
            (
                checkpoint.layers[layer_index + 1].activation.clone(),
                checkpoint.layers[layer_index + 1].precision,
            )
        } else {
            (checkpoint.top_down.clone(), top_precision)
        };
        let error_l2 = prediction_error_l2(&checkpoint.layers[layer_index], &lower);
        let layer = &mut checkpoint.layers[layer_index];
        predictive_step_cpu(
            &lower,
            &mut layer.activation,
            &upper,
            &mut layer.weights,
            &mut layer.bias,
            source_precision,
            upper_precision,
        )?;
        layer.prediction_error_l2 = error_l2;
        layer.precision = (source_precision / (1.0 + error_l2)).clamp(0.0, 1.0);
        layer.sequence = layer.sequence.saturating_add(1);
    }
    Ok(())
}

fn prediction_error_l2(layer: &ResidentCognitionLayer, lower: &[f32]) -> f32 {
    let error_sum = lower
        .iter()
        .zip(&layer.activation)
        .zip(&layer.weights)
        .zip(&layer.bias)
        .map(|(((lower, activation), weight), bias)| {
            let error = lower - (weight * activation + bias);
            error * error
        })
        .sum::<f32>();
    (error_sum / lower.len() as f32).sqrt()
}

pub(crate) fn cognition_step(
    checkpoint: &ResidentCognitionCheckpoint,
    snapshot_layer: Option<usize>,
) -> Result<CognitionStep, ComputeInputError> {
    let snapshot = snapshot_layer
        .map(|layer| {
            checkpoint
                .layers
                .get(layer)
                .map(|state| CognitionLayerSnapshot {
                    layer,
                    activation: state.activation.clone(),
                })
                .ok_or(ComputeInputError::InvalidCognitionSchedule)
        })
        .transpose()?;
    let dimension = checkpoint.sensor.len() as f32;
    let layers = checkpoint
        .layers
        .iter()
        .map(|layer| {
            let mean = layer.activation.iter().sum::<f32>() / dimension;
            let rms = (layer
                .activation
                .iter()
                .map(|value| value * value)
                .sum::<f32>()
                / dimension)
                .sqrt();
            CognitionLayerMetrics {
                sequence: layer.sequence,
                precision: layer.precision,
                prediction_error_l2: layer.prediction_error_l2,
                activation_mean: mean,
                activation_rms: rms,
            }
        })
        .collect();
    Ok(CognitionStep { layers, snapshot })
}

fn valid_precision(value: f32) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

pub struct ComputeExecutor {
    queue: Arc<WorkQueue>,
    circuit_open: Arc<AtomicBool>,
    next_id: AtomicU64,
    metrics: Arc<MetricAtoms>,
    selected: BackendKind,
    active: BackendKind,
    fallback_reason: Option<String>,
    worker: Option<JoinHandle<()>>,
}

impl ComputeExecutor {
    pub fn start_cpu(config: ExecutorConfig) -> Result<Self, StartError> {
        Self::start_with(config, BackendKind::Cpu, BackendKind::Cpu, None, || {
            Ok(Box::new(CpuBackend::default()))
        })
    }

    #[cfg(feature = "cuda")]
    pub fn start_cuda(config: ExecutorConfig) -> Result<Self, StartError> {
        Self::start_with(config, BackendKind::Cuda, BackendKind::Cuda, None, || {
            crate::device::CudaBackend::new()
                .map(|backend| Box::new(backend) as Box<dyn Backend>)
                .map_err(|error| error.to_string())
        })
    }

    #[cfg(feature = "cuda")]
    pub fn start_cuda_or_cpu(config: ExecutorConfig) -> Result<Self, StartError> {
        match Self::start_cuda(config) {
            Ok(executor) => Ok(executor),
            Err(StartError::Backend(reason)) => Self::start_with(
                config,
                BackendKind::Cuda,
                BackendKind::Cpu,
                Some(reason),
                || Ok(Box::new(CpuBackend::default())),
            ),
            Err(error) => Err(error),
        }
    }

    fn start_with<F>(
        config: ExecutorConfig,
        selected: BackendKind,
        active: BackendKind,
        fallback_reason: Option<String>,
        factory: F,
    ) -> Result<Self, StartError>
    where
        F: FnOnce() -> Result<Box<dyn Backend>, String> + Send + 'static,
    {
        if config.queue_capacity == 0 {
            return Err(StartError::ZeroCapacity);
        }
        let metrics = Arc::new(MetricAtoms::default());
        let queue = Arc::new(WorkQueue {
            capacity: config.queue_capacity,
            state: Mutex::new(QueueState {
                interactive: VecDeque::new(),
                bulk: VecDeque::new(),
                stopped: false,
            }),
            ready: Condvar::new(),
            metrics: Arc::clone(&metrics),
        });
        let circuit_open = Arc::new(AtomicBool::new(false));
        let worker_queue = Arc::clone(&queue);
        let worker_circuit = Arc::clone(&circuit_open);
        let worker_metrics = Arc::clone(&metrics);
        let (initialized_tx, initialized_rx) = mpsc::sync_channel(0);
        let worker = thread::Builder::new()
            .name("leash-compute-owner".to_string())
            .spawn(move || {
                let initialized = std::panic::catch_unwind(std::panic::AssertUnwindSafe(factory))
                    .map_err(|_| "compute backend initialization panicked".to_string())
                    .and_then(|result| result);
                let mut backend = match initialized {
                    Ok(backend) => {
                        let _ = initialized_tx.send(Ok(()));
                        backend
                    }
                    Err(error) => {
                        let _ = initialized_tx.send(Err(error));
                        return;
                    }
                };
                worker_loop(
                    &mut *backend,
                    &worker_queue,
                    &worker_circuit,
                    &worker_metrics,
                );
            })
            .map_err(|error| StartError::Thread(error.to_string()))?;

        match initialized_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                queue,
                circuit_open,
                next_id: AtomicU64::new(1),
                metrics,
                selected,
                active,
                fallback_reason,
                worker: Some(worker),
            }),
            Ok(Err(error)) => {
                let _ = worker.join();
                Err(StartError::Backend(error))
            }
            Err(error) => {
                let _ = worker.join();
                Err(StartError::Thread(error.to_string()))
            }
        }
    }

    pub fn submit(
        &self,
        priority: JobPriority,
        deadline: Option<Instant>,
        job: ComputeJob,
    ) -> Result<JobTicket, SubmitError> {
        if self.circuit_open.load(Ordering::Acquire) {
            self.metrics.rejected.fetch_add(1, Ordering::Relaxed);
            return Err(SubmitError::CircuitOpen);
        }
        let raw_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if raw_id == u64::MAX {
            self.circuit_open.store(true, Ordering::Release);
            self.metrics.rejected.fetch_add(1, Ordering::Relaxed);
            return Err(SubmitError::CircuitOpen);
        }
        let id = JobId(raw_id);
        let cancelled = Arc::new(AtomicBool::new(false));
        let (reply, result) = mpsc::channel();
        self.queue.push(
            priority,
            WorkItem {
                id,
                job,
                deadline,
                cancelled: Arc::clone(&cancelled),
                reply,
            },
        )?;
        Ok(JobTicket {
            id,
            cancelled,
            result,
        })
    }

    pub fn status(&self) -> BackendStatus {
        BackendStatus {
            selected: self.selected,
            active: self.active,
            degraded: self.selected != self.active || self.circuit_open.load(Ordering::Acquire),
            fallback_reason: self.fallback_reason.clone(),
            circuit_open: self.circuit_open.load(Ordering::Acquire),
        }
    }

    pub fn metrics(&self) -> ExecutorMetrics {
        self.metrics.snapshot()
    }

    pub fn shutdown(mut self) {
        self.stop_and_join();
    }

    fn stop_and_join(&mut self) {
        self.queue.stop();
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                self.circuit_open.store(true, Ordering::Release);
            }
        }
    }
}

impl Drop for ComputeExecutor {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

fn worker_loop(
    backend: &mut dyn Backend,
    queue: &WorkQueue,
    circuit_open: &AtomicBool,
    metrics: &MetricAtoms,
) {
    while let Some(item) = queue.pop() {
        let _job_id = item.id;
        let outcome = if circuit_open.load(Ordering::Acquire) {
            Err(WorkError::CircuitOpen)
        } else if item.cancelled.load(Ordering::Acquire) {
            metrics.cancelled.fetch_add(1, Ordering::Relaxed);
            Err(WorkError::Cancelled)
        } else if item
            .deadline
            .is_some_and(|deadline| Instant::now() > deadline)
        {
            metrics.deadline_expired.fetch_add(1, Ordering::Relaxed);
            Err(WorkError::DeadlineExpired)
        } else {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                backend.execute(item.job)
            })) {
                Ok(result) => result,
                Err(_) => {
                    circuit_open.store(true, Ordering::Release);
                    Err(WorkError::WorkerPanicked)
                }
            }
        };
        let outcome = if outcome.is_ok()
            && item
                .deadline
                .is_some_and(|deadline| Instant::now() > deadline)
        {
            circuit_open.store(true, Ordering::Release);
            metrics.deadline_expired.fetch_add(1, Ordering::Relaxed);
            Err(WorkError::DeadlineExpired)
        } else {
            outcome
        };
        match &outcome {
            Ok(_) => {
                metrics.completed.fetch_add(1, Ordering::Relaxed);
            }
            Err(WorkError::Backend(_) | WorkError::WorkerPanicked) => {
                circuit_open.store(true, Ordering::Release);
                metrics.failed.fetch_add(1, Ordering::Relaxed);
            }
            Err(WorkError::Cancelled | WorkError::DeadlineExpired) => {}
            Err(_) => {
                metrics.failed.fetch_add(1, Ordering::Relaxed);
            }
        }
        let _ = item.reply.send(outcome);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct OrderedBackend {
        calls: usize,
        started: Option<mpsc::SyncSender<()>>,
        release: Receiver<()>,
        order: Arc<Mutex<Vec<i8>>>,
    }

    impl Backend for OrderedBackend {
        fn execute(&mut self, job: ComputeJob) -> Result<ComputeResult, WorkError> {
            let ComputeJob::ProjectOccupancy { cells, .. } = job else {
                panic!("unexpected test job")
            };
            self.calls += 1;
            if self.calls == 1 {
                self.started.take().unwrap().send(()).unwrap();
                self.release.recv().unwrap();
            }
            self.order.lock().unwrap().push(cells[0]);
            Ok(ComputeResult::Occupancy(vec![i32::from(cells[0])]))
        }
    }

    struct PanicBackend;

    impl Backend for PanicBackend {
        fn execute(&mut self, _job: ComputeJob) -> Result<ComputeResult, WorkError> {
            panic!("injected backend panic")
        }
    }

    fn occupancy(value: i8) -> ComputeJob {
        ComputeJob::ProjectOccupancy {
            cells: vec![value],
            depth: 2,
        }
    }

    #[test]
    fn cpu_executor_returns_typed_results_and_metrics() {
        let executor = ComputeExecutor::start_cpu(ExecutorConfig { queue_capacity: 2 }).unwrap();
        let ticket = executor
            .submit(JobPriority::Interactive, None, occupancy(100))
            .unwrap();
        assert_eq!(ticket.id().get(), 1);
        assert_eq!(
            ticket.wait().unwrap(),
            ComputeResult::Occupancy(vec![100, 100])
        );
        let metrics = executor.metrics();
        assert_eq!(metrics.submitted, 1);
        assert_eq!(metrics.completed, 1);
    }

    #[test]
    fn expired_job_never_reaches_backend() {
        let executor = ComputeExecutor::start_cpu(ExecutorConfig::default()).unwrap();
        let ticket = executor
            .submit(
                JobPriority::Bulk,
                Some(Instant::now() - Duration::from_millis(1)),
                occupancy(1),
            )
            .unwrap();
        assert_eq!(ticket.wait(), Err(WorkError::DeadlineExpired));
        assert_eq!(executor.metrics().deadline_expired, 1);
        assert!(!executor.status().circuit_open);
    }

    #[test]
    fn cancelled_job_has_an_explicit_outcome() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let (started_tx, started_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::sync_channel(0);
        let backend_order = Arc::clone(&order);
        let executor = ComputeExecutor::start_with(
            ExecutorConfig::default(),
            BackendKind::Cpu,
            BackendKind::Cpu,
            None,
            move || {
                Ok(Box::new(OrderedBackend {
                    calls: 0,
                    started: Some(started_tx),
                    release: release_rx,
                    order: backend_order,
                }))
            },
        )
        .unwrap();
        let first = executor
            .submit(JobPriority::Interactive, None, occupancy(1))
            .unwrap();
        started_rx.recv().unwrap();
        let cancelled = executor
            .submit(JobPriority::Bulk, None, occupancy(2))
            .unwrap();
        cancelled.cancel();
        release_tx.send(()).unwrap();
        first.wait().unwrap();
        assert_eq!(cancelled.wait(), Err(WorkError::Cancelled));
        assert_eq!(executor.metrics().cancelled, 1);
    }

    #[test]
    fn zero_capacity_is_rejected() {
        assert!(matches!(
            ComputeExecutor::start_cpu(ExecutorConfig { queue_capacity: 0 }),
            Err(StartError::ZeroCapacity)
        ));
    }

    #[test]
    fn invalid_inputs_are_typed_failures_without_opening_circuit() {
        let executor = ComputeExecutor::start_cpu(ExecutorConfig::default()).unwrap();
        let ticket = executor
            .submit(
                JobPriority::Interactive,
                None,
                ComputeJob::ProjectOccupancy {
                    cells: vec![1],
                    depth: 0,
                },
            )
            .unwrap();
        assert_eq!(
            ticket.wait(),
            Err(WorkError::InvalidInput(ComputeInputError::ZeroDepth))
        );
        assert!(!executor.status().circuit_open);
    }

    #[test]
    fn interactive_work_preempts_bulk_and_queue_rejects_newest() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let (started_tx, started_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::sync_channel(0);
        let backend_order = Arc::clone(&order);
        let executor = ComputeExecutor::start_with(
            ExecutorConfig { queue_capacity: 2 },
            BackendKind::Cpu,
            BackendKind::Cpu,
            None,
            move || {
                Ok(Box::new(OrderedBackend {
                    calls: 0,
                    started: Some(started_tx),
                    release: release_rx,
                    order: backend_order,
                }))
            },
        )
        .unwrap();
        let first = executor
            .submit(JobPriority::Bulk, None, occupancy(1))
            .unwrap();
        started_rx.recv().unwrap();
        let bulk = executor
            .submit(JobPriority::Bulk, None, occupancy(2))
            .unwrap();
        let interactive = executor
            .submit(JobPriority::Interactive, None, occupancy(3))
            .unwrap();
        assert!(matches!(
            executor.submit(JobPriority::Interactive, None, occupancy(4)),
            Err(SubmitError::QueueFull)
        ));
        assert_eq!(executor.metrics().high_watermark, 2);
        release_tx.send(()).unwrap();
        first.wait().unwrap();
        interactive.wait().unwrap();
        bulk.wait().unwrap();
        assert_eq!(*order.lock().unwrap(), [1, 3, 2]);
    }

    #[test]
    fn backend_panic_opens_the_circuit_without_panicking_the_runtime() {
        let executor = ComputeExecutor::start_with(
            ExecutorConfig::default(),
            BackendKind::Cuda,
            BackendKind::Cuda,
            None,
            || Ok(Box::new(PanicBackend)),
        )
        .unwrap();
        let ticket = executor
            .submit(JobPriority::Interactive, None, occupancy(1))
            .unwrap();
        assert_eq!(ticket.wait(), Err(WorkError::WorkerPanicked));
        assert!(executor.status().circuit_open);
        assert!(matches!(
            executor.submit(JobPriority::Interactive, None, occupancy(2)),
            Err(SubmitError::CircuitOpen)
        ));
    }

    #[test]
    fn initialization_panic_is_a_typed_start_failure() {
        assert!(matches!(
            ComputeExecutor::start_with(
                ExecutorConfig::default(),
                BackendKind::Cuda,
                BackendKind::Cuda,
                None,
                || panic!("injected initialization panic"),
            ),
            Err(StartError::Backend(reason)) if reason == "compute backend initialization panicked"
        ));
    }

    #[test]
    fn resident_cognition_advances_only_due_layers_and_round_trips_checkpoint() {
        let executor = ComputeExecutor::start_cpu(ExecutorConfig::default()).unwrap();
        let initial = cognition_checkpoint_fixture(16, 3);
        assert_eq!(
            executor
                .submit(
                    JobPriority::Interactive,
                    None,
                    ComputeJob::CognitionLoad {
                        checkpoint: initial.clone(),
                    },
                )
                .unwrap()
                .wait(),
            Ok(ComputeResult::CognitionLoaded)
        );
        let result = executor
            .submit(
                JobPriority::Interactive,
                None,
                ComputeJob::CognitionAdvance {
                    sensor: vec![1.0; 16],
                    sensor_precision: 1.0,
                    top_down: vec![0.25; 16],
                    top_precision: 0.5,
                    due_layers: vec![true, false, true],
                    snapshot_layer: Some(2),
                },
            )
            .unwrap()
            .wait()
            .unwrap();
        let ComputeResult::CognitionAdvanced(step) = result else {
            panic!("unexpected cognition result")
        };
        assert_eq!(step.layers[0].sequence, 1);
        assert_eq!(step.layers[1].sequence, 0);
        assert_eq!(step.layers[2].sequence, 1);
        assert_eq!(step.snapshot.unwrap().activation.len(), 16);

        let checkpoint = executor
            .submit(
                JobPriority::Interactive,
                None,
                ComputeJob::CognitionCheckpoint,
            )
            .unwrap()
            .wait()
            .unwrap();
        let ComputeResult::CognitionCheckpoint(checkpoint) = checkpoint else {
            panic!("unexpected checkpoint result")
        };
        assert_ne!(checkpoint, initial);
        validate_cognition_checkpoint(&checkpoint).unwrap();

        let restored = ComputeExecutor::start_cpu(ExecutorConfig::default()).unwrap();
        restored
            .submit(
                JobPriority::Interactive,
                None,
                ComputeJob::CognitionLoad {
                    checkpoint: checkpoint.clone(),
                },
            )
            .unwrap()
            .wait()
            .unwrap();
        let restored_checkpoint = restored
            .submit(
                JobPriority::Interactive,
                None,
                ComputeJob::CognitionCheckpoint,
            )
            .unwrap()
            .wait()
            .unwrap();
        assert_eq!(
            restored_checkpoint,
            ComputeResult::CognitionCheckpoint(checkpoint)
        );
    }

    #[test]
    fn resident_cognition_rejects_malformed_state_and_schedule() {
        let executor = ComputeExecutor::start_cpu(ExecutorConfig::default()).unwrap();
        let mut malformed = cognition_checkpoint_fixture(4, 3);
        malformed.layers[1].bias.pop();
        let result = executor
            .submit(
                JobPriority::Interactive,
                None,
                ComputeJob::CognitionLoad {
                    checkpoint: malformed,
                },
            )
            .unwrap()
            .wait();
        assert_eq!(
            result,
            Err(WorkError::InvalidInput(
                ComputeInputError::InvalidCognitionState
            ))
        );
        assert!(!executor.status().circuit_open);
    }

    fn cognition_checkpoint_fixture(
        dimension: usize,
        layers: usize,
    ) -> ResidentCognitionCheckpoint {
        ResidentCognitionCheckpoint {
            schema_version: RESIDENT_COGNITION_SCHEMA_VERSION.to_string(),
            sensor: vec![0.0; dimension],
            top_down: vec![0.0; dimension],
            layers: (0..layers)
                .map(|index| ResidentCognitionLayer {
                    activation: vec![0.0; dimension],
                    weights: vec![0.75 + index as f32 * 0.05; dimension],
                    bias: vec![0.0; dimension],
                    sequence: 0,
                    precision: 0.0,
                    prediction_error_l2: 0.0,
                })
                .collect(),
        }
    }
}
