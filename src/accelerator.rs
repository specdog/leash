use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::config::AcceleratorBackend;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct AcceleratorStatus {
    pub requested: AcceleratorBackend,
    pub active: AcceleratorBackend,
    pub available: bool,
    pub required: bool,
    pub message: String,
}

pub fn resolve_accelerator(
    requested: AcceleratorBackend,
    required: bool,
) -> Result<AcceleratorStatus> {
    match requested {
        AcceleratorBackend::None => {
            if required {
                bail!("accelerator is required but no accelerator backend was selected");
            }
            Ok(AcceleratorStatus {
                requested,
                active: AcceleratorBackend::None,
                available: true,
                required,
                message: "no accelerator requested".to_string(),
            })
        }
        AcceleratorBackend::Cpu => Ok(AcceleratorStatus {
            requested,
            active: AcceleratorBackend::Cpu,
            available: true,
            required,
            message: "CPU accelerator fallback active".to_string(),
        }),
        AcceleratorBackend::Cuda => cuda_status(required),
    }
}

#[cfg(feature = "cuda")]
fn cuda_status(required: bool) -> Result<AcceleratorStatus> {
    Ok(AcceleratorStatus {
        requested: AcceleratorBackend::Cuda,
        active: AcceleratorBackend::Cuda,
        available: true,
        required,
        message: "CUDA feature enabled; device probe deferred to backend implementation"
            .to_string(),
    })
}

#[cfg(not(feature = "cuda"))]
fn cuda_status(required: bool) -> Result<AcceleratorStatus> {
    if required {
        bail!("CUDA accelerator requested as required but crate was built without cuda feature");
    }
    Ok(AcceleratorStatus {
        requested: AcceleratorBackend::Cuda,
        active: AcceleratorBackend::Cpu,
        available: false,
        required,
        message: "CUDA feature not enabled; CPU fallback active".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_backend_is_available_without_hardware() {
        let status = resolve_accelerator(AcceleratorBackend::Cpu, true).unwrap();
        assert_eq!(status.active, AcceleratorBackend::Cpu);
        assert!(status.available);
        assert!(status.required);
    }

    #[test]
    fn required_none_backend_is_rejected() {
        let err = resolve_accelerator(AcceleratorBackend::None, true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("required"));
    }

    #[test]
    fn cuda_selection_has_ci_safe_behavior() {
        let status = resolve_accelerator(AcceleratorBackend::Cuda, false).unwrap();
        assert_eq!(status.requested, AcceleratorBackend::Cuda);
        if cfg!(feature = "cuda") {
            assert_eq!(status.active, AcceleratorBackend::Cuda);
            assert!(status.available);
        } else {
            assert_eq!(status.active, AcceleratorBackend::Cpu);
            assert!(!status.available);
        }
    }

    #[cfg(not(feature = "cuda"))]
    #[test]
    fn required_cuda_requires_feature_backend() {
        let err = resolve_accelerator(AcceleratorBackend::Cuda, true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("without cuda feature"));
    }
}
