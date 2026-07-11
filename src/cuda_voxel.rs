use std::{
    path::Path,
    sync::{Arc, OnceLock},
};

use anyhow::{anyhow, ensure, Context, Result};
use cudarc::{
    driver::{CudaContext, CudaFunction, CudaStream, LaunchConfig, PushKernelArg},
    nvrtc::compile_ptx,
};

const KERNEL: &str = r#"
extern "C" __global__ void project_occupancy(
    const signed char* cells,
    int* output,
    unsigned int cell_count,
    unsigned int depth
) {
    unsigned int index = blockIdx.x * blockDim.x + threadIdx.x;
    unsigned int output_count = cell_count * depth;
    if (index >= output_count) return;
    signed char occupancy = cells[index / depth];
    output[index] = occupancy > 0 ? (int)occupancy : 0;
}
"#;

struct CudaVoxelizer {
    stream: Arc<CudaStream>,
    function: CudaFunction,
}

static VOXELIZER: OnceLock<Result<CudaVoxelizer, String>> = OnceLock::new();

fn voxelizer() -> Result<&'static CudaVoxelizer> {
    VOXELIZER
        .get_or_init(|| {
            if !cuda_device_node_present() {
                return Err("no local CUDA device node is present".to_string());
            }
            std::panic::catch_unwind(CudaVoxelizer::new)
                .map_err(|_| "CUDA driver dynamic loading panicked".to_string())?
                .map_err(|error| format!("{error:#}"))
        })
        .as_ref()
        .map_err(|error| anyhow!(error.clone()))
}

fn cuda_device_node_present() -> bool {
    ["/dev/nvidiactl", "/dev/nvidia0", "/dev/nvhost-gpu"]
        .iter()
        .any(|path| Path::new(path).exists())
}

impl CudaVoxelizer {
    fn new() -> Result<Self> {
        let device_count = CudaContext::device_count().context("query CUDA device count")?;
        ensure!(device_count > 0, "no CUDA device is available");
        let context = CudaContext::new(0).context("create CUDA device 0 context")?;
        let ptx = compile_ptx(KERNEL).context("compile voxel kernel with NVRTC")?;
        let module = context.load_module(ptx).context("load voxel CUDA module")?;
        let function = module
            .load_function("project_occupancy")
            .context("load project_occupancy CUDA kernel")?;
        Ok(Self {
            stream: context.default_stream(),
            function,
        })
    }

    fn project(&self, cells: &[i8], depth: u32) -> Result<Vec<i32>> {
        ensure!(depth > 0, "voxel depth must be positive");
        let output_count = cells
            .len()
            .checked_mul(depth as usize)
            .context("voxel output length overflow")?;
        let cells_device = self
            .stream
            .clone_htod(cells)
            .context("copy occupancy cells to CUDA")?;
        let mut output_device = self
            .stream
            .alloc_zeros::<i32>(output_count)
            .context("allocate CUDA voxel output")?;
        let cell_count = u32::try_from(cells.len()).context("occupancy grid is too large")?;
        let launch = LaunchConfig::for_num_elems(output_count as u32);
        unsafe {
            self.stream
                .launch_builder(&self.function)
                .arg(&cells_device)
                .arg(&mut output_device)
                .arg(&cell_count)
                .arg(&depth)
                .launch(launch)
        }
        .context("launch CUDA voxel kernel")?;
        self.stream
            .clone_dtoh(&output_device)
            .context("copy CUDA voxel output to host")
    }
}

pub fn probe() -> Result<()> {
    let output = voxelizer()?.project(&[0, 100, -1], 2)?;
    ensure!(
        output == [0, 0, 100, 100, 0, 0],
        "CUDA voxel kernel returned incorrect output"
    );
    Ok(())
}

pub fn project_occupancy(cells: &[i8], depth: u32) -> Result<Vec<i32>> {
    voxelizer()?.project(cells, depth)
}
