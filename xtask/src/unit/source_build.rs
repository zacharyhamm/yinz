use super::*;

#[test]
fn source_targets_are_explicitly_linux_64_bit() {
    for target in ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"] {
        assert!(validate_target(target).is_ok());
    }
    for target in [
        "aarch64-apple-darwin",
        "i686-unknown-linux-gnu",
        "../../outside",
        "",
    ] {
        assert!(validate_target(target).is_err());
    }
}

#[test]
fn source_build_rejects_shared_or_symlinked_target_directories() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    validate_target_directory(root.path(), &target).unwrap();
    std::fs::create_dir(&target).unwrap();
    validate_target_directory(root.path(), &target).unwrap();
    let shared = tempfile::tempdir().unwrap();
    assert!(validate_target_directory(root.path(), shared.path()).is_err());
    std::fs::remove_dir(&target).unwrap();
    std::os::unix::fs::symlink(shared.path(), &target).unwrap();
    assert!(validate_target_directory(root.path(), &target).is_err());
}

#[test]
fn source_build_compares_canonical_checkout_paths() {
    let root = tempfile::tempdir().unwrap();
    let alias = root.path().join("alias");
    let checkout = root.path().join("checkout");
    std::fs::create_dir(&checkout).unwrap();
    std::os::unix::fs::symlink(&checkout, &alias).unwrap();
    validate_target_directory(&alias, &checkout.join("target")).unwrap();
    validate_target_directory(&checkout, &alias.join("target")).unwrap();
}
