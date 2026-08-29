//! ROS 2 message conversion and bounded callback contracts for Leash.

#![forbid(unsafe_code)]

use std::fmt;

use leash_core::{
    ActivityId, Base, BeliefId, DifferentialDrive, DurationNanos, Effect, Frame, FrameName, Map,
    Meters, MetersPerSecond, MetersPerSecondSquared, MonotonicNanos, NormalizedDrive, Odom, Pose2,
    Precision, Proposal, ProposalError, ProposalId, Radians, RadiansPerSecond, Sensor, Sequence,
    Stamped,
};
use leash_runtime::{
    bounded_lane, latest_slot, BoundedReceiver, BoundedSender, LaneSnapshot, LatestPublisher,
    LatestReader, LatestSnapshot, OverflowPolicy, PublishError, SafetyRequestError, SendError,
    SupervisorHandle, SupervisorSubmitError, TransitionTicket,
};

pub const ROS_BOUNDARY_VERSION: &str = "leash.ros2-boundary.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RosTime {
    pub sec: i32,
    pub nanosec: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockCorrelationError {
    BeforeCorrelationRange,
    Overflow,
    RosSecondsOutOfRange,
    InvalidRosTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockCorrelation {
    pub monotonic_origin: MonotonicNanos,
    pub ros_origin_ns: u64,
}

impl ClockCorrelation {
    pub fn to_ros(self, at: MonotonicNanos) -> Result<RosTime, ClockCorrelationError> {
        let ros_ns = if at >= self.monotonic_origin {
            self.ros_origin_ns
                .checked_add(at.get() - self.monotonic_origin.get())
                .ok_or(ClockCorrelationError::Overflow)?
        } else {
            self.ros_origin_ns
                .checked_sub(self.monotonic_origin.get() - at.get())
                .ok_or(ClockCorrelationError::BeforeCorrelationRange)?
        };
        let seconds = ros_ns / 1_000_000_000;
        Ok(RosTime {
            sec: i32::try_from(seconds).map_err(|_| ClockCorrelationError::RosSecondsOutOfRange)?,
            nanosec: (ros_ns % 1_000_000_000) as u32,
        })
    }

    pub fn to_monotonic(self, at: RosTime) -> Result<MonotonicNanos, ClockCorrelationError> {
        if at.sec < 0 || at.nanosec >= 1_000_000_000 {
            return Err(ClockCorrelationError::InvalidRosTime);
        }
        let ros_ns = (at.sec as u64)
            .checked_mul(1_000_000_000)
            .and_then(|seconds| seconds.checked_add(u64::from(at.nanosec)))
            .ok_or(ClockCorrelationError::Overflow)?;
        let monotonic_ns = if ros_ns >= self.ros_origin_ns {
            self.monotonic_origin
                .get()
                .checked_add(ros_ns - self.ros_origin_ns)
                .ok_or(ClockCorrelationError::Overflow)?
        } else {
            self.monotonic_origin
                .get()
                .checked_sub(self.ros_origin_ns - ros_ns)
                .ok_or(ClockCorrelationError::BeforeCorrelationRange)?
        };
        Ok(MonotonicNanos::new(monotonic_ns))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub stamp: RosTime,
    pub frame_id: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vector3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quaternion {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Twist {
    pub linear: Vector3,
    pub angular: Vector3,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LaserScanMessage {
    pub header: Header,
    pub angle_min: f32,
    pub angle_max: f32,
    pub angle_increment: f32,
    pub range_min: f32,
    pub range_max: f32,
    pub ranges: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImuMessage {
    pub header: Header,
    pub orientation: Quaternion,
    pub angular_velocity: Vector3,
    pub linear_acceleration: Vector3,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OdometryMessage {
    pub header: Header,
    pub child_frame_id: String,
    pub position: Vector3,
    pub orientation: Quaternion,
    pub twist: Twist,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TransformStampedMessage {
    pub header: Header,
    pub child_frame_id: String,
    pub translation: Vector3,
    pub rotation: Quaternion,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OccupancyGridMessage {
    pub header: Header,
    pub resolution: f32,
    pub width: u32,
    pub height: u32,
    pub origin: Vector3,
    pub origin_orientation: Quaternion,
    pub data: Vec<i8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PoseStampedMessage {
    pub header: Header,
    pub position: Vector3,
    pub orientation: Quaternion,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PathMessage {
    pub header: Header,
    pub poses: Vec<PoseStampedMessage>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScanObservation {
    pub frame: Frame<Sensor>,
    pub at: MonotonicNanos,
    pub angle_min: Radians,
    pub angle_increment: Radians,
    pub range_min: Meters,
    pub range_max: Meters,
    pub ranges: Box<[Option<Meters>]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImuObservation {
    pub frame: Frame<Sensor>,
    pub at: MonotonicNanos,
    pub orientation: Quaternion,
    pub angular_velocity: [RadiansPerSecond; 3],
    pub linear_acceleration: [MetersPerSecondSquared; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub struct OdomObservation {
    pub pose: Pose2<Odom>,
    pub child_frame: Frame<Base>,
    pub at: MonotonicNanos,
    pub linear: MetersPerSecond,
    pub angular: RadiansPerSecond,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MapObservation {
    pub frame: Frame<Map>,
    pub at: MonotonicNanos,
    pub resolution: Meters,
    pub width: u32,
    pub height: u32,
    pub origin: Pose2<Map>,
    pub cells: Box<[i8]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PathProposal {
    pub frame: Frame<Map>,
    pub at: MonotonicNanos,
    pub poses: Box<[Pose2<Map>]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanarTransform<Parent, Child> {
    pub parent: Frame<Parent>,
    pub child: Frame<Child>,
    pub at: MonotonicNanos,
    pub x: Meters,
    pub y: Meters,
    pub yaw: Radians,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NavigationGoal {
    pub activity_id: ActivityId,
    pub pose: Pose2<Map>,
    pub received_at: MonotonicNanos,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NavigationFeedback {
    pub activity_id: ActivityId,
    pub current_pose: Pose2<Map>,
    pub received_at: MonotonicNanos,
    pub distance_remaining: Meters,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nav2SourceState {
    pub connected: bool,
    pub goal_active: bool,
    pub last_localization: MonotonicNanos,
    pub last_scan: MonotonicNanos,
    pub maximum_age: DurationNanos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav2Unavailable {
    Disconnected,
    NoActiveGoal,
    FutureLocalization,
    FutureScan,
    StaleLocalization,
    StaleScan,
}

impl Nav2SourceState {
    pub fn readiness(self, now: MonotonicNanos) -> Result<(), Nav2Unavailable> {
        if !self.connected {
            return Err(Nav2Unavailable::Disconnected);
        }
        if !self.goal_active {
            return Err(Nav2Unavailable::NoActiveGoal);
        }
        let localization_age = now
            .duration_since(self.last_localization)
            .map_err(|_| Nav2Unavailable::FutureLocalization)?;
        if localization_age > self.maximum_age {
            return Err(Nav2Unavailable::StaleLocalization);
        }
        let scan_age = now
            .duration_since(self.last_scan)
            .map_err(|_| Nav2Unavailable::FutureScan)?;
        if scan_age > self.maximum_age {
            return Err(Nav2Unavailable::StaleScan);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav2StopReason {
    StaleProposal,
    SourceUnavailable(Nav2Unavailable),
    ExplicitCancellation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav2DispatchError {
    UnsupportedEffect,
    Supervisor(SupervisorSubmitError),
    Safety(SafetyRequestError),
}

pub enum Nav2DispatchAcceptance {
    Transition(TransitionTicket),
    SafetyStop {
        request_sequence: u64,
        reason: Nav2StopReason,
    },
}

#[derive(Clone)]
pub struct Nav2ProposalDispatcher {
    supervisor: SupervisorHandle,
}

impl Nav2ProposalDispatcher {
    pub const fn new(supervisor: SupervisorHandle) -> Self {
        Self { supervisor }
    }

    pub fn dispatch(
        &self,
        proposal: Proposal,
        source: Nav2SourceState,
        now: MonotonicNanos,
    ) -> Result<Nav2DispatchAcceptance, Nav2DispatchError> {
        match proposal.effect {
            Effect::ProposeStop => self.safety_stop(Nav2StopReason::ExplicitCancellation),
            Effect::ProposeDrive(command) => {
                if !proposal.is_fresh_at(now) {
                    return self.safety_stop(Nav2StopReason::StaleProposal);
                }
                if let Err(reason) = source.readiness(now) {
                    return self.safety_stop(Nav2StopReason::SourceUnavailable(reason));
                }
                self.supervisor
                    .submit(leash_core::ControlInput::Drive {
                        command,
                        deadline: proposal.deadline,
                    })
                    .map(Nav2DispatchAcceptance::Transition)
                    .map_err(Nav2DispatchError::Supervisor)
            }
            _ => Err(Nav2DispatchError::UnsupportedEffect),
        }
    }

    fn safety_stop(
        &self,
        reason: Nav2StopReason,
    ) -> Result<Nav2DispatchAcceptance, Nav2DispatchError> {
        self.supervisor
            .stop()
            .map(|request_sequence| Nav2DispatchAcceptance::SafetyStop {
                request_sequence,
                reason,
            })
            .map_err(Nav2DispatchError::Safety)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionError {
    InvalidFrame,
    InvalidTime,
    NonFinite,
    InvalidRangeBounds,
    InvalidScanLength,
    AngleExtentMismatch,
    InvalidQuaternion,
    NonPlanar,
    FrameMismatch,
    InvalidGridLength,
    InvalidGridCell,
    InvalidResolution,
    EmptyPath,
    UnsupportedTwist,
    WheelSpeedExceeded,
    InvalidKinematics,
    Proposal(ProposalError),
}

impl fmt::Display for ConversionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ROS boundary conversion failed: {self:?}")
    }
}

impl std::error::Error for ConversionError {}

impl From<ProposalError> for ConversionError {
    fn from(value: ProposalError) -> Self {
        Self::Proposal(value)
    }
}

pub fn scan_to_ros(
    scan: &ScanObservation,
    clock: ClockCorrelation,
) -> Result<LaserScanMessage, ConversionError> {
    validate_scan(
        scan.angle_min.get(),
        scan.angle_increment.get(),
        scan.range_min.get(),
        scan.range_max.get(),
        scan.ranges.len(),
    )?;
    let angle_max = scan.angle_min.get()
        + scan.angle_increment.get() * scan.ranges.len().saturating_sub(1) as f64;
    Ok(LaserScanMessage {
        header: header(&scan.frame, scan.at, clock)?,
        angle_min: finite_f32(scan.angle_min.get())?,
        angle_max: finite_f32(angle_max)?,
        angle_increment: finite_f32(scan.angle_increment.get())?,
        range_min: finite_f32(scan.range_min.get())?,
        range_max: finite_f32(scan.range_max.get())?,
        ranges: scan
            .ranges
            .iter()
            .map(|range| {
                range.map_or(Ok(f32::NAN), |range| {
                    if range.get() < scan.range_min.get() || range.get() > scan.range_max.get() {
                        return Err(ConversionError::InvalidRangeBounds);
                    }
                    finite_f32(range.get())
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
    })
}

pub fn scan_from_ros(
    scan: &LaserScanMessage,
    clock: ClockCorrelation,
) -> Result<ScanObservation, ConversionError> {
    validate_scan(
        f64::from(scan.angle_min),
        f64::from(scan.angle_increment),
        f64::from(scan.range_min),
        f64::from(scan.range_max),
        scan.ranges.len(),
    )?;
    let expected_angle_max = f64::from(scan.angle_min)
        + f64::from(scan.angle_increment) * scan.ranges.len().saturating_sub(1) as f64;
    let tolerance = f64::from(scan.angle_increment.abs()).max(1.0) * 1e-5;
    if (expected_angle_max - f64::from(scan.angle_max)).abs() > tolerance {
        return Err(ConversionError::AngleExtentMismatch);
    }
    let range_min = f64::from(scan.range_min);
    let range_max = f64::from(scan.range_max);
    let ranges = scan
        .ranges
        .iter()
        .map(|range| {
            if !range.is_finite() || f64::from(*range) < range_min || f64::from(*range) > range_max
            {
                Ok(None)
            } else {
                Meters::new(f64::from(*range))
                    .map(Some)
                    .map_err(|_| ConversionError::NonFinite)
            }
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_boxed_slice();
    Ok(ScanObservation {
        frame: frame_from_ros(&scan.header.frame_id)?,
        at: monotonic_from_header(&scan.header, clock)?,
        angle_min: Radians::new(f64::from(scan.angle_min))
            .map_err(|_| ConversionError::NonFinite)?,
        angle_increment: Radians::new(f64::from(scan.angle_increment))
            .map_err(|_| ConversionError::NonFinite)?,
        range_min: Meters::new(range_min).map_err(|_| ConversionError::NonFinite)?,
        range_max: Meters::new(range_max).map_err(|_| ConversionError::NonFinite)?,
        ranges,
    })
}

pub fn imu_to_ros(
    imu: &ImuObservation,
    clock: ClockCorrelation,
) -> Result<ImuMessage, ConversionError> {
    validate_quaternion(imu.orientation)?;
    Ok(ImuMessage {
        header: header(&imu.frame, imu.at, clock)?,
        orientation: imu.orientation,
        angular_velocity: Vector3 {
            x: imu.angular_velocity[0].get(),
            y: imu.angular_velocity[1].get(),
            z: imu.angular_velocity[2].get(),
        },
        linear_acceleration: Vector3 {
            x: imu.linear_acceleration[0].get(),
            y: imu.linear_acceleration[1].get(),
            z: imu.linear_acceleration[2].get(),
        },
    })
}

pub fn imu_from_ros(
    imu: &ImuMessage,
    clock: ClockCorrelation,
) -> Result<ImuObservation, ConversionError> {
    validate_quaternion(imu.orientation)?;
    validate_vector(imu.angular_velocity)?;
    validate_vector(imu.linear_acceleration)?;
    Ok(ImuObservation {
        frame: frame_from_ros(&imu.header.frame_id)?,
        at: monotonic_from_header(&imu.header, clock)?,
        orientation: imu.orientation,
        angular_velocity: [
            RadiansPerSecond::new(imu.angular_velocity.x)
                .map_err(|_| ConversionError::NonFinite)?,
            RadiansPerSecond::new(imu.angular_velocity.y)
                .map_err(|_| ConversionError::NonFinite)?,
            RadiansPerSecond::new(imu.angular_velocity.z)
                .map_err(|_| ConversionError::NonFinite)?,
        ],
        linear_acceleration: [
            MetersPerSecondSquared::new(imu.linear_acceleration.x)
                .map_err(|_| ConversionError::NonFinite)?,
            MetersPerSecondSquared::new(imu.linear_acceleration.y)
                .map_err(|_| ConversionError::NonFinite)?,
            MetersPerSecondSquared::new(imu.linear_acceleration.z)
                .map_err(|_| ConversionError::NonFinite)?,
        ],
    })
}

pub fn odom_to_ros(
    odom: &OdomObservation,
    clock: ClockCorrelation,
) -> Result<OdometryMessage, ConversionError> {
    Ok(OdometryMessage {
        header: header(&odom.pose.frame, odom.at, clock)?,
        child_frame_id: odom.child_frame.name().as_str().to_string(),
        position: Vector3 {
            x: odom.pose.x.get(),
            y: odom.pose.y.get(),
            z: 0.0,
        },
        orientation: yaw_quaternion(odom.pose.yaw),
        twist: Twist {
            linear: Vector3 {
                x: odom.linear.get(),
                ..Vector3::default()
            },
            angular: Vector3 {
                z: odom.angular.get(),
                ..Vector3::default()
            },
        },
    })
}

pub fn odom_from_ros(
    odom: &OdometryMessage,
    clock: ClockCorrelation,
) -> Result<OdomObservation, ConversionError> {
    require_planar_position(odom.position)?;
    require_planar_twist(odom.twist)?;
    let yaw = yaw_from_quaternion(odom.orientation)?;
    Ok(OdomObservation {
        pose: Pose2::new(
            frame_from_ros(&odom.header.frame_id)?,
            Meters::new(odom.position.x).map_err(|_| ConversionError::NonFinite)?,
            Meters::new(odom.position.y).map_err(|_| ConversionError::NonFinite)?,
            yaw,
        ),
        child_frame: frame_from_ros(&odom.child_frame_id)?,
        at: monotonic_from_header(&odom.header, clock)?,
        linear: MetersPerSecond::new(odom.twist.linear.x)
            .map_err(|_| ConversionError::NonFinite)?,
        angular: RadiansPerSecond::new(odom.twist.angular.z)
            .map_err(|_| ConversionError::NonFinite)?,
    })
}

pub fn transform_to_ros<Parent, Child>(
    transform: &PlanarTransform<Parent, Child>,
    clock: ClockCorrelation,
) -> Result<TransformStampedMessage, ConversionError> {
    Ok(TransformStampedMessage {
        header: header(&transform.parent, transform.at, clock)?,
        child_frame_id: transform.child.name().as_str().to_string(),
        translation: Vector3 {
            x: transform.x.get(),
            y: transform.y.get(),
            z: 0.0,
        },
        rotation: yaw_quaternion(transform.yaw),
    })
}

pub fn transform_from_ros<Parent, Child>(
    transform: &TransformStampedMessage,
    clock: ClockCorrelation,
) -> Result<PlanarTransform<Parent, Child>, ConversionError> {
    require_planar_position(transform.translation)?;
    Ok(PlanarTransform {
        parent: frame_from_ros(&transform.header.frame_id)?,
        child: frame_from_ros(&transform.child_frame_id)?,
        at: monotonic_from_header(&transform.header, clock)?,
        x: Meters::new(transform.translation.x).map_err(|_| ConversionError::NonFinite)?,
        y: Meters::new(transform.translation.y).map_err(|_| ConversionError::NonFinite)?,
        yaw: yaw_from_quaternion(transform.rotation)?,
    })
}

pub fn map_to_ros(
    map: &MapObservation,
    clock: ClockCorrelation,
) -> Result<OccupancyGridMessage, ConversionError> {
    if map.resolution.get() <= 0.0 {
        return Err(ConversionError::InvalidResolution);
    }
    if map.origin.frame.name() != map.frame.name() {
        return Err(ConversionError::FrameMismatch);
    }
    validate_grid(map.width, map.height, &map.cells)?;
    Ok(OccupancyGridMessage {
        header: header(&map.frame, map.at, clock)?,
        resolution: finite_f32(map.resolution.get())?,
        width: map.width,
        height: map.height,
        origin: Vector3 {
            x: map.origin.x.get(),
            y: map.origin.y.get(),
            z: 0.0,
        },
        origin_orientation: yaw_quaternion(map.origin.yaw),
        data: map.cells.to_vec(),
    })
}

pub fn map_from_ros(
    map: &OccupancyGridMessage,
    clock: ClockCorrelation,
) -> Result<MapObservation, ConversionError> {
    if !map.resolution.is_finite() || map.resolution <= 0.0 {
        return Err(ConversionError::InvalidResolution);
    }
    require_planar_position(map.origin)?;
    validate_grid(map.width, map.height, &map.data)?;
    let frame: Frame<Map> = frame_from_ros(&map.header.frame_id)?;
    Ok(MapObservation {
        origin: Pose2::new(
            frame.clone(),
            Meters::new(map.origin.x).map_err(|_| ConversionError::NonFinite)?,
            Meters::new(map.origin.y).map_err(|_| ConversionError::NonFinite)?,
            yaw_from_quaternion(map.origin_orientation)?,
        ),
        frame,
        at: monotonic_from_header(&map.header, clock)?,
        resolution: Meters::new(f64::from(map.resolution))
            .map_err(|_| ConversionError::NonFinite)?,
        width: map.width,
        height: map.height,
        cells: map.data.clone().into_boxed_slice(),
    })
}

pub fn localization_to_ros(
    pose: &Pose2<Map>,
    at: MonotonicNanos,
    clock: ClockCorrelation,
) -> Result<PoseStampedMessage, ConversionError> {
    Ok(PoseStampedMessage {
        header: header(&pose.frame, at, clock)?,
        position: Vector3 {
            x: pose.x.get(),
            y: pose.y.get(),
            z: 0.0,
        },
        orientation: yaw_quaternion(pose.yaw),
    })
}

pub fn localization_from_ros(
    pose: &PoseStampedMessage,
    clock: ClockCorrelation,
) -> Result<(Pose2<Map>, MonotonicNanos), ConversionError> {
    require_planar_position(pose.position)?;
    let at = monotonic_from_header(&pose.header, clock)?;
    Ok((
        Pose2::new(
            frame_from_ros(&pose.header.frame_id)?,
            Meters::new(pose.position.x).map_err(|_| ConversionError::NonFinite)?,
            Meters::new(pose.position.y).map_err(|_| ConversionError::NonFinite)?,
            yaw_from_quaternion(pose.orientation)?,
        ),
        at,
    ))
}

pub fn navigation_goal_to_ros(
    goal: &NavigationGoal,
    clock: ClockCorrelation,
) -> Result<PoseStampedMessage, ConversionError> {
    localization_to_ros(&goal.pose, goal.received_at, clock)
}

pub fn navigation_goal_from_ros(
    activity_id: ActivityId,
    pose: &PoseStampedMessage,
    clock: ClockCorrelation,
) -> Result<NavigationGoal, ConversionError> {
    let (pose, received_at) = localization_from_ros(pose, clock)?;
    Ok(NavigationGoal {
        activity_id,
        pose,
        received_at,
    })
}

pub fn navigation_feedback_from_ros(
    activity_id: ActivityId,
    current_pose: &PoseStampedMessage,
    distance_remaining: f64,
    clock: ClockCorrelation,
) -> Result<NavigationFeedback, ConversionError> {
    if !distance_remaining.is_finite() || distance_remaining < 0.0 {
        return Err(ConversionError::InvalidRangeBounds);
    }
    let (current_pose, received_at) = localization_from_ros(current_pose, clock)?;
    Ok(NavigationFeedback {
        activity_id,
        current_pose,
        received_at,
        distance_remaining: Meters::new(distance_remaining)
            .map_err(|_| ConversionError::NonFinite)?,
    })
}

pub fn path_to_ros(
    path: &PathProposal,
    clock: ClockCorrelation,
) -> Result<PathMessage, ConversionError> {
    if path.poses.is_empty() {
        return Err(ConversionError::EmptyPath);
    }
    if path
        .poses
        .iter()
        .any(|pose| pose.frame.name() != path.frame.name())
    {
        return Err(ConversionError::FrameMismatch);
    }
    let header = header(&path.frame, path.at, clock)?;
    Ok(PathMessage {
        header: header.clone(),
        poses: path
            .poses
            .iter()
            .map(|pose| PoseStampedMessage {
                header: header.clone(),
                position: Vector3 {
                    x: pose.x.get(),
                    y: pose.y.get(),
                    z: 0.0,
                },
                orientation: yaw_quaternion(pose.yaw),
            })
            .collect(),
    })
}

pub fn path_from_ros(
    path: &PathMessage,
    clock: ClockCorrelation,
) -> Result<PathProposal, ConversionError> {
    if path.poses.is_empty() {
        return Err(ConversionError::EmptyPath);
    }
    let frame: Frame<Map> = frame_from_ros(&path.header.frame_id)?;
    let poses = path
        .poses
        .iter()
        .map(|pose| {
            if pose.header.frame_id != path.header.frame_id {
                return Err(ConversionError::FrameMismatch);
            }
            require_planar_position(pose.position)?;
            Ok(Pose2::new(
                frame.clone(),
                Meters::new(pose.position.x).map_err(|_| ConversionError::NonFinite)?,
                Meters::new(pose.position.y).map_err(|_| ConversionError::NonFinite)?,
                yaw_from_quaternion(pose.orientation)?,
            ))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_boxed_slice();
    Ok(PathProposal {
        frame,
        at: monotonic_from_header(&path.header, clock)?,
        poses,
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Nav2Kinematics {
    pub track_width: Meters,
    pub maximum_wheel_speed: MetersPerSecond,
}

impl Nav2Kinematics {
    pub fn new(
        track_width: Meters,
        maximum_wheel_speed: MetersPerSecond,
    ) -> Result<Self, ConversionError> {
        if track_width.get() <= 0.0 || maximum_wheel_speed.get() <= 0.0 {
            return Err(ConversionError::InvalidKinematics);
        }
        Ok(Self {
            track_width,
            maximum_wheel_speed,
        })
    }
}

#[allow(clippy::too_many_arguments)]
pub fn cmd_vel_to_proposal(
    twist: Twist,
    kinematics: Nav2Kinematics,
    id: ProposalId,
    activity_id: ActivityId,
    created_at: MonotonicNanos,
    deadline: MonotonicNanos,
    priority: u8,
    belief_lineage: impl Into<Box<[BeliefId]>>,
) -> Result<Proposal, ConversionError> {
    if [
        twist.linear.x,
        twist.linear.y,
        twist.linear.z,
        twist.angular.x,
        twist.angular.y,
        twist.angular.z,
    ]
    .iter()
    .any(|value| !value.is_finite())
    {
        return Err(ConversionError::NonFinite);
    }
    if twist.linear.y != 0.0
        || twist.linear.z != 0.0
        || twist.angular.x != 0.0
        || twist.angular.y != 0.0
    {
        return Err(ConversionError::UnsupportedTwist);
    }
    let half_track = kinematics.track_width.get() / 2.0;
    let left_mps = twist.linear.x - twist.angular.z * half_track;
    let right_mps = twist.linear.x + twist.angular.z * half_track;
    let maximum = kinematics.maximum_wheel_speed.get();
    if left_mps.abs() > maximum || right_mps.abs() > maximum {
        return Err(ConversionError::WheelSpeedExceeded);
    }
    let left = NormalizedDrive::new(left_mps / maximum)
        .map_err(|_| ConversionError::WheelSpeedExceeded)?;
    let right = NormalizedDrive::new(right_mps / maximum)
        .map_err(|_| ConversionError::WheelSpeedExceeded)?;
    Ok(Proposal::new(
        id,
        activity_id,
        Effect::ProposeDrive(DifferentialDrive::new(left, right)),
        created_at,
        deadline,
        priority,
        belief_lineage,
    )?)
}

pub fn cancel_to_proposal(
    id: ProposalId,
    activity_id: ActivityId,
    at: MonotonicNanos,
    belief_lineage: impl Into<Box<[BeliefId]>>,
) -> Result<Proposal, ConversionError> {
    Ok(Proposal::new(
        id,
        activity_id,
        Effect::ProposeStop,
        at,
        at,
        u8::MAX,
        belief_lineage,
    )?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reliability {
    BestEffort,
    Reliable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Durability {
    Volatile,
    TransientLocal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QosProfile {
    pub reliability: Reliability,
    pub durability: Durability,
    pub depth: usize,
}

pub const SENSOR_QOS: QosProfile = QosProfile {
    reliability: Reliability::BestEffort,
    durability: Durability::Volatile,
    depth: 1,
};

pub const COMMAND_QOS: QosProfile = QosProfile {
    reliability: Reliability::Reliable,
    durability: Durability::Volatile,
    depth: 8,
};

pub const MAP_QOS: QosProfile = QosProfile {
    reliability: Reliability::Reliable,
    durability: Durability::TransientLocal,
    depth: 1,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RosQueueSnapshot {
    pub scans: LatestSnapshot,
    pub proposals: LaneSnapshot,
}

#[derive(Clone)]
pub struct RosCallbackHandle {
    scans: LatestPublisher<ScanObservation>,
    proposals: BoundedSender<Proposal>,
}

pub struct RosIngressQueues {
    handle: RosCallbackHandle,
    scans: LatestReader<ScanObservation>,
    proposals: BoundedReceiver<Proposal>,
}

#[derive(Debug, PartialEq)]
pub enum ScanSubmitError {
    OutOfOrder(Box<Stamped<ScanObservation>>),
}

#[derive(Debug, PartialEq)]
pub enum ProposalSubmitError {
    Full(Box<Proposal>),
    Closed(Box<Proposal>),
}

impl RosIngressQueues {
    pub fn new(proposal_capacity: usize) -> Result<Self, ConversionError> {
        let (scan_publisher, scans) = latest_slot();
        let (proposal_sender, proposals) =
            bounded_lane(proposal_capacity, OverflowPolicy::RejectNewest)
                .map_err(|_| ConversionError::InvalidScanLength)?;
        Ok(Self {
            handle: RosCallbackHandle {
                scans: scan_publisher,
                proposals: proposal_sender,
            },
            scans,
            proposals,
        })
    }

    pub fn handle(&self) -> RosCallbackHandle {
        self.handle.clone()
    }

    pub fn take_scan(&mut self) -> Option<Stamped<ScanObservation>> {
        self.scans.take()
    }

    pub fn take_proposal(&mut self) -> Option<Proposal> {
        self.proposals.try_recv()
    }

    pub fn snapshot(&self) -> RosQueueSnapshot {
        RosQueueSnapshot {
            scans: self.scans.snapshot(),
            proposals: self.proposals.snapshot(),
        }
    }
}

impl RosCallbackHandle {
    pub fn submit_scan(
        &self,
        sequence: Sequence,
        scan: ScanObservation,
    ) -> Result<(), ScanSubmitError> {
        self.scans
            .publish(Stamped::new(scan.at, sequence, scan))
            .map(|_| ())
            .map_err(|PublishError::SequenceNotIncreasing(scan)| {
                ScanSubmitError::OutOfOrder(Box::new(scan))
            })
    }

    pub fn submit_proposal(&self, proposal: Proposal) -> Result<(), ProposalSubmitError> {
        match self.proposals.try_send(proposal) {
            Ok(_) => Ok(()),
            Err(SendError::Full(proposal)) => Err(ProposalSubmitError::Full(Box::new(proposal))),
            Err(SendError::Closed(proposal)) => {
                Err(ProposalSubmitError::Closed(Box::new(proposal)))
            }
        }
    }
}

fn header<F>(
    frame: &Frame<F>,
    at: MonotonicNanos,
    clock: ClockCorrelation,
) -> Result<Header, ConversionError> {
    Ok(Header {
        stamp: clock.to_ros(at).map_err(|_| ConversionError::InvalidTime)?,
        frame_id: frame.name().as_str().to_string(),
    })
}

fn finite_f32(value: f64) -> Result<f32, ConversionError> {
    let value = value as f32;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(ConversionError::NonFinite)
    }
}

fn monotonic_from_header(
    header: &Header,
    clock: ClockCorrelation,
) -> Result<MonotonicNanos, ConversionError> {
    clock
        .to_monotonic(header.stamp)
        .map_err(|_| ConversionError::InvalidTime)
}

fn yaw_quaternion(yaw: Radians) -> Quaternion {
    let half = yaw.get() / 2.0;
    Quaternion {
        x: 0.0,
        y: 0.0,
        z: half.sin(),
        w: half.cos(),
    }
}

fn yaw_from_quaternion(value: Quaternion) -> Result<Radians, ConversionError> {
    validate_quaternion(value)?;
    if value.x.abs() > 1e-6 || value.y.abs() > 1e-6 {
        return Err(ConversionError::NonPlanar);
    }
    Radians::new(2.0 * value.z.atan2(value.w)).map_err(|_| ConversionError::NonFinite)
}

fn validate_quaternion(value: Quaternion) -> Result<(), ConversionError> {
    let norm = value.x * value.x + value.y * value.y + value.z * value.z + value.w * value.w;
    if !norm.is_finite() || (norm - 1.0).abs() > 1e-3 {
        return Err(ConversionError::InvalidQuaternion);
    }
    Ok(())
}

fn validate_vector(value: Vector3) -> Result<(), ConversionError> {
    if [value.x, value.y, value.z]
        .iter()
        .any(|component| !component.is_finite())
    {
        return Err(ConversionError::NonFinite);
    }
    Ok(())
}

fn require_planar_position(value: Vector3) -> Result<(), ConversionError> {
    validate_vector(value)?;
    if value.z.abs() > 1e-6 {
        return Err(ConversionError::NonPlanar);
    }
    Ok(())
}

fn require_planar_twist(value: Twist) -> Result<(), ConversionError> {
    validate_vector(value.linear)?;
    validate_vector(value.angular)?;
    if value.linear.y.abs() > 1e-6
        || value.linear.z.abs() > 1e-6
        || value.angular.x.abs() > 1e-6
        || value.angular.y.abs() > 1e-6
    {
        return Err(ConversionError::NonPlanar);
    }
    Ok(())
}

fn validate_scan(
    angle_min: f64,
    angle_increment: f64,
    range_min: f64,
    range_max: f64,
    sample_count: usize,
) -> Result<(), ConversionError> {
    if [angle_min, angle_increment, range_min, range_max]
        .iter()
        .any(|value| !value.is_finite())
    {
        return Err(ConversionError::NonFinite);
    }
    if sample_count == 0 || angle_increment == 0.0 {
        return Err(ConversionError::InvalidScanLength);
    }
    if range_min < 0.0 || range_max < range_min {
        return Err(ConversionError::InvalidRangeBounds);
    }
    Ok(())
}

fn validate_grid(width: u32, height: u32, cells: &[i8]) -> Result<(), ConversionError> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .ok_or(ConversionError::InvalidGridLength)?;
    if cells.len() != expected {
        return Err(ConversionError::InvalidGridLength);
    }
    if cells.iter().any(|cell| !(-1..=100).contains(cell)) {
        return Err(ConversionError::InvalidGridCell);
    }
    Ok(())
}

pub fn frame_from_ros<F>(name: &str) -> Result<Frame<F>, ConversionError> {
    FrameName::new(name)
        .map(Frame::new)
        .map_err(|_| ConversionError::InvalidFrame)
}

pub fn precision_from_covariance(variance: f64) -> Result<Precision, ConversionError> {
    if !variance.is_finite() || variance < 0.0 {
        return Err(ConversionError::NonFinite);
    }
    Precision::new(1.0 / (1.0 + variance)).map_err(|_| ConversionError::NonFinite)
}

#[cfg(test)]
mod tests {
    use leash_core::{ProducerEpoch, Sequence};

    use super::*;

    fn frame<F>(name: &str) -> Frame<F> {
        frame_from_ros(name).unwrap()
    }

    fn clock() -> ClockCorrelation {
        ClockCorrelation {
            monotonic_origin: MonotonicNanos::new(100),
            ros_origin_ns: 2_000_000_000,
        }
    }

    fn proposal_id(value: u64) -> ProposalId {
        ProposalId::new(
            ProducerEpoch::new(1).unwrap(),
            Sequence::new(value).unwrap(),
        )
    }

    fn activity_id() -> ActivityId {
        ActivityId::new(ProducerEpoch::new(2).unwrap(), Sequence::new(1).unwrap())
    }

    fn belief_id() -> BeliefId {
        BeliefId::new(ProducerEpoch::new(3).unwrap(), Sequence::new(1).unwrap())
    }

    fn assert_pose_close<Tag>(actual: &Pose2<Tag>, expected: &Pose2<Tag>) {
        assert_eq!(actual.frame.name(), expected.frame.name());
        assert!((actual.x.get() - expected.x.get()).abs() < 1e-12);
        assert!((actual.y.get() - expected.y.get()).abs() < 1e-12);
        assert!((actual.yaw.get() - expected.yaw.get()).abs() < 1e-12);
    }

    #[test]
    fn clock_correlation_and_scan_conversion_preserve_frame_and_missing_ranges() {
        let scan = ScanObservation {
            frame: frame("base_scan"),
            at: MonotonicNanos::new(150),
            angle_min: Radians::new(-1.0).unwrap(),
            angle_increment: Radians::new(0.5).unwrap(),
            range_min: Meters::new(0.05).unwrap(),
            range_max: Meters::new(12.0).unwrap(),
            ranges: Box::new([Some(Meters::new(1.0).unwrap()), None]),
        };
        let ros = scan_to_ros(&scan, clock()).unwrap();
        assert_eq!(ros.header.frame_id, "base_scan");
        assert_eq!(ros.header.stamp.sec, 2);
        assert_eq!(ros.header.stamp.nanosec, 50);
        assert_eq!(ros.ranges[0], 1.0);
        assert!(ros.ranges[1].is_nan());
        let round_trip = scan_from_ros(&ros, clock()).unwrap();
        assert_eq!(round_trip.at, scan.at);
        assert_eq!(round_trip.frame.name().as_str(), "base_scan");
        assert_eq!(round_trip.ranges[0].unwrap().get(), 1.0);
        assert_eq!(round_trip.ranges[1], None);
    }

    #[test]
    fn checked_round_trips_cover_imu_odom_transform_map_localization_and_path() {
        let imu = ImuObservation {
            frame: frame("imu_link"),
            at: MonotonicNanos::new(200),
            orientation: yaw_quaternion(Radians::new(0.25).unwrap()),
            angular_velocity: [RadiansPerSecond::new(0.0).unwrap(); 3],
            linear_acceleration: [MetersPerSecondSquared::new(0.0).unwrap(); 3],
        };
        let imu_round_trip = imu_from_ros(&imu_to_ros(&imu, clock()).unwrap(), clock()).unwrap();
        assert_eq!(imu_round_trip, imu);

        let odom = OdomObservation {
            pose: Pose2::new(
                frame("odom"),
                Meters::new(1.0).unwrap(),
                Meters::new(-2.0).unwrap(),
                Radians::new(0.5).unwrap(),
            ),
            child_frame: frame("base_link"),
            at: MonotonicNanos::new(220),
            linear: MetersPerSecond::new(0.3).unwrap(),
            angular: RadiansPerSecond::new(-0.2).unwrap(),
        };
        let odom_round_trip =
            odom_from_ros(&odom_to_ros(&odom, clock()).unwrap(), clock()).unwrap();
        assert_eq!(odom_round_trip, odom);

        let transform = PlanarTransform::<Odom, Base> {
            parent: frame("odom"),
            child: frame("base_link"),
            at: MonotonicNanos::new(230),
            x: Meters::new(1.0).unwrap(),
            y: Meters::new(2.0).unwrap(),
            yaw: Radians::new(0.3).unwrap(),
        };
        let transform_round_trip = transform_from_ros::<Odom, Base>(
            &transform_to_ros(&transform, clock()).unwrap(),
            clock(),
        )
        .unwrap();
        assert_eq!(transform_round_trip, transform);

        let map_frame = frame("map");
        let map = MapObservation {
            frame: map_frame.clone(),
            at: MonotonicNanos::new(240),
            resolution: Meters::new(0.05).unwrap(),
            width: 2,
            height: 2,
            origin: Pose2::new(
                map_frame.clone(),
                Meters::new(0.0).unwrap(),
                Meters::new(0.0).unwrap(),
                Radians::new(0.0).unwrap(),
            ),
            cells: Box::new([0, 50, 100, -1]),
        };
        let map_round_trip = map_from_ros(&map_to_ros(&map, clock()).unwrap(), clock()).unwrap();
        assert_eq!(map_round_trip.frame, map.frame);
        assert_eq!(map_round_trip.at, map.at);
        assert!((map_round_trip.resolution.get() - map.resolution.get()).abs() < 1e-8);
        assert_eq!(map_round_trip.width, map.width);
        assert_eq!(map_round_trip.height, map.height);
        assert_eq!(map_round_trip.origin, map.origin);
        assert_eq!(map_round_trip.cells, map.cells);

        let pose = Pose2::new(
            map_frame.clone(),
            Meters::new(3.0).unwrap(),
            Meters::new(4.0).unwrap(),
            Radians::new(-0.2).unwrap(),
        );
        let (pose_round_trip, pose_at) = localization_from_ros(
            &localization_to_ros(&pose, MonotonicNanos::new(250), clock()).unwrap(),
            clock(),
        )
        .unwrap();
        assert_pose_close(&pose_round_trip, &pose);
        assert_eq!(pose_at, MonotonicNanos::new(250));

        let path = PathProposal {
            frame: map_frame,
            at: MonotonicNanos::new(260),
            poses: Box::new([pose]),
        };
        let path_round_trip =
            path_from_ros(&path_to_ros(&path, clock()).unwrap(), clock()).unwrap();
        assert_eq!(path_round_trip.frame, path.frame);
        assert_eq!(path_round_trip.at, path.at);
        assert_eq!(path_round_trip.poses.len(), path.poses.len());
        assert_pose_close(&path_round_trip.poses[0], &path.poses[0]);
    }

    #[test]
    fn reverse_conversions_reject_malformed_or_non_planar_ros_messages() {
        let invalid_time = RosTime {
            sec: 1,
            nanosec: 1_000_000_000,
        };
        assert_eq!(
            clock().to_monotonic(invalid_time),
            Err(ClockCorrelationError::InvalidRosTime)
        );

        let non_planar = PoseStampedMessage {
            header: Header {
                stamp: clock().to_ros(MonotonicNanos::new(200)).unwrap(),
                frame_id: "map".to_string(),
            },
            position: Vector3 {
                z: 1.0,
                ..Vector3::default()
            },
            orientation: yaw_quaternion(Radians::new(0.0).unwrap()),
        };
        assert_eq!(
            localization_from_ros(&non_planar, clock()),
            Err(ConversionError::NonPlanar)
        );
    }

    #[test]
    fn nav2_source_readiness_is_fail_closed_and_time_checked() {
        let ready = Nav2SourceState {
            connected: true,
            goal_active: true,
            last_localization: MonotonicNanos::new(90),
            last_scan: MonotonicNanos::new(95),
            maximum_age: DurationNanos::new(20),
        };
        assert_eq!(ready.readiness(MonotonicNanos::new(100)), Ok(()));
        assert_eq!(
            Nav2SourceState {
                connected: false,
                ..ready
            }
            .readiness(MonotonicNanos::new(100)),
            Err(Nav2Unavailable::Disconnected)
        );
        assert_eq!(
            Nav2SourceState {
                last_localization: MonotonicNanos::new(70),
                ..ready
            }
            .readiness(MonotonicNanos::new(100)),
            Err(Nav2Unavailable::StaleLocalization)
        );
        assert_eq!(
            Nav2SourceState {
                last_scan: MonotonicNanos::new(101),
                ..ready
            }
            .readiness(MonotonicNanos::new(100)),
            Err(Nav2Unavailable::FutureScan)
        );
    }

    #[test]
    fn nav2_velocity_is_only_a_typed_proposal() {
        let proposal = cmd_vel_to_proposal(
            Twist {
                linear: Vector3 {
                    x: 0.5,
                    ..Vector3::default()
                },
                angular: Vector3 {
                    z: 1.0,
                    ..Vector3::default()
                },
            },
            Nav2Kinematics::new(
                Meters::new(0.4).unwrap(),
                MetersPerSecond::new(1.0).unwrap(),
            )
            .unwrap(),
            proposal_id(1),
            activity_id(),
            MonotonicNanos::new(10),
            MonotonicNanos::new(20),
            10,
            Box::new([belief_id()]) as Box<[BeliefId]>,
        )
        .unwrap();
        let Effect::ProposeDrive(drive) = proposal.effect else {
            panic!("expected drive proposal")
        };
        assert!((drive.left.get() - 0.3).abs() < 1e-9);
        assert!((drive.right.get() - 0.7).abs() < 1e-9);
    }

    #[test]
    fn unsupported_or_overspeed_nav2_twists_fail_closed() {
        let kinematics = Nav2Kinematics::new(
            Meters::new(0.4).unwrap(),
            MetersPerSecond::new(1.0).unwrap(),
        )
        .unwrap();
        let unsupported = Twist {
            linear: Vector3 {
                y: 0.1,
                ..Vector3::default()
            },
            ..Twist::default()
        };
        assert_eq!(
            cmd_vel_to_proposal(
                unsupported,
                kinematics,
                proposal_id(1),
                activity_id(),
                MonotonicNanos::new(10),
                MonotonicNanos::new(20),
                1,
                Box::new([belief_id()]) as Box<[BeliefId]>,
            ),
            Err(ConversionError::UnsupportedTwist)
        );
    }

    #[test]
    fn callback_queues_use_latest_scan_and_bounded_reject_newest_proposals() {
        let mut queues = RosIngressQueues::new(1).unwrap();
        let handle = queues.handle();
        let scan = ScanObservation {
            frame: frame("scan"),
            at: MonotonicNanos::new(1),
            angle_min: Radians::new(0.0).unwrap(),
            angle_increment: Radians::new(1.0).unwrap(),
            range_min: Meters::new(0.1).unwrap(),
            range_max: Meters::new(10.0).unwrap(),
            ranges: Box::new([Some(Meters::new(1.0).unwrap())]),
        };
        handle
            .submit_scan(Sequence::new(1).unwrap(), scan.clone())
            .unwrap();
        let mut newer = scan;
        newer.at = MonotonicNanos::new(2);
        handle
            .submit_scan(Sequence::new(2).unwrap(), newer)
            .unwrap();
        assert_eq!(queues.take_scan().unwrap().at, MonotonicNanos::new(2));

        let first = cancel_to_proposal(
            proposal_id(1),
            activity_id(),
            MonotonicNanos::new(1),
            Box::new([belief_id()]) as Box<[BeliefId]>,
        )
        .unwrap();
        let second = cancel_to_proposal(
            proposal_id(2),
            activity_id(),
            MonotonicNanos::new(2),
            Box::new([belief_id()]) as Box<[BeliefId]>,
        )
        .unwrap();
        handle.submit_proposal(first).unwrap();
        assert!(matches!(
            handle.submit_proposal(second),
            Err(ProposalSubmitError::Full(_))
        ));
    }

    #[test]
    fn qos_contracts_are_explicit_for_sensors_commands_and_maps() {
        assert_eq!(SENSOR_QOS.depth, 1);
        assert_eq!(SENSOR_QOS.reliability, Reliability::BestEffort);
        assert_eq!(COMMAND_QOS.reliability, Reliability::Reliable);
        assert_eq!(MAP_QOS.durability, Durability::TransientLocal);
    }
}
