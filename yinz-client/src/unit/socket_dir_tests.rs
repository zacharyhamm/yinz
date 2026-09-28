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
