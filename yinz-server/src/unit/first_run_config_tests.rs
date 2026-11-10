//! First-start detection: the server writes the comment-only bootstrap config and launches the
//! setup wizard only when that write created the file.
use super::*;
use std::fs;

fn assets(config_file: &Path) -> CliAssets {
    CliAssets {
        config_file_path: Some(config_file.to_path_buf()),
        ..Default::default()
    }
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_clean_start_writes_only_the_bootstrap_and_a_restart_does_not() {
    let dir = tempfile::tempdir().unwrap();
    let config_file = dir.path().join("config.kdl");
    let assets = assets(&config_file);

    assert!(
        write_first_run_config(&assets),
        "a first start launches the wizard"
    );
    assert_eq!(fs::read_to_string(&config_file).unwrap(), FIRST_RUN_CONFIG);
    // No defaults are materialized, and no optional folder is created eagerly.
    assert_eq!(entries(dir.path()), vec!["config.kdl"]);

    assert!(
        !write_first_run_config(&assets),
        "a restart is not a first start"
    );
    assert_eq!(fs::read_to_string(&config_file).unwrap(), FIRST_RUN_CONFIG);
    assert_eq!(entries(dir.path()), vec!["config.kdl"]);
}

#[test]
fn a_live_update_server_never_writes_the_bootstrap() {
    let dir = tempfile::tempdir().unwrap();
    let config_file = dir.path().join("config.kdl");
    let assets = CliAssets {
        update_target: Some("replacement".into()),
        ..assets(&config_file)
    };
    assert!(!write_first_run_config(&assets));
    assert!(entries(dir.path()).is_empty());
}

#[test]
fn an_existing_config_or_missing_path_is_not_a_first_start() {
    let dir = tempfile::tempdir().unwrap();
    let config_file = dir.path().join("config.kdl");
    fs::write(&config_file, "mouse_mode false\n").unwrap();
    assert!(!write_first_run_config(&assets(&config_file)));
    assert_eq!(
        fs::read_to_string(&config_file).unwrap(),
        "mouse_mode false\n"
    );
    assert_eq!(entries(dir.path()), vec!["config.kdl"]);

    assert!(!write_first_run_config(&CliAssets::default()));
}

#[cfg(unix)]
#[test]
fn a_dangling_config_symlink_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let config_file = dir.path().join("config.kdl");
    let target = dir.path().join("elsewhere.kdl");
    std::os::unix::fs::symlink(&target, &config_file).unwrap();
    assert!(!write_first_run_config(&assets(&config_file)));
    assert!(!target.exists());
    assert_eq!(fs::read_link(&config_file).unwrap(), target);
}

#[test]
fn concurrent_first_starts_write_one_bootstrap_and_launch_one_wizard() {
    let dir = tempfile::tempdir().unwrap();
    let config_file = dir.path().join("config.kdl");
    let starts = 8;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(starts));
    let threads: Vec<_> = (0..starts)
        .map(|_| {
            let barrier = barrier.clone();
            let assets = assets(&config_file);
            std::thread::spawn(move || {
                barrier.wait();
                write_first_run_config(&assets)
            })
        })
        .collect();
    let launched = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .filter(|launched| *launched)
        .count();
    assert_eq!(launched, 1);
    assert_eq!(fs::read_to_string(&config_file).unwrap(), FIRST_RUN_CONFIG);
    assert_eq!(entries(dir.path()), vec!["config.kdl"]);
}

#[cfg(unix)]
#[test]
fn an_unwritable_config_directory_is_not_a_first_start() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("yinz");
    fs::create_dir(&config_dir).unwrap();
    fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o555)).unwrap();
    // Privileged users can write despite the mode bits; honor OS access checks.
    let can_write = tempfile::NamedTempFile::new_in(&config_dir).is_ok();
    let launched = write_first_run_config(&assets(&config_dir.join("config.kdl")));
    if can_write {
        assert!(launched);
    } else {
        assert!(!launched);
        assert!(entries(&config_dir).is_empty());
    }
    fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o755)).unwrap();
}
