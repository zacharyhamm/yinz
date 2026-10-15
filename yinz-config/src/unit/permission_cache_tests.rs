use super::*;
use std::io::Read;

#[test]
fn stale_permission_writers_preserve_other_grants_and_revocations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("permissions.kdl");
    let load = || PermissionCache::from_path_or_default(Some(path.clone()));
    let mut first = load();
    first.cache("first".into(), vec![PermissionType::ReadApplicationState]);
    first.write_to_file().unwrap();
    let mut stale = load();
    let mut old_reader = fs::File::open(&path).unwrap();

    first.cache("first".into(), vec![]);
    first.write_to_file().unwrap();
    stale.cache("second".into(), vec![PermissionType::ReadApplicationState]);
    stale.write_to_file().unwrap();
    first.write_to_file().unwrap();

    let saved = load();
    assert_eq!(saved.get_permissions("first".into()), Some(&vec![]));
    assert_eq!(
        saved.get_permissions("second".into()),
        Some(&vec![PermissionType::ReadApplicationState])
    );
    // Publication replaces the inode instead of exposing a truncated live cache.
    let mut original = String::new();
    old_reader.read_to_string(&mut original).unwrap();
    assert_eq!(
        PermissionCache::from_string(original).unwrap()["first"],
        vec![PermissionType::ReadApplicationState]
    );
}

#[test]
fn parallel_permission_writers_merge_and_corrupt_cache_is_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("permissions.kdl");
    let ready = std::sync::Barrier::new(8);
    std::thread::scope(|scope| {
        for index in 0..8 {
            let path = &path;
            let ready = &ready;
            scope.spawn(move || {
                let mut cache = PermissionCache::from_path_or_default(Some(path.clone()));
                cache.cache(
                    index.to_string(),
                    vec![PermissionType::ReadApplicationState],
                );
                ready.wait();
                cache.write_to_file().unwrap();
            });
        }
    });
    assert_eq!(
        PermissionCache::from_path_or_default(Some(path.clone()))
            .granted
            .len(),
        8
    );
    fs::write(&path, "broken {").unwrap();
    let mut cache = PermissionCache::from_path_or_default(Some(path.clone()));
    cache.cache("new".into(), vec![]);
    assert_eq!(
        cache.write_to_file().unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
    assert_eq!(fs::read_to_string(path).unwrap(), "broken {");
}

#[test]
fn additive_grants_merge_with_current_permissions_after_a_concurrent_revocation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("permissions.kdl");
    let load = || PermissionCache::from_path_or_default(Some(path.clone()));
    let mut first = load();
    first.cache("plugin".into(), vec![PermissionType::ReadApplicationState]);
    first.write_to_file().unwrap();
    let mut stale = load();
    first.cache("plugin".into(), vec![]);
    first.write_to_file().unwrap();
    stale.grant(
        "plugin".into(),
        vec![PermissionType::ChangeApplicationState],
    );
    stale.write_to_file().unwrap();
    assert_eq!(
        load().get_permissions("plugin".into()),
        Some(&vec![PermissionType::ChangeApplicationState])
    );

    first.grant(
        "plugin".into(),
        vec![PermissionType::OpenTerminalsOrPlugins],
    );
    first.write_to_file().unwrap();
    let saved = load();
    let permissions = saved.get_permissions("plugin".into()).unwrap();
    assert!(permissions.contains(&PermissionType::ChangeApplicationState));
    assert!(permissions.contains(&PermissionType::OpenTerminalsOrPlugins));
    assert!(!permissions.contains(&PermissionType::ReadApplicationState));
}

#[test]
fn permission_publication_preserves_symlinks_and_refuses_a_dangling_target() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.kdl");
    let link = dir.path().join("permissions.kdl");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let mut cache = PermissionCache::from_path_or_default(Some(link.clone()));
    cache.grant("plugin".into(), vec![PermissionType::ReadApplicationState]);
    assert_eq!(
        cache.write_to_file().unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    assert!(link.is_symlink());
    fs::write(&target, "").unwrap();
    cache.write_to_file().unwrap();
    assert!(link.is_symlink());
    assert_eq!(
        PermissionCache::from_path_or_default(Some(target)).get_permissions("plugin".into()),
        Some(&vec![PermissionType::ReadApplicationState])
    );
}

#[test]
fn normalized_permission_keys_preserve_legacy_symlink_grants() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("space here.scm");
    std::fs::write(&source, "(void)").unwrap();
    let link = dir.path().join("legacy.scm");
    std::os::unix::fs::symlink(&source, &link).unwrap();
    let path = dir.path().join("permissions.kdl");
    let mut cache = PermissionCache::from_path_or_default(Some(path.clone()));
    cache.cache(
        link.to_string_lossy().into_owned(),
        vec![PermissionType::ReadApplicationState],
    );
    cache.write_to_file().unwrap();
    let canonical = source
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(cache.check_permissions(
        canonical.clone(),
        &vec![PermissionType::ReadApplicationState]
    ));
    cache.grant(canonical.clone(), vec![PermissionType::RunCommands]);
    cache.write_to_file().unwrap();
    let saved = PermissionCache::from_path_or_default(Some(path));
    assert!(saved.check_permissions(
        canonical,
        &vec![
            PermissionType::ReadApplicationState,
            PermissionType::RunCommands
        ]
    ));
}

#[test]
fn legacy_permission_migration_respects_concurrent_and_pending_revocations() {
    for (revoke_canonical, same_write) in [(false, false), (true, false), (false, true)] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.scm");
        fs::write(&source, "(void)").unwrap();
        let link = dir.path().join("legacy.scm");
        std::os::unix::fs::symlink(&source, &link).unwrap();
        let legacy = link.to_string_lossy().into_owned();
        let canonical = source
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let path = dir.path().join("permissions.kdl");
        let load = || PermissionCache::from_path_or_default(Some(path.clone()));
        let mut writer = load();
        writer.cache(legacy.clone(), vec![PermissionType::RunCommands]);
        writer.write_to_file().unwrap();
        let mut stale = load();
        let revoked = if revoke_canonical {
            &canonical
        } else {
            &legacy
        };
        if !same_write {
            writer.cache(revoked.clone(), vec![]);
            writer.write_to_file().unwrap();
        }
        stale.grant(
            canonical.clone(),
            vec![PermissionType::ReadApplicationState],
        );
        if same_write {
            stale.cache(revoked.clone(), vec![]);
        }
        stale.write_to_file().unwrap();
        assert_eq!(
            load().get_permissions(canonical),
            Some(&vec![PermissionType::ReadApplicationState]),
            "revoke_canonical={revoke_canonical}, same_write={same_write}"
        );
    }
}
