use std::{error::Error, hint::black_box, time::Instant};

use leash_cuda::{
    BackendKind, ComputeExecutor, ComputeJob, ComputeResult, ExecutorConfig, JobPriority,
    PredictiveState,
};

const DEFAULT_ITERATIONS: usize = 20;
const WARMUP_ITERATIONS: usize = 10;

fn main() -> Result<(), Box<dyn Error>> {
    let iterations = parse_iterations()?;
    let cpu = ComputeExecutor::start_cpu(ExecutorConfig { queue_capacity: 2 })?;
    let cuda = ComputeExecutor::start_cuda(ExecutorConfig { queue_capacity: 2 })?;
    if cuda.status().active != BackendKind::Cuda {
        return Err("CUDA benchmark did not activate CUDA".into());
    }

    let profiles = profiles();
    let mut results = Vec::with_capacity(profiles.len());
    for profile in profiles {
        let (cpu_first, cpu_first_ns) = timed_execute(&cpu, profile.job.clone())?;
        let (cuda_first, cuda_first_ns) = timed_execute(&cuda, profile.job.clone())?;
        assert_parity(&cpu_first, &cuda_first)?;
        for _ in 1..WARMUP_ITERATIONS {
            black_box(execute(&cpu, profile.job.clone())?);
            black_box(execute(&cuda, profile.job.clone())?);
        }
        let (cpu_times, cuda_times) = measure_alternating(&cpu, &cuda, &profile.job, iterations)?;
        results.push(BenchmarkResult {
            name: profile.name,
            elements: profile.elements,
            cpu_first_ns,
            cuda_first_ns,
            cpu: Distribution::new(cpu_times)?,
            cuda: Distribution::new(cuda_times)?,
        });
    }

    let cuda_status = cuda.status();
    let cuda_metrics = cuda.metrics();
    print!(
        concat!(
            "{{\"schema_version\":\"leash.cuda-benchmark.v1\",",
            "\"iterations\":{},\"warmup_iterations\":{},",
            "\"includes\":[\"queue\",\"host_to_device\",\"kernel\",",
            "\"synchronization\",\"device_to_host\"],\"workloads\":["
        ),
        iterations, WARMUP_ITERATIONS
    );
    for (index, result) in results.iter().enumerate() {
        if index != 0 {
            print!(",");
        }
        let speedup_milli = result
            .cpu
            .p50
            .saturating_mul(1_000)
            .checked_div(result.cuda.p50)
            .unwrap_or(u64::MAX);
        let winner = if speedup_milli > 1_000 { "cuda" } else { "cpu" };
        print!(
            concat!(
                "{{\"name\":\"{}\",\"elements\":{},",
                "\"first_ns\":{{\"cpu\":{},\"cuda\":{}}},",
                "\"cpu_ns\":{{\"p50\":{},\"p95\":{},\"p99\":{},\"max\":{}}},",
                "\"cuda_ns\":{{\"p50\":{},\"p95\":{},\"p99\":{},\"max\":{}}},",
                "\"cpu_over_cuda_speedup_milli\":{},\"winner\":\"{}\"}}"
            ),
            result.name,
            result.elements,
            result.cpu_first_ns,
            result.cuda_first_ns,
            result.cpu.p50,
            result.cpu.p95,
            result.cpu.p99,
            result.cpu.maximum,
            result.cuda.p50,
            result.cuda.p95,
            result.cuda.p99,
            result.cuda.maximum,
            speedup_milli,
            winner,
        );
    }
    println!(
        concat!(
            "],\"cuda_status\":{{\"active\":\"{:?}\",\"degraded\":{},",
            "\"circuit_open\":{}}},\"cuda_queue\":{{\"completed\":{},",
            "\"failed\":{},\"high_watermark\":{}}}}}"
        ),
        cuda_status.active,
        cuda_status.degraded,
        cuda_status.circuit_open,
        cuda_metrics.completed,
        cuda_metrics.failed,
        cuda_metrics.high_watermark,
    );
    cpu.shutdown();
    cuda.shutdown();
    Ok(())
}

fn parse_iterations() -> Result<usize, Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let Some(flag) = args.next() else {
        return Ok(DEFAULT_ITERATIONS);
    };
    if flag != "--iterations" {
        return Err(format!("unknown argument {flag}; expected --iterations N").into());
    }
    let iterations = args
        .next()
        .ok_or("--iterations requires a value")?
        .parse::<usize>()?;
    if iterations == 0 {
        return Err("--iterations must be positive".into());
    }
    if args.next().is_some() {
        return Err("unexpected trailing benchmark argument".into());
    }
    Ok(iterations)
}

fn execute(executor: &ComputeExecutor, job: ComputeJob) -> Result<ComputeResult, Box<dyn Error>> {
    Ok(executor
        .submit(JobPriority::Interactive, None, job)?
        .wait()?)
}

fn timed_execute(
    executor: &ComputeExecutor,
    job: ComputeJob,
) -> Result<(ComputeResult, u64), Box<dyn Error>> {
    let started = Instant::now();
    let result = execute(executor, job)?;
    let elapsed = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
    Ok((result, elapsed))
}

fn measure_alternating(
    cpu: &ComputeExecutor,
    cuda: &ComputeExecutor,
    job: &ComputeJob,
    iterations: usize,
) -> Result<(Vec<u64>, Vec<u64>), Box<dyn Error>> {
    let mut cpu_samples = Vec::with_capacity(iterations);
    let mut cuda_samples = Vec::with_capacity(iterations);
    for index in 0..iterations {
        if index.is_multiple_of(2) {
            let (result, elapsed) = timed_execute(cpu, job.clone())?;
            black_box(result);
            cpu_samples.push(elapsed);
            let (result, elapsed) = timed_execute(cuda, job.clone())?;
            black_box(result);
            cuda_samples.push(elapsed);
        } else {
            let (result, elapsed) = timed_execute(cuda, job.clone())?;
            black_box(result);
            cuda_samples.push(elapsed);
            let (result, elapsed) = timed_execute(cpu, job.clone())?;
            black_box(result);
            cpu_samples.push(elapsed);
        }
    }
    Ok((cpu_samples, cuda_samples))
}

struct WorkloadProfile {
    name: &'static str,
    elements: usize,
    job: ComputeJob,
}

fn profiles() -> Vec<WorkloadProfile> {
    vec![
        occupancy("voxel_small", 160 * 160, 8),
        occupancy("voxel_large", 400 * 400, 16),
        lidar("lidar_small", 720),
        lidar("lidar_large", 10_000),
        rgb("camera_small", 320, 240),
        rgb("camera_large", 640, 480),
        cognition("cognition_small", 4_096),
        cognition("cognition_large", 65_536),
    ]
}

fn occupancy(name: &'static str, cells: usize, depth: u32) -> WorkloadProfile {
    let cells = (0..cells)
        .map(|index| match index % 3 {
            0 => -1,
            1 => 0,
            _ => 100,
        })
        .collect::<Vec<_>>();
    WorkloadProfile {
        name,
        elements: cells.len().saturating_mul(depth as usize),
        job: ComputeJob::ProjectOccupancy { cells, depth },
    }
}

fn lidar(name: &'static str, count: usize) -> WorkloadProfile {
    let ranges_m = (0..count)
        .map(|index| {
            if index % 127 == 0 {
                f32::NAN
            } else {
                0.1 + (index % 1_100) as f32 * 0.01
            }
        })
        .collect::<Vec<_>>();
    WorkloadProfile {
        name,
        elements: ranges_m.len(),
        job: ComputeJob::LidarTransform {
            ranges_m,
            angle_min_rad: -core::f32::consts::PI,
            angle_increment_rad: core::f32::consts::TAU / count as f32,
            range_min_m: 0.05,
            range_max_m: 12.0,
            yaw_offset_rad: 0.1,
            clockwise: false,
        },
    }
}

fn rgb(name: &'static str, width: usize, height: usize) -> WorkloadProfile {
    let values = width.saturating_mul(height).saturating_mul(3);
    let input = (0..values)
        .map(|index| (index % 256) as u8)
        .collect::<Vec<_>>();
    WorkloadProfile {
        name,
        elements: input.len(),
        job: ComputeJob::NormalizeRgbU8 {
            input,
            mean: [0.485, 0.456, 0.406],
            inverse_std: [4.366_812, 4.464_286, 4.444_444],
        },
    }
}

fn cognition(name: &'static str, count: usize) -> WorkloadProfile {
    let lower = (0..count)
        .map(|index| (index % 100) as f32 * 0.01 - 0.5)
        .collect::<Vec<_>>();
    let top_down = (0..count)
        .map(|index| (index % 50) as f32 * 0.01 - 0.25)
        .collect::<Vec<_>>();
    WorkloadProfile {
        name,
        elements: count,
        job: ComputeJob::PredictiveStep {
            lower,
            state: PredictiveState {
                state: vec![0.1; count],
                weights: vec![0.75; count],
                bias: vec![0.0; count],
            },
            top_down,
            source_precision: 1.0,
            top_precision: 0.5,
        },
    }
}

struct BenchmarkResult {
    name: &'static str,
    elements: usize,
    cpu_first_ns: u64,
    cuda_first_ns: u64,
    cpu: Distribution,
    cuda: Distribution,
}

struct Distribution {
    p50: u64,
    p95: u64,
    p99: u64,
    maximum: u64,
}

impl Distribution {
    fn new(mut samples: Vec<u64>) -> Result<Self, Box<dyn Error>> {
        if samples.is_empty() {
            return Err("benchmark distribution has no samples".into());
        }
        samples.sort_unstable();
        Ok(Self {
            p50: percentile(&samples, 50),
            p95: percentile(&samples, 95),
            p99: percentile(&samples, 99),
            maximum: *samples.last().expect("non-empty samples"),
        })
    }
}

fn percentile(sorted: &[u64], percentile: usize) -> u64 {
    let rank = sorted.len().saturating_mul(percentile).saturating_add(99) / 100;
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

fn assert_parity(cpu: &ComputeResult, cuda: &ComputeResult) -> Result<(), Box<dyn Error>> {
    match (cpu, cuda) {
        (ComputeResult::Occupancy(cpu), ComputeResult::Occupancy(cuda)) => {
            if cpu != cuda {
                return Err("occupancy parity failed".into());
            }
        }
        (ComputeResult::Lidar(cpu), ComputeResult::Lidar(cuda)) => {
            if cpu.len() != cuda.len() {
                return Err("lidar result lengths differ".into());
            }
            for (cpu, cuda) in cpu.iter().zip(cuda) {
                if cpu.valid != cuda.valid
                    || cpu.valid && (!close(cpu.x_m, cuda.x_m) || !close(cpu.y_m, cuda.y_m))
                {
                    return Err("lidar parity failed".into());
                }
            }
        }
        (ComputeResult::NormalizedRgb(cpu), ComputeResult::NormalizedRgb(cuda)) => {
            assert_float_slice(cpu, cuda)?;
        }
        (ComputeResult::Predictive(cpu), ComputeResult::Predictive(cuda)) => {
            assert_float_slice(&cpu.state, &cuda.state)?;
            assert_float_slice(&cpu.weights, &cuda.weights)?;
            assert_float_slice(&cpu.bias, &cuda.bias)?;
        }
        _ => return Err("CPU and CUDA result variants differ".into()),
    }
    Ok(())
}

fn assert_float_slice(cpu: &[f32], cuda: &[f32]) -> Result<(), Box<dyn Error>> {
    if cpu.len() != cuda.len() {
        return Err("floating-point result lengths differ".into());
    }
    if cpu.iter().zip(cuda).any(|(cpu, cuda)| !close(*cpu, *cuda)) {
        return Err("floating-point parity failed".into());
    }
    Ok(())
}

fn close(left: f32, right: f32) -> bool {
    (left - right).abs() <= 1e-5 + 1e-5 * left.abs().max(right.abs())
}
