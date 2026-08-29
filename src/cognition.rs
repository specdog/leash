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

pub const COGNITION_CONTRACT_VERSION: &str = "qualia.cognition.v1";
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
            bail!("semantic priors may only target Qualia layer 6");
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
    pub zero_motion: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CheckpointPayload {
    contract: CognitionCheckpointV1,
    layer_weights: Vec<Vec<f32>>,
    layer_biases: Vec<Vec<f32>>,
}

#[derive(Debug, Clone)]
struct LayerState {
    activation: Vec<f32>,
    weights: Vec<f32>,
    bias: Vec<f32>,
    sequence: u64,
    precision: f32,
    prediction_error_l2: f32,
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
    backend: Arc<str>,
    owner: Arc<str>,
    checkpoint_dir: Arc<PathBuf>,
    boundary_tx: broadcast::Sender<CognitionBoundaryFrameV1>,
    #[cfg(feature = "cuda")]
    cuda: Option<Arc<Mutex<crate::cuda_cognition::CudaCognition>>>,
}

impl CognitionRuntime {
    pub fn new(accelerator: &AcceleratorStatus, owner: &str) -> Self {
        #[cfg(feature = "cuda")]
        let cuda = (accelerator.active == AcceleratorBackend::Cuda)
            .then(crate::cuda_cognition::CudaCognition::new)
            .transpose()
            .ok()
            .flatten()
            .map(|runtime| Arc::new(Mutex::new(runtime)));
        #[cfg(feature = "cuda")]
        let backend = if cuda.is_some() { "cuda" } else { "cpu" };
        #[cfg(not(feature = "cuda"))]
        let backend = match accelerator.active {
            AcceleratorBackend::Cuda => "cpu-fallback",
            _ => "cpu",
        };
        let (boundary_tx, _) = broadcast::channel(32);
        let state = CognitionState {
            last_checkpoint_at_ms: now_ms(),
            ..CognitionState::default()
        };
        Self {
            state: Arc::new(Mutex::new(state)),
            backend: Arc::from(backend),
            owner: Arc::from(owner.trim()),
            checkpoint_dir: Arc::new(default_checkpoint_dir()),
            boundary_tx,
            #[cfg(feature = "cuda")]
            cuda,
        }
    }

    pub fn capabilities(&self) -> CognitionCapabilitiesV1 {
        CognitionCapabilitiesV1 {
            schema_version: COGNITION_CONTRACT_VERSION.to_string(),
            runtime: "leash".to_string(),
            owner: self.owner.to_string(),
            state_dim: COGNITION_STATE_DIM,
            owned_layers: vec![0, 1, 2],
            sensor_plane: SENSOR_LAYER,
            backend: self.backend.to_string(),
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
        #[cfg(feature = "cuda")]
        if let Some(cuda) = &self.cuda {
            if let Err(error) = cuda.lock().update_sensor(&encoded) {
                tracing::warn!(?error, "CUDA cognition sensor upload failed");
            }
        }
    }

    pub fn tick(&self, now_ms: u128) {
        let mut publish_boundary = false;
        let should_checkpoint;
        {
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
                #[cfg(feature = "cuda")]
                if let Some(cuda) = &self.cuda {
                    if let Err(error) =
                        cuda.lock()
                            .step(layer_index, lower_precision, top_precision)
                    {
                        tracing::warn!(?error, layer_index, "CUDA cognition step failed");
                    }
                }
                if layer_index == 2 {
                    publish_boundary = true;
                }
            }
            should_checkpoint = now_ms.saturating_sub(state.last_checkpoint_at_ms)
                >= COGNITION_CHECKPOINT_INTERVAL_MS;
        }

        if publish_boundary {
            let _ = self.boundary_tx.send(self.boundary(now_ms));
        }
        if should_checkpoint {
            if let Err(error) = self.checkpoint_at(now_ms) {
                tracing::warn!(?error, "cognition checkpoint failed");
            }
        }
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
        CognitionStatusV1 {
            ok: true,
            capabilities: self.capabilities(),
            layers: self.snapshots(now_ms),
            boundary: self.boundary(now_ms),
            last_checkpoint: self.state.lock().last_checkpoint.clone(),
            zero_motion,
        }
    }

    pub fn submit_boundary(&self, frame: CognitionBoundaryFrameV1, now_ms: u128) -> Result<()> {
        validate_boundary(&frame, now_ms, "qualia", "leash", 3)?;
        let mut state = self.state.lock();
        state.top_down.copy_from_slice(&frame.latent);
        state.top_down_precision = frame.precision;
        state.top_down_expires_at_ms = frame
            .expires_at_ms
            .min(now_ms.saturating_add(COGNITION_BOUNDARY_TIMEOUT_MS));
        #[cfg(feature = "cuda")]
        if let Some(cuda) = &self.cuda {
            cuda.lock().update_top_down(&frame.latent)?;
        }
        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CognitionBoundaryFrameV1> {
        self.boundary_tx.subscribe()
    }

    pub fn checkpoint(&self) -> Result<CognitionCheckpointV1> {
        self.checkpoint_at(now_ms())
    }

    fn checkpoint_at(&self, created_at_ms: u128) -> Result<CognitionCheckpointV1> {
        fs::create_dir_all(self.checkpoint_dir.as_ref()).with_context(|| {
            format!(
                "create cognition checkpoint directory {}",
                self.checkpoint_dir.display()
            )
        })?;
        let mut state = self.state.lock();
        let digest = state_digest(&state.layers[2].activation);
        let checkpoint_id = format!("leash-{created_at_ms}-{}", &digest[..12]);
        let path = self.checkpoint_dir.join(format!("{checkpoint_id}.json"));
        let contract = CognitionCheckpointV1 {
            schema_version: COGNITION_CONTRACT_VERSION.to_string(),
            checkpoint_id,
            created_at_ms,
            runtime: "leash".to_string(),
            backend: self.backend.to_string(),
            layer_sequences: state.layers.iter().map(|layer| layer.sequence).collect(),
            state_digest: digest,
            path: path.display().to_string(),
        };
        let payload = CheckpointPayload {
            contract: contract.clone(),
            layer_weights: state
                .layers
                .iter()
                .map(|layer| layer.weights.clone())
                .collect(),
            layer_biases: state
                .layers
                .iter()
                .map(|layer| layer.bias.clone())
                .collect(),
        };
        let bytes = serde_json::to_vec(&payload).context("serialize cognition checkpoint")?;
        fs::write(&path, bytes)
            .with_context(|| format!("write cognition checkpoint {}", path.display()))?;
        state.last_checkpoint_at_ms = created_at_ms;
        state.last_checkpoint = Some(contract.clone());
        Ok(contract)
    }
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
    let mean = layer.activation.iter().copied().sum::<f32>() / COGNITION_STATE_DIM as f32;
    let rms = (layer
        .activation
        .iter()
        .map(|value| value * value)
        .sum::<f32>()
        / COGNITION_STATE_DIM as f32)
        .sqrt();
    CognitionLayerSnapshotV1 {
        schema_version: COGNITION_CONTRACT_VERSION.to_string(),
        ts_ms: now_ms,
        layer: index as u8,
        owner: "leash".to_string(),
        cadence_hz: LAYER_CADENCE_HZ[index],
        sequence: layer.sequence,
        precision: layer.precision,
        prediction_error_l2: layer.prediction_error_l2,
        activation_mean: mean,
        activation_rms: rms,
        fresh: source_age_ms.is_some_and(|age| age <= COGNITION_BOUNDARY_TIMEOUT_MS),
        source_ts_ms,
        source_age_ms,
    }
}

fn boundary_from_layer(layer: &LayerState, now_ms: u128) -> CognitionBoundaryFrameV1 {
    CognitionBoundaryFrameV1 {
        schema_version: COGNITION_CONTRACT_VERSION.to_string(),
        ts_ms: now_ms,
        expires_at_ms: now_ms.saturating_add(COGNITION_BOUNDARY_TIMEOUT_MS),
        source: "leash".to_string(),
        destination: "qualia".to_string(),
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
        frame.source = "qualia".to_string();
        frame.destination = "leash".to_string();
        frame.layer = 3;
        validate_boundary(&frame, now, "qualia", "leash", 3).unwrap();
        frame.state_digest = "bad".to_string();
        assert!(validate_boundary(&frame, now, "qualia", "leash", 3).is_err());
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
}
