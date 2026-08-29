use std::{fs, path::PathBuf};

#[test]
fn ros_boundary_cannot_depend_on_hardware_gateway_or_cuda() {
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest = fs::read_to_string(crate_root.join("Cargo.toml")).unwrap();
    for forbidden in ["serialport", "leash-waveshare", "leash-gateway", "cudarc"] {
        assert!(
            !manifest.contains(forbidden),
            "ROS boundary leaked forbidden dependency {forbidden}"
        );
    }

    let native_manifest = fs::read_to_string(
        crate_root.join("../../implementations/waveshare-ugv/ros2-native/Cargo.toml"),
    )
    .unwrap();
    for forbidden in ["serialport", "leash-waveshare", "leash-gateway", "cudarc"] {
        assert!(
            !native_manifest.contains(forbidden),
            "native rclrs node leaked forbidden dependency {forbidden}"
        );
    }

    let native_source = fs::read_to_string(crate_root.join("src/native.rs")).unwrap();
    for forbidden in ["ActuationPort", "submit_drive", "WaveshareActuationPort"] {
        assert!(
            !native_source.contains(forbidden),
            "ROS callback gained direct actuation capability {forbidden}"
        );
    }
}
