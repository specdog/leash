use std::{fs, path::PathBuf};

#[test]
fn waveshare_depends_only_on_core_orchestration_and_json_framing() {
    let manifest =
        fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    for forbidden in ["tokio", "axum", "rmcp", "cudarc", "rclrs"] {
        assert!(
            !manifest.contains(forbidden),
            "Waveshare owner leaked forbidden dependency {forbidden}"
        );
    }
}
