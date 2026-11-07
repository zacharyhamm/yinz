use super::*;

#[test]
fn linux_artifacts_match_the_host_architecture_on_linux_and_macos() {
    for (arch, expected) in [
        ("x86_64", "x86_64-unknown-linux-gnu/release/yinz"),
        ("aarch64", "aarch64-unknown-linux-gnu/release/yinz"),
    ] {
        let binary = Path::new(linux_target(arch).unwrap()).join("release/yinz");
        assert_eq!(binary, Path::new(expected));
        assert!(binary.is_relative());
    }
    assert!(linux_target("riscv64").is_err());
}

#[test]
fn phase_markers_use_the_format_the_issue_lifecycle_parses() {
    assert_eq!(phase_marker("lint", true), ">> ci phase lint: passed");
    assert_eq!(
        phase_marker("format --check", false),
        ">> ci phase format --check: failed"
    );
    let mut failed = Vec::new();
    record_phase(&mut failed, "lint", true);
    record_phase(&mut failed, "test", false);
    assert_eq!(failed, ["test"]);
}
