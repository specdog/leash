use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{bail, Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;

use crate::{accelerator::AcceleratorStatus, config::AcceleratorBackend, types::TelemetryFrame};

pub const COGNITION_CONTRACT_VERSION: &str = "leash.cognition.v1";
pub const COGNITION_STATE_VERSION: &str = "leash.cognition-state.v1";
pub const COGNITION_STATE_DIM: usize = 1_024;
pub const COGNITION_BOUNDARY_TIMEOUT_MS: u128 = 500;
pub const COGNITION_CHECKPOINT_INTERVAL_MS: u128 = 60_000;
pub const SENSOR_LAYER: u8 = 7;
pub const LEASH_LAYER_COUNT: usize = 3;
const LAYER_CADENCE_HZ: [f32; LEASH_LAYER_COUNT] = [200.0, 100.0, 20.0];
const LAYER_INTERVAL_MS: [u128; LEASH_LAYER_COUNT] = [5, 10, 50];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct CognitionCapabilitiesV1 {
    pub schema_version: String,
    pub runtime: String,
    pub owner: String,
    pub state_dim: usize,
    pub owned_layers: Vec<u8>,
    pub sensor_plane: u8,
    pub backend: String,
    pub cadences_hz: Vec<f32>,
    pub cross_boundary_timeout_ms: u128,
    pub checkpoint_interval_ms: u128,
    pub semantic_prior_target_layer: u8,
    pub motor_authority: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct CognitionLayerSnapshotV1 {
    pub schema_version: String,
    pub ts_ms: u128,
    pub layer: u8,
    pub owner: String,
    pub cadence_hz: f32,
    pub sequence: u64,
    pub precision: f32,
    pub prediction_error_l2: f32,
    pub activation_mean: f32,
    pub activation_rms: f32,
    pub fresh: bool,
    pub source_ts_ms: Option<u128>,
    pub source_age_ms: Option<u128>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct CognitionSnapshotsV1 {
    pub schema_version: String,
    pub layers: Vec<CognitionLayerSnapshotV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct CognitionBoundaryFrameV1 {
    pub schema_version: String,
    pub ts_ms: u128,
    pub expires_at_ms: u128,
    pub source: String,
    pub destination: String,
    pub layer: u8,
    pub sequence: u64,
    pub precision: f32,
    pub state_digest: String,
    /// A 1024-value latent state (4 KiB as f32), never model weights.
    pub latent: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct SemanticPriorV1 {
    pub schema_version: String,
    pub prior_id: String,
    pub proposition: String,
    pub evidence_refs: Vec<String>,
    pub confidence: f32,
    pub created_at_ms: u128,
    pub expires_at_ms: u128,
    pub target_layer: u8,
    pub source: String,
}

impl SemanticPriorV1 {
    pub fn validate(&self, now_ms: u128) -> Result<()> {
        if self.schema_version != COGNITION_CONTRACT_VERSION {
            bail!("unsupported cognition contract version");
        }
        if self.prior_id.trim().is_empty() || self.proposition.trim().is_empty() {
            bail!("semantic prior id and proposition are required");
        }
        if self.evidence_refs.is_empty()
            || self
                .evidence_refs
                .iter()
                .any(|reference| reference.trim().is_empty())
        {
            bail!("semantic priors require non-empty evidence references");
        }
        if !(0.0..=1.0).contains(&self.confidence) || !self.confidence.is_finite() {
            bail!("semantic prior confidence must be finite and within 0..=1");
        }
        if self.target_layer != 6 {
            bail!("semantic priors may only target cognition layer 6");
        }
        if self.expires_at_ms <= self.created_at_ms || self.expires_at_ms <= now_ms {
            bail!("semantic prior is expired or has an invalid lifetime");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct CognitionCheckpointV1 {
    pub schema_version: String,
    pub checkpoint_id: String,
    pub created_at_ms: u128,
    pub runtime: String,
    pub backend: String,
    pub layer_sequences: Vec<u64>,
    pub state_digest: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct CognitionStatusV1 {
    pub ok: bool,
    pub capabilities: CognitionCapabilitiesV1,
    pub layers: Vec<CognitionLayerSnapshotV1>,
    pub boundary: CognitionBoundaryFrameV1,
    pub last_checkpoint: Option<CognitionCheckpointV1>,
    pub backend_status: CognitionBackendStatusV1,
    pub zero_motion: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct CognitionBackendStatusV1 {
    pub selected: String,
    pub active: String,
    pub degraded: bool,
    pub fallback_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CheckpointPayload {
    contract: CognitionCheckpointV1,
    state: CognitionCheckpointStateV1,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct CognitionCheckpointLayerV1 {
    activation: Vec<f32>,
    weights: Vec<f32>,
    bias: Vec<f32>,
    sequence: u64,
    precision: f32,
    prediction_error_l2: f32,
    activation_mean: f32,
    activation_rms: f32,
    last_tick_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct CognitionCheckpointStateV1 {
    schema_version: String,
    sensor: Vec<f32>,
    sensor_ts_ms: Option<u128>,
    layers: Vec<CognitionCheckpointLayerV1>,
    top_down: Vec<f32>,
    top_down_precision: f32,
    top_down_expires_at_ms: u128,
}

#[derive(Debug, Clone)]
struct LayerState {
    activation: Vec<f32>,
    weights: Vec<f32>,
    bias: Vec<f32>,
    sequence: u64,
    precision: f32,
    prediction_error_l2: f32,
    activation_mean: f32,
    activation_rms: f32,
    last_tick_ms: u128,
}

impl LayerState {
    fn new(layer: usize) -> Self {
        Self {
            activation: vec![0.0; COGNITION_STATE_DIM],
            weights: vec![0.75 + layer as f32 * 0.05; COGNITION_STATE_DIM],
            bias: vec![0.0; COGNITION_STATE_DIM],
            sequence: 0,
            precision: 0.0,
            prediction_error_l2: 0.0,
            activation_mean: 0.0,
            activation_rms: 0.0,
            last_tick_ms: 0,
        }
    }
}

#[derive(Debug)]
struct CognitionState {
    sensor: Vec<f32>,
    sensor_ts_ms: Option<u128>,
    layers: Vec<LayerState>,
    top_down: Vec<f32>,
    top_down_precision: f32,
    top_down_expires_at_ms: u128,
    last_checkpoint_at_ms: u128,
    last_checkpoint: Option<CognitionCheckpointV1>,
}

impl Default for CognitionState {
    fn default() -> Self {
        Self {
            sensor: vec![0.0; COGNITION_STATE_DIM],
            sensor_ts_ms: None,
            layers: (0..LEASH_LAYER_COUNT).map(LayerState::new).collect(),
            top_down: vec![0.0; COGNITION_STATE_DIM],
            top_down_precision: 0.0,
            top_down_expires_at_ms: 0,
            last_checkpoint_at_ms: 0,
            last_checkpoint: None,
        }
    }
}

#[derive(Clone)]
pub struct CognitionRuntime {
    state: Arc<Mutex<CognitionState>>,
    backend_status: Arc<Mutex<CognitionBackendStatusV1>>,
    compute_lock: Arc<Mutex<()>>,
    owner: Arc<str>,
    checkpoint_dir: Arc<PathBuf>,
    boundary_tx: broadcast::Sender<CognitionBoundaryFrameV1>,
}

impl CognitionRuntime {
    pub fn new(accelerator: &AcceleratorStatus, owner: &str) -> Self {
        let (boundary_tx, _) = broadcast::channel(32);
        let state = CognitionState {
            last_checkpoint_at_ms: now_ms(),
            ..CognitionState::default()
        };
        let selected = if accelerator.requested == AcceleratorBackend::Cuda {
            "cuda"
        } else {
            "cpu"
        };
        let mut backend_status = CognitionBackendStatusV1 {
            selected: selected.to_string(),
            active: "cpu".to_string(),
            degraded: accelerator.requested == AcceleratorBackend::Cuda
                && accelerator.active != AcceleratorBackend::Cuda,
            fallback_reason: (accelerator.requested == AcceleratorBackend::Cuda
                && accelerator.active != AcceleratorBackend::Cuda)
                .then(|| accelerator.message.clone()),
        };
        if accelerator.active == AcceleratorBackend::Cuda {
            backend_status.active = "cuda".to_string();
        }
        let runtime = Self {
            state: Arc::new(Mutex::new(state)),
            backend_status: Arc::new(Mutex::new(backend_status)),
            compute_lock: Arc::new(Mutex::new(())),
            owner: Arc::from(owner.trim()),
            checkpoint_dir: Arc::new(default_checkpoint_dir()),
            boundary_tx,
        };
        if accelerator.active == AcceleratorBackend::Cuda {
            #[cfg(feature = "cuda")]
            if let Err(error) = runtime.load_cuda_from_host() {
                runtime.fallback_to_cpu(format!("initialize resident CUDA cognition: {error}"));
            }
            #[cfg(not(feature = "cuda"))]
            runtime.fallback_to_cpu(
                "CUDA was selected but the cognition CUDA feature is not compiled".to_string(),
            );
        }
        runtime
    }

    pub fn capabilities(&self) -> CognitionCapabilitiesV1 {
        let backend = self.backend_status.lock().active.clone();
        CognitionCapabilitiesV1 {
            schema_version: COGNITION_CONTRACT_VERSION.to_string(),
            runtime: "leash".to_string(),
            owner: self.owner.to_string(),
            state_dim: COGNITION_STATE_DIM,
            owned_layers: vec![0, 1, 2],
            sensor_plane: SENSOR_LAYER,
            backend,
            cadences_hz: LAYER_CADENCE_HZ.to_vec(),
            cross_boundary_timeout_ms: COGNITION_BOUNDARY_TIMEOUT_MS,
            checkpoint_interval_ms: COGNITION_CHECKPOINT_INTERVAL_MS,
            semantic_prior_target_layer: 6,
            motor_authority: false,
        }
    }

    pub fn ingest_telemetry(&self, telemetry: &TelemetryFrame) {
        let encoded = encode_sensor_frame(telemetry);
        let mut state = self.state.lock();
        state.sensor.copy_from_slice(&encoded);
        state.sensor_ts_ms = Some(telemetry.ts_ms);
    }

    pub fn tick(&self, now_ms: u128) {
        let outcome: Result<(bool, bool)> = if self.backend_status.lock().active == "cuda" {
            #[cfg(feature = "cuda")]
            {
                self.tick_cuda(now_ms)
            }
            #[cfg(not(feature = "cuda"))]
            {
                unreachable!("CUDA cognition cannot be active without the CUDA feature")
            }
        } else {
            Ok(self.tick_cpu(now_ms))
        };
        let (publish_boundary, should_checkpoint) = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                self.mark_cuda_failed(error.to_string());
                return;
            }
        };

        if publish_boundary {
            let _ = self.boundary_tx.send(self.boundary(now_ms));
        }
        if should_checkpoint {
            if let Err(error) = self.checkpoint_at(now_ms) {
                tracing::warn!(?error, "cognition checkpoint failed");
            }
        }
    }

    fn tick_cpu(&self, now_ms: u128) -> (bool, bool) {
        let mut publish_boundary = false;
        let mut state = self.state.lock();
        let sensor_precision = freshness_precision(state.sensor_ts_ms, now_ms);
        let external_precision = if now_ms <= state.top_down_expires_at_ms {
            state.top_down_precision
        } else {
            0.0
        };

        for (layer_index, interval_ms) in LAYER_INTERVAL_MS.iter().copied().enumerate() {
            if state.layers[layer_index].last_tick_ms != 0
                && now_ms.saturating_sub(state.layers[layer_index].last_tick_ms) < interval_ms
            {
                continue;
            }
            let lower = if layer_index == 0 {
                state.sensor.clone()
            } else {
                state.layers[layer_index - 1].activation.clone()
            };
            let lower_precision = if layer_index == 0 {
                sensor_precision
            } else {
                state.layers[layer_index - 1].precision
            };
            let (top_down, top_precision) = if layer_index + 1 < LEASH_LAYER_COUNT {
                (
                    state.layers[layer_index + 1].activation.clone(),
                    state.layers[layer_index + 1].precision,
                )
            } else {
                (state.top_down.clone(), external_precision)
            };
            update_layer(
                &mut state.layers[layer_index],
                &lower,
                &top_down,
                lower_precision,
                top_precision,
                now_ms,
            );
            if layer_index == 2 {
                publish_boundary = true;
            }
        }
        let should_checkpoint =
            now_ms.saturating_sub(state.last_checkpoint_at_ms) >= COGNITION_CHECKPOINT_INTERVAL_MS;
        (publish_boundary, should_checkpoint)
    }

    #[cfg(feature = "cuda")]
    fn tick_cuda(&self, now_ms: u128) -> Result<(bool, bool)> {
        let _compute = self.compute_lock.lock();
        let (sensor, sensor_precision, top_down, top_precision, due_layers, should_checkpoint) = {
            let state = self.state.lock();
            let sensor_precision = freshness_precision(state.sensor_ts_ms, now_ms);
            let top_precision = if now_ms <= state.top_down_expires_at_ms {
                state.top_down_precision
            } else {
                0.0
            };
            let due_layers = LAYER_INTERVAL_MS
                .iter()
                .enumerate()
                .map(|(index, interval_ms)| {
                    state.layers[index].last_tick_ms == 0
                        || now_ms.saturating_sub(state.layers[index].last_tick_ms) >= *interval_ms
                })
                .collect::<Vec<_>>();
            (
                state.sensor.clone(),
                sensor_precision,
                state.top_down.clone(),
                top_precision,
                due_layers,
                now_ms.saturating_sub(state.last_checkpoint_at_ms)
                    >= COGNITION_CHECKPOINT_INTERVAL_MS,
            )
        };
        let publish_boundary = due_layers[2];
        let result = crate::cuda_voxel::execute(leash_cuda::ComputeJob::CognitionAdvance {
            sensor,
            sensor_precision,
            top_down,
            top_precision,
            due_layers: due_layers.clone(),
            snapshot_layer: publish_boundary.then_some(2),
        })?;
        let leash_cuda::ComputeResult::CognitionAdvanced(step) = result else {
            bail!("CUDA cognition returned the wrong result variant");
        };
        if step.layers.len() != LEASH_LAYER_COUNT {
            bail!("CUDA cognition returned the wrong layer count");
        }
        let mut state = self.state.lock();
        for (index, metrics) in step.layers.into_iter().enumerate() {
            let layer = &mut state.layers[index];
            layer.sequence = metrics.sequence;
            layer.precision = metrics.precision;
            layer.prediction_error_l2 = metrics.prediction_error_l2;
            layer.activation_mean = metrics.activation_mean;
            layer.activation_rms = metrics.activation_rms;
            if due_layers[index] {
                layer.last_tick_ms = now_ms;
            }
        }
        if let Some(snapshot) = step.snapshot {
            if snapshot.layer != 2 || snapshot.activation.len() != COGNITION_STATE_DIM {
                bail!("CUDA cognition returned a malformed layer snapshot");
            }
            state.layers[2].activation = snapshot.activation;
        } else if publish_boundary {
            bail!("CUDA cognition omitted the declared layer-2 snapshot");
        }
        Ok((publish_boundary, should_checkpoint))
    }

    pub fn snapshots(&self, now_ms: u128) -> Vec<CognitionLayerSnapshotV1> {
        let state = self.state.lock();
        let source_age_ms = state.sensor_ts_ms.map(|ts| now_ms.saturating_sub(ts));
        state
            .layers
            .iter()
            .enumerate()
            .map(|(index, layer)| {
                layer_snapshot(index, layer, state.sensor_ts_ms, source_age_ms, now_ms)
            })
            .collect()
    }

    pub fn boundary(&self, now_ms: u128) -> CognitionBoundaryFrameV1 {
        let state = self.state.lock();
        let layer = &state.layers[2];
        boundary_from_layer(layer, now_ms)
    }

    pub fn status(&self, now_ms: u128, zero_motion: bool) -> CognitionStatusV1 {
        let backend_status = self.backend_status.lock().clone();
        CognitionStatusV1 {
            ok: !backend_status.degraded,
            capabilities: self.capabilities(),
            layers: self.snapshots(now_ms),
            boundary: self.boundary(now_ms),
            last_checkpoint: self.state.lock().last_checkpoint.clone(),
            backend_status,
            zero_motion,
        }
    }

    pub fn submit_boundary(&self, frame: CognitionBoundaryFrameV1, now_ms: u128) -> Result<()> {
        validate_boundary(&frame, now_ms, "planner", "leash", 3)?;
        let mut state = self.state.lock();
        state.top_down.copy_from_slice(&frame.latent);
        state.top_down_precision = frame.precision;
        state.top_down_expires_at_ms = frame
            .expires_at_ms
            .min(now_ms.saturating_add(COGNITION_BOUNDARY_TIMEOUT_MS));
        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CognitionBoundaryFrameV1> {
        self.boundary_tx.subscribe()
    }

    pub fn checkpoint(&self) -> Result<CognitionCheckpointV1> {
        self.checkpoint_at(now_ms())
    }

    pub fn restore_from_checkpoint(
        accelerator: &AcceleratorStatus,
        owner: &str,
        path: impl AsRef<Path>,
    ) -> Result<Self> {
        let path = path.as_ref();
        let bytes = fs::read(path)
            .with_context(|| format!("read cognition checkpoint {}", path.display()))?;
        let payload: CheckpointPayload =
            serde_json::from_slice(&bytes).context("decode cognition checkpoint")?;
        validate_checkpoint_payload(&payload)?;
        let runtime = Self::new(accelerator, owner);
        let restored = runtime_state_from_checkpoint(&payload.state, &payload.contract)?;
        *runtime.state.lock() = restored;
        if runtime.backend_status.lock().active == "cuda" {
            #[cfg(feature = "cuda")]
            if let Err(error) = runtime.load_cuda_from_host() {
                runtime.fallback_to_cpu(format!("restore resident CUDA cognition: {error}"));
            }
        }
        Ok(runtime)
    }

    fn checkpoint_at(&self, created_at_ms: u128) -> Result<CognitionCheckpointV1> {
        fs::create_dir_all(self.checkpoint_dir.as_ref()).with_context(|| {
            format!(
                "create cognition checkpoint directory {}",
                self.checkpoint_dir.display()
            )
        })?;
        let _compute = self.compute_lock.lock();
        let active_backend = self.backend_status.lock().active.clone();
        #[cfg(feature = "cuda")]
        let resident = if active_backend == "cuda" {
            let result = crate::cuda_voxel::execute(leash_cuda::ComputeJob::CognitionCheckpoint)?;
            let leash_cuda::ComputeResult::CognitionCheckpoint(checkpoint) = result else {
                bail!("CUDA cognition returned the wrong checkpoint variant");
            };
            Some(checkpoint)
        } else {
            None
        };
        let mut state = self.state.lock();
        #[cfg(feature = "cuda")]
        let checkpoint_state = resident.as_ref().map_or_else(
            || checkpoint_state_from_host(&state),
            |resident| checkpoint_state_from_resident(&state, resident),
        );
        #[cfg(not(feature = "cuda"))]
        let checkpoint_state = checkpoint_state_from_host(&state);
        let digest = checkpoint_state_digest(&checkpoint_state);
        let checkpoint_id = format!("leash-{created_at_ms}-{}", &digest[..12]);
        let path = self.checkpoint_dir.join(format!("{checkpoint_id}.json"));
        let contract = CognitionCheckpointV1 {
            schema_version: COGNITION_CONTRACT_VERSION.to_string(),
            checkpoint_id,
            created_at_ms,
            runtime: "leash".to_string(),
            backend: active_backend,
            layer_sequences: checkpoint_state
                .layers
                .iter()
                .map(|layer| layer.sequence)
                .collect(),
            state_digest: digest,
            path: path.display().to_string(),
        };
        let payload = CheckpointPayload {
            contract: contract.clone(),
            state: checkpoint_state,
        };
        let bytes = serde_json::to_vec(&payload).context("serialize cognition checkpoint")?;
        fs::write(&path, bytes)
            .with_context(|| format!("write cognition checkpoint {}", path.display()))?;
        state.last_checkpoint_at_ms = created_at_ms;
        state.last_checkpoint = Some(contract.clone());
        Ok(contract)
    }

    #[cfg(feature = "cuda")]
    fn load_cuda_from_host(&self) -> Result<()> {
        let _compute = self.compute_lock.lock();
        let checkpoint = resident_checkpoint_from_host(&self.state.lock());
        let result =
            crate::cuda_voxel::execute(leash_cuda::ComputeJob::CognitionLoad { checkpoint })?;
        if !matches!(result, leash_cuda::ComputeResult::CognitionLoaded) {
            bail!("CUDA cognition returned the wrong load result");
        }
        let status = crate::cuda_voxel::backend_status()?;
        if status.active != leash_cuda::BackendKind::Cuda || status.degraded || status.circuit_open
        {
            bail!("CUDA executor is not healthy after cognition load");
        }
        Ok(())
    }

    fn fallback_to_cpu(&self, reason: String) {
        let mut status = self.backend_status.lock();
        status.active = "cpu".to_string();
        status.degraded = true;
        status.fallback_reason = Some(reason);
    }

    fn mark_cuda_failed(&self, reason: String) {
        let mut status = self.backend_status.lock();
        status.degraded = true;
        status.fallback_reason = Some(reason);
    }
}

fn checkpoint_state_from_host(state: &CognitionState) -> CognitionCheckpointStateV1 {
    CognitionCheckpointStateV1 {
        schema_version: COGNITION_STATE_VERSION.to_string(),
        sensor: state.sensor.clone(),
        sensor_ts_ms: state.sensor_ts_ms,
        layers: state
            .layers
            .iter()
            .map(|layer| CognitionCheckpointLayerV1 {
                activation: layer.activation.clone(),
                weights: layer.weights.clone(),
                bias: layer.bias.clone(),
                sequence: layer.sequence,
                precision: layer.precision,
                prediction_error_l2: layer.prediction_error_l2,
                activation_mean: layer.activation_mean,
                activation_rms: layer.activation_rms,
                last_tick_ms: layer.last_tick_ms,
            })
            .collect(),
        top_down: state.top_down.clone(),
        top_down_precision: state.top_down_precision,
        top_down_expires_at_ms: state.top_down_expires_at_ms,
    }
}

#[cfg(feature = "cuda")]
fn resident_checkpoint_from_host(
    state: &CognitionState,
) -> leash_cuda::ResidentCognitionCheckpoint {
    leash_cuda::ResidentCognitionCheckpoint {
        schema_version: leash_cuda::RESIDENT_COGNITION_SCHEMA_VERSION.to_string(),
        sensor: state.sensor.clone(),
        top_down: state.top_down.clone(),
        layers: state
            .layers
            .iter()
            .map(|layer| leash_cuda::ResidentCognitionLayer {
                activation: layer.activation.clone(),
                weights: layer.weights.clone(),
                bias: layer.bias.clone(),
                sequence: layer.sequence,
                precision: layer.precision,
                prediction_error_l2: layer.prediction_error_l2,
            })
            .collect(),
    }
}

#[cfg(feature = "cuda")]
fn checkpoint_state_from_resident(
    host: &CognitionState,
    resident: &leash_cuda::ResidentCognitionCheckpoint,
) -> CognitionCheckpointStateV1 {
    CognitionCheckpointStateV1 {
        schema_version: COGNITION_STATE_VERSION.to_string(),
        sensor: resident.sensor.clone(),
        sensor_ts_ms: host.sensor_ts_ms,
        layers: resident
            .layers
            .iter()
            .zip(&host.layers)
            .map(|(resident, host)| {
                let (activation_mean, activation_rms) = activation_metrics(&resident.activation);
                CognitionCheckpointLayerV1 {
                    activation: resident.activation.clone(),
                    weights: resident.weights.clone(),
                    bias: resident.bias.clone(),
                    sequence: resident.sequence,
                    precision: resident.precision,
                    prediction_error_l2: resident.prediction_error_l2,
                    activation_mean,
                    activation_rms,
                    last_tick_ms: host.last_tick_ms,
                }
            })
            .collect(),
        top_down: resident.top_down.clone(),
        top_down_precision: host.top_down_precision,
        top_down_expires_at_ms: host.top_down_expires_at_ms,
    }
}

fn validate_checkpoint_payload(payload: &CheckpointPayload) -> Result<()> {
    if payload.contract.schema_version != COGNITION_CONTRACT_VERSION
        || payload.contract.runtime != "leash"
        || payload.state.schema_version != COGNITION_STATE_VERSION
        || payload.state.sensor.len() != COGNITION_STATE_DIM
        || payload.state.top_down.len() != COGNITION_STATE_DIM
        || payload.state.layers.len() != LEASH_LAYER_COUNT
        || payload
            .state
            .sensor
            .iter()
            .chain(&payload.state.top_down)
            .any(|value| !value.is_finite())
        || !valid_precision(payload.state.top_down_precision)
    {
        bail!("cognition checkpoint state contract is invalid");
    }
    for layer in &payload.state.layers {
        if layer.activation.len() != COGNITION_STATE_DIM
            || layer.weights.len() != COGNITION_STATE_DIM
            || layer.bias.len() != COGNITION_STATE_DIM
            || layer
                .activation
                .iter()
                .chain(&layer.weights)
                .chain(&layer.bias)
                .any(|value| !value.is_finite())
            || !valid_precision(layer.precision)
            || !layer.prediction_error_l2.is_finite()
            || layer.prediction_error_l2 < 0.0
            || !layer.activation_mean.is_finite()
            || !layer.activation_rms.is_finite()
            || layer.activation_rms < 0.0
        {
            bail!("cognition checkpoint layer is invalid");
        }
    }
    let layer_sequences = payload
        .state
        .layers
        .iter()
        .map(|layer| layer.sequence)
        .collect::<Vec<_>>();
    if payload.contract.layer_sequences != layer_sequences
        || payload.contract.state_digest != checkpoint_state_digest(&payload.state)
    {
        bail!("cognition checkpoint identity does not match its state");
    }
    Ok(())
}

fn runtime_state_from_checkpoint(
    checkpoint: &CognitionCheckpointStateV1,
    contract: &CognitionCheckpointV1,
) -> Result<CognitionState> {
    let payload = CheckpointPayload {
        contract: contract.clone(),
        state: checkpoint.clone(),
    };
    validate_checkpoint_payload(&payload)?;
    Ok(CognitionState {
        sensor: checkpoint.sensor.clone(),
        sensor_ts_ms: checkpoint.sensor_ts_ms,
        layers: checkpoint
            .layers
            .iter()
            .map(|layer| LayerState {
                activation: layer.activation.clone(),
                weights: layer.weights.clone(),
                bias: layer.bias.clone(),
                sequence: layer.sequence,
                precision: layer.precision,
                prediction_error_l2: layer.prediction_error_l2,
                activation_mean: layer.activation_mean,
                activation_rms: layer.activation_rms,
                last_tick_ms: layer.last_tick_ms,
            })
            .collect(),
        top_down: checkpoint.top_down.clone(),
        top_down_precision: checkpoint.top_down_precision,
        top_down_expires_at_ms: checkpoint.top_down_expires_at_ms,
        last_checkpoint_at_ms: contract.created_at_ms,
        last_checkpoint: Some(contract.clone()),
    })
}

fn valid_precision(value: f32) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

pub fn validate_boundary(
    frame: &CognitionBoundaryFrameV1,
    now_ms: u128,
    expected_source: &str,
    expected_destination: &str,
    expected_layer: u8,
) -> Result<()> {
    if frame.schema_version != COGNITION_CONTRACT_VERSION {
        bail!("unsupported cognition contract version");
    }
    if frame.source != expected_source || frame.destination != expected_destination {
        bail!("cognition boundary source or destination mismatch");
    }
    if frame.layer != expected_layer {
        bail!("unexpected cognition boundary layer");
    }
    if frame.latent.len() != COGNITION_STATE_DIM {
        bail!("cognition boundary latent must contain {COGNITION_STATE_DIM} values");
    }
    if frame.latent.iter().any(|value| !value.is_finite()) {
        bail!("cognition boundary latent contains non-finite values");
    }
    if !(0.0..=1.0).contains(&frame.precision) || !frame.precision.is_finite() {
        bail!("cognition boundary precision must be finite and within 0..=1");
    }
    if frame.expires_at_ms <= now_ms
        || frame.expires_at_ms.saturating_sub(frame.ts_ms) > COGNITION_BOUNDARY_TIMEOUT_MS
    {
        bail!("cognition boundary is expired or exceeds the 500 ms freshness budget");
    }
    if state_digest(&frame.latent) != frame.state_digest {
        bail!("cognition boundary state digest mismatch");
    }
    Ok(())
}

pub fn encode_sensor_frame(telemetry: &TelemetryFrame) -> Vec<f32> {
    let mut encoded = vec![0.0; COGNITION_STATE_DIM];

    // 0..360: normalized 360-degree LiDAR ranges.
    if let Some(scan) = telemetry.sensors.range_scan.sample.as_ref() {
        if !scan.ranges_m.is_empty() {
            for (output_index, output) in encoded[..360].iter_mut().enumerate() {
                let source_index = output_index * scan.ranges_m.len() / 360;
                *output = scan.ranges_m[source_index]
                    .map(|range| (range / scan.range_max_m).clamp(0.0, 1.0) as f32)
                    .unwrap_or(0.0);
            }
        }
    }

    // 360..616: deterministic camera/detection features. Raw pixels stay in the
    // camera pipeline; only compact evidence enters cognition.
    for detection in telemetry.vision.detections.iter().take(32) {
        let mut hash = Sha256::new();
        hash.update(detection.label.as_bytes());
        let digest = hash.finalize();
        let bucket = 360 + usize::from(digest[0]);
        encoded[bucket] = encoded[bucket].max(detection.confidence.clamp(0.0, 1.0) as f32);
    }

    // 616..872: occupancy and height evidence.
    let cells = &telemetry.occupancy_grid.cells;
    if !cells.is_empty() {
        for output_index in 0..256 {
            let source_index = output_index * cells.len() / 256;
            encoded[616 + output_index] =
                (f32::from(cells[source_index]).max(0.0) / 100.0).clamp(0.0, 1.0);
        }
    }
    for voxel in telemetry
        .voxel_grid
        .voxels
        .iter()
        .filter(|voxel| voxel.occupancy > 0)
    {
        let bucket =
            616 + ((voxel.x as usize * 31 + voxel.y as usize * 17 + voxel.z as usize) % 256);
        encoded[bucket] = encoded[bucket].max(f32::from(voxel.occupancy) / 100.0);
    }

    // 872..896: IMU, odometry, and commanded-action evidence.
    if let Some(sample) = telemetry.sensors.imu.sample.as_ref() {
        encoded[872] = (sample.linear_acceleration_mps2.x / 20.0) as f32;
        encoded[873] = (sample.linear_acceleration_mps2.y / 20.0) as f32;
        encoded[874] = (sample.linear_acceleration_mps2.z / 20.0) as f32;
        encoded[875] = (sample.angular_velocity_radps.x / 10.0) as f32;
        encoded[876] = (sample.angular_velocity_radps.y / 10.0) as f32;
        encoded[877] = (sample.angular_velocity_radps.z / 10.0) as f32;
        if let Some(orientation) = sample.orientation_xyzw {
            encoded[878..882].copy_from_slice(&[
                orientation.x as f32,
                orientation.y as f32,
                orientation.z as f32,
                orientation.w as f32,
            ]);
        }
    }
    encoded[882] = telemetry.odometry_left.unwrap_or_default() as f32;
    encoded[883] = telemetry.odometry_right.unwrap_or_default() as f32;
    encoded[884] = telemetry.left_cmd as f32;
    encoded[885] = telemetry.right_cmd as f32;
    if let Some(odometry) = telemetry.odometry_pose.as_ref() {
        encoded[886] = odometry.pose.x_m as f32;
        encoded[887] = odometry.pose.y_m as f32;
        encoded[888] = odometry.pose.yaw_rad as f32;
        encoded[889] = odometry.covariance.first().copied().unwrap_or_default() as f32;
    }

    // 896..960: freshness/calibration plane. 960..1024 is reserved and zero.
    encoded[896] = freshness_precision(telemetry.sensors.range_scan.last_ms, telemetry.ts_ms);
    encoded[897] = freshness_precision(telemetry.sensors.imu.last_ms, telemetry.ts_ms);
    encoded[898] = freshness_precision(
        (telemetry.vision.observed_at_ms > 0).then_some(telemetry.vision.observed_at_ms),
        telemetry.ts_ms,
    );
    encoded[899] = freshness_precision(
        (telemetry.occupancy_grid.ts_ms > 0).then_some(telemetry.occupancy_grid.ts_ms),
        telemetry.ts_ms,
    );
    encoded[900] = freshness_precision(
        telemetry.odometry_pose.as_ref().map(|pose| pose.pose.ts_ms),
        telemetry.ts_ms,
    );
    encoded[901] = u8::from(telemetry.localization.pose.is_some()) as f32;
    encoded[902] = u8::from(telemetry.voxel_grid.observed_3d) as f32;
    encoded[903] =
        u8::from(telemetry.sensors.version == crate::types::SENSOR_CONTRACT_VERSION) as f32;
    encoded
}

fn update_layer(
    layer: &mut LayerState,
    lower: &[f32],
    top_down: &[f32],
    source_precision: f32,
    top_precision: f32,
    now_ms: u128,
) {
    let mut error_sum = 0.0_f32;
    for index in 0..COGNITION_STATE_DIM {
        let prediction = layer.weights[index] * layer.activation[index] + layer.bias[index];
        let bottom_up_error = lower[index] - prediction;
        let top_down_error = layer.activation[index] - top_down[index];
        let previous = layer.activation[index];
        layer.activation[index] = (previous
            + 0.12 * source_precision * layer.weights[index] * bottom_up_error
            - 0.05 * top_precision * top_down_error)
            .clamp(-4.0, 4.0);
        layer.weights[index] =
            (layer.weights[index] + 0.0005 * bottom_up_error * previous).clamp(0.2, 1.8);
        layer.bias[index] = (layer.bias[index] + 0.0001 * bottom_up_error).clamp(-1.0, 1.0);
        error_sum += bottom_up_error * bottom_up_error;
    }
    layer.prediction_error_l2 = (error_sum / COGNITION_STATE_DIM as f32).sqrt();
    layer.precision = (source_precision / (1.0 + layer.prediction_error_l2)).clamp(0.0, 1.0);
    (layer.activation_mean, layer.activation_rms) = activation_metrics(&layer.activation);
    layer.sequence = layer.sequence.saturating_add(1);
    layer.last_tick_ms = now_ms;
}

fn layer_snapshot(
    index: usize,
    layer: &LayerState,
    source_ts_ms: Option<u128>,
    source_age_ms: Option<u128>,
    now_ms: u128,
) -> CognitionLayerSnapshotV1 {
    CognitionLayerSnapshotV1 {
        schema_version: COGNITION_CONTRACT_VERSION.to_string(),
        ts_ms: now_ms,
        layer: index as u8,
        owner: "leash".to_string(),
        cadence_hz: LAYER_CADENCE_HZ[index],
        sequence: layer.sequence,
        precision: layer.precision,
        prediction_error_l2: layer.prediction_error_l2,
        activation_mean: layer.activation_mean,
        activation_rms: layer.activation_rms,
        fresh: source_age_ms.is_some_and(|age| age <= COGNITION_BOUNDARY_TIMEOUT_MS),
        source_ts_ms,
        source_age_ms,
    }
}

fn activation_metrics(activation: &[f32]) -> (f32, f32) {
    let dimension = activation.len() as f32;
    let mean = activation.iter().copied().sum::<f32>() / dimension;
    let rms = (activation.iter().map(|value| value * value).sum::<f32>() / dimension).sqrt();
    (mean, rms)
}

fn boundary_from_layer(layer: &LayerState, now_ms: u128) -> CognitionBoundaryFrameV1 {
    CognitionBoundaryFrameV1 {
        schema_version: COGNITION_CONTRACT_VERSION.to_string(),
        ts_ms: now_ms,
        expires_at_ms: now_ms.saturating_add(COGNITION_BOUNDARY_TIMEOUT_MS),
        source: "leash".to_string(),
        destination: "planner".to_string(),
        layer: 2,
        sequence: layer.sequence,
        precision: layer.precision,
        state_digest: state_digest(&layer.activation),
        latent: layer.activation.clone(),
    }
}

fn freshness_precision(source_ts_ms: Option<u128>, now_ms: u128) -> f32 {
    let Some(source_ts_ms) = source_ts_ms else {
        return 0.0;
    };
    let age_ms = now_ms.saturating_sub(source_ts_ms);
    (1.0 - age_ms as f32 / COGNITION_BOUNDARY_TIMEOUT_MS as f32).clamp(0.0, 1.0)
}

fn state_digest(values: &[f32]) -> String {
    let mut digest = Sha256::new();
    for value in values {
        digest.update(value.to_le_bytes());
    }
    format!("{:x}", digest.finalize())
}

fn checkpoint_state_digest(state: &CognitionCheckpointStateV1) -> String {
    let mut digest = Sha256::new();
    digest.update(state.schema_version.as_bytes());
    for value in &state.sensor {
        digest.update(value.to_le_bytes());
    }
    digest.update(state.sensor_ts_ms.unwrap_or_default().to_le_bytes());
    for layer in &state.layers {
        for values in [&layer.activation, &layer.weights, &layer.bias] {
            for value in values {
                digest.update(value.to_le_bytes());
            }
        }
        digest.update(layer.sequence.to_le_bytes());
        digest.update(layer.precision.to_le_bytes());
        digest.update(layer.prediction_error_l2.to_le_bytes());
        digest.update(layer.activation_mean.to_le_bytes());
        digest.update(layer.activation_rms.to_le_bytes());
        digest.update(layer.last_tick_ms.to_le_bytes());
    }
    for value in &state.top_down {
        digest.update(value.to_le_bytes());
    }
    digest.update(state.top_down_precision.to_le_bytes());
    digest.update(state.top_down_expires_at_ms.to_le_bytes());
    format!("{:x}", digest.finalize())
}

fn default_checkpoint_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("LEASH_COGNITION_CHECKPOINT_DIR") {
        return PathBuf::from(path);
    }
    if let Some(path) = std::env::var_os("XDG_STATE_HOME") {
        return Path::new(&path).join("leash/cognition");
    }
    if let Some(path) = std::env::var_os("HOME") {
        return Path::new(&path).join(".local/state/leash/cognition");
    }
    std::env::temp_dir().join("leash/cognition")
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Harness, HarnessConfig};

    #[tokio::test]
    async fn sensor_encoding_has_stable_partition_and_reserved_zeroes() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();
        let telemetry = harness.telemetry();
        let encoded = encode_sensor_frame(&telemetry);
        assert_eq!(encoded.len(), COGNITION_STATE_DIM);
        assert!(encoded[960..].iter().all(|value| *value == 0.0));
        assert!(encoded[896] > 0.0);
        assert!(encoded[897] > 0.0);
    }

    #[test]
    fn predictive_layers_decay_when_sensor_evidence_expires() {
        let accelerator =
            crate::accelerator::resolve_accelerator(AcceleratorBackend::Cpu, false).unwrap();
        let runtime = CognitionRuntime::new(&accelerator, "test-embodiment");
        assert_eq!(runtime.capabilities().owner, "test-embodiment");
        let now = now_ms();
        runtime.state.lock().sensor_ts_ms = Some(now);
        runtime.tick(now);
        assert!(runtime.snapshots(now)[0].precision > 0.0);
        runtime.tick(now + COGNITION_BOUNDARY_TIMEOUT_MS + 10);
        assert_eq!(runtime.snapshots(now + 510)[0].precision, 0.0);
    }

    #[test]
    fn boundary_rejects_bad_digest_and_long_lifetime() {
        let layer = LayerState::new(2);
        let now = now_ms();
        let mut frame = boundary_from_layer(&layer, now);
        frame.source = "planner".to_string();
        frame.destination = "leash".to_string();
        frame.layer = 3;
        validate_boundary(&frame, now, "planner", "leash", 3).unwrap();
        frame.state_digest = "bad".to_string();
        assert!(validate_boundary(&frame, now, "planner", "leash", 3).is_err());
    }

    #[test]
    fn semantic_priors_require_evidence_expiry_and_layer_six() {
        let now = now_ms();
        let prior = SemanticPriorV1 {
            schema_version: COGNITION_CONTRACT_VERSION.to_string(),
            prior_id: "door-1".to_string(),
            proposition: "door is likely open".to_string(),
            evidence_refs: vec!["frame:42".to_string()],
            confidence: 0.8,
            created_at_ms: now,
            expires_at_ms: now + 1_000,
            target_layer: 6,
            source: "operator-llm".to_string(),
        };
        prior.validate(now).unwrap();
        let mut invalid = prior.clone();
        invalid.target_layer = 5;
        assert!(invalid.validate(now).is_err());
    }

    #[test]
    fn versioned_checkpoint_restores_canonical_cpu_state_without_duplicate_tick() {
        let accelerator =
            crate::accelerator::resolve_accelerator(AcceleratorBackend::Cpu, false).unwrap();
        let mut runtime = CognitionRuntime::new(&accelerator, "checkpoint-source");
        let checkpoint_dir = std::env::temp_dir().join(format!(
            "leash-cognition-checkpoint-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        runtime.checkpoint_dir = Arc::new(checkpoint_dir.clone());
        let tick_ms = now_ms();
        runtime.state.lock().sensor_ts_ms = Some(tick_ms);
        runtime.tick(tick_ms);
        let before = runtime.boundary(tick_ms);
        let checkpoint = runtime.checkpoint_at(tick_ms + 1).unwrap();
        assert_eq!(checkpoint.layer_sequences, vec![1, 1, 1]);
        let mut tampered: CheckpointPayload =
            serde_json::from_slice(&fs::read(&checkpoint.path).unwrap()).unwrap();
        tampered.state.layers[0].bias[0] += 0.25;
        assert!(validate_checkpoint_payload(&tampered).is_err());

        let restored =
            CognitionRuntime::restore_from_checkpoint(&accelerator, "restored", &checkpoint.path)
                .unwrap();
        let after = restored.boundary(tick_ms + 1);
        assert_eq!(after.sequence, before.sequence);
        assert_eq!(after.state_digest, before.state_digest);
        assert_eq!(
            restored.status(tick_ms + 1, true).backend_status.active,
            "cpu"
        );
        restored.tick(tick_ms + LAYER_INTERVAL_MS[2]);
        assert_eq!(restored.boundary(tick_ms + 50).sequence, 2);
        fs::remove_dir_all(&checkpoint_dir).unwrap();
    }

    #[test]
    fn cognition_health_reports_cuda_selection_and_cpu_fallback_reason() {
        let accelerator = AcceleratorStatus {
            requested: AcceleratorBackend::Cuda,
            active: AcceleratorBackend::Cpu,
            available: false,
            required: false,
            message: "injected CUDA startup failure".to_string(),
            probes: Vec::new(),
        };
        let runtime = CognitionRuntime::new(&accelerator, "fallback-test");
        let status = runtime.status(now_ms(), true);
        assert_eq!(status.backend_status.selected, "cuda");
        assert_eq!(status.backend_status.active, "cpu");
        assert!(status.backend_status.degraded);
        assert_eq!(
            status.backend_status.fallback_reason.as_deref(),
            Some("injected CUDA startup failure")
        );
        assert_eq!(status.capabilities.backend, "cpu");
        assert!(!status.ok);
    }

    #[cfg(feature = "cuda")]
    #[test]
    fn cuda_runtime_advances_once_and_restores_its_checkpoint_on_cpu() {
        let accelerator =
            crate::accelerator::resolve_accelerator(AcceleratorBackend::Cuda, false).unwrap();
        if accelerator.active != AcceleratorBackend::Cuda {
            return;
        }
        let mut runtime = CognitionRuntime::new(&accelerator, "cuda-checkpoint-source");
        let checkpoint_dir = std::env::temp_dir().join(format!(
            "leash-cognition-cuda-checkpoint-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        runtime.checkpoint_dir = Arc::new(checkpoint_dir.clone());
        let tick_ms = now_ms();
        runtime.state.lock().sensor_ts_ms = Some(tick_ms);
        runtime.tick(tick_ms);
        let status = runtime.status(tick_ms, true);
        assert_eq!(status.backend_status.active, "cuda");
        assert!(!status.backend_status.degraded);
        assert_eq!(
            status
                .layers
                .iter()
                .map(|layer| layer.sequence)
                .collect::<Vec<_>>(),
            vec![1, 1, 1]
        );
        let boundary = runtime.boundary(tick_ms);
        let checkpoint = runtime.checkpoint_at(tick_ms + 1).unwrap();
        assert_eq!(checkpoint.backend, "cuda");

        let cpu = crate::accelerator::resolve_accelerator(AcceleratorBackend::Cpu, false).unwrap();
        let restored =
            CognitionRuntime::restore_from_checkpoint(&cpu, "cpu-restore", &checkpoint.path)
                .unwrap();
        let restored_boundary = restored.boundary(tick_ms + 1);
        assert_eq!(restored_boundary.sequence, boundary.sequence);
        assert_eq!(restored_boundary.state_digest, boundary.state_digest);
        fs::remove_dir_all(&checkpoint_dir).unwrap();
    }
}
