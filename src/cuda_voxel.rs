#[cfg(not(windows))]
use std::path::Path;
use std::{
    sync::OnceLock,
    time::{Duration, Instant},
};

use anyhow::{anyhow, ensure, Context, Result};
use leash_cuda::{
    BackendStatus, ComputeExecutor, ComputeJob, ComputeResult, ExecutorConfig, JobPriority,
    JobTicket, WorkError,
};

const COMPUTE_DEADLINE: Duration = Duration::from_millis(100);

static EXECUTOR: OnceLock<Result<ComputeExecutor, String>> = OnceLock::new();

fn executor() -> Result<&'static ComputeExecutor> {
    EXECUTOR
        .get_or_init(|| {
            if !platform_cuda_device_present() {
                return Err("no local CUDA device node is present".to_string());
            }
            ComputeExecutor::start_cuda(ExecutorConfig::default())
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|error| anyhow!(error.clone()))
}

#[cfg(windows)]
fn platform_cuda_device_present() -> bool {
    // Windows exposes CUDA through the driver API rather than Linux device nodes.
    // ComputeExecutor::start_cuda performs the authoritative driver/device checks.
    true
}

#[cfg(not(windows))]
fn platform_cuda_device_present() -> bool {
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
    match execute(ComputeJob::ProjectOccupancy {
        cells: cells.to_vec(),
        depth,
    })? {
        ComputeResult::Occupancy(output) => Ok(output),
        _ => Err(anyhow!("CUDA executor returned the wrong result variant")),
    }
}

pub(crate) fn execute(job: ComputeJob) -> Result<ComputeResult> {
    submit(
        JobPriority::Interactive,
        Some(Instant::now() + COMPUTE_DEADLINE),
        job,
    )?
    .wait()
    .map_err(work_error)
}

pub(crate) fn submit(
    priority: JobPriority,
    deadline: Option<Instant>,
    job: ComputeJob,
) -> Result<JobTicket> {
    executor()?
        .submit(priority, deadline, job)
        .context("submit work to the shared CUDA executor")
}

pub(crate) fn backend_status() -> Result<BackendStatus> {
    Ok(executor()?.status())
}

fn work_error(error: WorkError) -> anyhow::Error {
    anyhow!("CUDA compute failed: {error}")
}
