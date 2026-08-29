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

An `rclrs` executor package can map generated ROS messages to these DTOs when
built in a sourced ROS 2 environment. Callbacks enqueue observations or
proposals only. Velocity proposals still pass through the CPU safety supervisor
and the single Waveshare owner before they can become physical output.

The `rclrs` executor is deliberately not claimed by this crate yet. It must be
built and tested in a sourced ROS 2 environment with the rosbag equivalence
fixture before RV2-14 is complete.
