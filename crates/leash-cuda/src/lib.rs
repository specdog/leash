//! Precompiled CUDA kernels and CPU reference contracts for Leash.

#![forbid(unsafe_code)]

use core::fmt;

pub const ARTIFACT_SCHEMA_VERSION: &str = "leash.cuda-artifact.v1";
pub const ARTIFACT_SHA256: &str =
    "cbe6b07f918812d895c6bf881ac6eaa5ae10e9601b5a4e433a5ba354b09d604f";
pub const SOURCE_SHA256: &str = "5a1830b81d0eb3d805ed0069c677ea8d28c5eabd26b23a29443eefa6bfedb05e";
pub const TARGET_SM: &str = "sm_87";
pub const TARGET_PTX: &str = "compute_87";
pub const CUDA_SDK: &str = "12.9.0";
pub const KERNEL_NAMES: [&str; 4] = [
    "project_occupancy",
    "lidar_transform",
    "normalize_rgb_u8",
    "predictive_step",
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
        bytes: 23_360,
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
    InvalidNormalization,
}

impl fmt::Display for ComputeInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroDepth => formatter.write_str("voxel depth must be positive"),
            Self::LengthOverflow => formatter.write_str("compute output length overflowed"),
            Self::LengthMismatch => formatter.write_str("compute input lengths do not match"),
            Self::InvalidRangeBounds => formatter.write_str("lidar range bounds are invalid"),
            Self::InvalidNormalization => {
                formatter.write_str("RGB normalization parameters are invalid")
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

pub fn project_occupancy_cpu(cells: &[i8], depth: u32) -> Result<Vec<i32>, ComputeInputError> {
    if depth == 0 {
        return Err(ComputeInputError::ZeroDepth);
    }
    let output_count = cells
        .len()
        .checked_mul(depth as usize)
        .ok_or(ComputeInputError::LengthOverflow)?;
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
        assert_eq!(artifact.bytes, 23_360);
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
    }

    #[test]
    fn cpu_rgb_reference_uses_interleaved_channels() {
        let output =
            normalize_rgb_u8_cpu(&[0, 127, 255], [0.5, 0.5, 0.5], [2.0, 2.0, 2.0]).unwrap();
        assert!((output[0] + 1.0).abs() < 1e-6);
        assert!(output[1].abs() < 0.01);
        assert!((output[2] - 1.0).abs() < 1e-6);
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
}
