#![cfg(unix)]

use yinz_cli::args::CliAction;
use yinz_integration_tests::{keys, LayoutInfo, TestRunner, PROMPT, TERMINAL_SIZE};

const DO: &[u8] = b"\x1b[112;7u"; // Ctrl+Alt+p, encoded with CSI u.

#[test]
fn command_palette_refreshes_shortcuts_after_reconfigure_without_input() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(
            r#"
            load_plugins clear-defaults=true {}
            pane_animations false
            default_mode "locked"
            keybinds clear-defaults=true {
                locked {
                    bind "Ctrl Alt p" { ToggleCommandPalette; }
                    bind "Ctrl g" { SwitchToMode "Normal"; }
                }
                normal {
                    bind "Ctrl p" { SwitchToMode "Pane"; }
                    bind "Ctrl b" { SwitchToMode "Tmux"; }
                }
                pane { bind "f" { ToggleFocusFullscreen; }; }
                tmux { bind "z" { ToggleFocusFullscreen; }; }
            }
            "#,
        )
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(PROMPT);
    yinz.wait_until("shell ready", |snapshot| snapshot.contains("$"));
    yinz.send_stdin(DO);
    yinz.wait_until("palette opened", |snapshot| {
        snapshot.contains("Switch session")
    });
    yinz.send_stdin(b"Fullscreen");
    yinz.wait_until("regular route wins over Tmux route", |snapshot| {
        snapshot.contains("> Fullscreen")
            && snapshot.contains("Ctrl p → f")
            && !snapshot.contains("Ctrl g →")
    });

    // No keyboard input or terminal output after opening and filtering the palette.
    for (kdl, hint, old_hint) in [
        (
            r#"keybinds { pane { unbind "f"; bind "x" { ToggleFocusFullscreen; }; }; }"#,
            "Ctrl p → x",
            "Ctrl p → f",
        ),
        (
            r#"keybinds { normal { unbind "Ctrl p"; bind "Ctrl o" { SwitchToMode "Pane"; }; }; }"#,
            "Ctrl o → x",
            "Ctrl p → x",
        ),
        (
            r#"keybinds { pane { unbind "x"; }; }"#,
            "Ctrl b → z",
            "Ctrl o → x",
        ),
        (r#"keybinds { tmux { unbind "z"; }; }"#, "", "Ctrl b → z"),
        (
            r#"keybinds { normal { bind "Alt f" { ToggleFocusFullscreen; }; }; }"#,
            "Alt f",
            "Ctrl b → z",
        ),
    ] {
        assert_eq!(
            yinz.run_cli_action(CliAction::Reconfigure {
                kdl: Some(kdl.into()),
                file: None,
                write: false,
            }),
            0
        );
        yinz.wait_until("open palette reflects the new bindings", |snapshot| {
            snapshot.contains("> Fullscreen")
                && snapshot.contains("Fullscreen/restore pane")
                && snapshot.contains("Esc Close")
                && snapshot.contains(hint)
                && !snapshot.contains(old_hint)
                && !snapshot.contains("Ctrl g →")
        });
    }
    assert!(shell.stdin_bytes().is_empty());
    yinz.quit();
}

#[test]
fn command_palette_receives_original_keys_before_write_bindings() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(
            r#"
            load_plugins clear-defaults=true {}
            default_mode "locked"
            keybinds {
                locked {
                    bind "Esc" { Write 27; }
                    bind "x" { WriteChars "remapped"; }
                    bind "n" { Write 65; WriteChars "second"; }
                }
            }
            "#,
        )
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(PROMPT);
    yinz.wait_until("shell ready", |snapshot| snapshot.contains("$"));
    let before = shell.stdin_bytes();
    yinz.send_stdin(DO);
    yinz.wait_until("palette opened", |snapshot| {
        snapshot.contains("Switch session")
    });
    yinz.send_stdin(b"x");
    yinz.wait_until("WriteChars preserves the original key", |snapshot| {
        snapshot.contains("> x") && !snapshot.contains("remapped")
    });
    yinz.send_stdin(b"n");
    yinz.wait_until("multiple writes insert the original key once", |snapshot| {
        snapshot.contains("> xn") && !snapshot.contains("second")
    });
    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("Escape bound to Write dismisses the palette", |snapshot| {
        !snapshot.contains("Esc Close") && !snapshot.contains("> xn")
    });
    assert_eq!(shell.stdin_bytes(), before);
    yinz.send_stdin(b"xn\x1b");
    let expected = [before, b"remappedAsecond\x1b".to_vec()].concat();
    shell.wait_for_stdin(
        "bindings resume after dismissal without leaked input",
        |bytes| bytes == expected,
    );
    yinz.quit();
}

#[test]
fn command_palette_default_binding_captures_keys_and_paste_without_running_commands() {
    use yinz_types::{data::InputMode, input::actions::Action};

    let config = yinz_config::config::Config::from_default_assets().unwrap();
    for mode in config.keybinds.0.into_keys() {
        let mut yinz = TestRunner::new(TERMINAL_SIZE)
            .with_config(
                r#"
                load_plugins clear-defaults=true {}
                keybinds {
                    normal { bind "Ctrl x" { WriteChars "unlocked"; }; }
                    locked { bind "Ctrl x" { WriteChars "locked"; }; }
                }
                "#,
            )
            .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
            .start();
        let shell = yinz.expect_pty_spawn();
        shell.output(PROMPT);
        yinz.wait_until("shell ready", |snapshot| snapshot.contains("$"));
        assert_eq!(
            yinz.run_actions_reporting(vec![Action::SwitchModeForAllClients { input_mode: mode }])
                .0,
            0,
        );
        let mut expected = shell.stdin_bytes();
        for close in [DO, keys::ESC.as_slice()] {
            yinz.send_stdin(DO);
            yinz.wait_until("palette opens with an empty query", |snapshot| {
                snapshot.contains("Switch session") && snapshot.contains("New tab")
            });
            // The timed unlock prefix must not swallow the following g inside the palette.
            yinz.send_stdin(b"\x07g");
            yinz.wait_until(
                "Ctrl+g leaves the following key for the palette",
                |snapshot| snapshot.contains("> g"),
            );
            yinz.send_stdin(&keys::ctrl('u'));
            assert_eq!(
                yinz.run_actions_reporting(vec![
                    Action::SwitchToMode {
                        input_mode: InputMode::Normal
                    },
                    Action::SwitchModeForAllClients {
                        input_mode: InputMode::Normal
                    },
                ])
                .0,
                0,
            );
            // Ctrl+p normally enters Pane mode; it must stay inside the palette.
            yinz.send_stdin(&keys::ctrl('p'));
            yinz.send_stdin(b"N");
            yinz.send_stdin(b"\x1b[200~ew\npane\x1b[201~");
            yinz.wait_until("typing and paste filtered", |snapshot| {
                snapshot.contains("> New pane") && !snapshot.contains("New tab")
            });
            assert_eq!(shell.stdin_bytes(), expected, "{mode:?}");
            yinz.send_stdin(close);
            yinz.wait_until("toggle or Escape closes palette", |snapshot| {
                !snapshot.contains("Selected:") && !snapshot.contains("Esc Close")
            });
            yinz.send_stdin(b"still focused\x04");
            expected.extend_from_slice(b"still focused\x04");
            // Observe the server's keybinding mode without a quit binding ending the test.
            yinz.send_stdin(&keys::ctrl('x'));
            expected.extend_from_slice(if mode == InputMode::Locked {
                b"locked"
            } else {
                b"unlocked"
            });
            shell.wait_for_stdin(
                "original lock state and Ctrl+d passthrough restored",
                |bytes| bytes == expected,
            );
        }
        yinz.quit();
    }
}

#[test]
fn command_palette_captures_paste_before_every_mode_fallback() {
    for mode in ["renamepane", "renametab", "entersearch", "pane"] {
        let mut yinz = TestRunner::new(TERMINAL_SIZE)
            .with_config(&format!(
                r#"
                load_plugins clear-defaults=true {{}}
                default_mode "{mode}"
                pane_frame_style "full"
                "#,
            ))
            .with_layout(LayoutInfo::Stringified(
                r#"layout {
                    pane size=1 borderless=true { plugin location="yinz:tab-bar"; }
                    pane
                }"#
                .into(),
            ))
            .start();
        let shell = yinz.expect_pty_spawn();
        shell.output(PROMPT);
        yinz.wait_until("shell ready", |snapshot| snapshot.contains("$"));
        yinz.send_stdin(b"\x1b[200~base\x1b[201~");
        if mode != "pane" {
            yinz.wait_until("closed palette accepts rename/search paste", |snapshot| {
                snapshot.contains("base")
            });
        }
        // Opening and dismissing the palette also drains any ignored Pane-mode paste.
        yinz.send_stdin(DO);
        yinz.wait_until("palette opened", |snapshot| {
            snapshot.contains("Switch session")
        });
        yinz.send_stdin(&keys::ESC);
        let before = yinz.wait_until("baseline after dismissal", |snapshot| {
            !snapshot.contains("Esc Close") && snapshot.tab_bar_appears()
        });
        let stdin_before = shell.stdin_bytes();
        yinz.send_stdin(DO);
        yinz.wait_until("palette reopened", |snapshot| {
            snapshot.contains("Switch session")
        });
        yinz.send_stdin("\x1b[200~café\n東京\x1b[201~".as_bytes());
        yinz.send_stdin("\x1b[200~Ω\x1b[201~".as_bytes());
        yinz.wait_until("each paste reaches the palette exactly once", |snapshot| {
            snapshot.contains("> café 東京Ω ")
        });
        yinz.send_stdin(&keys::ESC);
        let after = yinz.wait_until("palette dismissed", |snapshot| {
            !snapshot.contains("Esc Close") && snapshot.tab_bar_appears()
        });
        yinz_integration_tests::assert_same_rendered_grid(
            &after,
            &before,
            &format!("paste must preserve the underlying {mode} state"),
        );
        assert_eq!(shell.stdin_bytes(), stdin_before, "{mode}");
        // Unlocked submodes return to Normal, where paste reaches the terminal.
        yinz.send_stdin("\x1b[200~é\x1b[201~".as_bytes());
        let expected = [stdin_before, "é".as_bytes().to_vec()].concat();
        shell.wait_for_stdin(
            "normal paste reaches the terminal after dismissal",
            |bytes| bytes == expected,
        );
        yinz.quit();
    }
}

#[test]
fn command_palette_closed_paste_reaches_normal_locked_and_synchronized_terminals() {
    use yinz_types::input::actions::Action;

    for mode in ["normal", "locked"] {
        let mut yinz = TestRunner::new(TERMINAL_SIZE)
            .with_config(&format!(
                "load_plugins clear-defaults=true {{}}\ndefault_mode \"{mode}\""
            ))
            .with_layout(LayoutInfo::Stringified("layout { pane; pane; }".into()))
            .start();
        let first = yinz.expect_pty_spawn();
        let second = yinz.expect_pty_spawn();
        first.output(b"\x1b[?2004hFIRST");
        second.output(b"\x1b[?2004hSECOND");
        yinz.wait_until("both terminals enabled bracketed paste", |snapshot| {
            snapshot.contains("FIRST") && snapshot.contains("SECOND")
        });
        let pasted = "\x1b[200~café\n東京\x1b[201~".as_bytes();
        yinz.send_stdin(pasted);
        // The layout focuses the first pane.
        first.wait_for_stdin("paste reaches the focused terminal", |bytes| {
            bytes == pasted
        });
        assert!(second.stdin_bytes().is_empty());
        assert_eq!(
            yinz.run_actions_reporting(vec![Action::ToggleActiveSyncTab])
                .0,
            0
        );
        yinz.send_stdin(pasted);
        first.wait_for_stdin("synchronized paste reaches the focused pane", |bytes| {
            bytes == [pasted, pasted].concat()
        });
        second.wait_for_stdin("synchronized paste reaches the other pane", |bytes| {
            bytes == pasted
        });
        yinz.quit();
    }
}

fn palette_query(yinz: &yinz_integration_tests::TestSession, query: &str) {
    yinz.send_stdin(DO);
    yinz.wait_until("palette opened", |snapshot| {
        snapshot.contains("Switch session")
    });
    yinz.send_stdin(query.as_bytes());
    yinz.wait_until("palette query rendered", |snapshot| {
        snapshot.contains(&format!("> {query}"))
    });
    yinz.send_stdin(&keys::ENTER);
}

fn replace_name(yinz: &yinz_integration_tests::TestSession, name: &str) {
    yinz.wait_until("name form opened", |snapshot| snapshot.contains("Name:"));
    yinz.send_stdin(&keys::ctrl('u'));
    yinz.send_stdin(name.as_bytes());
    yinz.send_stdin(b"\r\r");
}

#[test]
fn command_palette_creates_renames_navigates_and_confirms_closing() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}\npane_frame_style \"full\"")
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let root = yinz.expect_pty_spawn();
    root.output(PROMPT);
    yinz.wait_until("shell ready", |snapshot| snapshot.contains("$"));
    palette_query(&yinz, "Rename pane…");
    replace_name(&yinz, "palette-root");
    yinz.wait_until("root renamed", |snapshot| {
        snapshot.contains("palette-root") && !snapshot.contains("Name:")
    });

    palette_query(&yinz, "New pane…");
    yinz.wait_until("creation form opened", |snapshot| {
        snapshot.contains("Placement:")
    });
    yinz.send_stdin(b"palette-child\r/tmp\r\x1b[C\r\r");
    let child = yinz.expect_pty_spawn();
    child.output(b"CHILD");
    yinz.wait_until("named child created", |snapshot| {
        snapshot.contains("palette-child") && snapshot.contains("CHILD")
    });
    let Some(yinz_types::input::command::TerminalAction::RunCommand(command)) =
        child.terminal_action()
    else {
        panic!("expected shell command");
    };
    assert_eq!(command.cwd.as_deref(), Some(std::path::Path::new("/tmp")));
    yinz.send_stdin(b"child input");
    child.wait_for_stdin("new pane is focused", |bytes| {
        bytes.ends_with(b"child input")
    });
    assert!(root.stdin_bytes().is_empty());

    palette_query(&yinz, "New tab…");
    yinz.wait_until("new tab form opened", |snapshot| {
        snapshot.contains("Directory:")
    });
    yinz.send_stdin(b"palette-tab\r/tmp\r\r");
    let tab_shell = yinz.expect_pty_spawn();
    tab_shell.output(b"TAB CONTENT");
    yinz.wait_until("new tab focused", |snapshot| {
        snapshot.contains("TAB CONTENT") && !snapshot.contains("CHILD")
    });
    palette_query(&yinz, "Rename tab…");
    replace_name(&yinz, "palette-renamed-tab");
    yinz.wait_until("tab rename submitted", |snapshot| {
        !snapshot.contains("Name:") && snapshot.contains("TAB CONTENT")
    });

    // Direct pane destinations switch tabs as well as focus the pane.
    palette_query(&yinz, "palette-root");
    yinz.wait_until("original pane focused", |snapshot| {
        snapshot.contains("CHILD") && !snapshot.contains("TAB CONTENT")
    });
    yinz.send_stdin(b"root input");
    root.wait_for_stdin("destination receives input", |bytes| bytes == b"root input");
    palette_query(&yinz, "Switch tab…");
    yinz.send_stdin(b"palette-renamed-tab");
    yinz.wait_until("tab picker finds renamed tab", |snapshot| {
        snapshot.contains("> palette-renamed-tab")
    });
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("tab destination focused", |snapshot| {
        snapshot.contains("TAB CONTENT") && !snapshot.contains("CHILD")
    });

    palette_query(&yinz, "Close tab…");
    yinz.wait_until("tab confirmation identifies target and count", |snapshot| {
        snapshot.contains("palette-renamed-tab")
            && snapshot.contains("1 panes")
            && snapshot.contains("Cancel")
    });
    yinz.send_stdin(&keys::ENTER); // default Cancel
    yinz.wait_until("cancel restores search", |snapshot| {
        snapshot.contains("> Close tab…")
    });
    assert!(!tab_shell.is_exited());
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("confirmation reopened", |snapshot| {
        snapshot.contains("Cancel")
    });
    yinz.send_stdin(b"\x1b[B\r");
    yinz.wait_until("tab closed", |snapshot| {
        snapshot.contains("CHILD")
            && !snapshot.contains("TAB CONTENT")
            && !snapshot.contains("Cancel")
    });
    assert!(tab_shell.is_exited());

    palette_query(&yinz, "palette-child");
    yinz.wait_until("child focused", |snapshot| {
        !snapshot.contains("> palette-child")
    });
    palette_query(&yinz, "Close pane…");
    yinz.wait_until("pane confirmation identifies target", |snapshot| {
        snapshot.contains(&format!("Close [{}] palette-child", child.terminal_id()))
            && snapshot.contains("Cancel")
    });
    yinz.send_stdin(b"\x1b[B\r");
    yinz.wait_until("child closed", |snapshot| {
        !snapshot.contains("CHILD")
            && !snapshot.contains("Cancel")
            && snapshot.contains("palette-root")
    });
    assert!(child.is_exited());
    yinz.quit();
}

#[test]
fn command_palette_new_session_validates_then_hands_off_name_and_directory() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    yinz.expect_pty_spawn().output(PROMPT);
    yinz.wait_until("shell ready", |snapshot| snapshot.contains("$"));
    palette_query(&yinz, "New session…");
    yinz.wait_until("session form opened", |snapshot| snapshot.contains("Name:"));
    yinz.send_stdin(b"\r\r\r");
    yinz.wait_until("empty name rejected", |snapshot| {
        snapshot.contains("Session name cannot be empty")
    });
    yinz.send_stdin(b"\x1b[A\x1b[A");
    yinz.send_stdin(yinz.session_name().as_bytes());
    yinz.send_stdin(b"\r\r\r");
    yinz.wait_until("conflicting name rejected", |snapshot| {
        snapshot.contains("already exists")
    });
    yinz.send_stdin(b"\x1b[A\x1b[A");
    yinz.send_stdin(&keys::ctrl('u'));
    let name = format!("{}-new", yinz.session_name());
    yinz.send_stdin(name.as_bytes());
    yinz.send_stdin(b"\r/tmp\r\r");
    let connection = yinz.take_session_switch();
    assert_eq!(
        connection.name.as_ref().map(|name| name.as_str()),
        Some(name.as_str())
    );
    assert_eq!(
        connection.cwd.as_deref(),
        Some(std::path::Path::new("/tmp"))
    );
    assert!(connection.layout.is_none(), "use default session layout");
    yinz.quit();
}

#[test]
fn command_palette_switches_to_a_live_session() {
    let mut destination = TestRunner::new(TERMINAL_SIZE)
        .skip_concurrency_slot()
        .with_config("load_plugins clear-defaults=true {}\ndisable_session_metadata false")
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    destination.expect_pty_spawn().output(PROMPT);
    let mut source = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}\ndisable_session_metadata false")
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    source.expect_pty_spawn().output(PROMPT);
    source.wait_until("source ready", |snapshot| snapshot.contains("$"));
    palette_query(&source, "Switch session…");
    source.send_stdin(destination.session_name().as_bytes());
    source.wait_until("live session is offered", |snapshot| {
        snapshot.contains(&format!("> {}", destination.session_name()))
            && snapshot.contains("connected client")
            && snapshot.contains("up ")
    });
    source.send_stdin(&keys::ENTER);
    let connection = source.take_session_switch();
    assert_eq!(
        connection.name.as_ref().map(|name| name.as_str()),
        Some(destination.session_name())
    );
    assert!(connection.cwd.is_none());
    source.quit();
    destination.quit();
}

#[test]
fn command_palette_saves_and_renames_the_session() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}\nsession_serialization false")
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    yinz.expect_pty_spawn().output(PROMPT);
    yinz.wait_until("shell ready", |snapshot| snapshot.contains("$"));
    palette_query(&yinz, "Save session");
    yinz.wait_for_serialized_session();
    yinz.wait_until("save dismissed palette", |snapshot| {
        !snapshot.contains("Esc Close")
    });
    palette_query(&yinz, "Rename session…");
    let name = format!("{}-renamed", yinz.session_name());
    replace_name(&yinz, &name);
    yinz.wait_until("rename dismissed palette", |snapshot| {
        !snapshot.contains("Name:")
    });
    // Screen queue ordering makes a subsequent palette show the new session name.
    palette_query(&yinz, "Rename session…");
    yinz.wait_until("renamed session prefilled", |snapshot| {
        snapshot.contains(&name)
    });
    yinz.send_stdin(&keys::ESC);
    yinz.send_stdin(&keys::ESC);
    yinz.quit();
    assert_eq!(yinz.orphan_sweeps(), vec![name]);
}

#[test]
fn command_palette_detaches_only_its_client() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let pane = yinz.expect_pty_spawn();
    pane.output(PROMPT);
    yinz.wait_until("shell ready", |snapshot| snapshot.contains("$"));
    let other = yinz.attach_client(TERMINAL_SIZE);
    other.wait_until("other client ready", |snapshot| snapshot.contains("$"));
    palette_query(&yinz, "Detach");
    yinz.wait_until_raw_output("palette detached client", |bytes| {
        String::from_utf8_lossy(bytes).contains("Bye from Yinz!")
    });
    other.send_stdin(b"still attached");
    pane.wait_for_stdin("other client still works", |bytes| {
        bytes == b"still attached"
    });
    assert!(!pane.is_exited());
    other.quit();
    pane.exit(Some(0));
    yinz.wait_for_exit();
}

#[test]
fn command_palette_switches_to_an_exited_session() {
    use yinz_runtime::consts::YINZ_SESSION_INFO_CACHE_DIR;
    use yinz_runtime::session_checkpoint::write_checkpoint;
    std::fs::create_dir_all(&*YINZ_SESSION_INFO_CACHE_DIR).unwrap();
    let saved = tempfile::Builder::new()
        .prefix("exited-")
        .tempdir_in(&*YINZ_SESSION_INFO_CACHE_DIR)
        .unwrap();
    let name = saved.path().file_name().unwrap().to_str().unwrap();
    write_checkpoint(
        saved.path(),
        "",
        &("layout { pane; }".into(), Default::default()),
    )
    .unwrap();
    let mut source = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    source.expect_pty_spawn().output(PROMPT);
    source.wait_until("source ready", |snapshot| snapshot.contains("$"));
    palette_query(&source, "Switch session…");
    source.send_stdin(name.as_bytes());
    source.wait_until("exited session is offered", |snapshot| {
        snapshot.contains("Exited sessions")
            && snapshot.contains(name)
            && snapshot.contains("Enter to restore")
            && !snapshot.contains("Live sessions")
    });
    source.send_stdin(&keys::ENTER);
    let connection = source.take_session_switch();
    assert_eq!(
        connection.name.as_ref().map(|name| name.as_str()),
        Some(name)
    );
    assert!(connection.cwd.is_none());
    source.quit();
}

#[test]
fn mother_launches_from_its_keybinding_toggles_and_follows_tab_switches() {
    let elsewhere = tempfile::tempdir().unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}\npane_animations false")
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{ tab name=\"first\" focus=true {{ pane cwd={:?}; }}; tab name=\"second\" {{ pane; }}; }}",
            elsewhere.path().to_str().unwrap()
        )))
        .start();
    let first = yinz.expect_pty_spawn();
    let second = yinz.expect_pty_spawn();
    first.output(PROMPT);
    second.output(PROMPT);
    yinz.wait_until("first tab ready", |snapshot| snapshot.contains("$"));
    // Ctrl e enters Agent mode; m toggles mother and returns to Normal.
    let toggle = || {
        yinz.send_stdin(&keys::ctrl('e'));
        yinz.send_stdin(b"m");
    };

    // No mother yet: the key opens the palette at the agents to launch.
    toggle();
    yinz.wait_until("mother picker opened", |snapshot| {
        snapshot.contains("Launch mother") && snapshot.contains("codex")
    });
    yinz.send_stdin(b"claude");
    yinz.wait_until("claude selected", |snapshot| snapshot.contains("> claude"));
    yinz.send_stdin(&keys::ENTER);
    let mother = yinz.expect_pty_spawn();
    let command = mother.run_command();
    // The session's starting directory, not the focused pane's.
    assert_eq!(command.cwd, Some(std::env::current_dir().unwrap()));
    assert!(
        command.args.iter().any(|arg| arg == "claude"),
        "{command:?}"
    );
    mother.output(b"MOTHER READY\r\n");
    yinz.wait_until("mother is shown and the palette is gone", |snapshot| {
        snapshot.contains("MOTHER READY") && !snapshot.contains("Launch mother")
    });
    yinz.send_stdin(b"to-mother");
    mother.wait_for_stdin("mother has the keyboard", |bytes| {
        bytes.ends_with(b"to-mother")
    });

    toggle();
    yinz.wait_until("mother hidden", |snapshot| {
        !snapshot.contains("MOTHER READY")
    });
    yinz.send_stdin(b"to-first");
    first.wait_for_stdin("the pane underneath has the keyboard", |bytes| {
        bytes.ends_with(b"to-first")
    });

    toggle();
    yinz.wait_until("mother shown again", |snapshot| {
        snapshot.contains("MOTHER READY")
    });
    // Tab mode, next tab, back to Normal: a visible mother comes along with its keyboard.
    yinz.send_stdin(&keys::ctrl('t'));
    yinz.send_stdin(b"l");
    yinz.send_stdin(&keys::ENTER);
    yinz.send_stdin(b"after-switch");
    mother.wait_for_stdin("mother kept the keyboard across tabs", |bytes| {
        bytes.ends_with(b"after-switch")
    });
    yinz.wait_until("mother is drawn on the second tab", |snapshot| {
        snapshot.contains("MOTHER READY")
    });
    toggle();
    yinz.wait_until("mother hidden on the second tab", |snapshot| {
        !snapshot.contains("MOTHER READY")
    });
    yinz.send_stdin(b"to-second");
    second.wait_for_stdin("the second tab's pane has the keyboard", |bytes| {
        bytes.ends_with(b"to-second")
    });

    // Once its process exits it is no longer mother, and the key offers a new one over what it
    // left behind.
    toggle();
    yinz.wait_until("mother shown before it exits", |snapshot| {
        snapshot.contains("MOTHER READY")
    });
    mother.exit(Some(0));
    // The shipped `agent_exit` hook stands in for the exited agent in its pane.
    yinz.wait_until("the exited agent is replaced in its pane", |snapshot| {
        snapshot.contains("restart agent") && !snapshot.contains("MOTHER READY")
    });
    toggle();
    // The pane shows through the palette's translucent backing wherever the two overlap, so
    // each is matched by text the other leaves alone.
    yinz.wait_until("a new mother is offered over the exited one", |snapshot| {
        snapshot.contains("Enter Choose")
            && snapshot.contains("codex")
            && snapshot.contains("Enter: restart")
    });
    yinz.quit();
}

#[test]
fn mother_launches_in_the_configured_session_directory() {
    // Where the session's panes open, which is not where the server process happens to run.
    let start = tempfile::tempdir().unwrap();
    let start = start.path().canonicalize().unwrap();
    assert_ne!(start, std::env::current_dir().unwrap());
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!(
            "load_plugins clear-defaults=true {{}}\npane_animations false\ndefault_cwd {:?}",
            start.to_str().unwrap()
        ))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(PROMPT);
    yinz.wait_until("session ready", |snapshot| snapshot.contains("$"));
    yinz.send_stdin(&keys::ctrl('e'));
    yinz.send_stdin(b"m");
    yinz.wait_until("mother picker opened", |snapshot| {
        snapshot.contains("Launch mother") && snapshot.contains("codex")
    });
    yinz.send_stdin(b"claude");
    yinz.wait_until("claude selected", |snapshot| snapshot.contains("> claude"));
    yinz.send_stdin(&keys::ENTER);
    let mother = yinz.expect_pty_spawn();
    assert_eq!(mother.run_command().cwd, Some(start));
    yinz.quit();
}

#[test]
fn command_palette_launches_agent_in_selected_worktree_and_focuses_it() {
    let repository = tempfile::tempdir().unwrap();
    let root = repository.path().canonicalize().unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ],
        vec![
            "worktree",
            "add",
            ".worktrees/palette",
            "-b",
            "palette-branch",
        ],
    ] {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}\npane_animations false")
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{ pane cwd={:?}; }}",
            root.to_str().unwrap()
        )))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(PROMPT);
    yinz.wait_until("shell ready", |s| s.contains("$"));
    yinz.send_stdin(DO);
    yinz.wait_until("palette open", |s| s.contains("Switch session"));
    yinz.send_stdin(b"claude worktree");
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("caller repository offered", |s| {
        s.contains("Caller’s repository")
    });
    yinz.send_stdin(b"Caller");
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("searchable worktrees", |s| s.contains("palette-branch"));
    yinz.send_stdin(b"palette-branch");
    yinz.send_stdin(&keys::ENTER);
    let agent = yinz.expect_pty_spawn();
    let command = agent.run_command();
    assert_eq!(command.cwd, Some(root.join(".worktrees/palette")));
    assert!(
        command.args.iter().any(|arg| arg == "claude"),
        "{command:?}"
    );
    agent.output(b"PALETTE AGENT READY\r\n");
    yinz.wait_until("palette closes after launch", |s| {
        s.contains("PALETTE AGENT READY") && !s.contains("Do · Project:")
    });
    yinz.send_stdin(b"focused-agent");
    agent.wait_for_stdin("new agent is focused", |bytes| {
        bytes.ends_with(b"focused-agent")
    });
    assert!(!shell
        .stdin_bytes()
        .windows(13)
        .any(|s| s == b"focused-agent"));
    yinz.quit();
}

fn project_note_editor_flow(size: yinz_integration_tests::Size) {
    use yinz_cli::notes_todos::NotesTodos;
    use yinz_types::input::command::TerminalAction;
    let mut yinz = TestRunner::new(size)
        .with_config(
            r#"
            pane_animations false
            plugins {
                project-store location="yinz:project-store"
                agent-service location="yinz:agent-service"
            }
            load_plugins clear-defaults=true { "project-store"; "agent-service"; }
        "#,
        )
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(PROMPT);
    yinz.wait_until("shell ready", |grid| grid.contains("$"));
    let request = |command: NotesTodos| {
        let action = command.action(&mut std::io::empty()).unwrap();
        yinz_client::store_client::request(&yinz.session_name().into(), action, None).unwrap()
    };
    request(NotesTodos::ProjectRemember {
        name: "palette-notes".into(),
        root: "/work/palette-notes".into(),
        worktree_dir: None,
        firecracker_image: None,
    });
    let original = request(NotesTodos::NoteAdd {
        title: "Initial note".into(),
        project: Some("palette-notes".into()),
        global: false,
        body: Some("Body needle\n\nMarkdown paragraph".into()),
        body_file: None,
        tags: vec!["design".into()],
        author: Some("codex".into()),
        todo_ids: vec![],
    });
    let id = original["id"].as_u64().unwrap();
    request(NotesTodos::NoteAdd {
        title: "Other project".into(),
        project: None,
        global: true,
        body: Some("Body needle".into()),
        body_file: None,
        tags: vec![],
        author: None,
        todo_ids: vec![],
    });
    yinz.send_stdin(DO);
    yinz.wait_until("palette knows registered project", |grid| {
        grid.contains("Project: palette-notes")
    });
    yinz.send_stdin(b"Search project notes");
    yinz.wait_until("search command selected", |grid| {
        grid.contains("> Search project notes")
    });
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("recent notes", |grid| {
        grid.contains("Initial note") && grid.contains("Body needle")
    });
    assert!(!yinz.snapshot().contains("Other project"));
    yinz.send_stdin(b"needle");
    yinz.wait_until("body search result", |grid| {
        grid.contains(&format!("› #{id} Initial note"))
            && grid.contains("> needle")
            && !grid.contains("Loading…")
    });
    request(NotesTodos::NoteEdit {
        id,
        title: Some("Current note".into()),
        body: None,
        body_file: None,
        tags: vec![],
        clear_tags: false,
        expect_rev: original["rev"].as_i64(),
    });
    yinz.send_stdin(&keys::ENTER);
    let editor = yinz.expect_pty_spawn();
    let Some(TerminalAction::OpenFile(payload)) = editor.terminal_action() else {
        panic!("expected file editor");
    };
    let path = payload.path;
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# Current note\n\nBody needle\n\nMarkdown paragraph\n"
    );
    assert_eq!(payload.cwd, path.parent().map(std::path::Path::to_path_buf));
    let expected_cols = size.cols.min(120).saturating_sub(2) as u16;
    let expected_rows = (size.rows * 70 / 100).saturating_sub(2) as u16;
    editor.wait_for_size("finder sized editor", |cols, rows| {
        cols == expected_cols && rows == expected_rows
    });
    editor.output(b"NOTE EDITOR READY\r\n");
    yinz.wait_until("editor replaces palette", |grid| {
        grid.contains("NOTE EDITOR READY")
            && grid.contains(&format!("#{id} Current note"))
            && !grid.contains("Search project notes")
    });
    let grid = yinz.snapshot();
    assert_eq!(
        grid.row_of_line("NOTE EDITOR READY"),
        Some(size.rows * 15 / 100 + 1)
    );
    let lines = grid.lines();
    let line = lines
        .iter()
        .find(|line| line.contains("NOTE EDITOR READY"))
        .unwrap();
    assert_eq!(
        line.chars().position(|ch| ch == 'N'),
        Some((size.cols - size.cols.min(120)) / 2 + 1)
    );
    yinz.send_stdin(b"editor input");
    editor.wait_for_stdin("editor receives input", |bytes| {
        bytes.ends_with(b"editor input")
    });
    assert!(!shell
        .stdin_bytes()
        .windows(12)
        .any(|bytes| bytes == b"editor input"));
    let replacement = path.with_file_name("replacement.md");
    std::fs::write(&replacement, "# Updated note\n\nUpdated needle body\n").unwrap();
    std::fs::rename(replacement, &path).unwrap();
    editor.exit(Some(0));
    yinz.wait_until("previous note search restored", |grid| {
        grid.contains("> needle") && grid.contains(&format!("› #{id} Updated note"))
    });
    assert!(!path.exists());
    let after = request(NotesTodos::NoteGet { id });
    assert_eq!(after["title"], "Updated note");
    assert_eq!(after["body"], "Updated needle body");
    for key in ["tags", "todo_ids", "author", "project", "archived"] {
        assert_eq!(after[key], original[key], "{key}");
    }
    for outcome in ["invalid", "failed", "closed", "conflict"] {
        yinz.send_stdin(&keys::ENTER);
        let editor = yinz.expect_pty_spawn();
        let Some(TerminalAction::OpenFile(payload)) = editor.terminal_action() else {
            panic!("expected file editor");
        };
        let path = payload.path;
        editor.output(b"FAILURE TEST EDITOR READY\r\n");
        yinz.wait_until("failure test editor replaces search", |grid| {
            grid.contains("FAILURE TEST EDITOR READY")
                && grid.contains(&format!("#{id} Updated note"))
                && !grid.contains("Search project notes")
        });
        std::fs::write(
            &path,
            if outcome == "invalid" {
                "invalid changed contents"
            } else {
                "# Updated note\n\nMy changed needle body\n"
            },
        )
        .unwrap();
        let mut expected = request(NotesTodos::NoteGet { id });
        if outcome == "conflict" {
            expected = request(NotesTodos::NoteEdit {
                id,
                title: None,
                body: Some("Concurrent needle body".into()),
                body_file: None,
                tags: vec![],
                clear_tags: false,
                expect_rev: expected["rev"].as_i64(),
            });
        }
        if outcome == "closed" {
            assert_eq!(
                yinz.run_cli_action(CliAction::ClosePane {
                    pane_id: Some(format!("terminal_{}", editor.terminal_id()))
                }),
                0
            );
        } else {
            editor.exit(Some(if outcome == "failed" { 1 } else { 0 }));
        }
        yinz.wait_until(
            "failed edit restores search with recovery details",
            |grid| {
                grid.contains("Note editor details")
                    && grid.contains("> needle")
                    && !grid.contains("Loading…")
            },
        );
        let actual = request(NotesTodos::NoteGet { id });
        for key in [
            "title", "body", "rev", "tags", "todo_ids", "author", "project", "archived",
        ] {
            assert_eq!(actual[key], expected[key], "{outcome}: {key}");
        }
        assert!(path.exists(), "{outcome} must retain edited contents");
        yinz.send_stdin(&keys::ctrl('n'));
        yinz.send_stdin(&keys::ENTER);
        yinz.wait_until("recovery path can be read in full", |grid| {
            grid.contains("Note editor details")
                && grid
                    .text
                    .lines()
                    .map(|line| line.trim().trim_matches('│').trim())
                    .collect::<String>()
                    .contains(path.to_str().unwrap())
                && !grid.contains("> needle")
        });
        yinz.send_stdin(&keys::ESC);
        yinz.wait_until("recovery details return to search", |grid| {
            grid.contains("> needle") && !grid.contains("Loading…")
        });
        yinz.send_stdin(&keys::ctrl('p'));
        yinz.wait_until("edited note reselected", |grid| {
            grid.contains(&format!("› #{id} Updated note"))
        });
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("Escape returns to root", |grid| {
        grid.contains("> Search project notes") && !grid.contains("> needle")
    });
    yinz.quit();
}

#[test]
fn project_note_editor_uses_finder_geometry_on_a_large_terminal() {
    project_note_editor_flow(yinz_integration_tests::Size {
        cols: 180,
        rows: 60,
    });
}

#[test]
fn project_note_editor_clamps_to_a_small_terminal() {
    project_note_editor_flow(yinz_integration_tests::Size { cols: 70, rows: 20 });
}

#[test]
fn command_palette_agent_history_event_back_focus_and_archive_resume() {
    use agent_proto::{
        agents::{AgentRun, RunEntry, RunEntryKind, RunOutcome},
        store,
    };
    use yinz_types::data::PaneId;
    let root = yinz_integration_tests::test_env::init();
    let data = yinz_storage::data_access::DataAccess::sqlite(&root.join("data"));
    for definition in store::shared_defs() {
        data.plugin_data()
            .define_entity(store::SHARED, "test", &definition, false)
            .unwrap();
    }
    let run = AgentRun {
        id: "codex:palette-history".into(),
        pane_id: 1,
        yinz_session_name: "archived-session".into(),
        host_id: yinz_runtime::host_identity::host_id(),
        origin_run_id: None,
        previous_run_id: None,
        session_id: "palette-history".into(),
        kind: "codex".into(),
        cwd: Some(root.join("cwd").to_string_lossy().into_owned()),
        host: None,
        worktree: None,
        bubblewrap: None,
        started_at_ms: 100,
        ended_at_ms: Some(200),
        outcome: RunOutcome::Completed,
        revision: 1,
        summarized_revision: 0,
        summary: None,
        omitted_entries: 0,
        entries: vec![RunEntry {
            revision: 1,
            event_id: None,
            kind: RunEntryKind::Message,
            text: "**Unique retained palette conversation**\n\n- Exact second line\n\n```rust\nlet answer = 42;\n```\n\nReader-only final paragraph".into(),
            created_at_ms: Some(150),
            tool: None,
        }],
    };
    data.plugin_data()
        .apply(
            store::SHARED,
            &agent_proto::agents::rows::run_mutations(&run),
        )
        .unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}\npane_animations false")
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(PROMPT);
    yinz.wait_until("shell ready", |s| s.contains("$"));
    assert_eq!(
        yinz.run_cli_action(CliAction::AgentReport {
            pane_id: Some(PaneId::Terminal(shell.terminal_id()).to_string()),
            kind: Some("codex".into()),
            state: Some("idle".into()),
            detail: Some("paletteinspect".into()),
            cwd: None,
            hook_json: false,
            clear: false,
            run_id: Some(run.id.clone())
        }),
        0
    );
    yinz.send_stdin(DO);
    yinz.send_stdin(b"paletteinspect");
    yinz.wait_until("live agent search", |s| {
        s.contains("codex") && s.contains("idle")
    });
    yinz.send_stdin(b"\r");
    yinz.wait_until("agent timeline", |s| {
        s.contains("Unique retained palette conversation")
    });
    yinz.send_stdin(b"\r");
    yinz.wait_until("expanded event", |s| {
        s.contains("Reader-only final paragraph") && s.contains("Revision: 1")
    });
    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("back to timeline", |s| {
        s.contains("Focus") && s.contains("Latest / Refresh")
    });
    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("search restored", |s| s.contains("> paletteinspect"));
    yinz.send_stdin(b"\x1b[13;5u"); // Ctrl+Enter.
    yinz.send_stdin(b"focusedbyagent");
    shell.wait_for_stdin("direct focus", |bytes| bytes.ends_with(b"focusedbyagent"));
    yinz.send_stdin(DO);
    yinz.send_stdin(b"Archived agents");
    yinz.send_stdin(b"\r");
    yinz.wait_until("archive browser", |s| s.contains("Ended runs"));
    yinz.send_stdin(b"Unique retained palette conversation");
    yinz.wait_until("archive search hit", |s| {
        s.contains("Completed") && s.contains("Unique retained")
    });
    yinz.send_stdin(b"\x1b[B"); // Filters, then the matching run.
    yinz.send_stdin(b"\r");
    yinz.wait_until("archived info", |s| {
        s.contains("Resume") && s.contains("Latest / Refresh")
    });
    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("archive query restored", |s| {
        s.contains("> Unique retained palette conversation")
    });
    yinz.send_stdin(b"\r");
    yinz.wait_until("archive selection restored", |s| {
        s.contains("Resume") && s.contains("Latest / Refresh")
    });
    yinz.send_stdin(b"\t"); // Timeline -> actions.
    yinz.send_stdin(b"\r");
    let resumed = yinz.expect_pty_spawn();
    assert!(resumed
        .run_command()
        .args
        .iter()
        .any(|arg| arg == "palette-history"));
    resumed.output(b"RESUMED PALETTE AGENT\r\n");
    yinz.wait_until("resumed agent focused", |s| {
        s.contains("RESUMED PALETTE AGENT") && !s.contains("Latest / Refresh")
    });
    yinz.quit();
}

#[test]
fn theme_picker_can_cancel_while_config_file_is_locked() {
    let directory = tempfile::tempdir().unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_config(
            r#"
            load_plugins clear-defaults=true {}
            pane_animations false
            theme "nightfox"
            pane_frame_style "full"
            keybinds { normal { bind "Ctrl Alt p" { ToggleCommandPalette; }; }; }
            "#,
        )
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(PROMPT);
    let initial = yinz.wait_until("shell ready", |snapshot| snapshot.contains("$"));
    let frame = initial.cell_foreground(0, 0);
    let overlay = directory.path().join("config.d");
    std::fs::create_dir_all(&overlay).unwrap();
    // Dropping the file releases the lock, including if an input assertion panics.
    let lock = std::fs::File::create(overlay.join(".yinz-managed.kdl.lock")).unwrap();
    lock.lock().unwrap();

    yinz.send_stdin(DO);
    yinz.send_stdin(b"Change theme");
    yinz.send_stdin(b"\r");
    yinz.wait_until("theme picker ready", |snapshot| {
        snapshot.contains("Enter Save")
    });
    yinz.send_stdin(b"dracula");
    yinz.wait_until("theme selected", |snapshot| {
        snapshot.contains("> dracula") && snapshot.cell_foreground(0, 0) != frame
    });
    yinz.send_stdin(b"\r");
    yinz.wait_until("contended save reports an error", |snapshot| {
        snapshot.contains("Configuration is busy") && snapshot.contains("> dracula")
    });
    yinz.send_stdin(&keys::ESC);
    yinz.wait_until(
        "cancel restores the theme while the lock is held",
        |snapshot| snapshot.contains("Esc Close") && snapshot.cell_foreground(0, 0) == frame,
    );
    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("palette closes while the lock is held", |snapshot| {
        !snapshot.contains("Enter Save")
            && !snapshot.contains("Esc Close")
            && snapshot.contains("$")
            && snapshot.cell_foreground(0, 0) == frame
    });
    yinz.send_stdin(b"\x1b[200~after cancellation\x1b[201~");
    shell.wait_for_stdin(
        "input reaches the shell while the config lock is held",
        |bytes| bytes == b"after cancellation",
    );
    shell.output(b"\r\nRENDER WHILE LOCKED\r\n");
    yinz.wait_until(
        "terminal output renders while the lock is held",
        |snapshot| snapshot.contains("RENDER WHILE LOCKED"),
    );
    assert!(!overlay.join("yinz-managed.kdl").exists());

    yinz.send_stdin(DO);
    yinz.send_stdin(b"Change theme\r");
    yinz.wait_until("theme picker reopens", |snapshot| {
        snapshot.contains("Enter Save")
    });
    yinz.send_stdin(b"dracula\r");
    yinz.wait_until("second contended save reports an error", |snapshot| {
        snapshot.contains("Configuration is busy") && snapshot.contains("> dracula")
    });
    drop(lock);
    yinz.send_stdin(b"\r");
    yinz.wait_until("retry saves the theme and closes the picker", |snapshot| {
        !snapshot.contains("> dracula")
            && !snapshot.contains("Configuration is busy")
            && !snapshot.contains("Enter Save")
            && !snapshot.contains("Esc Close")
            && snapshot.contains("RENDER WHILE LOCKED")
            && snapshot.cell_foreground(0, 0) != frame
    });
    let saved = std::fs::read_to_string(overlay.join("yinz-managed.kdl")).unwrap();
    assert!(saved.contains("theme \"dracula\""));
    yinz.quit();
}

#[test]
fn theme_picker_preview_cancel_save_and_reload() {
    let directory = tempfile::tempdir().unwrap();
    let config = r#"
        load_plugins clear-defaults=true {}
        pane_animations false
        theme "nightfox"
        pane_frame_style "full"
        keybinds { normal { bind "Ctrl Alt p" { ToggleCommandPalette; }; }; }
    "#;
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_config(config)
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(PROMPT);
    let initial = yinz.wait_until("shell ready", |snapshot| snapshot.contains("$"));
    let frame = initial.cell_foreground(0, 0);
    let managed = directory.path().join("config.d/yinz-managed.kdl");
    yinz.send_stdin(DO);
    yinz.send_stdin(b"Change theme");
    yinz.send_stdin(b"\r");
    yinz.wait_until("theme picker ready", |snapshot| {
        snapshot.contains("Enter Save")
    });
    yinz.send_stdin(b"dracula");
    let preview = yinz.wait_until("preview repaints existing pane frame", |snapshot| {
        snapshot.contains("Enter Save")
            && snapshot.contains("> dracula")
            && snapshot.cell_foreground(0, 0) != frame
    });
    assert!(!managed.exists());
    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("cancel restores frame", |snapshot| {
        snapshot.contains("Esc Close") && snapshot.cell_foreground(0, 0) == frame
    });
    assert!(!managed.exists());
    yinz.send_stdin(b"\r");
    yinz.wait_until("theme picker reopened", |snapshot| {
        snapshot.contains("Enter Save")
    });
    yinz.send_stdin(b"dracula");
    yinz.wait_until("theme selected again", |snapshot| {
        snapshot.contains("> dracula")
            && snapshot.cell_foreground(0, 0) == preview.cell_foreground(0, 0)
    });
    yinz.send_stdin(b"\r");
    yinz.wait_until("saved picker closes", |snapshot| {
        !snapshot.contains("Enter Save")
            && !snapshot.contains("Esc Close")
            && snapshot.contains("$")
            && snapshot.cell_foreground(0, 0) == preview.cell_foreground(0, 0)
    });
    let saved = std::fs::read_to_string(&managed).unwrap();
    assert!(saved.contains("theme \"dracula\""), "{saved}");
    assert!(!saved.contains("theme_dark"), "{saved}");
    assert!(!saved.contains("theme_light"), "{saved}");
    yinz.quit();

    let mut reloaded = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(directory.path().to_owned())
        .with_config(config)
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let shell = reloaded.expect_pty_spawn();
    shell.output(PROMPT);
    reloaded.wait_until("saved theme survives reload", |snapshot| {
        snapshot.contains("$") && snapshot.cell_foreground(0, 0) == preview.cell_foreground(0, 0)
    });
    reloaded.quit();
}

#[test]
fn theme_picker_saving_active_light_theme_preserves_terminal_theme_queries() {
    let directory = tempfile::tempdir().unwrap();
    let config = r#"
        load_plugins clear-defaults=true {}
        pane_animations false
        theme "dayfox"
        keybinds { normal { bind "Ctrl Alt p" { ToggleCommandPalette; }; }; }
    "#;
    for reload in [false, true] {
        let mut yinz = TestRunner::new(TERMINAL_SIZE)
            .with_config_dir(directory.path().to_owned())
            .with_config(config)
            .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
            .start();
        let shell = yinz.expect_pty_spawn();
        shell.output(PROMPT);
        yinz.wait_until("shell ready", |snapshot| snapshot.contains("$"));
        if !reload {
            yinz.send_stdin(DO);
            yinz.send_stdin(b"Change theme\r");
            yinz.wait_until("theme picker ready", |snapshot| {
                snapshot.contains("Enter Save")
            });
            yinz.send_stdin(b"dayfox");
            yinz.wait_until("active light theme selected", |snapshot| {
                snapshot.contains("> dayfox")
            });
            yinz.send_stdin(b"\r");
            yinz.wait_until("saved picker closes", |snapshot| {
                !snapshot.contains("Enter Save")
                    && !snapshot.contains("Esc Close")
                    && snapshot.contains("$")
            });
        }
        shell.output(b"\x1b[?996n");
        shell.wait_for_stdin("terminal theme query still reports light", |bytes| {
            bytes.windows(9).any(|window| window == b"\x1b[?997;2n")
        });
        yinz.quit();
    }
}
