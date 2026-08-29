# leash-ros2

This crate is the stable, ROS-installation-free half of the native Rust ROS 2
boundary. It owns message-shaped DTOs, checked conversions, clock correlation,
QoS declarations, bounded callback queues, and Nav2 proposal conversion. It has
no hardware dependency and cannot write a motor.

The checked conversions cover laser scans, IMU samples, planar odometry and
transforms, occupancy grids, localization poses, and paths in both directions.
Nav2 goals, feedback, cancellation, and differential-drive velocity proposals
remain owned domain values. Sensor callbacks publish into a latest-value slot;
command callbacks use a bounded reject-newest queue.

`Nav2ProposalDispatcher` is the only bridge from a Nav2 velocity proposal to
control. It rejects stale proposals, checks ROS connection, active-goal,
localization, and lidar freshness, requests a priority stop on loss, and sends
valid drive proposals to `leash-runtime`'s CPU safety supervisor. It has no
reference to a serial implementation.

The feature-gated `native-rclrs` module and the
`implementations/waveshare-ugv/ros2-native` package map generated Humble
messages to these DTOs in a single owned executor. Callbacks enqueue
observations or proposals only. Velocity proposals still pass through the CPU
safety supervisor and the single Waveshare owner before they can become
physical output.

GitHub Actions run `33269287419` built the native package in a sourced ROS 2
Humble environment and exercised scan, IMU, odometry, transforms, maps,
localization, paths, and velocity proposals through generated messages. All
eight callbacks were accepted with the declared QoS depths, no queue or
conversion rejection, no executor error, and `hardware_access=false`. The
portable rosbag-export/Leash replay equivalence test runs in the normal
workspace suite. The durable result and artifact digest are recorded in
`evidence/github-humble-native-20260829.json`.
