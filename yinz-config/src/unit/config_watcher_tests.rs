use super::*;
use std::time::Duration;
use yinz_test_support::Watched;

const WAIT: Duration = Duration::from_secs(30);

struct Watching {
    runtime: tokio::runtime::Runtime,
    task: tokio::task::JoinHandle<()>,
    changes: Watched<Option<Config>>,
    main_lost: Watched<bool>,
}

impl Watching {
    fn start(main: PathBuf, config_dir: PathBuf) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let changes = Watched::new(None);
        let received = changes.clone();
        let ready = Watched::new(false);
        let registered = ready.clone();
        let main_lost = Watched::new(false);
        let lost = main_lost.clone();
        let task = runtime.spawn(async move {
            watch_config_file_changes_inner(
                main,
                Some(&config_dir),
                move |config| {
                    *received.lock() = Some(config);
                    async {}
                },
                move || *registered.lock() = true,
                move || *lost.lock() = true,
            )
            .await;
        });
        // Construct the guard before waiting, so a readiness timeout also cleans up.
        let watching = Self {
            runtime,
            task,
            changes,
            main_lost,
        };
        ready.wait_until("main-file watch registration", WAIT, |ready| {
            ready.then_some(())
        });
        watching
    }

    fn expect(&self, ligatures: Option<bool>, padding: f32, opacity: Option<f32>) {
        self.changes.wait_until_described(
            &format!("reloaded drop-in content: ligatures={ligatures:?}, padding={padding}, opacity={opacity:?}"),
            WAIT,
            |config| {
                config.as_ref().and_then(|config| {
                    (config.terminal.font_ligatures == ligatures
                        && config.terminal.padding == Some(padding)
                        && config.terminal.opacity == opacity)
                        .then_some(())
                })
            },
            |config| format!("{:?}", config.as_ref().map(|config| &config.terminal)),
        );
    }

    fn clear(&self) {
        *self.changes.lock() = None;
    }

    fn expect_underline(&self, offset: i32, thickness: i32) {
        self.changes.wait_until(
            "effective underline metrics after saved reload",
            WAIT,
            |loaded| {
                let terminal = &loaded.as_ref()?.terminal;
                ((terminal.underline_offset, terminal.underline_thickness)
                    == (Some(offset), Some(thickness)))
                    .then_some(())
            },
        );
    }
}

impl Drop for Watching {
    fn drop(&mut self) {
        self.task.abort();
        // Await cancellation before dropping the runtime. Dropping PollWatcher joins its
        // worker; the runtime also joins its workers, including during assertion unwinding.
        let result = self.runtime.block_on(&mut self.task);
        if !std::thread::panicking() {
            assert!(result.is_ok() || result.is_err_and(|error| error.is_cancelled()));
        }
    }
}

#[test]
#[ignore = "Known config.d recreation race (UX-104-2), accepted for the issue 104 merge"]
fn missing_drop_in_directory_is_discovered_and_reconciled_through_recreation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    // The explicit config dir is deliberately different from the main file's parent.
    let config_dir = root.join("settings");
    std::fs::create_dir(&config_dir).unwrap();
    let main = root.join("config.kdl");
    let original = "terminal { padding 2; }\n";
    std::fs::write(&main, original).unwrap();
    let drop_ins = config_dir.join("config.d");
    assert!(!drop_ins.exists());
    assert!(!config_dir.join("themes").exists());
    let watching = Watching::start(main.clone(), config_dir);
    assert!(
        !drop_ins.exists(),
        "readiness must precede directory creation"
    );

    std::fs::create_dir(&drop_ins).unwrap();
    let first = drop_ins.join("10-ligatures.kdl");
    std::fs::write(&first, "terminal { ligatures false; opacity 0.5; }").unwrap();
    watching.expect(Some(false), 2.0, Some(0.5));
    watching.clear();
    let modified = std::fs::metadata(&first).unwrap().modified().unwrap();
    std::fs::write(&first, "terminal { font_ligatures true; opacity 0.6; }").unwrap();
    // Deterministically exercise PollWatcher's coarse-mtime case, even on a slow host.
    std::fs::File::options()
        .write(true)
        .open(&first)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    watching.expect(Some(true), 2.0, Some(0.6));

    watching.clear();
    let last = drop_ins.join("20-last.kdl");
    std::fs::write(&last, "terminal { ligatures false; padding 3; }").unwrap();
    watching.expect(Some(false), 3.0, Some(0.6));
    watching.clear();
    // Invalid fragments are skipped as a whole, including the otherwise valid padding.
    std::fs::write(&last, "terminal { font_ligatures \"false\"; padding 99; }").unwrap();
    watching.expect(Some(true), 2.0, Some(0.6));
    watching.clear();
    std::fs::write(&last, "terminal { font_ligatures false; padding 4; }").unwrap();
    watching.expect(Some(false), 4.0, Some(0.6));

    watching.clear();
    std::fs::remove_dir_all(&drop_ins).unwrap();
    watching.expect(None, 2.0, None);
    watching.clear();
    std::fs::create_dir(&drop_ins).unwrap();
    std::fs::write(&first, "terminal { font_ligatures false; opacity 0.8; }").unwrap();
    watching.expect(Some(false), 2.0, Some(0.8));
    assert_eq!(std::fs::read_to_string(main).unwrap(), original);
}

#[test]
fn immediately_populated_first_drop_in_is_loaded_after_watch_registration() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let main = root.join("config.kdl");
    std::fs::write(&main, "terminal { padding 5; }").unwrap();
    let watching = Watching::start(main, root.clone());
    let drop_ins = root.join("config.d");
    assert!(!drop_ins.exists());
    assert!(!root.join("themes").exists());
    std::fs::create_dir(&drop_ins).unwrap();
    std::fs::write(
        drop_ins.join("20-last.kdl"),
        "terminal { ligatures false; }",
    )
    .unwrap();
    std::fs::write(
        drop_ins.join("10-first.kdl"),
        "terminal { font_ligatures true; opacity 0.7; }",
    )
    .unwrap();
    watching.expect(Some(false), 5.0, Some(0.7));
}

#[test]
fn underline_metrics_managed_writes_reload_and_recover_after_invalid_main_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let main = root.join("config.kdl");
    let drop_ins = root.join("config.d");
    std::fs::create_dir(&drop_ins).unwrap();
    let original = "terminal { underline_offset -2; underline_thickness 1; }";
    std::fs::write(&main, original).unwrap();
    let logs = captured_config_logs();
    let watching = Watching::start(main.clone(), root.clone());
    Config::write_managed_fragment(&drop_ins, "terminal { underline_thickness -2; }").unwrap();
    watching.expect_underline(-2, -2);

    std::fs::write(&main, "terminal { underline_offset 65; }").unwrap();
    Config::write_managed_fragment(&drop_ins, "terminal { underline_offset 0; }").unwrap();
    logs.wait_until(
        "watcher diagnostic for invalid underline offset",
        WAIT,
        |logs| {
            logs.iter()
                .find(|line| {
                    line.contains("Failed to reload config")
                        && line.contains(main.to_str().unwrap())
                        && line.contains("underline_offset")
                        && line.contains("between -64 and 64")
                })
                .map(|_| ())
        },
    );
    assert_eq!(
        watching
            .changes
            .lock()
            .as_ref()
            .unwrap()
            .terminal
            .underline_offset,
        Some(-2)
    );
    std::fs::write(&main, original).unwrap();
    watching.expect_underline(0, -2);
}

#[test]
fn underline_metrics_managed_write_is_published_when_missing_main_returns() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let main = root.join("config.kdl");
    let drop_ins = root.join("config.d");
    std::fs::create_dir(&drop_ins).unwrap();
    let original = "terminal { underline_offset -2; underline_thickness 1; }";
    std::fs::write(&main, original).unwrap();
    let watching = Watching::start(main.clone(), root);
    Config::write_managed_fragment(&drop_ins, "terminal { underline_thickness 2; }").unwrap();
    watching.expect_underline(-2, 2);

    std::fs::remove_file(&main).unwrap();
    watching
        .main_lost
        .wait_until("watcher unregistered the missing main", WAIT, |lost| {
            lost.then_some(())
        });
    Config::write_managed_fragment(&drop_ins, "terminal { underline_thickness 3; }").unwrap();
    std::fs::write(&main, original).unwrap();
    watching.expect_underline(-2, 3);

    let logs = captured_config_logs();
    *watching.main_lost.lock() = false;
    std::fs::remove_file(&main).unwrap();
    watching.main_lost.wait_until(
        "watcher noticed the second main-file removal",
        WAIT,
        |lost| lost.then_some(()),
    );
    std::fs::write(&main, "terminal { underline_thickness 65; }").unwrap();
    logs.wait_until("watcher rejected the malformed restoration", WAIT, |logs| {
        logs.iter()
            .any(|line| {
                line.contains("Failed to reload config")
                    && line.contains(main.to_str().unwrap())
                    && line.contains("underline_thickness")
            })
            .then_some(())
    });
    Config::write_managed_fragment(&drop_ins, "terminal { underline_thickness 4; }").unwrap();
    std::fs::write(&main, original).unwrap();
    watching.expect_underline(-2, 4);
}

#[cfg(unix)]
#[test]
fn underline_metrics_managed_write_is_published_when_nonregular_main_is_restored() {
    use std::{
        fs,
        os::unix::{ffi::OsStrExt, fs::symlink},
    };

    struct UnblockOnFailure<'a> {
        main: &'a Path,
        restored: &'a Path,
        pipe_alias: &'a Path,
    }

    impl Drop for UnblockOnFailure<'_> {
        fn drop(&mut self) {
            if std::thread::panicking() {
                // Remove the FIFO from the watched path before releasing any blocked
                // open on its old inode. The successful case never opens a writer.
                let _ = fs::rename(self.restored, self.main);
                let _ = nonblocking_open_options().write(true).open(self.pipe_alias);
            }
        }
    }

    for use_symlink in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let main = root.join("config.kdl");
        let drop_ins = root.join("config.d");
        let pipe = root.join("pipe");
        let replacement = root.join("replacement");
        let restored = root.join("restored");
        let original = "terminal { underline_offset -2; underline_thickness 1; }";
        fs::create_dir(&drop_ins).unwrap();
        fs::write(&main, original).unwrap();
        // Restore the very same bytes and mtime. The refused observation must
        // still count as a transition when readable input returns.
        fs::hard_link(&main, &restored).unwrap();
        let name = std::ffi::CString::new(pipe.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        if use_symlink {
            symlink(&pipe, &replacement).unwrap();
        } else {
            fs::hard_link(&pipe, &replacement).unwrap();
        }

        let logs = captured_config_logs();
        let watching = Watching::start(main.clone(), root);
        let _unblock = UnblockOnFailure {
            main: &main,
            restored: &restored,
            pipe_alias: &pipe,
        };
        Config::write_managed_fragment(&drop_ins, "terminal { underline_thickness 2; }").unwrap();
        watching.expect_underline(-2, 2);
        fs::rename(&replacement, &main).unwrap();
        logs.wait_until("watcher rejected the nonregular main", WAIT, |logs| {
            logs.iter()
                .any(|line| {
                    line.contains("Failed to reload config")
                        && line.contains(main.to_str().unwrap())
                        && line.contains("not a regular file")
                })
                .then_some(())
        });
        let managed =
            Config::write_managed_fragment(&drop_ins, "terminal { underline_thickness 3; }")
                .unwrap();
        assert_eq!(
            Config::from_path(&managed, None)
                .unwrap()
                .terminal
                .underline_thickness,
            Some(3)
        );
        assert!(!*watching.main_lost.lock());
        {
            let published = watching.changes.lock();
            let terminal = &published.as_ref().unwrap().terminal;
            assert_eq!(
                (terminal.underline_offset, terminal.underline_thickness),
                (Some(-2), Some(2))
            );
        }
        fs::rename(&restored, &main).unwrap();
        watching.expect_underline(-2, 3);
    }
}

#[test]
fn underline_metrics_watcher_observes_same_mtime_main_edits() {
    use std::{
        fs::{self, File, FileTimes},
        time::SystemTime,
    };

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let main = root.join("config.kdl");
    let replacement = root.join("replacement");
    let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1_750_000_000);
    let set_modified = |path: &Path, modified| {
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(modified))
            .unwrap();
    };
    fs::write(
        &main,
        "terminal { underline_offset -2; underline_thickness 1; }",
    )
    .unwrap();
    set_modified(&main, modified);
    let watching = Watching::start(main.clone(), root);

    fs::write(
        &replacement,
        "terminal { underline_offset -2; underline_thickness 3; }",
    )
    .unwrap();
    set_modified(&replacement, modified);
    fs::rename(&replacement, &main).unwrap();
    assert_eq!(fs::metadata(&main).unwrap().modified().unwrap(), modified);
    watching.expect_underline(-2, 3);

    watching.clear();
    set_modified(&main, modified + Duration::from_secs(1));
    watching.expect_underline(-2, 3);
}

#[test]
fn empty_first_drop_in_directory_is_watched_before_a_later_file_write() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let main = root.join("config.kdl");
    std::fs::write(&main, "terminal { padding 6; }").unwrap();
    let watching = Watching::start(main, root.clone());
    let drop_ins = root.join("config.d");
    assert!(!drop_ins.exists());
    std::fs::create_dir(&drop_ins).unwrap();
    // No file has changed: this callback can only follow topology discovery.
    watching.expect(None, 6.0, None);
    watching.clear();
    std::fs::write(
        drop_ins.join("10-later.kdl"),
        "terminal { ligatures false; }",
    )
    .unwrap();
    watching.expect(Some(false), 6.0, None);
}
