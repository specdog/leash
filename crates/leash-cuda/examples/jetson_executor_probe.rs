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
        ComputeJob::CollisionSectorReduce {
            ranges_m: vec![1.0, f32::NAN, 0.25, f32::INFINITY, 20.0],
            angle_min_rad: -0.2,
            angle_increment_rad: 0.1,
            range_min_m: 0.05,
            range_max_m: 12.0,
            sector_center_rad: 0.0,
            sector_half_width_rad: 0.21,
        },
        ComputeJob::LidarTransformAndCollision {
            ranges_m: vec![1.0, f32::NAN, 0.25, f32::INFINITY, 20.0],
            angle_min_rad: -0.2,
            angle_increment_rad: 0.1,
            range_min_m: 0.05,
            range_max_m: 12.0,
            yaw_offset_rad: 0.1,
            clockwise: false,
            sector_center_rad: 0.0,
            sector_half_width_rad: 0.21,
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
    for job in edge_and_maximum_jobs() {
        let cpu_result = run(&cpu, job.clone())?;
        let cuda_result = run(&cuda, job)?;
        assert_parity(&cpu_result, &cuda_result);
    }
    let mut seed = 0x05ee_d203_u32;
    for round in 0..32 {
        for job in randomized_jobs(&mut seed, round) {
            let cpu_result = run(&cpu, job.clone())?;
            let cuda_result = run(&cuda, job)?;
            assert_parity(&cpu_result, &cuda_result);
        }
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
        (ComputeResult::CollisionSector(cpu), ComputeResult::CollisionSector(cuda)) => {
            assert_eq!(cpu.sample_count, cuda.sample_count);
            match (cpu.min_range_m, cuda.min_range_m) {
                (Some(cpu), Some(cuda)) => assert!(close(cpu, cuda)),
                (None, None) => {}
                _ => panic!("CPU and CUDA collision minima differ"),
            }
        }
        (
            ComputeResult::Spatial {
                lidar: cpu_lidar,
                collision: cpu_collision,
            },
            ComputeResult::Spatial {
                lidar: cuda_lidar,
                collision: cuda_collision,
            },
        ) => {
            assert_parity(
                &ComputeResult::Lidar(cpu_lidar.clone()),
                &ComputeResult::Lidar(cuda_lidar.clone()),
            );
            assert_parity(
                &ComputeResult::CollisionSector(*cpu_collision),
                &ComputeResult::CollisionSector(*cuda_collision),
            );
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

#[cfg(feature = "cuda")]
fn edge_and_maximum_jobs() -> Vec<ComputeJob> {
    const MAX_TEST_ELEMENTS: usize = 1_000_000;
    vec![
        ComputeJob::ProjectOccupancy {
            cells: Vec::new(),
            depth: 1,
        },
        ComputeJob::LidarTransform {
            ranges_m: Vec::new(),
            angle_min_rad: 0.0,
            angle_increment_rad: 0.1,
            range_min_m: 0.05,
            range_max_m: 12.0,
            yaw_offset_rad: 0.0,
            clockwise: false,
        },
        ComputeJob::CollisionSectorReduce {
            ranges_m: Vec::new(),
            angle_min_rad: 0.0,
            angle_increment_rad: 0.1,
            range_min_m: 0.05,
            range_max_m: 12.0,
            sector_center_rad: 0.0,
            sector_half_width_rad: 0.5,
        },
        ComputeJob::NormalizeRgbU8 {
            input: Vec::new(),
            mean: [0.5; 3],
            inverse_std: [2.0; 3],
        },
        ComputeJob::PredictiveStep {
            lower: Vec::new(),
            state: PredictiveState {
                state: Vec::new(),
                weights: Vec::new(),
                bias: Vec::new(),
            },
            top_down: Vec::new(),
            source_precision: 1.0,
            top_precision: 0.5,
        },
        ComputeJob::ProjectOccupancy {
            cells: vec![100; MAX_TEST_ELEMENTS / 8],
            depth: 8,
        },
        ComputeJob::LidarTransform {
            ranges_m: vec![1.0; MAX_TEST_ELEMENTS],
            angle_min_rad: -core::f32::consts::PI,
            angle_increment_rad: core::f32::consts::TAU / MAX_TEST_ELEMENTS as f32,
            range_min_m: 0.05,
            range_max_m: 12.0,
            yaw_offset_rad: 0.0,
            clockwise: false,
        },
        ComputeJob::CollisionSectorReduce {
            ranges_m: vec![1.0; MAX_TEST_ELEMENTS],
            angle_min_rad: -core::f32::consts::PI,
            angle_increment_rad: core::f32::consts::TAU / MAX_TEST_ELEMENTS as f32,
            range_min_m: 0.05,
            range_max_m: 12.0,
            sector_center_rad: 0.0,
            sector_half_width_rad: 0.25,
        },
        ComputeJob::NormalizeRgbU8 {
            input: vec![127; MAX_TEST_ELEMENTS / 3 * 3],
            mean: [0.5; 3],
            inverse_std: [2.0; 3],
        },
        ComputeJob::PredictiveStep {
            lower: vec![0.5; MAX_TEST_ELEMENTS],
            state: PredictiveState {
                state: vec![0.1; MAX_TEST_ELEMENTS],
                weights: vec![0.75; MAX_TEST_ELEMENTS],
                bias: vec![0.0; MAX_TEST_ELEMENTS],
            },
            top_down: vec![0.0; MAX_TEST_ELEMENTS],
            source_precision: 1.0,
            top_precision: 0.5,
        },
    ]
}

#[cfg(feature = "cuda")]
fn randomized_jobs(seed: &mut u32, round: usize) -> Vec<ComputeJob> {
    let count = if round == 0 {
        0
    } else {
        (lcg(seed) % 4_096) as usize
    };
    let cells = (0..count).map(|_| lcg(seed) as i8).collect::<Vec<_>>();
    let ranges_m = (0..count)
        .map(|index| match index % 37 {
            0 => f32::NAN,
            1 => f32::INFINITY,
            _ => 0.01 + (lcg(seed) % 1_500) as f32 / 100.0,
        })
        .collect::<Vec<_>>();
    let rgb = (0..count.saturating_mul(3))
        .map(|_| lcg(seed) as u8)
        .collect::<Vec<_>>();
    let lower = (0..count).map(|_| random_unit(seed)).collect::<Vec<_>>();
    let state = (0..count).map(|_| random_unit(seed)).collect::<Vec<_>>();
    let top_down = (0..count).map(|_| random_unit(seed)).collect::<Vec<_>>();
    let weights = (0..count)
        .map(|_| 0.5 + random_unit(seed).abs())
        .collect::<Vec<_>>();
    let bias = (0..count)
        .map(|_| random_unit(seed) * 0.25)
        .collect::<Vec<_>>();
    let angle_increment_rad = core::f32::consts::TAU / count.max(1) as f32;
    let combined_ranges = ranges_m.clone();
    vec![
        ComputeJob::ProjectOccupancy {
            cells,
            depth: 1 + lcg(seed) % 8,
        },
        ComputeJob::LidarTransform {
            ranges_m: ranges_m.clone(),
            angle_min_rad: -core::f32::consts::PI,
            angle_increment_rad,
            range_min_m: 0.05,
            range_max_m: 12.0,
            yaw_offset_rad: random_unit(seed),
            clockwise: lcg(seed).is_multiple_of(2),
        },
        ComputeJob::CollisionSectorReduce {
            ranges_m,
            angle_min_rad: -core::f32::consts::PI,
            angle_increment_rad,
            range_min_m: 0.05,
            range_max_m: 12.0,
            sector_center_rad: random_unit(seed) * core::f32::consts::PI,
            sector_half_width_rad: 0.05 + (lcg(seed) % 250) as f32 / 100.0,
        },
        ComputeJob::LidarTransformAndCollision {
            ranges_m: combined_ranges,
            angle_min_rad: -core::f32::consts::PI,
            angle_increment_rad,
            range_min_m: 0.05,
            range_max_m: 12.0,
            yaw_offset_rad: random_unit(seed),
            clockwise: lcg(seed).is_multiple_of(2),
            sector_center_rad: random_unit(seed) * core::f32::consts::PI,
            sector_half_width_rad: 0.05 + (lcg(seed) % 250) as f32 / 100.0,
        },
        ComputeJob::NormalizeRgbU8 {
            input: rgb,
            mean: [0.485, 0.456, 0.406],
            inverse_std: [4.366_812, 4.464_286, 4.444_444],
        },
        ComputeJob::PredictiveStep {
            lower,
            state: PredictiveState {
                state,
                weights,
                bias,
            },
            top_down,
            source_precision: 0.5 + random_unit(seed).abs(),
            top_precision: 0.25 + random_unit(seed).abs(),
        },
    ]
}

#[cfg(feature = "cuda")]
fn random_unit(seed: &mut u32) -> f32 {
    lcg(seed) as f32 / u32::MAX as f32 * 2.0 - 1.0
}

#[cfg(feature = "cuda")]
fn lcg(seed: &mut u32) -> u32 {
    *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    *seed
}
