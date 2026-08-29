use std::{fs, path::PathBuf};

#[test]
fn gateway_has_no_hardware_compute_ros_or_transport_server_dependency() {
    let manifest =
        fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    for forbidden in ["serialport", "cudarc", "rclrs", "axum", "rmcp", "tokio"] {
        assert!(
            !manifest.contains(forbidden),
            "gateway leaked forbidden dependency {forbidden}"
        );
    }
}
