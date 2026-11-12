use super::*;
use std::time::Duration;
use yinz_test_support::Watched;

const WAIT: Duration = Duration::from_secs(30);

struct Watching {
    runtime: tokio::runtime::Runtime,
    task: tokio::task::JoinHandle<()>,
    changes: Watched<Option<Config>>,
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
        let task = runtime.spawn(async move {
            watch_config_file_changes_inner(
                main,
                Some(&config_dir),
                move |config| {
                    *received.lock() = Some(config);
                    async {}
                },
                move || *registered.lock() = true,
            )
            .await;
        });
        // Construct the guard before waiting, so a readiness timeout also cleans up.
        let watching = Self {
            runtime,
            task,
            changes,
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
