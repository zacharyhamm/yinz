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

mod first_run {
    use super::*;
    use crate::config::{FIRST_RUN_CONFIG, MANAGED_CONFIG_FILE_NAME};

    #[test]
    fn first_run_config_is_comment_only_and_loads_as_the_defaults() {
        let document: kdl::KdlDocument = FIRST_RUN_CONFIG.parse().unwrap();
        assert!(document.nodes().is_empty(), "{FIRST_RUN_CONFIG}");
        let defaults = Config::from_default_assets().unwrap();
        assert_eq!(
            Config::from_kdl(FIRST_RUN_CONFIG, Some(defaults.clone())).unwrap(),
            defaults
        );
    }

    /// Startup reads the default path; the watcher reloads through the explicit path. Both
    /// see the defaults with only the saved fragment on top.
    #[test]
    fn first_run_config_and_a_managed_fragment_load_as_defaults_plus_the_fragment() {
        let dir = tempfile::tempdir().unwrap();
        let config_file = dir.path().join("config.kdl");
        assert!(Config::write_config_to_disk_if_it_does_not_exist(
            FIRST_RUN_CONFIG.to_owned(),
            &Some(config_file.clone())
        ));
        let plain = Config::load(None, Some(dir.path()), false).unwrap();
        assert_eq!(
            plain,
            Config::from_default_assets()
                .unwrap()
                .resolve_plugin_repositories(None, Some(dir.path()))
                .unwrap()
        );
        let fragment =
            "pane_frames false\nkeybinds { normal { bind \"Ctrl Alt y\" { Quit; }; }; }\n";
        Config::write_managed_fragment(&dir.path().join("config.d"), fragment).unwrap();
        let expected = Config::from_kdl(fragment, Some(plain.clone())).unwrap();
        assert_ne!(expected, plain);
        assert_eq!(
            Config::load(None, Some(dir.path()), false).unwrap(),
            expected
        );
        assert_eq!(
            Config::load(Some(&config_file), Some(dir.path()), false).unwrap(),
            expected
        );
        assert_eq!(fs::read_to_string(&config_file).unwrap(), FIRST_RUN_CONFIG);
        assert!(dir
            .path()
            .join("config.d")
            .join(MANAGED_CONFIG_FILE_NAME)
            .exists());
    }

    /// What the bootstrap text promises: `config.d` is applied after `config.kdl` and wins, a
    /// wizard keymap replaces all of its keybinds, and removing the managed entry lets its own
    /// setting apply again.
    #[test]
    fn managed_settings_override_the_bootstrap_config_as_its_text_says() {
        use crate::editor::{keybinding_edits, KeybindingChange};
        use std::str::FromStr;
        use yinz_types::{data::InputMode, keys::KeyWithModifier};

        let ctrl_y = KeyWithModifier::from_str("Ctrl y").unwrap();
        let user_config = format!(
            "{FIRST_RUN_CONFIG}keybinds {{ normal {{ bind \"Ctrl y\" {{ SwitchToMode \"locked\"; }}; }}; }}\n"
        );
        let removal = keybinding_edits(&[KeybindingChange {
            mode: InputMode::Normal,
            key: ctrl_y.clone(),
            actions: None,
        }])
        .unwrap();
        let load_with_managed = |managed: &str| {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join("config.kdl"), &user_config).unwrap();
            Config::write_managed_fragment(&dir.path().join("config.d"), managed).unwrap();
            Config::load(None, Some(dir.path()), false).unwrap()
        };
        let binds_ctrl_y = |config: &Config| {
            config
                .keybinds
                .0
                .get(&InputMode::Normal)
                .is_some_and(|keys| keys.contains_key(&ctrl_y))
        };

        let wizard = load_with_managed(
            "keybinds clear-defaults=true { normal { bind \"Ctrl g\" { SwitchToMode \"locked\"; }; }; }",
        );
        assert!(
            !binds_ctrl_y(&wizard),
            "a wizard keymap replaces config.kdl's keybinds"
        );
        assert!(
            !binds_ctrl_y(&load_with_managed(&removal)),
            "a managed removal wins"
        );
        assert!(
            binds_ctrl_y(&load_with_managed("pane_frames false")),
            "without a managed entry for it, config.kdl's binding applies"
        );
    }

    #[cfg(unix)]
    #[test]
    fn first_run_config_never_replaces_or_writes_through_an_existing_path() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.kdl");

        let missing = dir.path().join("missing.kdl");
        symlink(&missing, &path).unwrap();
        assert!(!Config::write_config_to_disk_if_it_does_not_exist(
            FIRST_RUN_CONFIG.to_owned(),
            &Some(path.clone())
        ));
        assert!(!missing.exists());
        assert_eq!(fs::read_link(&path).unwrap(), missing);
        fs::remove_file(&path).unwrap();

        let user = dir.path().join("user.kdl");
        fs::write(&user, "mouse_mode false\n").unwrap();
        symlink(&user, &path).unwrap();
        assert!(!Config::write_config_to_disk_if_it_does_not_exist(
            FIRST_RUN_CONFIG.to_owned(),
            &Some(path.clone())
        ));
        assert_eq!(fs::read_link(&path).unwrap(), user);
        assert_eq!(fs::read_to_string(&user).unwrap(), "mouse_mode false\n");
        fs::remove_file(&path).unwrap();

        fs::write(&path, "pane_frames false\n").unwrap();
        assert!(!Config::write_config_to_disk_if_it_does_not_exist(
            FIRST_RUN_CONFIG.to_owned(),
            &Some(path.clone())
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "pane_frames false\n");
        // Only `config.kdl` and `user.kdl`: nothing staged was left behind.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn first_run_config_in_an_unwritable_directory_writes_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("yinz");
        fs::create_dir(&config_dir).unwrap();
        fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o555)).unwrap();
        // Privileged users can write despite the mode bits; honor OS access checks.
        let can_write = tempfile::NamedTempFile::new_in(&config_dir).is_ok();
        let path = config_dir.join("config.kdl");
        let written = Config::write_config_to_disk_if_it_does_not_exist(
            FIRST_RUN_CONFIG.to_owned(),
            &Some(path.clone()),
        );
        if can_write {
            assert!(written);
        } else {
            assert!(!written);
            assert_eq!(fs::read_dir(&config_dir).unwrap().count(), 0);
        }
        fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o755)).unwrap();
    }
}
