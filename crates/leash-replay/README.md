# leash-replay

`leash-replay` turns a checked, versioned JSON scenario into owned
`leash-core` inputs and executes the deterministic control kernel without I/O,
threads, sleeps, wall-clock reads, hardware, ROS, or CUDA.

Every run reports the final state digest, the ordered effect digest, and the
effect digest list for each event. `verify` compares those results with the
oracle stored in the scenario. This is the common evidence format for CPU
architecture checks, ROS/rosbag conversion, CUDA shadow comparison, and v1/v2
semantic diffs.

The baseline safety scenario is
[`fixtures/control-safety-v1.json`](fixtures/control-safety-v1.json).

The checked activity/belief replay is
[`fixtures/activity-belief-v1.json`](fixtures/activity-belief-v1.json). It
covers start, suspend/resume, cancel, succeed, stale-belief failure, belief
expiry, and deterministic competition between proposals.
