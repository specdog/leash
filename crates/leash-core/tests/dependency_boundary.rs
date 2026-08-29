#[test]
fn core_has_no_runtime_middleware_hardware_or_wire_dependencies() {
    let manifest = include_str!("../Cargo.toml");
    let dependencies = manifest
        .split_once("[dependencies]")
        .expect("core manifest must declare dependencies")
        .1
        .split("\n[")
        .next()
        .unwrap()
        .lines()
        .filter(|line| {
            let line = line.trim();
            !line.is_empty() && !line.starts_with('#')
        })
        .collect::<Vec<_>>();

    assert!(
        dependencies.is_empty(),
        "leash-core dependencies require an architecture review: {dependencies:?}"
    );
}
