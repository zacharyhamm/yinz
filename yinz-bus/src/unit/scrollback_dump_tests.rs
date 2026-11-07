use super::*;

#[test]
fn dump_paths_are_unique_files_in_the_private_folder() {
    let first = new_scrollback_dump_path();
    let second = new_scrollback_dump_path();
    assert_ne!(first, second);
    for path in [&first, &second] {
        assert!(path.starts_with(&*YINZ_TMP_DIR), "{}", path.display());
        assert_eq!(path.parent(), Some(scrollback_dump_dir().as_path()));
        assert!(is_scrollback_dump(path), "{}", path.display());
    }
}

#[test]
fn only_dump_paths_count_as_dumps() {
    assert!(!is_scrollback_dump(Path::new("/tmp/notes.dump")));
    assert!(!is_scrollback_dump(
        &scrollback_dump_dir().join("notes.txt")
    ));
    assert!(!is_scrollback_dump(
        &scrollback_dump_dir().join("nested").join("x.dump")
    ));
}

#[cfg(unix)]
#[test]
fn private_files_are_owner_only_in_an_owner_only_folder() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().expect("create test folder");
    let folder = root.path().join("dumps");
    let path = folder.join("one.dump");
    write_private_file(&path, b"scrollback").expect("write private file");

    let mode = |path: &Path| {
        std::fs::metadata(path)
            .expect("read metadata")
            .permissions()
            .mode()
            & 0o777
    };
    assert_eq!(mode(&folder), 0o700);
    assert_eq!(mode(&path), 0o600);
    assert_eq!(std::fs::read(&path).expect("read dump"), b"scrollback");
}

#[test]
fn private_files_never_replace_an_existing_file() {
    let root = tempfile::tempdir().expect("create test folder");
    let path = root.path().join("taken.dump");
    std::fs::write(&path, b"original").expect("write existing file");

    write_private_file(&path, b"scrollback").expect_err("an existing file is not overwritten");
    assert_eq!(std::fs::read(&path).expect("read file"), b"original");
}

#[cfg(unix)]
#[test]
fn private_files_never_follow_a_planted_symlink() {
    let root = tempfile::tempdir().expect("create test folder");
    let target = root.path().join("target");
    std::fs::write(&target, b"original").expect("write symlink target");
    let path = root.path().join("planted.dump");
    std::os::unix::fs::symlink(&target, &path).expect("plant symlink");

    write_private_file(&path, b"scrollback").expect_err("a symlink is not followed");
    assert_eq!(std::fs::read(&target).expect("read target"), b"original");
}

#[test]
fn removing_a_dump_deletes_it_and_tolerates_a_missing_one() {
    let path = new_scrollback_dump_path();
    write_private_file(&path, b"scrollback").expect("write dump");
    remove_scrollback_dump(&path).expect("remove dump");
    assert!(!path.exists());
    remove_scrollback_dump(&path).expect("a missing dump is not an error");
}

#[test]
fn removing_leaves_files_outside_the_dump_folder_alone() {
    let root = tempfile::tempdir().expect("create test folder");
    let path = root.path().join("keep.dump");
    std::fs::write(&path, b"user file").expect("write file");
    remove_scrollback_dump(&path).expect("refusal is not an error");
    assert!(path.exists());
}
