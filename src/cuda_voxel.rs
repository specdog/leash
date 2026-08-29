use std::{
    path::Path,
    sync::OnceLock,
    time::{Duration, Instant},
};

use anyhow::{anyhow, ensure, Context, Result};
use leash_cuda::{
    ComputeExecutor, ComputeJob, ComputeResult, ExecutorConfig, JobPriority, WorkError,
};

const COMPUTE_DEADLINE: Duration = Duration::from_millis(100);

static EXECUTOR: OnceLock<Result<ComputeExecutor, String>> = OnceLock::new();

fn executor() -> Result<&'static ComputeExecutor> {
    EXECUTOR
        .get_or_init(|| {
            if !cuda_device_node_present() {
                return Err("no local CUDA device node is present".to_string());
            }
            ComputeExecutor::start_cuda(ExecutorConfig::default())
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|error| anyhow!(error.clone()))
}

fn cuda_device_node_present() -> bool {
    ["/dev/nvidiactl", "/dev/nvidia0", "/dev/nvhost-gpu"]
        .iter()
        .any(|path| Path::new(path).exists())
}

pub fn probe() -> Result<()> {
    let output = project_occupancy(&[0, 100, -1], 2)?;
    ensure!(
        output == [0, 0, 100, 100, 0, 0],
        "prebuilt CUDA voxel kernel returned incorrect output"
    );
    Ok(())
}

pub fn project_occupancy(cells: &[i8], depth: u32) -> Result<Vec<i32>> {
    let ticket = executor()?
        .submit(
            JobPriority::Interactive,
            Some(Instant::now() + COMPUTE_DEADLINE),
            ComputeJob::ProjectOccupancy {
                cells: cells.to_vec(),
                depth,
            },
        )
        .context("submit voxel projection to the shared CUDA executor")?;
    match ticket.wait().map_err(work_error)? {
        ComputeResult::Occupancy(output) => Ok(output),
        _ => Err(anyhow!("CUDA executor returned the wrong result variant")),
    }
}

fn work_error(error: WorkError) -> anyhow::Error {
    anyhow!("CUDA voxel projection failed: {error}")
}
