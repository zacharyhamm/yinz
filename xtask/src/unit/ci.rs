use super::*;

#[test]
fn linux_artifacts_match_the_host_architecture_on_linux_and_macos() {
    for (arch, expected) in [
        ("x86_64", "x86_64-unknown-linux-gnu/release/zellij"),
        ("aarch64", "aarch64-unknown-linux-gnu/release/zellij"),
    ] {
        let binary = Path::new(linux_target(arch).unwrap()).join("release/zellij");
        assert_eq!(binary, Path::new(expected));
        assert!(binary.is_relative());
    }
    assert!(linux_target("riscv64").is_err());
}
