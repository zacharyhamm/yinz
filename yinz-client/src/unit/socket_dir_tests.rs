use std::os::unix::fs::PermissionsExt;

#[test]
fn prepare_socket_dir_creates_the_directory_with_mode_0700() {
    let dir = tempfile::tempdir().unwrap();
    let sock_dir = dir.path().join("a").join("b");

    crate::prepare_socket_dir(&sock_dir).unwrap();

    let mode = std::fs::metadata(&sock_dir).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
}

#[test]
fn prepare_socket_dir_preserves_secure_modes_and_tightens_insecure_ones() {
    let dir = tempfile::tempdir().unwrap();
    for mode in [0o700, 0o755, 0o2700] {
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(mode)).unwrap();
        crate::prepare_socket_dir(dir.path()).unwrap();
        let permissions = std::fs::metadata(dir.path()).unwrap().permissions();
        assert_eq!(permissions.mode() & 0o7777, 0o700);
    }
}

#[test]
fn prepare_socket_dir_reports_a_parent_that_is_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("not-a-dir");
    std::fs::write(&file, b"").unwrap();
    let sock_dir = file.join("sock");

    let message = crate::prepare_socket_dir(&sock_dir).unwrap_err();

    assert!(message.contains("socket directory"), "{message}");
    assert!(
        message.contains(&sock_dir.display().to_string()),
        "{message}"
    );
}

#[test]
fn prepare_socket_dir_rejects_symlinks_and_leaves_owned_parents_alone() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    std::fs::create_dir(&target).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
    let link = dir.path().join("sockets");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let error = crate::prepare_socket_dir(&link).unwrap_err();
    assert!(error.contains("socket directory"), "{error}");
    assert_eq!(
        target.metadata().unwrap().permissions().mode() & 0o777,
        0o755
    );

    // Parents the user owns are under the user's control, so their mode is left alone.
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o777)).unwrap();
    crate::prepare_socket_dir(&target.join("nested")).unwrap();
    assert_eq!(
        target.metadata().unwrap().permissions().mode() & 0o777,
        0o777
    );
}
