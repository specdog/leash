use std::{
    collections::{HashMap, VecDeque},
    fmt,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use leash_cuda::{
    spatial_window_transform_cpu, JobCancellation, JobPriority, LidarPoint, SpatialScan,
};
#[cfg(feature = "cuda")]
use leash_cuda::{ComputeJob, ComputeResult};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Semaphore};

use crate::{
    accelerator::AcceleratorStatus,
    config::AcceleratorBackend,
    types::{SensorDataStatus, TelemetryFrame},
};

pub const COMPUTE_JOB_SCHEMA_VERSION: &str = "leash.compute-job.v1";
pub const SPATIAL_EVIDENCE_SCHEMA_VERSION: &str = "leash.spatial-evidence.v1";
pub const COMPUTE_EVENT_SCHEMA_VERSION: &str = "leash.compute-event.v1";
const MAX_SCANS: usize = 32;
const SCAN_RING_CAPACITY: usize = 64;
const MAX_RAW_POINTS: usize = 20_000;
const MAX_JOBS: usize = 64;
const MAX_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_TIMEOUT_MS: u64 = 2_000;
const DEFAULT_MAX_AGE_MS: u64 = 10_000;
const CUDA_POINT_THRESHOLD: usize = 10_000;
const REQUIRED_SHADOW_MATCHES: u32 = 16;
const PARITY_TOLERANCE_M: f32 = 1.0e-4;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ComputeJobRequest {
    pub schema_version: String,
    #[serde(default)]
    pub job_id: Option<String>,
    pub idempotency_key: String,
    pub job_type: String,
    #[serde(default)]
    pub priority: ComputePriority,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub source: SpatialSourceSelector,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputePriority {
    #[default]
    Interactive,
    Bulk,
}

impl From<ComputePriority> for JobPriority {
    fn from(value: ComputePriority) -> Self {
        match value {
            ComputePriority::Interactive => Self::Interactive,
            ComputePriority::Bulk => Self::Bulk,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SpatialSourceSelector {
    #[serde(default = "default_scan_count")]
    pub scan_count: usize,
    #[serde(default = "default_max_age_ms")]
    pub max_age_ms: u64,
}

impl Default for SpatialSourceSelector {
    fn default() -> Self {
        Self {
            scan_count: default_scan_count(),
            max_age_ms: default_max_age_ms(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputeJobStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl ComputeJobStatus {
    fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ComputeJobSnapshot {
    pub schema_version: String,
    pub job_id: String,
    pub idempotency_key: String,
    pub job_type: String,
    pub status: ComputeJobStatus,
    pub submitted_at_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at_ms: Option<u128>,
    pub source_scans: Vec<SpatialScanReference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<SpatialEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComputeSubmitResponse {
    pub accepted: bool,
    pub idempotent_replay: bool,
    pub job: ComputeJobSnapshot,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpatialScanReference {
    pub producer_epoch: u64,
    pub sequence: u64,
    pub scan_ts_ms: u128,
    pub pose_ts_ms: u128,
    pub scan_frame_id: String,
    pub pose_frame_id: String,
    pub sample_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpatialEvidence {
    pub schema_version: String,
    pub frame_id: String,
    pub source_scans: Vec<SpatialScanReference>,
    pub point_count: usize,
    /// Flat `[x0, y0, x1, y1, ...]` coordinates in the declared frame.
    pub points_xy_m: Vec<f32>,
    pub compute: ComputeReceipt,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComputeReceipt {
    pub requested_backend: String,
    pub authoritative_backend: String,
    pub shadow_compared: bool,
    pub shadow_matches: u32,
    pub cuda_qualified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    pub input_point_count: usize,
    pub elapsed_us: u128,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComputeCapabilities {
    pub schema_version: String,
    pub job_types: Vec<String>,
    pub max_request_bytes: usize,
    pub max_result_bytes: usize,
    pub max_timeout_ms: u64,
    pub max_scans: usize,
    pub max_raw_points: usize,
    pub queue_capacity: usize,
    pub concurrent_jobs: usize,
    pub source_frame: String,
    pub safety_authority: String,
    pub cuda_point_threshold: usize,
    pub required_shadow_matches: u32,
    pub shadow_matches: u32,
    pub cuda_qualified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cuda_disqualified_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComputeEvent {
    pub schema_version: String,
    pub producer_epoch: u64,
    pub sequence: u64,
    pub ts_ms: u128,
    pub job_id: String,
    pub status: ComputeJobStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputeApiErrorKind {
    Invalid,
    Conflict,
    NotFound,
    Unavailable,
    QueueFull,
}

#[derive(Debug, Clone)]
pub struct ComputeApiError {
    pub kind: ComputeApiErrorKind,
    message: String,
}

impl ComputeApiError {
    fn new(kind: ComputeApiErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for ComputeApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ComputeApiError {}

#[derive(Clone)]
pub struct ComputeRuntime {
    inner: Arc<ComputeInner>,
}

struct ComputeInner {
    producer_epoch: u64,
    sequence: AtomicU64,
    next_job: AtomicU64,
    scans: Mutex<VecDeque<CapturedScan>>,
    jobs: Mutex<JobRegistry>,
    events: broadcast::Sender<ComputeEvent>,
    permits: Arc<Semaphore>,
    cuda_configured: bool,
    gate: Mutex<CudaGate>,
}

#[derive(Default)]
struct JobRegistry {
    entries: HashMap<String, JobEntry>,
    idempotency: HashMap<String, String>,
    order: VecDeque<String>,
}

struct JobEntry {
    snapshot: ComputeJobSnapshot,
    request: ComputeJobRequest,
    cancelled: Arc<AtomicBool>,
    accelerator_cancel: Option<JobCancellation>,
}

#[derive(Default)]
struct CudaGate {
    shadow_matches: u32,
    disqualified_reason: Option<String>,
}

#[derive(Debug, Clone)]
struct CapturedScan {
    reference: SpatialScanReference,
    scan: SpatialScan,
}

struct ExecutionOutcome {
    points: Vec<LidarPoint>,
    receipt: ComputeReceipt,
}

impl ComputeRuntime {
    pub fn new(accelerator: &AcceleratorStatus) -> Self {
        let (events, _) = broadcast::channel(128);
        Self {
            inner: Arc::new(ComputeInner {
                producer_epoch: epoch_seed(),
                sequence: AtomicU64::new(0),
                next_job: AtomicU64::new(0),
                scans: Mutex::new(VecDeque::with_capacity(SCAN_RING_CAPACITY)),
                jobs: Mutex::new(JobRegistry::default()),
                events,
                permits: Arc::new(Semaphore::new(2)),
                cuda_configured: accelerator.active == AcceleratorBackend::Cuda,
                gate: Mutex::new(CudaGate::default()),
            }),
        }
    }

    pub fn record_telemetry(&self, telemetry: &TelemetryFrame) {
        let range_scan = &telemetry.sensors.range_scan;
        if range_scan.status != SensorDataStatus::Available {
            return;
        }
        let (Some(sample), Some(pose)) = (&range_scan.sample, &telemetry.odometry_pose) else {
            return;
        };
        if sample.validate().is_err() || pose.pose.frame_id != "odom" {
            return;
        }
        let mut scans = self.inner.scans.lock();
        if scans
            .back()
            .is_some_and(|captured| captured.reference.scan_ts_ms == sample.ts_ms)
        {
            return;
        }
        let sequence = self.inner.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        let captured = CapturedScan {
            reference: SpatialScanReference {
                producer_epoch: self.inner.producer_epoch,
                sequence,
                scan_ts_ms: sample.ts_ms,
                pose_ts_ms: pose.pose.ts_ms,
                scan_frame_id: sample.frame_id.clone(),
                pose_frame_id: pose.pose.frame_id.clone(),
                sample_count: sample.ranges_m.len(),
            },
            scan: SpatialScan {
                ranges_m: sample
                    .ranges_m
                    .iter()
                    .map(|range| range.map_or(f32::NAN, |value| value as f32))
                    .collect(),
                angle_min_rad: sample.angle_min_rad as f32,
                angle_increment_rad: sample.angle_increment_rad as f32,
                range_min_m: sample.range_min_m as f32,
                range_max_m: sample.range_max_m as f32,
                clockwise: false,
                pose_x_m: pose.pose.x_m as f32,
                pose_y_m: pose.pose.y_m as f32,
                pose_yaw_rad: pose.pose.yaw_rad as f32,
            },
        };
        if scans.len() == SCAN_RING_CAPACITY {
            scans.pop_front();
        }
        scans.push_back(captured);
    }

    pub fn capabilities(&self) -> ComputeCapabilities {
        let gate = self.inner.gate.lock();
        ComputeCapabilities {
            schema_version: "leash.compute-capabilities.v1".to_string(),
            job_types: vec!["spatial_window".to_string()],
            max_request_bytes: 1_048_576,
            max_result_bytes: 1_048_576,
            max_timeout_ms: MAX_TIMEOUT_MS,
            max_scans: MAX_SCANS,
            max_raw_points: MAX_RAW_POINTS,
            queue_capacity: MAX_JOBS,
            concurrent_jobs: 2,
            source_frame: "odom".to_string(),
            safety_authority: "advisory-only; CPU collision and motor gates remain authoritative"
                .to_string(),
            cuda_point_threshold: CUDA_POINT_THRESHOLD,
            required_shadow_matches: REQUIRED_SHADOW_MATCHES,
            shadow_matches: gate.shadow_matches,
            cuda_qualified: self.inner.cuda_configured
                && gate.disqualified_reason.is_none()
                && gate.shadow_matches >= REQUIRED_SHADOW_MATCHES,
            cuda_disqualified_reason: gate.disqualified_reason.clone(),
        }
    }

    pub fn submit(
        &self,
        request: ComputeJobRequest,
    ) -> Result<ComputeSubmitResponse, ComputeApiError> {
        validate_request(&request)?;
        {
            let jobs = self.inner.jobs.lock();
            if let Some(job_id) = jobs.idempotency.get(&request.idempotency_key) {
                let job = jobs
                    .entries
                    .get(job_id)
                    .expect("idempotency index is valid");
                if job.request != request {
                    return Err(ComputeApiError::new(
                        ComputeApiErrorKind::Conflict,
                        "idempotency key was already used for a different job",
                    ));
                }
                return Ok(ComputeSubmitResponse {
                    accepted: true,
                    idempotent_replay: true,
                    job: job.snapshot.clone(),
                });
            }
        }

        let scans = self.select_scans(&request.source)?;
        let source_scans = scans
            .iter()
            .map(|captured| captured.reference.clone())
            .collect::<Vec<_>>();
        let job_id = request.job_id.clone().unwrap_or_else(|| {
            let sequence = self.inner.next_job.fetch_add(1, Ordering::Relaxed) + 1;
            format!("leash-{}-{sequence}", self.inner.producer_epoch)
        });
        validate_identifier("job_id", &job_id)?;

        let cancelled = Arc::new(AtomicBool::new(false));
        let snapshot = ComputeJobSnapshot {
            schema_version: COMPUTE_JOB_SCHEMA_VERSION.to_string(),
            job_id: job_id.clone(),
            idempotency_key: request.idempotency_key.clone(),
            job_type: request.job_type.clone(),
            status: ComputeJobStatus::Queued,
            submitted_at_ms: now_ms(),
            started_at_ms: None,
            completed_at_ms: None,
            source_scans,
            result: None,
            error: None,
        };
        {
            let mut jobs = self.inner.jobs.lock();
            evict_terminal_jobs(&mut jobs);
            if jobs.entries.len() >= MAX_JOBS {
                return Err(ComputeApiError::new(
                    ComputeApiErrorKind::QueueFull,
                    "compute job registry is full",
                ));
            }
            if jobs.entries.contains_key(&job_id) {
                return Err(ComputeApiError::new(
                    ComputeApiErrorKind::Conflict,
                    "job_id already exists",
                ));
            }
            jobs.idempotency
                .insert(request.idempotency_key.clone(), job_id.clone());
            jobs.order.push_back(job_id.clone());
            jobs.entries.insert(
                job_id.clone(),
                JobEntry {
                    snapshot: snapshot.clone(),
                    request: request.clone(),
                    cancelled: Arc::clone(&cancelled),
                    accelerator_cancel: None,
                },
            );
        }
        self.emit(&job_id, ComputeJobStatus::Queued);

        let runtime = self.clone();
        tokio::spawn(async move {
            runtime.run_job(job_id, request, scans, cancelled).await;
        });
        Ok(ComputeSubmitResponse {
            accepted: true,
            idempotent_replay: false,
            job: snapshot,
        })
    }

    pub fn get(&self, job_id: &str) -> Result<ComputeJobSnapshot, ComputeApiError> {
        self.inner
            .jobs
            .lock()
            .entries
            .get(job_id)
            .map(|entry| entry.snapshot.clone())
            .ok_or_else(|| ComputeApiError::new(ComputeApiErrorKind::NotFound, "job not found"))
    }

    pub fn cancel(&self, job_id: &str) -> Result<ComputeJobSnapshot, ComputeApiError> {
        let snapshot = {
            let mut jobs = self.inner.jobs.lock();
            let entry = jobs.entries.get_mut(job_id).ok_or_else(|| {
                ComputeApiError::new(ComputeApiErrorKind::NotFound, "job not found")
            })?;
            if entry.snapshot.status.terminal() {
                return Ok(entry.snapshot.clone());
            }
            entry.cancelled.store(true, Ordering::Release);
            if let Some(cancellation) = &entry.accelerator_cancel {
                cancellation.cancel();
            }
            entry.snapshot.status = ComputeJobStatus::Cancelled;
            entry.snapshot.completed_at_ms = Some(now_ms());
            entry.snapshot.error = Some("cancelled by client".to_string());
            entry.snapshot.clone()
        };
        self.emit(job_id, ComputeJobStatus::Cancelled);
        Ok(snapshot)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ComputeEvent> {
        self.inner.events.subscribe()
    }

    fn select_scans(
        &self,
        selector: &SpatialSourceSelector,
    ) -> Result<Vec<CapturedScan>, ComputeApiError> {
        let now = now_ms();
        let cutoff = now.saturating_sub(u128::from(selector.max_age_ms));
        let scans = self.inner.scans.lock();
        let mut selected = scans
            .iter()
            .rev()
            .filter(|captured| captured.reference.scan_ts_ms >= cutoff)
            .take(selector.scan_count)
            .cloned()
            .collect::<Vec<_>>();
        selected.reverse();
        if selected.is_empty() {
            return Err(ComputeApiError::new(
                ComputeApiErrorKind::Unavailable,
                "no fresh range scan with an odom pose is available",
            ));
        }
        let points = selected
            .iter()
            .map(|captured| captured.scan.ranges_m.len())
            .sum::<usize>();
        if points > MAX_RAW_POINTS {
            return Err(ComputeApiError::new(
                ComputeApiErrorKind::Invalid,
                format!("selected scan window has {points} points; maximum is {MAX_RAW_POINTS}"),
            ));
        }
        Ok(selected)
    }

    async fn run_job(
        &self,
        job_id: String,
        request: ComputeJobRequest,
        scans: Vec<CapturedScan>,
        cancelled: Arc<AtomicBool>,
    ) {
        let permit = match Arc::clone(&self.inner.permits).acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => {
                self.fail(&job_id, "compute worker stopped".to_string());
                return;
            }
        };
        if cancelled.load(Ordering::Acquire) {
            drop(permit);
            return;
        }
        self.set_running(&job_id);
        let inner = Arc::clone(&self.inner);
        let execution_job_id = job_id.clone();
        let spatial_scans = scans
            .iter()
            .map(|captured| captured.scan.clone())
            .collect::<Vec<_>>();
        let timeout = Duration::from_millis(request.timeout_ms);
        let execution = tokio::task::spawn_blocking(move || {
            execute_spatial(
                inner,
                &execution_job_id,
                spatial_scans,
                request.priority,
                timeout,
            )
        })
        .await;
        drop(permit);

        if cancelled.load(Ordering::Acquire) {
            self.mark_cancelled_if_needed(&job_id);
            return;
        }
        match execution {
            Ok(Ok(outcome)) => {
                let points_xy_m = outcome
                    .points
                    .iter()
                    .filter(|point| point.valid)
                    .flat_map(|point| [point.x_m, point.y_m])
                    .collect::<Vec<_>>();
                let evidence = SpatialEvidence {
                    schema_version: SPATIAL_EVIDENCE_SCHEMA_VERSION.to_string(),
                    frame_id: "odom".to_string(),
                    source_scans: scans
                        .iter()
                        .map(|captured| captured.reference.clone())
                        .collect(),
                    point_count: points_xy_m.len() / 2,
                    points_xy_m,
                    compute: outcome.receipt,
                };
                self.complete(&job_id, evidence);
            }
            Ok(Err(error)) => self.fail(&job_id, error),
            Err(error) => self.fail(&job_id, format!("compute task failed: {error}")),
        }
    }

    fn set_running(&self, job_id: &str) {
        if let Some(entry) = self.inner.jobs.lock().entries.get_mut(job_id) {
            if entry.snapshot.status == ComputeJobStatus::Queued {
                entry.snapshot.status = ComputeJobStatus::Running;
                entry.snapshot.started_at_ms = Some(now_ms());
                self.emit(job_id, ComputeJobStatus::Running);
            }
        }
    }

    fn complete(&self, job_id: &str, evidence: SpatialEvidence) {
        if let Some(entry) = self.inner.jobs.lock().entries.get_mut(job_id) {
            if entry.snapshot.status == ComputeJobStatus::Cancelled {
                return;
            }
            entry.accelerator_cancel = None;
            entry.snapshot.status = ComputeJobStatus::Completed;
            entry.snapshot.completed_at_ms = Some(now_ms());
            entry.snapshot.result = Some(evidence);
            entry.snapshot.error = None;
            self.emit(job_id, ComputeJobStatus::Completed);
        }
    }

    fn fail(&self, job_id: &str, error: String) {
        if let Some(entry) = self.inner.jobs.lock().entries.get_mut(job_id) {
            if entry.snapshot.status == ComputeJobStatus::Cancelled {
                return;
            }
            entry.accelerator_cancel = None;
            entry.snapshot.status = ComputeJobStatus::Failed;
            entry.snapshot.completed_at_ms = Some(now_ms());
            entry.snapshot.error = Some(error);
            self.emit(job_id, ComputeJobStatus::Failed);
        }
    }

    fn mark_cancelled_if_needed(&self, job_id: &str) {
        if let Some(entry) = self.inner.jobs.lock().entries.get_mut(job_id) {
            if !entry.snapshot.status.terminal() {
                entry.snapshot.status = ComputeJobStatus::Cancelled;
                entry.snapshot.completed_at_ms = Some(now_ms());
                entry.snapshot.error = Some("cancelled by client".to_string());
                self.emit(job_id, ComputeJobStatus::Cancelled);
            }
        }
    }

    fn emit(&self, job_id: &str, status: ComputeJobStatus) {
        let sequence = self.inner.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.inner.events.send(ComputeEvent {
            schema_version: COMPUTE_EVENT_SCHEMA_VERSION.to_string(),
            producer_epoch: self.inner.producer_epoch,
            sequence,
            ts_ms: now_ms(),
            job_id: job_id.to_string(),
            status,
        });
    }
}

fn execute_spatial(
    inner: Arc<ComputeInner>,
    job_id: &str,
    scans: Vec<SpatialScan>,
    priority: ComputePriority,
    timeout: Duration,
) -> Result<ExecutionOutcome, String> {
    let started = Instant::now();
    let input_point_count = scans.iter().map(|scan| scan.ranges_m.len()).sum::<usize>();
    let requested_backend = if inner.cuda_configured && input_point_count >= CUDA_POINT_THRESHOLD {
        "cuda"
    } else {
        "cpu"
    };

    let (points, authoritative_backend, shadow_compared, fallback_reason) = if requested_backend
        == "cpu"
    {
        (
            spatial_window_transform_cpu(&scans).map_err(|error| error.to_string())?,
            "cpu",
            false,
            None,
        )
    } else {
        let gate_snapshot = {
            let gate = inner.gate.lock();
            (gate.shadow_matches, gate.disqualified_reason.clone())
        };
        if let Some(reason) = gate_snapshot.1 {
            (
                spatial_window_transform_cpu(&scans).map_err(|error| error.to_string())?,
                "cpu-fallback",
                false,
                Some(reason),
            )
        } else if gate_snapshot.0 < REQUIRED_SHADOW_MATCHES {
            let cpu = spatial_window_transform_cpu(&scans).map_err(|error| error.to_string())?;
            match execute_cuda(&inner, job_id, scans.clone(), priority, started + timeout) {
                Ok(cuda) if spatial_points_match(&cpu, &cuda) => {
                    inner.gate.lock().shadow_matches += 1;
                    (cpu, "cpu-shadow", true, None)
                }
                Ok(_) => {
                    let reason =
                        "CUDA spatial parity mismatch; CPU remains authoritative".to_string();
                    inner.gate.lock().disqualified_reason = Some(reason.clone());
                    (cpu, "cpu-fallback", true, Some(reason))
                }
                Err(error) => {
                    let reason = format!("CUDA spatial shadow failed: {error}");
                    inner.gate.lock().disqualified_reason = Some(reason.clone());
                    (cpu, "cpu-fallback", true, Some(reason))
                }
            }
        } else {
            match execute_cuda(&inner, job_id, scans.clone(), priority, started + timeout) {
                Ok(cuda) => (cuda, "cuda", false, None),
                Err(error) => {
                    let reason = format!("CUDA spatial execution failed: {error}");
                    inner.gate.lock().disqualified_reason = Some(reason.clone());
                    (
                        spatial_window_transform_cpu(&scans)
                            .map_err(|cpu_error| cpu_error.to_string())?,
                        "cpu-fallback",
                        false,
                        Some(reason),
                    )
                }
            }
        }
    };
    if started.elapsed() > timeout {
        return Err("compute deadline expired".to_string());
    }
    let gate = inner.gate.lock();
    Ok(ExecutionOutcome {
        points,
        receipt: ComputeReceipt {
            requested_backend: requested_backend.to_string(),
            authoritative_backend: authoritative_backend.to_string(),
            shadow_compared,
            shadow_matches: gate.shadow_matches,
            cuda_qualified: inner.cuda_configured
                && gate.disqualified_reason.is_none()
                && gate.shadow_matches >= REQUIRED_SHADOW_MATCHES,
            fallback_reason,
            input_point_count,
            elapsed_us: started.elapsed().as_micros(),
        },
    })
}

#[cfg(feature = "cuda")]
fn execute_cuda(
    inner: &Arc<ComputeInner>,
    job_id: &str,
    scans: Vec<SpatialScan>,
    priority: ComputePriority,
    deadline: Instant,
) -> Result<Vec<LidarPoint>, String> {
    let ticket = crate::cuda_voxel::submit(
        priority.into(),
        Some(deadline),
        ComputeJob::SpatialWindowTransform { scans },
    )
    .map_err(|error| error.to_string())?;
    {
        let mut jobs = inner.jobs.lock();
        if let Some(entry) = jobs.entries.get_mut(job_id) {
            entry.accelerator_cancel = Some(ticket.cancellation());
            if entry.cancelled.load(Ordering::Acquire) {
                ticket.cancel();
            }
        }
    }
    match ticket.wait().map_err(|error| error.to_string())? {
        ComputeResult::SpatialWindow(points) => Ok(points),
        _ => Err("CUDA executor returned the wrong result variant".to_string()),
    }
}

#[cfg(not(feature = "cuda"))]
fn execute_cuda(
    _inner: &Arc<ComputeInner>,
    _job_id: &str,
    _scans: Vec<SpatialScan>,
    _priority: ComputePriority,
    _deadline: Instant,
) -> Result<Vec<LidarPoint>, String> {
    Err("CUDA support is not compiled".to_string())
}

fn spatial_points_match(cpu: &[LidarPoint], cuda: &[LidarPoint]) -> bool {
    cpu.len() == cuda.len()
        && cpu.iter().zip(cuda).all(|(cpu, cuda)| {
            cpu.valid == cuda.valid
                && (!cpu.valid
                    || ((cpu.x_m - cuda.x_m).abs() <= PARITY_TOLERANCE_M
                        && (cpu.y_m - cuda.y_m).abs() <= PARITY_TOLERANCE_M))
        })
}

fn validate_request(request: &ComputeJobRequest) -> Result<(), ComputeApiError> {
    if request.schema_version != COMPUTE_JOB_SCHEMA_VERSION {
        return Err(ComputeApiError::new(
            ComputeApiErrorKind::Invalid,
            format!("schema_version must be {COMPUTE_JOB_SCHEMA_VERSION}"),
        ));
    }
    if request.job_type != "spatial_window" {
        return Err(ComputeApiError::new(
            ComputeApiErrorKind::Invalid,
            "job_type must be spatial_window",
        ));
    }
    validate_identifier("idempotency_key", &request.idempotency_key)?;
    if !(1..=MAX_SCANS).contains(&request.source.scan_count) {
        return Err(ComputeApiError::new(
            ComputeApiErrorKind::Invalid,
            format!("source.scan_count must be between 1 and {MAX_SCANS}"),
        ));
    }
    if request.source.max_age_ms == 0 || request.source.max_age_ms > MAX_TIMEOUT_MS {
        return Err(ComputeApiError::new(
            ComputeApiErrorKind::Invalid,
            format!("source.max_age_ms must be between 1 and {MAX_TIMEOUT_MS}"),
        ));
    }
    if request.timeout_ms == 0 || request.timeout_ms > MAX_TIMEOUT_MS {
        return Err(ComputeApiError::new(
            ComputeApiErrorKind::Invalid,
            format!("timeout_ms must be between 1 and {MAX_TIMEOUT_MS}"),
        ));
    }
    Ok(())
}

fn validate_identifier(name: &str, value: &str) -> Result<(), ComputeApiError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(ComputeApiError::new(
            ComputeApiErrorKind::Invalid,
            format!("{name} must be 1-128 safe identifier characters"),
        ));
    }
    Ok(())
}

fn evict_terminal_jobs(jobs: &mut JobRegistry) {
    while jobs.entries.len() >= MAX_JOBS {
        let Some(index) = jobs.order.iter().position(|job_id| {
            jobs.entries
                .get(job_id)
                .is_some_and(|entry| entry.snapshot.status.terminal())
        }) else {
            break;
        };
        let Some(job_id) = jobs.order.remove(index) else {
            break;
        };
        if let Some(entry) = jobs.entries.remove(&job_id) {
            jobs.idempotency.remove(&entry.snapshot.idempotency_key);
        }
    }
}

const fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT_MS
}

const fn default_scan_count() -> usize {
    MAX_SCANS
}

const fn default_max_age_ms() -> u64 {
    DEFAULT_MAX_AGE_MS
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn epoch_seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    u64::try_from(nanos).unwrap_or(u64::MAX).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        adapter::simulated_range_scan,
        config::HarnessConfig,
        runtime::Harness,
        types::{Pose2d, PoseWithCovariance2d},
    };

    fn attach_fresh_scan_and_pose(telemetry: &mut TelemetryFrame) {
        let ts_ms = now_ms();
        telemetry.sensors.range_scan = simulated_range_scan(ts_ms);
        telemetry.odometry_pose = Some(PoseWithCovariance2d {
            pose: Pose2d {
                ts_ms,
                frame_id: "odom".to_string(),
                x_m: 0.0,
                y_m: 0.0,
                yaw_rad: 0.0,
            },
            covariance: vec![0.0; 9],
        });
    }

    #[tokio::test]
    async fn compute_job_is_idempotent_and_emits_odom_evidence() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();
        let mut telemetry = harness.telemetry();
        attach_fresh_scan_and_pose(&mut telemetry);
        harness.compute().record_telemetry(&telemetry);
        let request = ComputeJobRequest {
            schema_version: COMPUTE_JOB_SCHEMA_VERSION.to_string(),
            job_id: Some("test-job".to_string()),
            idempotency_key: "test-key".to_string(),
            job_type: "spatial_window".to_string(),
            priority: ComputePriority::Interactive,
            timeout_ms: 2_000,
            source: SpatialSourceSelector {
                scan_count: 1,
                max_age_ms: 10_000,
            },
        };
        let first = harness.compute().submit(request.clone()).unwrap();
        assert!(!first.idempotent_replay);
        let replay = harness.compute().submit(request.clone()).unwrap();
        assert!(replay.idempotent_replay);
        let mut conflicting = request;
        conflicting.timeout_ms += 1;
        let conflict = harness.compute().submit(conflicting).unwrap_err();
        assert_eq!(conflict.kind, ComputeApiErrorKind::Conflict);
        for _ in 0..100 {
            let snapshot = harness.compute().get("test-job").unwrap();
            if snapshot.status.terminal() {
                assert_eq!(snapshot.status, ComputeJobStatus::Completed);
                let evidence = snapshot.result.unwrap();
                assert_eq!(evidence.frame_id, "odom");
                assert!(!evidence.points_xy_m.is_empty());
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("compute job did not finish");
    }

    #[tokio::test]
    async fn stale_or_unposed_scans_are_not_selected() {
        let accelerator =
            crate::accelerator::resolve_accelerator(AcceleratorBackend::Cpu, false).unwrap();
        let runtime = ComputeRuntime::new(&accelerator);
        let mut telemetry = Harness::new(HarnessConfig::default()).unwrap().telemetry();
        attach_fresh_scan_and_pose(&mut telemetry);
        telemetry.odometry_pose = None;
        runtime.record_telemetry(&telemetry);
        let error = runtime
            .select_scans(&SpatialSourceSelector {
                scan_count: 1,
                max_age_ms: 10_000,
            })
            .unwrap_err();
        assert_eq!(error.kind, ComputeApiErrorKind::Unavailable);
    }
}
