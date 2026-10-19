use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};

#[test]
fn quality_hook_writes_do_not_follow_temporary_or_backup_symlinks() {
    let directory = tempfile::tempdir().unwrap();
    let settings = directory.path().join("settings.json");
    let victim = directory.path().join("unrelated");
    let backup_victim = directory.path().join("unrelated-backup");
    std::fs::write(&settings, "original").unwrap();
    std::fs::write(&victim, "keep").unwrap();
    std::fs::write(&backup_victim, "keep backup").unwrap();
    symlink(&victim, settings.with_extension("json.yinz-tmp")).unwrap();
    symlink(&backup_victim, settings.with_extension("json.bak")).unwrap();
    write_settings(&settings, &json!({ "hooks": {} })).unwrap();
    assert_eq!(std::fs::read_to_string(victim).unwrap(), "keep");
    assert_eq!(
        std::fs::read_to_string(backup_victim).unwrap(),
        "keep backup"
    );
    assert_eq!(
        std::fs::read_to_string(settings.with_extension("json.bak")).unwrap(),
        "original"
    );
    assert_eq!(read_settings(&settings).unwrap(), json!({ "hooks": {} }));
}

#[test]
fn quality_hook_settings_preserve_private_permissions_and_dotfile_symlinks() {
    let directory = tempfile::tempdir().unwrap();
    let original = directory.path().join("managed.json");
    let settings = directory.path().join("settings.json");
    std::fs::write(&original, "{}").unwrap();
    std::fs::set_permissions(&original, std::fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&original, &settings).unwrap();
    write_settings(&settings, &json!({ "hooks": {} })).unwrap();
    assert!(settings.is_symlink());
    assert_eq!(read_settings(&original).unwrap(), json!({ "hooks": {} }));
    assert_eq!(
        std::fs::metadata(&original).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(settings.with_extension("json.bak"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}
