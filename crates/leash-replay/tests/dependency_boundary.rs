use std::{fs, path::PathBuf};

#[test]
fn replay_has_no_runtime_middleware_hardware_ros_or_cuda_dependency() {
    let manifest =
        fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    for forbidden in [
        "leash-runtime",
        "leash-gateway",
        "leash-waveshare",
        "leash-ros2",
        "cudarc",
        "rclrs",
        "tokio",
    ] {
        assert!(
            !manifest.contains(forbidden),
            "replay leaked forbidden dependency {forbidden}"
        );
    }
}
