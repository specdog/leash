use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::config::AcceleratorBackend;

pub trait AcceleratorProvider: Send + Sync {
    fn backend(&self) -> AcceleratorBackend;
    fn compiled(&self) -> bool;
    fn available(&self) -> bool;
    fn message(&self) -> &'static str;

    fn probe(&self, selected: bool) -> AcceleratorProbe {
        AcceleratorProbe {
            backend: self.backend(),
            compiled: self.compiled(),
            available: self.available(),
            selected,
            message: self.message().to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct AcceleratorProbe {
    pub backend: AcceleratorBackend,
    pub compiled: bool,
    pub available: bool,
    pub selected: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct AcceleratorStatus {
    pub requested: AcceleratorBackend,
    pub active: AcceleratorBackend,
    pub available: bool,
    pub required: bool,
    pub message: String,
    pub probes: Vec<AcceleratorProbe>,
}

#[derive(Debug)]
struct CpuAccelerator;

impl AcceleratorProvider for CpuAccelerator {
    fn backend(&self) -> AcceleratorBackend {
        AcceleratorBackend::Cpu
    }

    fn compiled(&self) -> bool {
        true
    }

    fn available(&self) -> bool {
        true
    }

    fn message(&self) -> &'static str {
        "CPU accelerator backend available"
    }
}

#[derive(Debug)]
struct CudaAccelerator;

impl AcceleratorProvider for CudaAccelerator {
    fn backend(&self) -> AcceleratorBackend {
        AcceleratorBackend::Cuda
    }

    fn compiled(&self) -> bool {
        cfg!(feature = "cuda")
    }

    fn available(&self) -> bool {
        #[cfg(feature = "cuda")]
        {
            crate::cuda_voxel::probe().is_ok()
        }
        #[cfg(not(feature = "cuda"))]
        false
    }

    fn message(&self) -> &'static str {
        if cfg!(feature = "cuda") {
            "CUDA feature compiled; device and voxel kernel are probed at runtime"
        } else {
            "CUDA feature not compiled"
        }
    }
}

pub fn resolve_accelerator(
    requested: AcceleratorBackend,
    required: bool,
) -> Result<AcceleratorStatus> {
    let probes = probe_inventory(AcceleratorBackend::None);
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
                probes,
            })
        }
        AcceleratorBackend::Cpu => {
            let probes = probe_inventory(AcceleratorBackend::Cpu);
            Ok(AcceleratorStatus {
                requested,
                active: AcceleratorBackend::Cpu,
                available: true,
                required,
                message: "CPU accelerator backend active".to_string(),
                probes,
            })
        }
        AcceleratorBackend::Cuda => cuda_status(required),
    }
}

pub fn probe_inventory(selected: AcceleratorBackend) -> Vec<AcceleratorProbe> {
    let cpu = CpuAccelerator;
    let cuda = CudaAccelerator;
    vec![
        cpu.probe(selected == AcceleratorBackend::Cpu),
        cuda.probe(selected == AcceleratorBackend::Cuda),
    ]
}

fn cuda_status(required: bool) -> Result<AcceleratorStatus> {
    let cuda_probe = CudaAccelerator.probe(true);
    if cuda_probe.available {
        return Ok(AcceleratorStatus {
            requested: AcceleratorBackend::Cuda,
            active: AcceleratorBackend::Cuda,
            available: true,
            required,
            message: "CUDA accelerator active; voxel kernel probe passed".to_string(),
            probes: probe_inventory(AcceleratorBackend::Cuda),
        });
    }
    if required {
        bail!(
            "CUDA accelerator requested as required but unavailable: {}",
            cuda_probe.message
        );
    }
    Ok(AcceleratorStatus {
        requested: AcceleratorBackend::Cuda,
        active: AcceleratorBackend::Cpu,
        available: false,
        required,
        message: "CUDA accelerator unavailable; CPU fallback active".to_string(),
        probes: probe_inventory(AcceleratorBackend::Cpu),
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
        assert_eq!(
            probe_for(&status, AcceleratorBackend::Cpu),
            Some(&AcceleratorProbe {
                backend: AcceleratorBackend::Cpu,
                compiled: true,
                available: true,
                selected: true,
                message: "CPU accelerator backend available".to_string(),
            })
        );
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
        let cuda_available = cfg!(feature = "cuda") && CudaAccelerator.available();
        assert_eq!(status.available, cuda_available);
        assert_eq!(
            status.active,
            if cuda_available {
                AcceleratorBackend::Cuda
            } else {
                AcceleratorBackend::Cpu
            }
        );
        assert_eq!(
            probe_for(&status, AcceleratorBackend::Cuda).map(|probe| probe.compiled),
            Some(cfg!(feature = "cuda"))
        );
        assert_eq!(
            probe_for(&status, AcceleratorBackend::Cuda).map(|probe| probe.available),
            Some(cuda_available)
        );
        assert_eq!(
            probe_for(&status, AcceleratorBackend::Cpu).map(|probe| probe.selected),
            Some(!cuda_available)
        );
    }

    #[test]
    fn required_cuda_requires_available_backend() {
        if cfg!(feature = "cuda") && CudaAccelerator.available() {
            let status = resolve_accelerator(AcceleratorBackend::Cuda, true).unwrap();
            assert_eq!(status.active, AcceleratorBackend::Cuda);
            assert!(status.available);
        } else {
            let err = resolve_accelerator(AcceleratorBackend::Cuda, true)
                .unwrap_err()
                .to_string();
            assert!(err.contains("unavailable"));
        }
    }

    #[test]
    fn default_inventory_reports_compiled_backends() {
        let status = resolve_accelerator(AcceleratorBackend::None, false).unwrap();
        assert_eq!(
            probe_for(&status, AcceleratorBackend::Cpu).map(|probe| probe.compiled),
            Some(true)
        );
        assert_eq!(
            probe_for(&status, AcceleratorBackend::Cuda).map(|probe| probe.compiled),
            Some(cfg!(feature = "cuda"))
        );
        assert!(status.probes.iter().all(|probe| !probe.selected));
    }

    fn probe_for(
        status: &AcceleratorStatus,
        backend: AcceleratorBackend,
    ) -> Option<&AcceleratorProbe> {
        status.probes.iter().find(|probe| probe.backend == backend)
    }
}
