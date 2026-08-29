//! Precompiled CUDA kernels and CPU reference contracts for Leash.

#![deny(unsafe_code)]

mod executor;

#[cfg(feature = "cuda")]
#[allow(unsafe_code)]
mod device;

use core::fmt;

pub use executor::{
    BackendKind, BackendStatus, CognitionLayerMetrics, CognitionLayerSnapshot, CognitionStep,
    ComputeExecutor, ComputeJob, ComputeResult, ExecutorConfig, ExecutorMetrics, JobId,
    JobPriority, JobTicket, PredictiveState, ResidentCognitionCheckpoint, ResidentCognitionLayer,
    StartError, SubmitError, WorkError, RESIDENT_COGNITION_SCHEMA_VERSION,
};

pub const ARTIFACT_SCHEMA_VERSION: &str = "leash.cuda-artifact.v1";
pub const ARTIFACT_SHA256: &str =
    "c96e10ac48d9b8e58fc9f31eb5cdf58ad5cb907ddd0c49d98784344e86a640a4";
pub const SOURCE_SHA256: &str = "ffb6710f9611cf8984bbe44511e33a0d73e7564bba8e1f4b75987a8e888462be";
pub const TARGET_SM: &str = "sm_87";
pub const TARGET_PTX: &str = "compute_87";
pub const CUDA_SDK: &str = "12.9.0";
pub const KERNEL_NAMES: [&str; 6] = [
    "project_occupancy",
    "lidar_transform",
    "collision_sector_reduce",
    "normalize_rgb_u8",
    "predictive_step",
    "predictive_step_metrics",
];

#[cfg(feature = "cuda")]
pub static PREBUILT_FATBIN: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/leash_kernels.fatbin"));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KernelArtifact {
    pub schema_version: &'static str,
    pub sha256: &'static str,
    pub bytes: usize,
    pub cuda_sdk: &'static str,
    pub native_target: &'static str,
    pub ptx_target: &'static str,
    pub kernels: &'static [&'static str],
}

pub const fn artifact() -> KernelArtifact {
    KernelArtifact {
        schema_version: ARTIFACT_SCHEMA_VERSION,
        sha256: ARTIFACT_SHA256,
        bytes: 39_080,
        cuda_sdk: CUDA_SDK,
        native_target: TARGET_SM,
        ptx_target: TARGET_PTX,
        kernels: &KERNEL_NAMES,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputeInputError {
    ZeroDepth,
    LengthOverflow,
    LengthMismatch,
    InvalidRangeBounds,
    InvalidAngles,
    InvalidSector,
    InvalidNormalization,
    InvalidCognitionState,
    InvalidCognitionSchedule,
}

impl fmt::Display for ComputeInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroDepth => formatter.write_str("voxel depth must be positive"),
            Self::LengthOverflow => formatter.write_str("compute output length overflowed"),
            Self::LengthMismatch => formatter.write_str("compute input lengths do not match"),
            Self::InvalidRangeBounds => formatter.write_str("lidar range bounds are invalid"),
            Self::InvalidAngles => formatter.write_str("lidar angles are invalid"),
            Self::InvalidSector => formatter.write_str("collision sector is invalid"),
            Self::InvalidNormalization => {
                formatter.write_str("RGB normalization parameters are invalid")
            }
            Self::InvalidCognitionState => {
                formatter.write_str("resident cognition state is invalid or not loaded")
            }
            Self::InvalidCognitionSchedule => {
                formatter.write_str("resident cognition schedule is invalid")
            }
        }
    }
}

impl std::error::Error for ComputeInputError {}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LidarPoint {
    pub x_m: f32,
    pub y_m: f32,
    pub valid: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CollisionSector {
    pub min_range_m: Option<f32>,
    pub sample_count: u32,
}

fn projected_occupancy_len(cell_count: usize, depth: u32) -> Result<usize, ComputeInputError> {
    if depth == 0 {
        return Err(ComputeInputError::ZeroDepth);
    }
    let output_count = cell_count
        .checked_mul(depth as usize)
        .ok_or(ComputeInputError::LengthOverflow)?;
    if u32::try_from(cell_count).is_err() || u32::try_from(output_count).is_err() {
        return Err(ComputeInputError::LengthOverflow);
    }
    Ok(output_count)
}

pub fn project_occupancy_cpu(cells: &[i8], depth: u32) -> Result<Vec<i32>, ComputeInputError> {
    let output_count = projected_occupancy_len(cells.len(), depth)?;
    let mut output = Vec::with_capacity(output_count);
    for cell in cells {
        let occupancy = if *cell > 0 { i32::from(*cell) } else { 0 };
        output.extend(std::iter::repeat_n(occupancy, depth as usize));
    }
    Ok(output)
}

#[allow(clippy::too_many_arguments)]
pub fn lidar_transform_cpu(
    ranges_m: &[f32],
    angle_min_rad: f32,
    angle_increment_rad: f32,
    range_min_m: f32,
    range_max_m: f32,
    yaw_offset_rad: f32,
    clockwise: bool,
) -> Result<Vec<LidarPoint>, ComputeInputError> {
    if !range_min_m.is_finite()
        || !range_max_m.is_finite()
        || range_min_m < 0.0
        || range_max_m < range_min_m
    {
        return Err(ComputeInputError::InvalidRangeBounds);
    }
    if !angle_min_rad.is_finite() || !angle_increment_rad.is_finite() || !yaw_offset_rad.is_finite()
    {
        return Err(ComputeInputError::InvalidAngles);
    }
    u32::try_from(ranges_m.len()).map_err(|_| ComputeInputError::LengthOverflow)?;
    let direction = if clockwise { -1.0 } else { 1.0 };
    Ok(ranges_m
        .iter()
        .enumerate()
        .map(|(index, range)| {
            if !range.is_finite() || *range < range_min_m || *range > range_max_m {
                return LidarPoint::default();
            }
            let angle =
                yaw_offset_rad + direction * (angle_min_rad + index as f32 * angle_increment_rad);
            LidarPoint {
                x_m: *range * angle.cos(),
                y_m: *range * angle.sin(),
                valid: true,
            }
        })
        .collect())
}

#[allow(clippy::too_many_arguments)]
pub fn collision_sector_reduce_cpu(
    ranges_m: &[f32],
    angle_min_rad: f32,
    angle_increment_rad: f32,
    range_min_m: f32,
    range_max_m: f32,
    sector_center_rad: f32,
    sector_half_width_rad: f32,
) -> Result<CollisionSector, ComputeInputError> {
    if !range_min_m.is_finite()
        || !range_max_m.is_finite()
        || range_min_m < 0.0
        || range_max_m < range_min_m
    {
        return Err(ComputeInputError::InvalidRangeBounds);
    }
    if !angle_min_rad.is_finite() || !angle_increment_rad.is_finite() {
        return Err(ComputeInputError::InvalidAngles);
    }
    if !sector_center_rad.is_finite()
        || !sector_half_width_rad.is_finite()
        || !(0.0..=core::f32::consts::PI).contains(&sector_half_width_rad)
    {
        return Err(ComputeInputError::InvalidSector);
    }
    u32::try_from(ranges_m.len()).map_err(|_| ComputeInputError::LengthOverflow)?;

    let mut result = CollisionSector::default();
    for (index, range_m) in ranges_m.iter().copied().enumerate() {
        if !range_m.is_finite() || range_m < range_min_m || range_m > range_max_m {
            continue;
        }
        let angle = angle_min_rad + index as f32 * angle_increment_rad;
        let delta = (angle - sector_center_rad)
            .sin()
            .atan2((angle - sector_center_rad).cos());
        if delta.abs() > sector_half_width_rad {
            continue;
        }
        result.sample_count = result
            .sample_count
            .checked_add(1)
            .ok_or(ComputeInputError::LengthOverflow)?;
        let range_m = if range_m == 0.0 { 0.0 } else { range_m };
        result.min_range_m = Some(
            result
                .min_range_m
                .map_or(range_m, |minimum| minimum.min(range_m)),
        );
    }
    Ok(result)
}

pub fn normalize_rgb_u8_cpu(
    input: &[u8],
    mean: [f32; 3],
    inverse_std: [f32; 3],
) -> Result<Vec<f32>, ComputeInputError> {
    if !input.len().is_multiple_of(3)
        || mean.iter().any(|value| !value.is_finite())
        || inverse_std
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0)
    {
        return Err(ComputeInputError::InvalidNormalization);
    }
    Ok(input
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let channel = index % 3;
            (f32::from(*value) / 255.0 - mean[channel]) * inverse_std[channel]
        })
        .collect())
}

#[allow(clippy::too_many_arguments)]
pub fn predictive_step_cpu(
    lower: &[f32],
    state: &mut [f32],
    top_down: &[f32],
    weights: &mut [f32],
    bias: &mut [f32],
    source_precision: f32,
    top_precision: f32,
) -> Result<(), ComputeInputError> {
    let count = state.len();
    if lower.len() != count
        || top_down.len() != count
        || weights.len() != count
        || bias.len() != count
    {
        return Err(ComputeInputError::LengthMismatch);
    }
    for index in 0..count {
        let previous = state[index];
        let prediction = weights[index] * previous + bias[index];
        let bottom_up_error = lower[index] - prediction;
        let top_down_error = previous - top_down[index];
        let next = previous + 0.12 * source_precision * weights[index] * bottom_up_error
            - 0.05 * top_precision * top_down_error;
        state[index] = next.clamp(-4.0, 4.0);
        weights[index] = (weights[index] + 0.0005 * bottom_up_error * previous).clamp(0.2, 1.8);
        bias[index] = (bias[index] + 0.0001 * bottom_up_error).clamp(-1.0, 1.0);
    }
    Ok(())
}

#[cfg(feature = "cuda")]
pub fn probe_prebuilt_module() -> Result<KernelArtifact, String> {
    use cudarc::{driver::CudaContext, nvrtc::Ptx};

    std::panic::catch_unwind(|| {
        let context = CudaContext::new(0).map_err(|error| error.to_string())?;
        let module = context
            .load_module(Ptx::from_binary(PREBUILT_FATBIN.to_vec()))
            .map_err(|error| error.to_string())?;
        for name in KERNEL_NAMES {
            module
                .load_function(name)
                .map_err(|error| format!("load CUDA kernel {name}: {error}"))?;
        }
        Ok(artifact())
    })
    .map_err(|_| "CUDA driver dynamic loading panicked".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_metadata_is_specific_and_complete() {
        let artifact = artifact();
        assert_eq!(artifact.schema_version, "leash.cuda-artifact.v1");
        assert_eq!(artifact.sha256.len(), 64);
        assert_eq!(artifact.bytes, 39_080);
        assert_eq!(artifact.native_target, "sm_87");
        assert_eq!(artifact.ptx_target, "compute_87");
        assert_eq!(artifact.kernels, KERNEL_NAMES);
    }

    #[test]
    fn cpu_voxel_reference_defines_unknown_and_free_behavior() {
        assert_eq!(
            project_occupancy_cpu(&[-1, 0, 100], 2).unwrap(),
            [0, 0, 0, 0, 100, 100]
        );
        assert_eq!(
            project_occupancy_cpu(&[1], 0),
            Err(ComputeInputError::ZeroDepth)
        );
        assert_eq!(
            projected_occupancy_len(u32::MAX as usize, 2),
            Err(ComputeInputError::LengthOverflow)
        );
        assert_eq!(
            projected_occupancy_len(u32::MAX as usize, 1),
            Ok(u32::MAX as usize)
        );
    }

    #[test]
    fn cpu_lidar_reference_filters_and_transforms() {
        let points = lidar_transform_cpu(
            &[1.0, f32::NAN, 20.0],
            0.0,
            core::f32::consts::FRAC_PI_2,
            0.05,
            12.0,
            0.0,
            false,
        )
        .unwrap();
        assert!(points[0].valid);
        assert!((points[0].x_m - 1.0).abs() < 1e-6);
        assert!(!points[1].valid);
        assert!(!points[2].valid);
        assert_eq!(
            lidar_transform_cpu(&[], f32::NAN, 0.1, 0.05, 12.0, 0.0, false),
            Err(ComputeInputError::InvalidAngles)
        );
        assert_eq!(
            lidar_transform_cpu(&[], 0.0, 0.1, 12.0, 0.05, 0.0, false),
            Err(ComputeInputError::InvalidRangeBounds)
        );
    }

    #[test]
    fn cpu_collision_reference_defines_empty_non_finite_and_wrapped_sectors() {
        assert_eq!(
            collision_sector_reduce_cpu(&[], 0.0, 0.1, 0.05, 12.0, 0.0, 0.5).unwrap(),
            CollisionSector::default()
        );
        let result = collision_sector_reduce_cpu(
            &[1.0, f32::NAN, f32::INFINITY, 0.25, 20.0],
            core::f32::consts::PI - 0.2,
            0.1,
            0.05,
            12.0,
            -core::f32::consts::PI,
            0.21,
        )
        .unwrap();
        assert_eq!(result.sample_count, 2);
        assert_eq!(result.min_range_m, Some(0.25));
        assert_eq!(
            collision_sector_reduce_cpu(&[], 0.0, 0.1, 0.05, 12.0, 0.0, f32::INFINITY),
            Err(ComputeInputError::InvalidSector)
        );
        assert_eq!(
            collision_sector_reduce_cpu(&[], 0.0, 0.1, 0.05, 12.0, 0.0, -0.1),
            Err(ComputeInputError::InvalidSector)
        );
    }

    #[test]
    fn randomized_collision_reference_respects_the_result_contract() {
        let mut seed = 0x5eed_1234_u32;
        for _ in 0..64 {
            let count = (lcg(&mut seed) % 512) as usize;
            let ranges = (0..count)
                .map(|index| match index % 31 {
                    0 => f32::NAN,
                    1 => f32::INFINITY,
                    _ => 0.01 + (lcg(&mut seed) % 1_500) as f32 / 100.0,
                })
                .collect::<Vec<_>>();
            let center = (lcg(&mut seed) as f32 / u32::MAX as f32 - 0.5) * core::f32::consts::TAU;
            let half_width = 0.05 + (lcg(&mut seed) % 250) as f32 / 100.0;
            let actual = collision_sector_reduce_cpu(
                &ranges,
                -core::f32::consts::PI,
                core::f32::consts::TAU / count.max(1) as f32,
                0.05,
                12.0,
                center,
                half_width,
            )
            .unwrap();
            assert!(actual.sample_count <= count as u32);
            assert!(actual.min_range_m.is_none() == (actual.sample_count == 0));
            if let Some(minimum) = actual.min_range_m {
                assert!((0.05..=12.0).contains(&minimum));
            }
        }
    }

    #[test]
    fn cpu_rgb_reference_uses_interleaved_channels() {
        let output =
            normalize_rgb_u8_cpu(&[0, 127, 255], [0.5, 0.5, 0.5], [2.0, 2.0, 2.0]).unwrap();
        assert!((output[0] + 1.0).abs() < 1e-6);
        assert!(output[1].abs() < 0.01);
        assert!((output[2] - 1.0).abs() < 1e-6);
        assert!(normalize_rgb_u8_cpu(&[], [0.5; 3], [2.0; 3])
            .unwrap()
            .is_empty());
        assert_eq!(
            normalize_rgb_u8_cpu(&[0, 1], [0.5; 3], [2.0; 3]),
            Err(ComputeInputError::InvalidNormalization)
        );
    }

    #[test]
    fn cpu_cognition_reference_updates_canonical_state_once() {
        let lower = [1.0, -1.0];
        let mut state = [0.5, -0.5];
        let top_down = [0.25, -0.25];
        let mut weights = [0.75, 0.75];
        let mut bias = [0.0, 0.0];
        predictive_step_cpu(
            &lower,
            &mut state,
            &top_down,
            &mut weights,
            &mut bias,
            1.0,
            0.5,
        )
        .unwrap();
        assert!(state[0] > 0.5);
        assert!(state[1] < -0.5);
        assert_ne!(weights, [0.75, 0.75]);
        assert_ne!(bias, [0.0, 0.0]);
    }

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *seed
    }
}
