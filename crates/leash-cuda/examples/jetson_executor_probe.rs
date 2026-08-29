#[cfg(feature = "cuda")]
use leash_cuda::{
    ComputeExecutor, ComputeJob, ComputeResult, ExecutorConfig, JobPriority, PredictiveState,
};

#[cfg(not(feature = "cuda"))]
fn main() {
    eprintln!("enable the cuda feature to run the Jetson executor probe");
    std::process::exit(2);
}

#[cfg(feature = "cuda")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cpu = ComputeExecutor::start_cpu(ExecutorConfig::default())?;
    let cuda = ComputeExecutor::start_cuda(ExecutorConfig::default())?;
    let jobs = [
        ComputeJob::ProjectOccupancy {
            cells: vec![-1, 0, 100],
            depth: 2,
        },
        ComputeJob::LidarTransform {
            ranges_m: vec![1.0, f32::NAN, 20.0],
            angle_min_rad: 0.0,
            angle_increment_rad: core::f32::consts::FRAC_PI_2,
            range_min_m: 0.05,
            range_max_m: 12.0,
            yaw_offset_rad: 0.0,
            clockwise: false,
        },
        ComputeJob::NormalizeRgbU8 {
            input: vec![0, 127, 255],
            mean: [0.5; 3],
            inverse_std: [2.0; 3],
        },
        ComputeJob::PredictiveStep {
            lower: vec![1.0, -1.0],
            state: PredictiveState {
                state: vec![0.5, -0.5],
                weights: vec![0.75, 0.75],
                bias: vec![0.0, 0.0],
            },
            top_down: vec![0.25, -0.25],
            source_precision: 1.0,
            top_precision: 0.5,
        },
    ];

    for job in jobs {
        let cpu_result = run(&cpu, job.clone())?;
        let cuda_result = run(&cuda, job)?;
        assert_parity(&cpu_result, &cuda_result);
    }
    let status = cuda.status();
    let metrics = cuda.metrics();
    println!(
        "leash cudarc executor probe passed: active={:?}, jobs={}, circuit_open={}",
        status.active, metrics.completed, status.circuit_open
    );
    Ok(())
}

#[cfg(feature = "cuda")]
fn run(
    executor: &ComputeExecutor,
    job: ComputeJob,
) -> Result<ComputeResult, Box<dyn std::error::Error>> {
    Ok(executor
        .submit(JobPriority::Interactive, None, job)?
        .wait()?)
}

#[cfg(feature = "cuda")]
fn assert_parity(cpu: &ComputeResult, cuda: &ComputeResult) {
    match (cpu, cuda) {
        (ComputeResult::Occupancy(cpu), ComputeResult::Occupancy(cuda)) => {
            assert_eq!(cpu, cuda);
        }
        (ComputeResult::Lidar(cpu), ComputeResult::Lidar(cuda)) => {
            assert_eq!(cpu.len(), cuda.len());
            for (cpu, cuda) in cpu.iter().zip(cuda) {
                assert_eq!(cpu.valid, cuda.valid);
                if cpu.valid {
                    assert!(close(cpu.x_m, cuda.x_m));
                    assert!(close(cpu.y_m, cuda.y_m));
                }
            }
        }
        (ComputeResult::NormalizedRgb(cpu), ComputeResult::NormalizedRgb(cuda)) => {
            assert_float_slice(cpu, cuda);
        }
        (ComputeResult::Predictive(cpu), ComputeResult::Predictive(cuda)) => {
            assert_float_slice(&cpu.state, &cuda.state);
            assert_float_slice(&cpu.weights, &cuda.weights);
            assert_float_slice(&cpu.bias, &cuda.bias);
        }
        _ => panic!("CPU and CUDA returned different result variants"),
    }
}

#[cfg(feature = "cuda")]
fn assert_float_slice(cpu: &[f32], cuda: &[f32]) {
    assert_eq!(cpu.len(), cuda.len());
    for (cpu, cuda) in cpu.iter().zip(cuda) {
        assert!(close(*cpu, *cuda), "CPU {cpu} != CUDA {cuda}");
    }
}

#[cfg(feature = "cuda")]
fn close(left: f32, right: f32) -> bool {
    (left - right).abs() <= 1e-5 + 1e-5 * left.abs().max(right.abs())
}
