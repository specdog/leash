use std::{error::Error, thread, time::Duration};

use leash_core::{
    ActivityId, BeliefId, Meters, MetersPerSecond, MonotonicNanos, ProducerEpoch, Sequence,
};
use leash_ros2::{
    ClockCorrelation, NativeRosExecutor, NativeRosExecutorConfig, Nav2Kinematics, RosIngressQueues,
};

fn main() -> Result<(), Box<dyn Error>> {
    let duration = parse_duration()?;
    let mut queues = RosIngressQueues::new(8)?;
    let config = NativeRosExecutorConfig::new(
        ClockCorrelation {
            monotonic_origin: MonotonicNanos::ZERO,
            ros_origin_ns: 0,
        },
        Nav2Kinematics::new(Meters::new(0.4)?, MetersPerSecond::new(1.0)?)?,
        ActivityId::new(ProducerEpoch::new(91)?, Sequence::new(1)?),
        ProducerEpoch::new(92)?,
        Box::new([BeliefId::new(ProducerEpoch::new(93)?, Sequence::new(1)?)]),
    )?;
    let mut executor = NativeRosExecutor::from_env(queues.handle(), config)?;
    let halt = executor.halt_handle();
    let timer = thread::spawn(move || {
        thread::sleep(duration);
        halt.halt_spinning();
    });
    let errors = executor.spin_default();
    timer.join().map_err(|_| "native ROS timer panicked")?;
    let metrics = executor.metrics();
    let snapshot = queues.snapshot();
    let received_scan = queues.take_scan().is_some();
    let received_imu = queues.take_imu().is_some();
    let received_odometry = queues.take_odometry().is_some();
    let received_transform = queues.take_transform().is_some();
    let received_map = queues.take_map().is_some();
    let received_localization = queues.take_localization().is_some();
    let received_path = queues.take_path().is_some();
    let received_proposal = queues.take_proposal().is_some();
    println!(
        concat!(
            "{{\"schema_version\":\"leash.native-ros2-smoke.v1\",",
            "\"executor_errors\":{},\"callbacks_accepted\":{},",
            "\"conversion_rejected\":{},\"queue_rejected\":{},",
            "\"received\":{{\"scan\":{},\"imu\":{},\"odometry\":{},",
            "\"transform\":{},\"map\":{},\"localization\":{},",
            "\"path\":{},\"proposal\":{}}},",
            "\"qos_depths\":{{\"sensor\":{},\"command\":{},\"map\":{}}},",
            "\"proposal_capacity\":{},\"hardware_access\":false}}"
        ),
        errors.len(),
        metrics.callbacks_accepted,
        metrics.conversion_rejected,
        metrics.queue_rejected,
        received_scan,
        received_imu,
        received_odometry,
        received_transform,
        received_map,
        received_localization,
        received_path,
        received_proposal,
        leash_ros2::SENSOR_QOS.depth,
        leash_ros2::COMMAND_QOS.depth,
        leash_ros2::MAP_QOS.depth,
        snapshot.proposals.capacity,
    );
    if !errors.is_empty() {
        return Err(format!("native ROS executor reported {} errors", errors.len()).into());
    }
    Ok(())
}

fn parse_duration() -> Result<Duration, Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let Some(flag) = args.next() else {
        return Ok(Duration::from_millis(250));
    };
    if flag != "--duration-ms" {
        return Err(format!("unknown argument {flag}; expected --duration-ms N").into());
    }
    let duration = args
        .next()
        .ok_or("--duration-ms requires a value")?
        .parse::<u64>()?;
    if duration == 0 || args.next().is_some() {
        return Err("duration must be positive with no trailing arguments".into());
    }
    Ok(Duration::from_millis(duration))
}
