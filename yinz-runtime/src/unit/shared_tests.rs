use super::*;

#[cfg(unix)]
fn private_tempdir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;

    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}

#[test]
fn terminal_titles_cannot_terminate_the_osc_or_inject_terminal_commands() {
    let title = make_terminal_title("日本語\x07\x1b]52;c;payload\x1b\\\r\n\u{9c}\u{2028}", None);
    let payload = title
        .strip_prefix("\x1b]0;")
        .unwrap()
        .strip_suffix('\x07')
        .unwrap();
    assert!(payload.contains("日本語"));
    assert!(!payload.chars().any(char::is_control));
    assert!(!payload.contains('\u{2028}'));
    assert!(make_terminal_title("plain title", None).ends_with("plain title\x07"));
}

#[test]
fn terminal_titles_lead_with_the_hostname() {
    let osc = |title: &str| format!("\x1b]0;{title}\x07");
    assert_eq!(
        format_terminal_title("box", Some("main"), "vim"),
        osc("box:main | vim")
    );
    assert_eq!(
        format_terminal_title("box", Some("main"), ""),
        osc("box:main")
    );
    assert_eq!(format_terminal_title("box", None, "vim"), osc("box | vim"));
    assert_eq!(format_terminal_title("box", None, ""), osc("box"));
    assert_eq!(
        format_terminal_title("", Some("main"), "vim"),
        osc("main | vim")
    );
}

#[cfg(unix)]
#[test]
fn secure_dirs_are_created_with_missing_parents_and_owner_only_access() {
    use std::os::unix::fs::PermissionsExt;

    let temp = private_tempdir();
    let dir = temp.path().join("missing").join("nested").join("sockets");
    ensure_secure_dir(&dir).unwrap();
    assert!(dir.is_dir());
    for path in [
        temp.path().join("missing"),
        temp.path().join("missing/nested"),
    ] {
        assert_eq!(
            path.metadata().unwrap().permissions().mode() & 0o7777,
            0o700
        );
    }
    assert_eq!(dir.metadata().unwrap().permissions().mode() & 0o7777, 0o700);

    // A second call tightens a directory that already exists with looser access.
    set_permissions(&dir, 0o755).unwrap();
    ensure_secure_dir(&dir).unwrap();
    assert_eq!(dir.metadata().unwrap().permissions().mode() & 0o7777, 0o700);
}

#[cfg(unix)]
#[test]
fn secure_dirs_traverse_execute_only_ancestors() {
    use std::os::unix::fs::PermissionsExt;

    let temp = private_tempdir();
    let ancestor = temp.path().join("searchable");
    let child = ancestor.join("private");
    std::fs::create_dir_all(&child).unwrap();
    set_permissions(&child, 0o755).unwrap();
    set_permissions(&ancestor, 0o100).unwrap();
    let result = ensure_secure_dir(&child);
    let mode = child
        .metadata()
        .map(|metadata| metadata.permissions().mode() & 0o7777);
    // Restore read access before assertions so a failure still permits cleanup.
    set_permissions(&ancestor, 0o700).unwrap();
    result.unwrap();
    assert_eq!(mode.unwrap(), 0o700);
}

#[cfg(unix)]
#[test]
fn secure_dirs_refuse_symlinks_at_the_final_name_without_changing_the_target() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let temp = private_tempdir();
    let target = temp.path().join("target");
    std::fs::create_dir(&target).unwrap();
    set_permissions(&target, 0o755).unwrap();
    std::fs::write(target.join("keep"), "unchanged").unwrap();
    let link = temp.path().join("link");
    symlink(&target, &link).unwrap();
    assert!(ensure_secure_dir(&link).is_err());
    assert!(crate::logging::atomic_create_dir(&link).is_err());
    assert_eq!(
        target.metadata().unwrap().permissions().mode() & 0o7777,
        0o755
    );
    assert_eq!(
        std::fs::read_to_string(target.join("keep")).unwrap(),
        "unchanged"
    );
}

#[cfg(unix)]
#[test]
fn secure_dirs_reject_shared_writable_paths_before_creating_or_chmodding() {
    use std::os::unix::fs::PermissionsExt;

    let temp = private_tempdir();
    let shared = temp.path().join("shared");
    std::fs::create_dir(&shared).unwrap();
    let leaf = shared.join("private");
    std::fs::create_dir(&leaf).unwrap();
    set_permissions(&shared, 0o777).unwrap();
    set_permissions(&leaf, 0o755).unwrap();
    assert!(ensure_secure_dir(&shared).is_err());
    assert!(ensure_secure_dir(&leaf).is_err());
    assert!(ensure_secure_dir(&shared.join("missing/child")).is_err());
    assert!(!shared.join("missing").exists());
    assert_eq!(
        shared.metadata().unwrap().permissions().mode() & 0o7777,
        0o777
    );
    assert_eq!(
        leaf.metadata().unwrap().permissions().mode() & 0o7777,
        0o755
    );
}

#[cfg(unix)]
#[test]
fn secure_dirs_follow_trusted_ancestor_aliases_and_check_their_targets() {
    use std::os::unix::fs::symlink;

    let temp = private_tempdir();
    let target = temp.path().join("target");
    std::fs::create_dir(&target).unwrap();
    set_permissions(&target, 0o700).unwrap();
    let absolute = temp.path().join("absolute");
    let relative = temp.path().join("relative");
    symlink(&target, &absolute).unwrap();
    symlink("absolute/../target", &relative).unwrap();
    ensure_secure_dir(&relative.join("missing/private")).unwrap();
    assert!(target.join("missing/private").is_dir());

    set_permissions(&target, 0o777).unwrap();
    assert!(ensure_secure_dir(&relative.join("other")).is_err());
    assert!(!target.join("other").exists());

    let cycle = temp.path().join("cycle");
    symlink("cycle", &cycle).unwrap();
    assert!(ensure_secure_dir(&cycle.join("private")).is_err());
}

#[cfg(unix)]
#[test]
fn secure_dirs_reject_files_and_allow_concurrent_private_creation() {
    let temp = private_tempdir();
    let file = temp.path().join("file");
    std::fs::write(&file, "unchanged").unwrap();
    assert!(ensure_secure_dir(&file).is_err());
    assert!(ensure_secure_dir(&file.join("child")).is_err());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "unchanged");

    let path = temp.path().join("missing/nested/private");
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| ensure_secure_dir(&path).unwrap());
        }
    });
    ensure_secure_dir(&path).unwrap();
}
