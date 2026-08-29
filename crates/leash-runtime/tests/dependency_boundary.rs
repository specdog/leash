#[test]
fn runtime_depends_only_on_the_domain_core() {
    let manifest = include_str!("../Cargo.toml");
    let dependencies = manifest
        .split_once("[dependencies]")
        .expect("runtime manifest must declare dependencies")
        .1;

    assert!(dependencies.contains("leash-core"));
    for forbidden in [
        "tokio",
        "serde",
        "serde_json",
        "axum",
        "rmcp",
        "rclrs",
        "cudarc",
        "serialport",
    ] {
        assert!(
            !dependencies.contains(forbidden),
            "leash-runtime cannot depend directly on {forbidden}"
        );
    }
}
