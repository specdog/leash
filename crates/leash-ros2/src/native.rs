//! Native `rclrs` executor and generated ROS message adapters.

use std::{
    any::Any,
    fmt,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};

use leash_core::{
    ActivityId, Base, BeliefId, DurationNanos, Effect, MonotonicNanos, Odom, ProducerEpoch,
    Proposal, ProposalId, Sequence,
};
use rclrs::{CreateBasicExecutor, IntoPrimitiveOptions};
use ros_env::{builtin_interfaces, geometry_msgs, nav_msgs, sensor_msgs, std_msgs, tf2_msgs};

use crate::{
    cancel_to_proposal, cmd_vel_to_proposal, imu_from_ros, imu_to_ros, localization_from_ros,
    localization_to_ros, map_from_ros, map_to_ros, navigation_goal_from_ros, odom_from_ros,
    odom_to_ros, path_from_ros, path_to_ros, scan_from_ros, scan_to_ros, transform_from_ros,
    transform_to_ros, ClockCorrelation, ConversionError, Header, ImuMessage, ImuObservation,
    LaserScanMessage, MapObservation, Nav2Kinematics, NavigationGoal, OccupancyGridMessage,
    OdomObservation, OdometryMessage, PathMessage, PathProposal, PlanarTransform,
    PoseStampedMessage, Quaternion, RosCallbackHandle, RosTime, TransformStampedMessage, Twist,
    Vector3,
};

pub fn scan_from_generated(
    message: &sensor_msgs::msg::LaserScan,
    clock: ClockCorrelation,
) -> Result<crate::ScanObservation, ConversionError> {
    scan_from_ros(
        &LaserScanMessage {
            header: header_from_generated(&message.header),
            angle_min: message.angle_min,
            angle_max: message.angle_max,
            angle_increment: message.angle_increment,
            range_min: message.range_min,
            range_max: message.range_max,
            ranges: message.ranges.clone(),
        },
        clock,
    )
}

pub fn scan_to_generated(
    observation: &crate::ScanObservation,
    clock: ClockCorrelation,
) -> Result<sensor_msgs::msg::LaserScan, ConversionError> {
    let message = scan_to_ros(observation, clock)?;
    Ok(sensor_msgs::msg::LaserScan {
        header: header_to_generated(message.header),
        angle_min: message.angle_min,
        angle_max: message.angle_max,
        angle_increment: message.angle_increment,
        range_min: message.range_min,
        range_max: message.range_max,
        ranges: message.ranges,
        ..sensor_msgs::msg::LaserScan::default()
    })
}

pub fn imu_from_generated(
    message: &sensor_msgs::msg::Imu,
    clock: ClockCorrelation,
) -> Result<ImuObservation, ConversionError> {
    imu_from_ros(
        &ImuMessage {
            header: header_from_generated(&message.header),
            orientation: quaternion_from_generated(&message.orientation),
            angular_velocity: vector_from_generated(&message.angular_velocity),
            linear_acceleration: vector_from_generated(&message.linear_acceleration),
        },
        clock,
    )
}

pub fn imu_to_generated(
    observation: &ImuObservation,
    clock: ClockCorrelation,
) -> Result<sensor_msgs::msg::Imu, ConversionError> {
    let message = imu_to_ros(observation, clock)?;
    Ok(sensor_msgs::msg::Imu {
        header: header_to_generated(message.header),
        orientation: quaternion_to_generated(message.orientation),
        angular_velocity: vector_to_generated(message.angular_velocity),
        linear_acceleration: vector_to_generated(message.linear_acceleration),
        ..sensor_msgs::msg::Imu::default()
    })
}

pub fn odometry_from_generated(
    message: &nav_msgs::msg::Odometry,
    clock: ClockCorrelation,
) -> Result<OdomObservation, ConversionError> {
    odom_from_ros(
        &OdometryMessage {
            header: header_from_generated(&message.header),
            child_frame_id: message.child_frame_id.clone(),
            position: point_from_generated(&message.pose.pose.position),
            orientation: quaternion_from_generated(&message.pose.pose.orientation),
            twist: twist_from_generated(&message.twist.twist),
        },
        clock,
    )
}

pub fn odometry_to_generated(
    observation: &OdomObservation,
    clock: ClockCorrelation,
) -> Result<nav_msgs::msg::Odometry, ConversionError> {
    let message = odom_to_ros(observation, clock)?;
    let mut generated = nav_msgs::msg::Odometry {
        header: header_to_generated(message.header),
        child_frame_id: message.child_frame_id,
        ..nav_msgs::msg::Odometry::default()
    };
    generated.pose.pose.position = point_to_generated(message.position);
    generated.pose.pose.orientation = quaternion_to_generated(message.orientation);
    generated.twist.twist = twist_to_generated(message.twist);
    Ok(generated)
}

pub fn transform_from_generated(
    message: &geometry_msgs::msg::TransformStamped,
    clock: ClockCorrelation,
) -> Result<PlanarTransform<Odom, Base>, ConversionError> {
    transform_from_ros(
        &TransformStampedMessage {
            header: header_from_generated(&message.header),
            child_frame_id: message.child_frame_id.clone(),
            translation: vector_from_generated(&message.transform.translation),
            rotation: quaternion_from_generated(&message.transform.rotation),
        },
        clock,
    )
}

pub fn transform_to_generated(
    transform: &PlanarTransform<Odom, Base>,
    clock: ClockCorrelation,
) -> Result<geometry_msgs::msg::TransformStamped, ConversionError> {
    let message = transform_to_ros(transform, clock)?;
    Ok(geometry_msgs::msg::TransformStamped {
        header: header_to_generated(message.header),
        child_frame_id: message.child_frame_id,
        transform: geometry_msgs::msg::Transform {
            translation: vector_to_generated(message.translation),
            rotation: quaternion_to_generated(message.rotation),
        },
    })
}

pub fn map_from_generated(
    message: &nav_msgs::msg::OccupancyGrid,
    clock: ClockCorrelation,
) -> Result<MapObservation, ConversionError> {
    map_from_ros(
        &OccupancyGridMessage {
            header: header_from_generated(&message.header),
            resolution: message.info.resolution,
            width: message.info.width,
            height: message.info.height,
            origin: point_from_generated(&message.info.origin.position),
            origin_orientation: quaternion_from_generated(&message.info.origin.orientation),
            data: message.data.clone(),
        },
        clock,
    )
}

pub fn map_to_generated(
    observation: &MapObservation,
    clock: ClockCorrelation,
) -> Result<nav_msgs::msg::OccupancyGrid, ConversionError> {
    let message = map_to_ros(observation, clock)?;
    let mut generated = nav_msgs::msg::OccupancyGrid {
        header: header_to_generated(message.header),
        data: message.data,
        ..nav_msgs::msg::OccupancyGrid::default()
    };
    generated.info.resolution = message.resolution;
    generated.info.width = message.width;
    generated.info.height = message.height;
    generated.info.origin.position = point_to_generated(message.origin);
    generated.info.origin.orientation = quaternion_to_generated(message.origin_orientation);
    Ok(generated)
}

pub fn localization_from_generated(
    message: &geometry_msgs::msg::PoseWithCovarianceStamped,
    clock: ClockCorrelation,
) -> Result<crate::LocalizationObservation, ConversionError> {
    let (pose, at) = localization_from_ros(
        &PoseStampedMessage {
            header: header_from_generated(&message.header),
            position: point_from_generated(&message.pose.pose.position),
            orientation: quaternion_from_generated(&message.pose.pose.orientation),
        },
        clock,
    )?;
    Ok(crate::LocalizationObservation { pose, at })
}

pub fn localization_to_generated(
    observation: &crate::LocalizationObservation,
    clock: ClockCorrelation,
) -> Result<geometry_msgs::msg::PoseWithCovarianceStamped, ConversionError> {
    let message = localization_to_ros(&observation.pose, observation.at, clock)?;
    let mut generated = geometry_msgs::msg::PoseWithCovarianceStamped {
        header: header_to_generated(message.header),
        ..geometry_msgs::msg::PoseWithCovarianceStamped::default()
    };
    generated.pose.pose.position = point_to_generated(message.position);
    generated.pose.pose.orientation = quaternion_to_generated(message.orientation);
    Ok(generated)
}

pub fn path_from_generated(
    message: &nav_msgs::msg::Path,
    clock: ClockCorrelation,
) -> Result<PathProposal, ConversionError> {
    path_from_ros(
        &PathMessage {
            header: header_from_generated(&message.header),
            poses: message
                .poses
                .iter()
                .map(|pose| PoseStampedMessage {
                    header: header_from_generated(&pose.header),
                    position: point_from_generated(&pose.pose.position),
                    orientation: quaternion_from_generated(&pose.pose.orientation),
                })
                .collect(),
        },
        clock,
    )
}

pub fn path_to_generated(
    proposal: &PathProposal,
    clock: ClockCorrelation,
) -> Result<nav_msgs::msg::Path, ConversionError> {
    let message = path_to_ros(proposal, clock)?;
    Ok(nav_msgs::msg::Path {
        header: header_to_generated(message.header),
        poses: message
            .poses
            .into_iter()
            .map(|pose| geometry_msgs::msg::PoseStamped {
                header: header_to_generated(pose.header),
                pose: geometry_msgs::msg::Pose {
                    position: point_to_generated(pose.position),
                    orientation: quaternion_to_generated(pose.orientation),
                },
            })
            .collect(),
    })
}

fn header_from_generated(header: &std_msgs::msg::Header) -> Header {
    Header {
        stamp: RosTime {
            sec: header.stamp.sec,
            nanosec: header.stamp.nanosec,
        },
        frame_id: header.frame_id.clone(),
    }
}

fn header_to_generated(header: Header) -> std_msgs::msg::Header {
    std_msgs::msg::Header {
        stamp: builtin_interfaces::msg::Time {
            sec: header.stamp.sec,
            nanosec: header.stamp.nanosec,
        },
        frame_id: header.frame_id,
    }
}

fn vector_from_generated(vector: &geometry_msgs::msg::Vector3) -> Vector3 {
    Vector3 {
        x: vector.x,
        y: vector.y,
        z: vector.z,
    }
}

fn vector_to_generated(vector: Vector3) -> geometry_msgs::msg::Vector3 {
    geometry_msgs::msg::Vector3 {
        x: vector.x,
        y: vector.y,
        z: vector.z,
    }
}

fn point_from_generated(point: &geometry_msgs::msg::Point) -> Vector3 {
    Vector3 {
        x: point.x,
        y: point.y,
        z: point.z,
    }
}

fn point_to_generated(point: Vector3) -> geometry_msgs::msg::Point {
    geometry_msgs::msg::Point {
        x: point.x,
        y: point.y,
        z: point.z,
    }
}

fn quaternion_from_generated(quaternion: &geometry_msgs::msg::Quaternion) -> Quaternion {
    Quaternion {
        x: quaternion.x,
        y: quaternion.y,
        z: quaternion.z,
        w: quaternion.w,
    }
}

fn quaternion_to_generated(quaternion: Quaternion) -> geometry_msgs::msg::Quaternion {
    geometry_msgs::msg::Quaternion {
        x: quaternion.x,
        y: quaternion.y,
        z: quaternion.z,
        w: quaternion.w,
    }
}

fn twist_from_generated(twist: &geometry_msgs::msg::Twist) -> Twist {
    Twist {
        linear: vector_from_generated(&twist.linear),
        angular: vector_from_generated(&twist.angular),
    }
}

fn twist_to_generated(twist: Twist) -> geometry_msgs::msg::Twist {
    geometry_msgs::msg::Twist {
        linear: vector_to_generated(twist.linear),
        angular: vector_to_generated(twist.angular),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeRosTopics {
    pub scan: String,
    pub imu: String,
    pub odometry: String,
    pub transforms: String,
    pub map: String,
    pub localization: String,
    pub path: String,
    pub velocity_proposal: String,
    pub navigation_goal: String,
    pub cancellation: String,
}

impl Default for NativeRosTopics {
    fn default() -> Self {
        Self {
            scan: "/scan".to_string(),
            imu: "/imu/data".to_string(),
            odometry: "/odom".to_string(),
            transforms: "/tf".to_string(),
            map: "/map".to_string(),
            localization: "/amcl_pose".to_string(),
            path: "/plan".to_string(),
            velocity_proposal: "/cmd_vel".to_string(),
            navigation_goal: "/goal_pose".to_string(),
            cancellation: "/leash/nav2/cancel".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NativeRosExecutorConfig {
    pub node_name: String,
    pub topics: NativeRosTopics,
    pub clock: ClockCorrelation,
    pub kinematics: Nav2Kinematics,
    pub activity_id: ActivityId,
    pub proposal_epoch: ProducerEpoch,
    pub belief_lineage: Box<[BeliefId]>,
    pub command_ttl: DurationNanos,
    pub command_priority: u8,
}

impl NativeRosExecutorConfig {
    pub fn new(
        clock: ClockCorrelation,
        kinematics: Nav2Kinematics,
        activity_id: ActivityId,
        proposal_epoch: ProducerEpoch,
        belief_lineage: impl Into<Box<[BeliefId]>>,
    ) -> Result<Self, NativeRosStartError> {
        let belief_lineage = belief_lineage.into();
        if belief_lineage.is_empty() {
            return Err(NativeRosStartError::InvalidConfig(
                "belief lineage must not be empty",
            ));
        }
        Ok(Self {
            node_name: "leash_native_boundary".to_string(),
            topics: NativeRosTopics::default(),
            clock,
            kinematics,
            activity_id,
            proposal_epoch,
            belief_lineage,
            command_ttl: DurationNanos::from_millis(100)
                .expect("100 ms is representable in nanoseconds"),
            command_priority: 128,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeRosStartError {
    InvalidConfig(&'static str),
    Rclrs(String),
}

impl fmt::Display for NativeRosStartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(message) => {
                write!(formatter, "invalid native ROS config: {message}")
            }
            Self::Rclrs(message) => write!(formatter, "start native ROS executor: {message}"),
        }
    }
}

impl std::error::Error for NativeRosStartError {}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NativeRosMetrics {
    pub callbacks_accepted: u64,
    pub conversion_rejected: u64,
    pub queue_rejected: u64,
    pub sequence_exhausted: u64,
}

#[derive(Default)]
struct NativeRosMetricAtoms {
    callbacks_accepted: AtomicU64,
    conversion_rejected: AtomicU64,
    queue_rejected: AtomicU64,
    sequence_exhausted: AtomicU64,
}

impl NativeRosMetricAtoms {
    fn snapshot(&self) -> NativeRosMetrics {
        NativeRosMetrics {
            callbacks_accepted: self.callbacks_accepted.load(Ordering::Relaxed),
            conversion_rejected: self.conversion_rejected.load(Ordering::Relaxed),
            queue_rejected: self.queue_rejected.load(Ordering::Relaxed),
            sequence_exhausted: self.sequence_exhausted.load(Ordering::Relaxed),
        }
    }
}

pub struct NativeRosExecutor {
    executor: rclrs::Executor,
    _node: rclrs::Node,
    _subscriptions: Vec<Box<dyn Any + Send + Sync>>,
    metrics: Arc<NativeRosMetricAtoms>,
}

impl NativeRosExecutor {
    pub fn from_env(
        ingress: RosCallbackHandle,
        config: NativeRosExecutorConfig,
    ) -> Result<Self, NativeRosStartError> {
        validate_native_config(&config)?;
        let context = rclrs::Context::default_from_env()
            .map_err(|error| NativeRosStartError::Rclrs(error.to_string()))?;
        let executor = context.create_basic_executor();
        let node = executor
            .create_node(config.node_name.as_str())
            .map_err(|error| NativeRosStartError::Rclrs(error.to_string()))?;
        let metrics = Arc::new(NativeRosMetricAtoms::default());
        let worker = node.create_worker(CallbackState::new(
            ingress,
            config.clone(),
            Arc::clone(&metrics),
        ));
        let mut subscriptions: Vec<Box<dyn Any + Send + Sync>> = Vec::with_capacity(10);

        let scan = worker
            .create_subscription::<sensor_msgs::msg::LaserScan, _>(
                config.topics.scan.as_str().best_effort().keep_last(1),
                |state: &mut CallbackState, message: sensor_msgs::msg::LaserScan| {
                    let result = scan_from_generated(&message, state.config.clock);
                    state.submit_scan(result);
                },
            )
            .map_err(|error| NativeRosStartError::Rclrs(error.to_string()))?;
        subscriptions.push(Box::new(scan));

        let imu = worker
            .create_subscription::<sensor_msgs::msg::Imu, _>(
                config.topics.imu.as_str().best_effort().keep_last(1),
                |state: &mut CallbackState, message: sensor_msgs::msg::Imu| {
                    let result = imu_from_generated(&message, state.config.clock);
                    state.submit_imu(result);
                },
            )
            .map_err(|error| NativeRosStartError::Rclrs(error.to_string()))?;
        subscriptions.push(Box::new(imu));

        let odometry = worker
            .create_subscription::<nav_msgs::msg::Odometry, _>(
                config.topics.odometry.as_str().best_effort().keep_last(1),
                |state: &mut CallbackState, message: nav_msgs::msg::Odometry| {
                    let result = odometry_from_generated(&message, state.config.clock);
                    state.submit_odometry(result);
                },
            )
            .map_err(|error| NativeRosStartError::Rclrs(error.to_string()))?;
        subscriptions.push(Box::new(odometry));

        let transforms = worker
            .create_subscription::<tf2_msgs::msg::TFMessage, _>(
                config.topics.transforms.as_str().best_effort().keep_last(1),
                |state: &mut CallbackState, message: tf2_msgs::msg::TFMessage| {
                    for transform in message.transforms {
                        let result = transform_from_generated(&transform, state.config.clock);
                        state.submit_transform(result);
                    }
                },
            )
            .map_err(|error| NativeRosStartError::Rclrs(error.to_string()))?;
        subscriptions.push(Box::new(transforms));

        let map = worker
            .create_subscription::<nav_msgs::msg::OccupancyGrid, _>(
                config.topics.map.as_str().transient_local().keep_last(1),
                |state: &mut CallbackState, message: nav_msgs::msg::OccupancyGrid| {
                    let result = map_from_generated(&message, state.config.clock);
                    state.submit_map(result);
                },
            )
            .map_err(|error| NativeRosStartError::Rclrs(error.to_string()))?;
        subscriptions.push(Box::new(map));

        let localization = worker
            .create_subscription::<geometry_msgs::msg::PoseWithCovarianceStamped, _>(
                config.topics.localization.as_str().keep_last(1),
                |state: &mut CallbackState,
                 message: geometry_msgs::msg::PoseWithCovarianceStamped| {
                    let result = localization_from_generated(&message, state.config.clock);
                    state.submit_localization(result);
                },
            )
            .map_err(|error| NativeRosStartError::Rclrs(error.to_string()))?;
        subscriptions.push(Box::new(localization));

        let path = worker
            .create_subscription::<nav_msgs::msg::Path, _>(
                config.topics.path.as_str().keep_last(1),
                |state: &mut CallbackState, message: nav_msgs::msg::Path| {
                    let result = path_from_generated(&message, state.config.clock);
                    state.submit_path(result);
                },
            )
            .map_err(|error| NativeRosStartError::Rclrs(error.to_string()))?;
        subscriptions.push(Box::new(path));

        let velocity = worker
            .create_subscription::<geometry_msgs::msg::Twist, _>(
                config.topics.velocity_proposal.as_str().keep_last(8),
                |state: &mut CallbackState, message: geometry_msgs::msg::Twist| {
                    state.submit_velocity(twist_from_generated(&message));
                },
            )
            .map_err(|error| NativeRosStartError::Rclrs(error.to_string()))?;
        subscriptions.push(Box::new(velocity));

        let goal = worker
            .create_subscription::<geometry_msgs::msg::PoseStamped, _>(
                config.topics.navigation_goal.as_str().keep_last(8),
                |state: &mut CallbackState, message: geometry_msgs::msg::PoseStamped| {
                    state.submit_goal(message);
                },
            )
            .map_err(|error| NativeRosStartError::Rclrs(error.to_string()))?;
        subscriptions.push(Box::new(goal));

        let cancellation = worker
            .create_subscription::<std_msgs::msg::Bool, _>(
                config.topics.cancellation.as_str().keep_last(8),
                |state: &mut CallbackState, message: std_msgs::msg::Bool| {
                    if message.data {
                        state.submit_cancellation();
                    }
                },
            )
            .map_err(|error| NativeRosStartError::Rclrs(error.to_string()))?;
        subscriptions.push(Box::new(cancellation));

        Ok(Self {
            executor,
            _node: node,
            _subscriptions: subscriptions,
            metrics,
        })
    }

    pub fn metrics(&self) -> NativeRosMetrics {
        self.metrics.snapshot()
    }

    pub fn halt_handle(&self) -> Arc<rclrs::ExecutorCommands> {
        Arc::clone(self.executor.commands())
    }

    pub fn spin(&mut self, options: rclrs::SpinOptions) -> Vec<rclrs::RclrsError> {
        self.executor.spin(options)
    }

    pub fn spin_once(&mut self) -> Vec<rclrs::RclrsError> {
        self.executor.spin(rclrs::SpinOptions::spin_once())
    }

    pub fn spin_default(&mut self) -> Vec<rclrs::RclrsError> {
        self.executor.spin(rclrs::SpinOptions::default())
    }
}

fn validate_native_config(config: &NativeRosExecutorConfig) -> Result<(), NativeRosStartError> {
    if config.node_name.is_empty() {
        return Err(NativeRosStartError::InvalidConfig(
            "node name must not be empty",
        ));
    }
    if config.belief_lineage.is_empty() {
        return Err(NativeRosStartError::InvalidConfig(
            "belief lineage must not be empty",
        ));
    }
    if config.command_ttl == DurationNanos::ZERO {
        return Err(NativeRosStartError::InvalidConfig(
            "command TTL must be positive",
        ));
    }
    for topic in [
        &config.topics.scan,
        &config.topics.imu,
        &config.topics.odometry,
        &config.topics.transforms,
        &config.topics.map,
        &config.topics.localization,
        &config.topics.path,
        &config.topics.velocity_proposal,
        &config.topics.navigation_goal,
        &config.topics.cancellation,
    ] {
        if topic.is_empty() {
            return Err(NativeRosStartError::InvalidConfig(
                "topic names must not be empty",
            ));
        }
    }
    Ok(())
}

struct CallbackState {
    ingress: RosCallbackHandle,
    config: NativeRosExecutorConfig,
    metrics: Arc<NativeRosMetricAtoms>,
    steady_origin: Instant,
    next_scan: u64,
    next_imu: u64,
    next_odometry: u64,
    next_transform: u64,
    next_map: u64,
    next_localization: u64,
    next_path: u64,
    next_proposal: u64,
}

impl CallbackState {
    fn new(
        ingress: RosCallbackHandle,
        config: NativeRosExecutorConfig,
        metrics: Arc<NativeRosMetricAtoms>,
    ) -> Self {
        Self {
            ingress,
            config,
            metrics,
            steady_origin: Instant::now(),
            next_scan: 1,
            next_imu: 1,
            next_odometry: 1,
            next_transform: 1,
            next_map: 1,
            next_localization: 1,
            next_path: 1,
            next_proposal: 1,
        }
    }

    fn submit_scan(&mut self, result: Result<crate::ScanObservation, ConversionError>) {
        let Ok(value) = self.converted(result) else {
            return;
        };
        let Some(sequence) = take_sequence(&mut self.next_scan, &self.metrics) else {
            return;
        };
        self.record_queue(self.ingress.submit_scan(sequence, value).is_ok());
    }

    fn submit_imu(&mut self, result: Result<ImuObservation, ConversionError>) {
        let Ok(value) = self.converted(result) else {
            return;
        };
        let Some(sequence) = take_sequence(&mut self.next_imu, &self.metrics) else {
            return;
        };
        self.record_queue(self.ingress.submit_imu(sequence, value).is_ok());
    }

    fn submit_odometry(&mut self, result: Result<OdomObservation, ConversionError>) {
        let Ok(value) = self.converted(result) else {
            return;
        };
        let Some(sequence) = take_sequence(&mut self.next_odometry, &self.metrics) else {
            return;
        };
        self.record_queue(self.ingress.submit_odometry(sequence, value).is_ok());
    }

    fn submit_transform(&mut self, result: Result<PlanarTransform<Odom, Base>, ConversionError>) {
        let Ok(value) = self.converted(result) else {
            return;
        };
        let Some(sequence) = take_sequence(&mut self.next_transform, &self.metrics) else {
            return;
        };
        self.record_queue(self.ingress.submit_transform(sequence, value).is_ok());
    }

    fn submit_map(&mut self, result: Result<MapObservation, ConversionError>) {
        let Ok(value) = self.converted(result) else {
            return;
        };
        let Some(sequence) = take_sequence(&mut self.next_map, &self.metrics) else {
            return;
        };
        self.record_queue(self.ingress.submit_map(sequence, value).is_ok());
    }

    fn submit_localization(
        &mut self,
        result: Result<crate::LocalizationObservation, ConversionError>,
    ) {
        let Ok(value) = self.converted(result) else {
            return;
        };
        let Some(sequence) = take_sequence(&mut self.next_localization, &self.metrics) else {
            return;
        };
        self.record_queue(self.ingress.submit_localization(sequence, value).is_ok());
    }

    fn submit_path(&mut self, result: Result<PathProposal, ConversionError>) {
        let Ok(value) = self.converted(result) else {
            return;
        };
        let Some(sequence) = take_sequence(&mut self.next_path, &self.metrics) else {
            return;
        };
        self.record_queue(self.ingress.submit_path(sequence, value).is_ok());
    }

    fn submit_velocity(&mut self, twist: Twist) {
        let Some((id, now, deadline)) = self.next_proposal_context() else {
            return;
        };
        let proposal = cmd_vel_to_proposal(
            twist,
            self.config.kinematics,
            id,
            self.config.activity_id,
            now,
            deadline,
            self.config.command_priority,
            self.config.belief_lineage.clone(),
        );
        self.submit_proposal(proposal);
    }

    fn submit_goal(&mut self, message: geometry_msgs::msg::PoseStamped) {
        let dto = PoseStampedMessage {
            header: header_from_generated(&message.header),
            position: point_from_generated(&message.pose.position),
            orientation: quaternion_from_generated(&message.pose.orientation),
        };
        let goal = navigation_goal_from_ros(self.config.activity_id, &dto, self.config.clock);
        let Ok(NavigationGoal {
            pose, received_at, ..
        }) = self.converted(goal)
        else {
            return;
        };
        let Some(sequence) = take_sequence(&mut self.next_proposal, &self.metrics) else {
            return;
        };
        let Some(deadline) = received_at.checked_add(self.config.command_ttl).ok() else {
            self.reject_conversion();
            return;
        };
        self.submit_proposal(
            Proposal::new(
                ProposalId::new(self.config.proposal_epoch, sequence),
                self.config.activity_id,
                Effect::SetNavigationGoal(pose),
                received_at,
                deadline,
                self.config.command_priority,
                self.config.belief_lineage.clone(),
            )
            .map_err(ConversionError::Proposal),
        );
    }

    fn submit_cancellation(&mut self) {
        let Some((id, now, _deadline)) = self.next_proposal_context() else {
            return;
        };
        let proposal = cancel_to_proposal(
            id,
            self.config.activity_id,
            now,
            self.config.belief_lineage.clone(),
        );
        self.submit_proposal(proposal);
    }

    fn submit_proposal(&self, result: Result<Proposal, ConversionError>) {
        let Ok(proposal) = result else {
            self.reject_conversion();
            return;
        };
        self.record_queue(self.ingress.submit_proposal(proposal).is_ok());
    }

    fn next_proposal_context(&mut self) -> Option<(ProposalId, MonotonicNanos, MonotonicNanos)> {
        let sequence = take_sequence(&mut self.next_proposal, &self.metrics)?;
        let now = self.now()?;
        let deadline = match now.checked_add(self.config.command_ttl) {
            Ok(deadline) => deadline,
            Err(_) => {
                self.reject_conversion();
                return None;
            }
        };
        Some((
            ProposalId::new(self.config.proposal_epoch, sequence),
            now,
            deadline,
        ))
    }

    fn now(&self) -> Option<MonotonicNanos> {
        let elapsed = u64::try_from(self.steady_origin.elapsed().as_nanos()).ok()?;
        self.config
            .clock
            .monotonic_origin
            .get()
            .checked_add(elapsed)
            .map(MonotonicNanos::new)
    }

    fn converted<T>(&self, result: Result<T, ConversionError>) -> Result<T, ()> {
        result.map_err(|_| self.reject_conversion())
    }

    fn reject_conversion(&self) {
        self.metrics
            .conversion_rejected
            .fetch_add(1, Ordering::Relaxed);
    }

    fn record_queue(&self, accepted: bool) {
        if accepted {
            self.metrics
                .callbacks_accepted
                .fetch_add(1, Ordering::Relaxed);
        } else {
            self.metrics.queue_rejected.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn take_sequence(next: &mut u64, metrics: &NativeRosMetricAtoms) -> Option<Sequence> {
    let sequence = Sequence::new(*next).ok();
    *next = next.checked_add(1).unwrap_or(0);
    if sequence.is_none() || *next == 0 {
        metrics.sequence_exhausted.fetch_add(1, Ordering::Relaxed);
    }
    sequence
}
