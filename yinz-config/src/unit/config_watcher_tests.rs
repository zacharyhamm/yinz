use super::*;
use std::time::{Duration, Instant};
use yinz_test_support::Watched;

const WAIT: Duration = Duration::from_secs(30);

struct Watching {
    runtime: tokio::runtime::Runtime,
    task: tokio::task::JoinHandle<()>,
    changes: Watched<Vec<Publication>>,
    main_lost: Watched<bool>,
}

struct Publication {
    config: Config,
    at: Instant,
}

impl Watching {
    fn start(main: PathBuf, config_dir: PathBuf) -> Self {
        Self::start_with_main_read(main, config_dir, |_| {})
    }

    fn start_with_main_read(
        main: PathBuf,
        config_dir: PathBuf,
        on_main_read: impl Fn(&str) + Send + 'static,
    ) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let changes = Watched::new(Vec::new());
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
                    received.lock().push(Publication {
                        config,
                        at: Instant::now(),
                    });
                    async {}
                },
                move || *registered.lock() = true,
                move || *lost.lock() = true,
                on_main_read,
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
            |changes| {
                let terminal = &changes.last()?.config.terminal;
                (terminal.font_ligatures == ligatures
                    && terminal.padding == Some(padding)
                    && terminal.opacity == opacity)
                    .then_some(())
            },
            |changes| format!("{:?}", changes.last().map(|change| &change.config.terminal)),
        );
    }

    fn clear(&self) {
        self.changes.lock().clear();
    }

    fn expect_underline(&self, offset: i32, thickness: i32) {
        self.changes.wait_until_described(
            "effective underline metrics after saved reload",
            WAIT,
            |loaded| {
                let terminal = &loaded.last()?.config.terminal;
                ((terminal.underline_offset, terminal.underline_thickness)
                    == (Some(offset), Some(thickness)))
                    .then_some(())
            },
            |changes| format!("{:?}", changes.last().map(|change| &change.config.terminal)),
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
            .last()
            .unwrap()
            .config
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
            let terminal = &published.last().unwrap().config.terminal;
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
fn underline_metrics_watcher_settles_in_place_main_edits() {
    use std::{cell::Cell, fs, io::Write};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let main = root.join("config.kdl");
    let initial = "terminal { underline_thickness 2; }\nmouse_mode false";
    let valid = "terminal { underline_thickness 3; }\nmouse_mode false";
    let invalid = "terminal { underline_thickness 65; }\nmouse_mode true";
    let corrected = "terminal { underline_thickness 4; }\nmouse_mode false";
    fs::write(&main, initial).unwrap();
    let finish = Watched::new(None::<&str>);
    let observations = Watched::new(Vec::new());
    let watching = Watching::start_with_main_read(main.clone(), root, {
        let main = main.clone();
        let finish = finish.clone();
        let observations = observations.clone();
        let completed = Cell::new(None);
        move |text| {
            if text.is_empty() {
                if let Some(replacement) = finish.lock().take() {
                    // Complete this same in-place edit after the watcher captured the
                    // truncated snapshot, before it can act on that snapshot.
                    fs::OpenOptions::new()
                        .write(true)
                        .open(&main)
                        .unwrap()
                        .write_all(replacement.as_bytes())
                        .unwrap();
                    completed.set(Some(replacement));
                }
            } else if completed.get() == Some(text) {
                observations
                    .lock()
                    .push((completed.take().unwrap(), Instant::now()));
            }
        }
    });
    let rewrite = |text| {
        *finish.lock() = Some(text);
        fs::File::create(&main).unwrap();
    };
    let expect_only = |thickness, text| {
        watching
            .changes
            .wait_until("settled adjusted underline", WAIT, |changes| {
                (changes.last()?.config.terminal.underline_thickness == Some(thickness))
                    .then_some(())
            });
        let observed_at = observations
            .lock()
            .iter()
            .find(|entry| entry.0 == text)
            .unwrap()
            .1;
        for change in watching.changes.lock().iter() {
            assert_eq!(change.config.terminal.underline_thickness, Some(thickness));
            assert_eq!(change.config.options.mouse_mode, Some(false));
            assert!(change.at.duration_since(observed_at) >= Duration::from_millis(100));
        }
    };

    rewrite(valid);
    expect_only(3, valid);
    let logs = captured_config_logs();
    rewrite(invalid);
    logs.wait_until("watcher rejected the finished invalid edit", WAIT, |logs| {
        logs.iter()
            .any(|line| {
                line.contains(main.to_str().unwrap())
                    && line.contains("underline_thickness")
                    && line.contains("between -64 and 64")
            })
            .then_some(())
    });
    {
        let changes = watching.changes.lock();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].config.terminal.underline_thickness, Some(3));
        assert_eq!(changes[0].config.options.mouse_mode, Some(false));
    }

    watching.clear();
    rewrite(corrected);
    expect_only(4, corrected);

    watching.clear();
    fs::File::create(&main).unwrap();
    watching
        .changes
        .wait_until("settled intentionally empty main", WAIT, |changes| {
            (changes
                .last()?
                .config
                .terminal
                .underline_thickness
                .is_none())
            .then_some(())
        });
}

#[cfg(unix)]
#[test]
fn underline_metrics_watcher_rejects_nonregular_layout_inputs_and_recovers() {
    use std::{
        fs,
        os::unix::{ffi::OsStrExt, fs::symlink},
    };
    use yinz_types::{data::InputMode, data::LayoutInfo, input::actions::Action};

    struct UnblockOnFailure<'a> {
        paths: [&'a Path; 3],
        pipe: &'a Path,
    }

    impl Drop for UnblockOnFailure<'_> {
        fn drop(&mut self) {
            if std::thread::panicking() {
                for path in self.paths {
                    let _ = fs::remove_file(path);
                }
                let _ = nonblocking_open_options().write(true).open(self.pipe);
            }
        }
    }

    for use_symlink in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let main = root.join("config.kdl");
        let drop_ins = root.join("config.d");
        let layouts = root.join("layouts");
        let themes = root.join("themes");
        for directory in [&drop_ins, &layouts, &themes] {
            fs::create_dir(directory).unwrap();
        }
        let default_layout = layouts.join("default.kdl");
        let switch_layout = layouts.join("switch.kdl");
        let switch_swap = switch_layout.with_extension("swap.kdl");
        let swap = layouts.join("default.swap.kdl");
        let contents = layouts.join("preview.txt");
        let pipe = root.join("pipe");
        fs::write(
            &main,
            format!(
                r#"layout_dir "{}"
theme "watcher-control"
terminal {{ underline_offset -2; }}
keybinds {{ normal {{ bind "Ctrl Alt y" {{ SwitchSession {{ name "other"; layout "switch"; }}; }}; }}; }}"#,
                layouts.display(),
            ),
        )
        .unwrap();
        fs::write(
            &default_layout,
            "layout { pane contents_file=\"preview.txt\"; }\nterminal { padding 5; }\nmouse_mode false",
        )
        .unwrap();
        fs::write(
            &switch_layout,
            r#"layout { tab name="Regular layout" { pane; }; }"#,
        )
        .unwrap();
        fs::write(
            themes.join("watcher.kdl"),
            include_str!("../../assets/themes/adwaita.kdl")
                .replace("\"adwaita\"", "\"watcher-control\""),
        )
        .unwrap();
        let name = std::ffi::CString::new(pipe.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let place_pipe = |path: &Path| {
            if use_symlink {
                symlink(&pipe, path).unwrap();
            } else {
                fs::hard_link(&pipe, path).unwrap();
            }
        };
        let save = |thickness| {
            Config::write_managed_fragment(
                &drop_ins,
                &format!("terminal {{ underline_thickness {thickness}; }}"),
            )
            .unwrap();
        };
        let logs = captured_config_logs();
        let wait_for_refusal = |path: &Path| {
            logs.wait_until_described(
                "watcher rejected the nonregular dependency",
                WAIT,
                |logs| {
                    logs.iter()
                        .any(|line| {
                            line.contains("Failed to reload config")
                                && line.contains(path.to_str().unwrap())
                                && line.contains("not a regular file")
                        })
                        .then_some(())
                },
                |logs| format!("{logs:?}"),
            );
        };
        let watching = Watching::start(main.clone(), root);
        let _unblock = UnblockOnFailure {
            paths: [&swap, &contents, &switch_swap],
            pipe: &pipe,
        };
        save(2);
        watching.expect_underline(-2, 2);

        // SwitchSession metadata comes from its primary layout. A swap there
        // is irrelevant unless that action is invoked.
        place_pipe(&switch_swap);
        place_pipe(&swap);
        save(3);
        wait_for_refusal(&swap);
        save(4);
        assert_eq!(
            Config::load_saved(Some(&main), drop_ins.parent())
                .unwrap()
                .terminal
                .underline_thickness,
            Some(4)
        );
        // Move the failure to another selected input before a retry can publish.
        // Neither layout input is watched, so after the second refusal a repair
        // must be picked up without another main or drop-in edit.
        place_pipe(&contents);
        fs::remove_file(&swap).unwrap();
        wait_for_refusal(&contents);
        assert!(watching
            .changes
            .lock()
            .iter()
            .all(|change| { change.config.terminal.underline_thickness == Some(2) }));
        fs::remove_file(&contents).unwrap();
        watching.expect_underline(-2, 4);

        for change in watching.changes.lock().iter() {
            let config = &change.config;
            assert!(matches!(config.terminal.underline_thickness, Some(2 | 4)));
            assert_eq!(config.terminal.underline_offset, Some(-2));
            assert_eq!(config.terminal.padding, Some(5.0));
            assert_eq!(config.options.mouse_mode, Some(false));
            assert_eq!(config.options.theme.as_deref(), Some("watcher-control"));
            assert!(config.themes.inner().contains_key("watcher-control"));
            let [Action::SwitchSession {
                layout: Some(LayoutInfo::File(path, metadata)),
                ..
            }] = config.keybinds.0[&InputMode::Normal][&"Ctrl Alt y".parse().unwrap()].as_slice()
            else {
                panic!("expected a SwitchSession action with on-disk layout metadata");
            };
            assert_eq!(Path::new(path), switch_layout);
            assert_eq!(metadata.tabs.len(), 1);
            assert_eq!(metadata.tabs[0].name.as_deref(), Some("Regular layout"));
        }
    }
}

#[cfg(unix)]
#[test]
fn underline_metrics_watcher_recovers_after_intermediate_layout_parse_error() {
    watcher_recovers_after_swap_parse_error(true);
}

#[cfg(unix)]
#[test]
fn underline_metrics_watcher_recovers_after_initial_layout_parse_error() {
    watcher_recovers_after_swap_parse_error(false);
}

#[cfg(unix)]
fn watcher_recovers_after_swap_parse_error(io_failure_first: bool) {
    use std::{fs, os::unix::ffi::OsStrExt};

    for restore_swap in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let main = root.join("config.kdl");
        let drop_ins = root.join("config.d");
        let layouts = root.join("layouts");
        fs::create_dir(&drop_ins).unwrap();
        fs::create_dir(&layouts).unwrap();
        fs::write(&main, "terminal { underline_offset -2; }").unwrap();
        fs::write(
            layouts.join("default.kdl"),
            "layout { pane; }\nterminal { padding 5; }\nmouse_mode false",
        )
        .unwrap();
        let swap = layouts.join("default.swap.kdl");
        let save = |thickness| {
            Config::write_managed_fragment(
                &drop_ins,
                &format!("terminal {{ underline_thickness {thickness}; }}"),
            )
            .unwrap();
        };
        let logs = captured_config_logs();
        let wait_for_diagnostic = |detail: &str| {
            logs.wait_until_described(
                detail,
                WAIT,
                |logs| {
                    logs.iter()
                        .any(|line| {
                            line.contains("Failed to reload config")
                                && line.contains(swap.to_str().unwrap())
                                && line.contains(detail)
                        })
                        .then_some(())
                },
                |logs| format!("{logs:?}"),
            );
        };
        let watching = Watching::start(main.clone(), root.clone());
        save(2);
        watching.expect_underline(-2, 2);
        watching.clear();

        if io_failure_first {
            let name = std::ffi::CString::new(swap.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            save(4);
            wait_for_diagnostic("not a regular file");
            // Keep the swap continuously present during the transition.
            let replacement = layouts.join("replacement.kdl");
            fs::write(&replacement, "layout { pane; }").unwrap();
            fs::rename(replacement, &swap).unwrap();
        } else {
            fs::write(&swap, "layout { pane; }").unwrap();
            save(4);
        }
        wait_for_diagnostic("Swap layouts should not have their own layout node");
        assert_eq!(
            Config::load_checked(Some(&main), Some(&root), false)
                .unwrap()
                .terminal
                .underline_thickness,
            Some(4)
        );
        assert!(watching.changes.lock().is_empty());

        if restore_swap {
            fs::write(
                &swap,
                "swap_tiled_layout name=\"usable\" { tab { pane; pane; }; }",
            )
            .unwrap();
        } else {
            fs::remove_file(&swap).unwrap();
        }
        watching.expect_underline(-2, 4);
        for change in watching.changes.lock().iter() {
            assert_eq!(change.config.terminal.underline_thickness, Some(4));
            assert_eq!(change.config.terminal.padding, Some(5.0));
            assert_eq!(change.config.options.mouse_mode, Some(false));
        }
    }
}

#[test]
fn underline_metrics_watcher_recovers_after_initial_layout_parse_error_at_registration() {
    use std::fs;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let main = root.join("config.kdl");
    let layouts = root.join("layouts");
    fs::create_dir(&layouts).unwrap();
    fs::write(
        &main,
        "terminal { underline_offset -3; underline_thickness 6; }",
    )
    .unwrap();
    let layout = layouts.join("default.kdl");
    fs::write(&layout, "layout {").unwrap();
    let logs = captured_config_logs();
    let watching = Watching::start(main, root);
    logs.wait_until("malformed primary layout at registration", WAIT, |logs| {
        logs.iter()
            .any(|line| {
                line.contains("Failed to reload config") && line.contains(layout.to_str().unwrap())
            })
            .then_some(())
    });
    assert!(watching.changes.lock().is_empty());

    fs::write(
        &layout,
        "layout { pane; }\nterminal { padding 5; }\nmouse_mode false",
    )
    .unwrap();
    watching.expect_underline(-3, 6);
    let changes = watching.changes.lock();
    assert_eq!(changes[0].config.terminal.padding, Some(5.0));
    assert_eq!(changes[0].config.options.mouse_mode, Some(false));
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
