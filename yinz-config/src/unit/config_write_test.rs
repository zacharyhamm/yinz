use super::Config;
use std::fs;
use std::io::Read;

#[test]
fn managed_overlay_tab_plugins_replace_persisted_lists_and_root_edits() {
    let initial_list = r#"overlay_tab_plugins { "yinz:configuration" label="Agents"; "yinz:strider" label="Files"; }"#;
    for initial in [
        initial_list.to_owned(),
        format!("configuration_edits {{ set \"overlay_tab_plugins\" {{ {initial_list}; }}; }}"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let overlay_dir = dir.path().join(super::CONFIG_OVERLAY_DIR_NAME);
        Config::write_managed_fragment(
            &overlay_dir,
            "mouse_mode false\nconfiguration_edits { set \"pane_frames\" { pane_frames false; }; }",
        )
        .unwrap();

        for (fragment, expected_list) in [
            (initial.as_str(), initial_list),
            (
                r#"overlay_tab_plugins { "yinz:strider" label="Tasks"; }"#,
                r#"overlay_tab_plugins { "yinz:strider" label="Tasks"; }"#,
            ),
            (
                "scroll_buffer_size 1234",
                r#"overlay_tab_plugins { "yinz:strider" label="Tasks"; }"#,
            ),
            ("overlay_tab_plugins {}", "overlay_tab_plugins {}"),
        ] {
            let path = Config::write_managed_fragment(&overlay_dir, fragment).unwrap();
            let loaded = Config::load(None, Some(dir.path()), false).unwrap();
            assert_eq!(
                loaded.overlay_tab_plugins,
                Config::from_kdl(expected_list, None)
                    .unwrap()
                    .overlay_tab_plugins,
                "initial: {initial}, fragment: {fragment}"
            );
            assert_eq!(loaded.options.mouse_mode, Some(false));
            assert_eq!(loaded.options.pane_frames, Some(false));
            if fragment.starts_with("overlay_tab_plugins") {
                let persisted: kdl::KdlDocument =
                    fs::read_to_string(path).unwrap().parse().unwrap();
                let edits = crate::editor::patches_from_kdl(
                    persisted.get(crate::editor::EDITS_BLOCK).unwrap(),
                )
                .unwrap();
                assert_eq!(edits.len(), 1);
                assert_eq!(edits[0].path, vec!["pane_frames"]);
            }
        }
    }
}

#[test]
fn managed_fragment_guard_runs_under_lock_and_rejection_preserves_the_file() {
    for original in [None, Some("// keep this comment\nmouse_mode false\n")] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(super::MANAGED_CONFIG_FILE_NAME);
        if let Some(original) = original {
            fs::write(&path, original).unwrap();
        }
        let error =
            Config::try_write_managed_fragment_guarded(dir.path(), "mouse_mode true\n", || {
                let lock = fs::OpenOptions::new()
                    .write(true)
                    .open(dir.path().join(".yinz-managed.kdl.lock"))
                    .unwrap();
                assert!(matches!(lock.try_lock(), Err(fs::TryLockError::WouldBlock)));
                Err("cancelled".into())
            })
            .unwrap_err();
        assert_eq!(error, "cancelled");
        if let Some(original) = original {
            assert_eq!(fs::read_to_string(&path).unwrap(), original);
        } else {
            assert!(!path.exists());
        }
        // Rejection drops the staged file and releases the lock for subsequent writers.
        assert_eq!(
            fs::read_dir(dir.path()).unwrap().count(),
            1 + usize::from(original.is_some())
        );
        Config::write_managed_fragment(dir.path(), "mouse_mode true\n").unwrap();
        assert!(fs::read_to_string(path)
            .unwrap()
            .contains("mouse_mode true"));
    }
}

#[test]
fn managed_fragment_contention_returns_without_waiting_and_allows_retry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(super::MANAGED_CONFIG_FILE_NAME);
    let original = "// keep this comment\nmouse_mode false\n";
    fs::write(&path, original).unwrap();
    let lock = fs::File::create(dir.path().join(".yinz-managed.kdl.lock")).unwrap();
    lock.lock().unwrap();
    std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::channel();
        let dir = dir.path();
        let worker = scope.spawn(move || {
            tx.send(Config::try_write_managed_fragment_guarded(
                dir,
                "mouse_mode true\n",
                || Ok(()),
            ))
            .unwrap();
        });
        let result = rx.recv_timeout(std::time::Duration::from_secs(5));
        // Release before asserting so a blocking regression cannot hang the scoped worker.
        drop(lock);
        worker.join().unwrap();
        assert_eq!(
            result.unwrap().unwrap_err(),
            "Configuration is busy; try saving again"
        );
    });
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
    Config::try_write_managed_fragment_guarded(dir.path(), "mouse_mode true\n", || Ok(())).unwrap();
    assert!(fs::read_to_string(path)
        .unwrap()
        .contains("mouse_mode true"));
}

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
