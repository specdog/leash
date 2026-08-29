use leash_ros2::{
    imu_from_ros, localization_from_ros, map_from_ros, odom_from_ros, path_from_ros, scan_from_ros,
    transform_from_ros, ClockCorrelation, Header, ImuMessage, LaserScanMessage,
    OccupancyGridMessage, OdometryMessage, PathMessage, PlanarTransform, PoseStampedMessage,
    Quaternion, RosTime, TransformStampedMessage, Twist, Vector3,
};
use serde::Deserialize;
use serde_json::{json, Value};

const ROSBAG_EXPORT: &str = include_str!("../fixtures/rosbag2-export-v1.json");
const LEASH_EVENTS: &str = include_str!("../fixtures/leash-domain-events-v1.json");

#[derive(Deserialize)]
struct BagExport {
    schema_version: String,
    recording: Recording,
    clock: BagClock,
    messages: Vec<BagMessage>,
}

#[derive(Deserialize)]
struct Recording {
    storage_id: String,
    serialization_format: String,
    ros_distro: String,
}

#[derive(Deserialize)]
struct BagClock {
    monotonic_origin_ns: u64,
    ros_origin_ns: u64,
}

#[derive(Deserialize)]
#[serde(tag = "type", content = "message")]
enum BagMessage {
    #[serde(rename = "sensor_msgs/msg/LaserScan")]
    Scan(BagScan),
    #[serde(rename = "sensor_msgs/msg/Imu")]
    Imu(BagImu),
    #[serde(rename = "nav_msgs/msg/Odometry")]
    Odometry(BagOdometry),
    #[serde(rename = "geometry_msgs/msg/TransformStamped")]
    Transform(BagTransform),
    #[serde(rename = "nav_msgs/msg/OccupancyGrid")]
    Map(BagMap),
    #[serde(rename = "geometry_msgs/msg/PoseStamped")]
    Localization(BagPose),
    #[serde(rename = "nav_msgs/msg/Path")]
    Path(BagPath),
}

#[derive(Clone, Deserialize)]
struct BagHeader {
    sec: i32,
    nanosec: u32,
    frame_id: String,
}

#[derive(Deserialize)]
struct BagScan {
    header: BagHeader,
    angle_min: f32,
    angle_max: f32,
    angle_increment: f32,
    range_min: f32,
    range_max: f32,
    ranges: Vec<Option<f32>>,
}

#[derive(Deserialize)]
struct BagImu {
    header: BagHeader,
    orientation: [f64; 4],
    angular_velocity: [f64; 3],
    linear_acceleration: [f64; 3],
}

#[derive(Deserialize)]
struct BagOdometry {
    header: BagHeader,
    child_frame_id: String,
    position: [f64; 3],
    orientation: [f64; 4],
    linear_x: f64,
    angular_z: f64,
}

#[derive(Deserialize)]
struct BagTransform {
    header: BagHeader,
    child_frame_id: String,
    translation: [f64; 3],
    rotation: [f64; 4],
}

#[derive(Deserialize)]
struct BagMap {
    header: BagHeader,
    resolution: f32,
    width: u32,
    height: u32,
    origin: [f64; 3],
    origin_orientation: [f64; 4],
    data: Vec<i8>,
}

#[derive(Deserialize)]
struct BagPose {
    header: BagHeader,
    position: [f64; 3],
    orientation: [f64; 4],
}

#[derive(Deserialize)]
struct BagPath {
    header: BagHeader,
    poses: Vec<BagPose>,
}

#[test]
fn recorded_rosbag_export_and_leash_replay_produce_equivalent_domain_events() {
    let bag: BagExport = serde_json::from_str(ROSBAG_EXPORT).unwrap();
    assert_eq!(bag.schema_version, "leash.rosbag2-export.v1");
    assert_eq!(bag.recording.storage_id, "sqlite3");
    assert_eq!(bag.recording.serialization_format, "cdr");
    assert_eq!(bag.recording.ros_distro, "humble");
    let clock = ClockCorrelation {
        monotonic_origin: leash_core::MonotonicNanos::new(bag.clock.monotonic_origin_ns),
        ros_origin_ns: bag.clock.ros_origin_ns,
    };
    let actual = bag
        .messages
        .into_iter()
        .map(|message| canonical_event(message, clock))
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let expected: Value = serde_json::from_str(LEASH_EVENTS).unwrap();
    assert_eq!(expected["schema_version"], "leash.ros-domain-events.v1");
    assert_eq!(Value::Array(actual), expected["events"]);
}

fn canonical_event(
    message: BagMessage,
    clock: ClockCorrelation,
) -> Result<Value, leash_ros2::ConversionError> {
    Ok(match message {
        BagMessage::Scan(scan) => {
            let observation = scan_from_ros(
                &LaserScanMessage {
                    header: header(scan.header),
                    angle_min: scan.angle_min,
                    angle_max: scan.angle_max,
                    angle_increment: scan.angle_increment,
                    range_min: scan.range_min,
                    range_max: scan.range_max,
                    ranges: scan
                        .ranges
                        .into_iter()
                        .map(|range| range.unwrap_or(f32::NAN))
                        .collect(),
                },
                clock,
            )?;
            json!({
                "kind": "scan",
                "at_ns": observation.at.get(),
                "frame": observation.frame.name().as_str(),
                "ranges_m": observation.ranges.iter().map(|range| range.map(|range| range.get())).collect::<Vec<_>>()
            })
        }
        BagMessage::Imu(imu) => {
            let observation = imu_from_ros(
                &ImuMessage {
                    header: header(imu.header),
                    orientation: quaternion(imu.orientation),
                    angular_velocity: vector(imu.angular_velocity),
                    linear_acceleration: vector(imu.linear_acceleration),
                },
                clock,
            )?;
            json!({
                "kind": "imu",
                "at_ns": observation.at.get(),
                "frame": observation.frame.name().as_str(),
                "angular_z_rad_s": observation.angular_velocity[2].get(),
                "linear_z_m_s2": observation.linear_acceleration[2].get()
            })
        }
        BagMessage::Odometry(odometry) => {
            let observation = odom_from_ros(
                &OdometryMessage {
                    header: header(odometry.header),
                    child_frame_id: odometry.child_frame_id,
                    position: vector(odometry.position),
                    orientation: quaternion(odometry.orientation),
                    twist: Twist {
                        linear: Vector3 {
                            x: odometry.linear_x,
                            ..Vector3::default()
                        },
                        angular: Vector3 {
                            z: odometry.angular_z,
                            ..Vector3::default()
                        },
                    },
                },
                clock,
            )?;
            json!({
                "kind": "odometry",
                "at_ns": observation.at.get(),
                "frame": observation.pose.frame.name().as_str(),
                "child_frame": observation.child_frame.name().as_str(),
                "x_m": observation.pose.x.get(),
                "y_m": observation.pose.y.get(),
                "linear_m_s": observation.linear.get(),
                "angular_rad_s": observation.angular.get()
            })
        }
        BagMessage::Transform(transform) => {
            let observation: PlanarTransform<leash_core::Odom, leash_core::Base> =
                transform_from_ros(
                    &TransformStampedMessage {
                        header: header(transform.header),
                        child_frame_id: transform.child_frame_id,
                        translation: vector(transform.translation),
                        rotation: quaternion(transform.rotation),
                    },
                    clock,
                )?;
            json!({
                "kind": "transform",
                "at_ns": observation.at.get(),
                "parent_frame": observation.parent.name().as_str(),
                "child_frame": observation.child.name().as_str(),
                "x_m": observation.x.get(),
                "y_m": observation.y.get()
            })
        }
        BagMessage::Map(map) => {
            let observation = map_from_ros(
                &OccupancyGridMessage {
                    header: header(map.header),
                    resolution: map.resolution,
                    width: map.width,
                    height: map.height,
                    origin: vector(map.origin),
                    origin_orientation: quaternion(map.origin_orientation),
                    data: map.data,
                },
                clock,
            )?;
            json!({
                "kind": "map",
                "at_ns": observation.at.get(),
                "frame": observation.frame.name().as_str(),
                "resolution_m": observation.resolution.get(),
                "width": observation.width,
                "height": observation.height,
                "cells": observation.cells
            })
        }
        BagMessage::Localization(localization) => {
            let (pose, at) = localization_from_ros(&pose(localization), clock)?;
            json!({
                "kind": "localization",
                "at_ns": at.get(),
                "frame": pose.frame.name().as_str(),
                "x_m": pose.x.get(),
                "y_m": pose.y.get()
            })
        }
        BagMessage::Path(path) => {
            let proposal = path_from_ros(
                &PathMessage {
                    header: header(path.header),
                    poses: path.poses.into_iter().map(pose).collect(),
                },
                clock,
            )?;
            json!({
                "kind": "path",
                "at_ns": proposal.at.get(),
                "frame": proposal.frame.name().as_str(),
                "poses": proposal.poses.iter().map(|pose| [pose.x.get(), pose.y.get()]).collect::<Vec<_>>()
            })
        }
    })
}

fn header(header: BagHeader) -> Header {
    Header {
        stamp: RosTime {
            sec: header.sec,
            nanosec: header.nanosec,
        },
        frame_id: header.frame_id,
    }
}

fn vector(values: [f64; 3]) -> Vector3 {
    Vector3 {
        x: values[0],
        y: values[1],
        z: values[2],
    }
}

fn quaternion(values: [f64; 4]) -> Quaternion {
    Quaternion {
        x: values[0],
        y: values[1],
        z: values[2],
        w: values[3],
    }
}

fn pose(pose: BagPose) -> PoseStampedMessage {
    PoseStampedMessage {
        header: header(pose.header),
        position: vector(pose.position),
        orientation: quaternion(pose.orientation),
    }
}
