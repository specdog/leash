use std::{fs, path::PathBuf};

#[test]
fn ros_boundary_cannot_depend_on_hardware_gateway_or_cuda() {
    let manifest =
        fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    for forbidden in ["serialport", "leash-waveshare", "leash-gateway", "cudarc"] {
        assert!(
            !manifest.contains(forbidden),
            "ROS boundary leaked forbidden dependency {forbidden}"
        );
    }
}
