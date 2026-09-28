use super::Config;
use std::fs;
use std::io::Read;

#[test]
fn config_save_atomically_replaces_the_file_and_keeps_its_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.kdl");
    let original = "// original configuration\n";
    fs::write(&path, original).unwrap();
    let mut reader = fs::File::open(&path).unwrap();
    let config = Config::from_default_assets().unwrap();
    config
        .write_config_to_disk(config.to_string(false).unwrap(), &path)
        .unwrap();

    let mut old_contents = String::new();
    reader.read_to_string(&mut old_contents).unwrap();
    assert_eq!(
        old_contents, original,
        "an open reader must see the complete old file"
    );
    assert_eq!(
        fs::read_to_string(path.with_extension("kdl.bak")).unwrap(),
        original
    );
    assert_ne!(fs::read_to_string(&path).unwrap(), original);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
}

#[cfg(unix)]
#[test]
fn config_save_preserves_symlinks_permissions_and_existing_backup_links() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("dotfiles.kdl");
    let path = dir.path().join("config.kdl");
    let backup_target = dir.path().join("missing-backup");
    fs::write(&target, "// original\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
    symlink("dotfiles.kdl", &path).unwrap();
    symlink(&backup_target, target.with_extension("kdl.bak")).unwrap();
    let config = Config::from_default_assets().unwrap();
    config
        .write_config_to_disk(config.to_string(false).unwrap(), &path)
        .unwrap();

    assert_eq!(
        fs::read_link(&path).unwrap(),
        std::path::Path::new("dotfiles.kdl")
    );
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert!(
        !backup_target.exists(),
        "a dangling backup link must never be followed"
    );
    assert_eq!(
        fs::read_to_string(target.with_extension("kdl.bak.1")).unwrap(),
        "// original\n"
    );
}

#[cfg(unix)]
#[test]
fn config_save_respects_write_access_for_read_only_files_and_symlinks() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().canonicalize().unwrap().join("dotfiles.kdl");
    let link = dir.path().join("config.kdl");
    let original = "// original\n";
    fs::write(&target, original).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o444)).unwrap();
    symlink(&target, &link).unwrap();
    let can_write = fs::OpenOptions::new().write(true).open(&target).is_ok();
    let config = Config::from_default_assets().unwrap();

    for path in [&target, &link] {
        let result = config.write_config_to_disk(config.to_string(false).unwrap(), path);
        if can_write {
            // Privileged users can write despite the mode bits; honor OS access checks.
            result.unwrap();
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.path.as_ref(), Some(&target));
            assert_eq!(fs::read_to_string(&target).unwrap(), original);
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
        }
        assert_eq!(fs::read_link(&link).unwrap(), target);
    }
}

#[cfg(unix)]
#[test]
fn initial_config_does_not_follow_a_dangling_symlink_or_overwrite_a_file() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.kdl");
    let target = dir.path().join("missing.kdl");
    symlink(&target, &path).unwrap();
    assert!(!Config::write_config_to_disk_if_it_does_not_exist(
        "new".into(),
        &Some(path.clone())
    ));
    assert!(!target.exists());
    assert!(fs::symlink_metadata(&path)
        .unwrap()
        .file_type()
        .is_symlink());
    fs::remove_file(&path).unwrap();
    assert!(Config::write_config_to_disk_if_it_does_not_exist(
        "first".into(),
        &Some(path.clone())
    ));
    assert!(!Config::write_config_to_disk_if_it_does_not_exist(
        "second".into(),
        &Some(path.clone())
    ));
    assert_eq!(fs::read_to_string(path).unwrap(), "first");
}

#[test]
fn invalid_generated_backup_comment_cannot_replace_the_live_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config\ninvalid \".kdl");
    let original = "// original\n";
    fs::write(&path, original).unwrap();
    let config = Config::from_default_assets().unwrap();
    assert!(config
        .write_config_to_disk(config.to_string(false).unwrap(), &path)
        .is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), original);
}
