#![cfg(unix)]

use yinz_cli::args::CliAction;
use yinz_integration_tests::{keys, LayoutInfo, TestRunner, PROMPT, TERMINAL_SIZE};

const DO: &[u8] = b"\x1b[112;7u"; // Ctrl+Alt+p, encoded with CSI u.

#[path = "unit/command_palette/last_project.rs"]
mod last_project;

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
        snapshot.contains("Switch current project")
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
        snapshot.contains("Switch current project")
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
                snapshot.contains("Switch current project") && snapshot.contains("New tab")
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
            if mode == InputMode::Notifications {
                // Browsing resumes after dismissal; leave it before checking terminal input.
                assert_eq!(
                    yinz.run_actions_reporting(vec![Action::SwitchToMode {
                        input_mode: InputMode::Normal,
                    }])
                    .0,
                    0
                );
            }
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
            snapshot.contains("Switch current project")
        });
        yinz.send_stdin(&keys::ESC);
        let before = yinz.wait_until("baseline after dismissal", |snapshot| {
            !snapshot.contains("Esc Close") && snapshot.tab_bar_appears()
        });
        let stdin_before = shell.stdin_bytes();
        yinz.send_stdin(DO);
        yinz.wait_until("palette reopened", |snapshot| {
            snapshot.contains("Switch current project")
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
        snapshot.contains("Switch current project")
    });
    yinz.send_stdin(query.as_bytes());
    yinz.wait_until("palette query rendered", |snapshot| {
        snapshot.contains(&format!("> {query}")) && snapshot.contains(&format!("› {query}"))
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
        .with_config("load_plugins clear-defaults=true { \"yinz:session-palette\" { shared true; }; }\npane_frame_style \"full\"")
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
    palette_query(&yinz, "Find pane…");
    yinz.wait_until("pane picker ready", |s| {
        s.contains("Esc Back") && !s.contains("Working")
    });
    yinz.send_stdin(b"terminal_0");
    yinz.wait_until("pane found", |s| {
        s.contains("› [terminal_0] palette-root") && !s.contains("Loading")
    });
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("original pane focused", |snapshot| {
        snapshot.contains("CHILD") && !snapshot.contains("TAB CONTENT")
    });
    yinz.send_stdin(b"root input");
    root.wait_for_stdin("destination receives input", |bytes| bytes == b"root input");
    palette_query(&yinz, "Find tab…");
    yinz.wait_until("tab picker ready", |s| {
        s.contains("Esc Back") && !s.contains("Working")
    });
    yinz.send_stdin(b"palette-renamed-tab");
    yinz.wait_until("tab picker finds renamed tab", |snapshot| {
        snapshot
            .lines()
            .iter()
            .any(|line| line.contains('›') && line.contains("palette-renamed-tab"))
            && !snapshot.contains("Loading")
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

    palette_query(&yinz, "Find pane…");
    yinz.wait_until("pane picker ready", |s| {
        s.contains("Esc Back") && !s.contains("Working")
    });
    yinz.send_stdin(b"palette-child");
    yinz.wait_until("child found", |s| {
        s.contains("› [terminal_1] palette-child") && !s.contains("Loading")
    });
    yinz.send_stdin(&keys::ENTER);
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
        .with_config(
            "load_plugins clear-defaults=true { \"yinz:session-palette\" { shared true; }; }",
        )
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    yinz.expect_pty_spawn().output(PROMPT);
    yinz.wait_until("shell ready", |snapshot| snapshot.contains("$"));
    palette_query(&yinz, "New session…");
    yinz.wait_until("session form opened", |snapshot| {
        snapshot.contains("Name (optional):")
    });
    yinz.send_stdin(b"bad/name\r\r");
    yinz.wait_until("invalid name rejected", |snapshot| {
        snapshot.contains("valid session name")
    });
    yinz.send_stdin(b"\x1b[A");
    yinz.send_stdin(&keys::ctrl('u'));
    let name = format!("{}-new", yinz.session_name());
    yinz.send_stdin(name.as_bytes());
    yinz.send_stdin(b"\r\x1b[B\r");
    yinz.wait_until("configuration preserves name", |s| {
        s.contains("Layout (type to search)") && s.contains(&name)
    });
    yinz.send_stdin(b"\r\r/tmp\r\r");
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
    // The in-process host's session scanner belongs to the first server.
    let mut source = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true { \"yinz:session-palette\" { shared true; }; }\ndisable_session_metadata false")
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    source.expect_pty_spawn().output(PROMPT);
    let mut destination = TestRunner::new(TERMINAL_SIZE)
        .skip_concurrency_slot()
        .with_config("load_plugins clear-defaults=true { \"yinz:session-palette\" { shared true; }; }\ndisable_session_metadata false")
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    destination.expect_pty_spawn().output(PROMPT);

    source.wait_until("source ready", |snapshot| snapshot.contains("$"));
    // Production servers each have their own process environment.
    yinz_runtime::envs::set_session_name(yinz_types::internal_ids::SessionName::new(
        source.session_name(),
    ));
    palette_query(&source, "Switch session…");
    source.wait_until("session picker ready", |s| {
        s.contains("Esc Back") && !s.contains("Working")
    });
    source.send_stdin(destination.session_name().as_bytes());
    source.wait_until("live session is offered", |snapshot| {
        snapshot.contains(&format!("› {}", destination.session_name()))
            && snapshot.contains("Running")
            && !snapshot.contains("Loading")
    });
    source.send_stdin(&keys::ENTER);
    let connection = source.take_session_switch();
    assert_eq!(
        connection.name.as_ref().map(|name| name.as_str()),
        Some(destination.session_name())
    );

    source.quit();
    destination.quit();
}

#[test]
fn command_palette_saves_and_renames_the_session() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true { \"yinz:session-palette\" { shared true; }; }\nsession_serialization false")
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    yinz.expect_pty_spawn().output(PROMPT);
    yinz.wait_until("shell ready", |snapshot| snapshot.contains("$"));
    palette_query(&yinz, "Save current session");
    yinz.wait_for_serialized_session();
    yinz.wait_until("save dismissed palette", |snapshot| {
        !snapshot.contains("Esc Close")
    });
    palette_query(&yinz, "Rename current session…");
    let name = format!("{}-renamed", yinz.session_name());
    replace_name(&yinz, &name);
    yinz.wait_until("rename dismissed palette", |snapshot| {
        !snapshot.contains("Name:")
    });
    // Screen queue ordering makes a subsequent palette show the new session name.
    palette_query(&yinz, "Rename current session…");
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
        .with_config(
            "load_plugins clear-defaults=true { \"yinz:session-palette\" { shared true; }; }",
        )
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    source.expect_pty_spawn().output(PROMPT);
    source.wait_until("source ready", |snapshot| snapshot.contains("$"));
    palette_query(&source, "Switch session…");
    source.wait_until("session picker ready", |s| {
        s.contains("Esc Back") && !s.contains("Working")
    });
    source.send_stdin(name.as_bytes());
    source.wait_until("exited session is offered", |snapshot| {
        snapshot.contains(&format!("› {name}"))
            && snapshot.contains("Saved")
            && !snapshot.contains("Loading")
    });
    source.send_stdin(&keys::ENTER);
    let connection = source.take_session_switch();
    assert_eq!(
        connection.name.as_ref().map(|name| name.as_str()),
        Some(name)
    );

    source.quit();
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
    yinz.wait_until("palette open", |s| s.contains("Switch current project"));
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
    assert_eq!(
        yinz_types::agent::agent_kind_of(&command.command, &command.args).as_deref(),
        Some("claude")
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
            notification_timeout 1
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

fn assert_terminal_background(shell: &yinz_integration_tests::FakePtyHandle, rgb: (u8, u8, u8)) {
    // Querying apps use raw mode: echoing the reply would set an application color override.
    shell.disable_echo();
    let before = shell.stdin_bytes().len();
    shell.output(b"\x1b]11;?\x07");
    let (r, g, b) = rgb;
    let expected = format!("\x1b]11;rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}\x07");
    let bytes = shell.wait_for_stdin("terminal background reply", |bytes| {
        bytes[before..].starts_with(b"\x1b]11;") && bytes[before..].ends_with(b"\x07")
    });
    assert_eq!(String::from_utf8_lossy(&bytes[before..]), expected);
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
    yinz.wait_until("theme destination", |snapshot| {
        snapshot.contains("Apply to session")
    });
    yinz.send_stdin(b"\x1b[C\r\r");
    yinz.wait_until("contended save reports an error", |snapshot| {
        snapshot.contains("Configuration is busy") && snapshot.contains("Apply theme dracula")
    });
    assert_terminal_background(&shell, (0x19, 0x23, 0x30));
    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("palette closes while the lock is held", |snapshot| {
        !snapshot.contains("Enter Save")
            && !snapshot.contains("Esc Close")
            && snapshot.contains("$")
            && snapshot.cell_foreground(0, 0) == frame
    });
    let before_paste = shell.stdin_bytes().len();
    yinz.send_stdin(b"\x1b[200~after cancellation\x1b[201~");
    shell.wait_for_stdin(
        "input reaches the shell while the config lock is held",
        |bytes| &bytes[before_paste..] == b"after cancellation",
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
    yinz.wait_until("theme destination again", |snapshot| {
        snapshot.contains("Apply to session")
    });
    yinz.send_stdin(b"\x1b[C\r\r");
    yinz.wait_until("second contended save reports an error", |snapshot| {
        snapshot.contains("Configuration is busy") && snapshot.contains("Apply theme dracula")
    });
    drop(lock);
    yinz.send_stdin(b"\r");
    yinz.wait_until("retry saves the theme and closes the picker", |snapshot| {
        !snapshot.contains("> dracula")
            && snapshot.contains("Completed: Apply theme")
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
    assert_terminal_background(&shell, (0x19, 0x23, 0x30));
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
    assert_terminal_background(&shell, (0x28, 0x2a, 0x36));
    assert!(!managed.exists());
    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("cancel restores frame", |snapshot| {
        snapshot.contains("Esc Close") && snapshot.cell_foreground(0, 0) == frame
    });
    assert!(!managed.exists());
    assert_terminal_background(&shell, (0x19, 0x23, 0x30));
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
    yinz.wait_until("theme destination", |snapshot| {
        snapshot.contains("Apply to session")
    });
    yinz.send_stdin(b"\x1b[C\r\r");
    yinz.wait_until("saved picker closes", |snapshot| {
        !snapshot.contains("Enter Save")
            && !snapshot.contains("Esc Close")
            && snapshot.contains("$")
            && snapshot.cell_foreground(0, 0) == preview.cell_foreground(0, 0)
            && snapshot.contains("Completed: Apply theme")
    });
    assert_terminal_background(&shell, (0x28, 0x2a, 0x36));
    let saved = std::fs::read_to_string(&managed).unwrap();
    assert!(saved.contains("theme \"dracula\""), "{saved}");
    let loaded = yinz_config::config::Config::load(None, Some(directory.path()), false).unwrap();
    assert!(loaded.options.theme_dark.is_none(), "{saved}");
    assert!(loaded.options.theme_light.is_none(), "{saved}");
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
    assert_terminal_background(&shell, (0x28, 0x2a, 0x36));
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
            yinz.wait_until("theme destination", |snapshot| {
                snapshot.contains("Apply to session")
            });
            yinz.send_stdin(b"\x1b[C\r\r");
            yinz.wait_until("saved picker closes", |snapshot| {
                !snapshot.contains("Enter Save")
                    && !snapshot.contains("Esc Close")
                    && snapshot.contains("$")
                    && snapshot.contains("Completed: Apply theme")
            });
        }
        shell.output(b"\x1b[?996n");
        shell.wait_for_stdin("terminal theme query still reports light", |bytes| {
            bytes.windows(9).any(|window| window == b"\x1b[?997;2n")
        });
        yinz.quit();
    }
}

#[test]
fn command_palette_reports_readable_builtin_completion() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}\npane_animations false")
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let root = yinz.expect_pty_spawn();
    root.output(PROMPT);
    yinz.wait_until("shell ready", |snapshot| snapshot.contains("$"));
    palette_query(&yinz, "Split right");
    let child = yinz.expect_pty_spawn();
    child.output(b"CHILD");
    yinz.wait_until("readable split completion", |snapshot| {
        snapshot.contains("Completed: Split right") && snapshot.contains("CHILD")
    });
    yinz.quit();
}

#[test]
fn command_palette_rejected_float_reports_failure_and_keeps_the_pane_tiled() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}\npane_animations false")
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let root = yinz.expect_pty_spawn();
    root.output(PROMPT);
    yinz.wait_until("shell ready", |snapshot| snapshot.contains("$"));
    palette_query(&yinz, "Float/embed pane");
    yinz.wait_until("rejected float reports failure", |snapshot| {
        snapshot.contains("Could not float/embed pane") && !snapshot.contains("Esc Close")
    });
    let (status, panes) = yinz.run_cli_action_reporting(CliAction::ListPanes {
        tab: false,
        command: false,
        state: true,
        geometry: true,
        all: true,
        json: true,
    });
    assert_eq!(status, 0);
    let panes: serde_json::Value = serde_json::from_str(&panes).unwrap();
    let terminals: Vec<_> = panes
        .as_array()
        .unwrap()
        .iter()
        .filter(|pane| pane["is_plugin"] == false)
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0]["is_floating"], false, "{panes}");
    assert_ne!(
        yinz.run_cli_action(CliAction::TogglePaneEmbedOrFloating {
            pane_id: Some(format!("terminal_{}", root.terminal_id().get())),
        }),
        0
    );
    yinz.quit();
}

#[test]
fn command_palette_float_embed_succeeds_and_small_surface_rejects_embedding() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(
            "load_plugins clear-defaults=true {}\npane_animations false\nnotification_timeout 1",
        )
        .with_layout(LayoutInfo::Stringified(
            "layout split_direction=\"vertical\" { pane; pane; }".into(),
        ))
        .start();
    let first = yinz.expect_pty_spawn();
    let second = yinz.expect_pty_spawn();
    first.output(PROMPT);
    second.output(PROMPT);
    yinz.wait_until("shell ready", |snapshot| snapshot.contains("$"));
    for floating in [true, false, true] {
        palette_query(&yinz, "Float/embed pane");
        yinz.wait_until("float/embed completion", |snapshot| {
            snapshot.contains("Completed: Float/embed pane") && !snapshot.contains("Esc Close")
        });
        let (status, panes) = yinz.run_cli_action_reporting(CliAction::ListPanes {
            tab: false,
            command: false,
            state: true,
            geometry: true,
            all: true,
            json: true,
        });
        assert_eq!(status, 0);
        let panes: serde_json::Value = serde_json::from_str(&panes).unwrap();
        assert_eq!(
            panes
                .as_array()
                .unwrap()
                .iter()
                .filter(|pane| pane["is_floating"] == true)
                .count(),
            usize::from(floating),
            "{panes}"
        );
        yinz.wait_until("previous completion toast expires", |snapshot| {
            !snapshot.contains("Completed: Float/embed pane")
        });
    }
    yinz.resize(yinz_integration_tests::Size { cols: 5, rows: 3 });
    yinz.wait_until("small terminal applied", |snapshot| {
        snapshot.row_count() == 3
    });
    let (status, panes) = yinz.run_cli_action_reporting(CliAction::ListPanes {
        tab: false,
        command: false,
        state: true,
        geometry: true,
        all: true,
        json: true,
    });
    assert_eq!(status, 0);
    let panes: serde_json::Value = serde_json::from_str(&panes).unwrap();
    let floating = panes
        .as_array()
        .unwrap()
        .iter()
        .find(|pane| pane["is_floating"] == true)
        .unwrap();
    let (status, message) = yinz.run_cli_action_reporting(CliAction::TogglePaneEmbedOrFloating {
        pane_id: Some(format!("terminal_{}", floating["id"])),
    });
    assert_ne!(status, 0, "{message}");
    assert!(message.contains("not enough room"), "{message}");
    let (_, after) = yinz.run_cli_action_reporting(CliAction::ListPanes {
        tab: false,
        command: false,
        state: true,
        geometry: true,
        all: true,
        json: true,
    });
    let after: serde_json::Value = serde_json::from_str(&after).unwrap();
    assert_eq!(after, panes);
    yinz.quit();
}

#[test]
fn command_palette_suspends_notifications_mode_and_restores_history() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(
            "load_plugins clear-defaults=true {}\npane_animations false\nnotification_timeout 1",
        )
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let root = yinz.expect_pty_spawn();
    root.output(PROMPT);
    yinz.wait_until("shell ready", |snapshot| snapshot.contains("$"));
    assert_eq!(
        yinz.run_cli_action(CliAction::Notify {
            text: "Retained history".into()
        }),
        0
    );
    yinz.wait_until("toast appears", |snapshot| {
        snapshot.contains("Retained history")
    });
    yinz.wait_until("toast expires", |snapshot| {
        !snapshot.contains("Retained history")
    });
    yinz.send_stdin(&keys::ctrl('o'));
    yinz.send_stdin(b"n");
    yinz.wait_until("history opens", |snapshot| {
        snapshot.contains("Retained history")
    });
    yinz.send_stdin(DO);
    yinz.wait_until("palette suspends history", |snapshot| {
        snapshot.contains("Switch current project") && !snapshot.contains("Retained history")
    });
    yinz.send_stdin(b"Rename tab");
    yinz.wait_until("palette receives keys in notifications mode", |snapshot| {
        snapshot.contains("> Rename tab")
    });
    assert_eq!(
        yinz.run_cli_action(CliAction::Notify {
            text: "Live over palette".into()
        }),
        0
    );
    yinz.wait_until("live toast appears above palette", |snapshot| {
        snapshot.contains("Live over palette")
            && snapshot.contains("> Rename tab")
            && !snapshot.contains("Retained history")
    });
    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("history restored", |snapshot| {
        snapshot.contains("Retained history") && !snapshot.contains("Esc Close")
    });
    assert!(root.stdin_bytes().is_empty());
    yinz.quit();
}

#[test]
fn session_palette_loads_by_default_and_keeps_clients_independent() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    yinz.expect_pty_spawn().output(PROMPT);
    yinz.wait_until("shell ready", |s| s.contains("$"));
    palette_query(&yinz, "New session…");
    yinz.wait_until("session form loaded by default", |s| {
        s.contains("Name (optional):")
    });
    yinz.send_stdin(b"private-first-client");
    let other = yinz.attach_client(TERMINAL_SIZE);
    other.wait_until("other client ready", |s| s.contains("$"));
    other.send_stdin(DO);
    other.send_stdin(b"New session");
    other.wait_until("shared provider serves second client", |s| {
        s.contains("› New session…")
    });
    other.send_stdin(&keys::ENTER);
    other.wait_until("independent form", |s| {
        s.contains("Name (optional):") && !s.contains("private-first-client")
    });
    other.send_stdin(b"private-second-client");
    other.wait_until("second input", |s| s.contains("private-second-client"));
    assert!(yinz.snapshot().contains("private-first-client"));
    assert!(!yinz.snapshot().contains("private-second-client"));
    other.quit();
    yinz.quit();
}

#[test]
fn session_palette_notifications_and_cancel_first_disconnect_confirmation() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(
            "load_plugins clear-defaults=true { \"yinz:session-palette\" { shared true; }; }",
        )
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(PROMPT);
    yinz.wait_until("shell ready", |s| s.contains("$"));
    assert_eq!(
        yinz.run_cli_action(CliAction::Notify {
            text: "palette notification full message".into()
        }),
        0
    );
    palette_query(&yinz, "Notifications…");
    yinz.wait_until("notification listed", |s| {
        s.contains("› palette notification full message")
    });
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("notification detail", |s| {
        s.contains("Home/End") && s.contains("palette notification full message")
    });
    yinz.send_stdin(DO);
    yinz.wait_until("palette closed", |s| !s.contains("Home/End"));
    let other = yinz.attach_client(TERMINAL_SIZE);
    other.wait_until("other client ready", |s| s.contains("$"));
    palette_query(&yinz, "Disconnect other clients");
    yinz.wait_until("disconnection confirmation", |s| {
        s.contains("Cancel") && s.contains("processes keep running")
    });
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("cancel returned to root", |s| s.contains("Esc Close"));
    other.send_stdin(b"still connected");
    shell.wait_for_stdin("Cancel kept other client connected", |bytes| {
        bytes.ends_with(b"still connected")
    });
    other.quit();
    yinz.quit();
}

#[test]
fn session_palette_directory_picker_keeps_notifications_suspended_until_close() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(
            "load_plugins clear-defaults=true { \"yinz:session-palette\" { shared true; }; }\npane_animations false\nnotification_timeout 1",
        )
        .with_layout(LayoutInfo::Stringified(
            "layout { pane cwd=\"/tmp\"; }".into(),
        ))
        .start();
    yinz.expect_pty_spawn().output(PROMPT);
    yinz.wait_until("shell ready", |s| s.contains("$"));
    assert_eq!(
        yinz.run_cli_action(CliAction::Notify {
            text: "Retained picker history".into()
        }),
        0
    );
    yinz.wait_until("toast appears", |s| s.contains("Retained picker history"));
    yinz.wait_until("toast expires", |s| !s.contains("Retained picker history"));
    yinz.send_stdin(&keys::ctrl('o'));
    yinz.send_stdin(b"n");
    yinz.wait_until("history opens", |s| s.contains("Retained picker history"));
    palette_query(&yinz, "New session…");
    yinz.wait_until("name form", |s| s.contains("Name (optional):"));
    yinz.send_stdin(b"kept-name\r\x1b[B\r");
    yinz.wait_until("configuration form", |s| {
        s.contains("Directory:") && s.contains("kept-name")
    });
    yinz.send_stdin(b"\x1b[A\r");
    let picker = yinz.expect_pty_spawn();
    picker.output(b"PALETTE DIRECTORY PICKER");
    yinz.wait_until("picker owns screen", |s| {
        s.contains("PALETTE DIRECTORY PICKER")
            && !s.contains("Configure new session")
            && !s.contains("Retained picker history")
    });
    let picker_keys = b"\x1b[A\x1b[B\r x";
    yinz.send_stdin(picker_keys);
    picker.wait_for_stdin("picker receives navigation, Enter and Space", |bytes| {
        bytes.ends_with(picker_keys)
    });
    let Some(yinz_types::input::command::TerminalAction::RunCommand(command)) =
        picker.terminal_action()
    else {
        panic!("picker command")
    };
    let script = &command.args[1];
    let output = script
        .split_once("out='")
        .unwrap()
        .1
        .split_once('\'')
        .unwrap()
        .0;
    std::fs::write(output, "/tmp\0").unwrap();
    picker.exit(Some(0));
    yinz.wait_until("chosen directory and name preserved", |s| {
        s.contains("Directory: /tmp")
            && s.contains("kept-name")
            && !s.contains("Retained picker history")
    });
    yinz.send_stdin(&keys::ENTER);
    let cancelled = yinz.expect_pty_spawn();
    cancelled.exit(Some(130));
    yinz.wait_until("cancel preserves directory and name", |s| {
        s.contains("Directory: /tmp")
            && s.contains("kept-name")
            && !s.contains("Working")
            && !s.contains("Retained picker history")
    });
    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("back preserves initial name", |s| {
        s.contains("kept-name") && !s.contains("Directory:")
    });
    yinz.send_stdin(DO);
    yinz.wait_until("closing palette restores history", |s| {
        s.contains("Retained picker history") && !s.contains("Name (optional):")
    });
    yinz.quit();
}

#[test]
fn session_palette_finds_tabs_in_other_sessions() {
    session_palette_cross_session_destination("Find tab…");
}

#[test]
fn session_palette_finds_panes_in_other_sessions() {
    session_palette_cross_session_destination("Find pane…");
}

fn session_palette_cross_session_destination(command: &str) {
    // The in-process host's session scanner belongs to the first server.
    let mut source = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true { \"yinz:session-palette\" { shared true; }; }\ndisable_session_metadata false")
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    source.expect_pty_spawn().output(PROMPT);
    source.wait_until("source ready", |s| s.contains("$"));
    let mut destination = TestRunner::new(TERMINAL_SIZE)
        .skip_concurrency_slot()
        .with_config("load_plugins clear-defaults=true {}\ndisable_session_metadata false")
        .with_layout(LayoutInfo::Stringified("layout { tab name=\"first\" { pane; }; tab name=\"target-tab\" { pane name=\"cross-pane\"; }; }".into()))
        .start();
    destination.expect_pty_spawn().output(PROMPT);
    let target = destination.expect_pty_spawn();
    target.output(b"CROSS SESSION TARGET");
    destination.wait_until("destination ready", |s| s.contains("$"));
    // Production servers each have their own process environment.
    yinz_runtime::envs::set_session_name(yinz_types::internal_ids::SessionName::new(
        source.session_name(),
    ));
    palette_query(&source, command);
    source.wait_until("destination picker ready", |s| {
        s.contains("Esc Back") && !s.contains("Working")
    });
    // The lone pane lends its title to the tab, replacing the layout's name.
    source.send_stdin(b"cross-pane");
    source.wait_until("cross-session result", |s| {
        s.lines()
            .iter()
            .any(|line| line.contains('›') && line.contains("cross-pane"))
            && s.contains(destination.session_name())
            && !s.contains("Loading")
    });
    source.send_stdin(&keys::ENTER);
    let connection = source.take_session_switch();
    assert_eq!(
        connection.name.as_ref().map(|name| name.as_str()),
        Some(destination.session_name())
    );
    assert_eq!(
        connection.tab_position,
        Some(yinz_types::internal_ids::TabNumber::new(2))
    );
    if command == "Find pane…" {
        assert_eq!(
            connection.pane_id,
            Some(yinz_types::data::PaneId::Terminal(target.terminal_id()))
        );
    }
    source.quit();
    destination.quit();
}

#[test]
fn session_palette_confirms_killing_others_and_deleting_their_saved_state() {
    use yinz_types::input::actions::Action;

    // The in-process host's session scanner belongs to the first server.
    let mut source = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true { \"yinz:session-palette\" { shared true; }; }\ndisable_session_metadata false")
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let shell = source.expect_pty_spawn();
    shell.output(PROMPT);
    source.wait_until("source ready", |s| s.contains("$"));
    let mut destination = TestRunner::new(TERMINAL_SIZE)
        .skip_concurrency_slot()
        .with_config("load_plugins clear-defaults=true {}\ndisable_session_metadata false")
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let other = destination.expect_pty_spawn();
    other.output(PROMPT);
    destination.wait_until("destination ready", |s| s.contains("$"));
    assert_eq!(
        destination
            .run_actions_reporting(vec![Action::SaveSession])
            .0,
        0
    );
    destination.wait_for_serialized_session();
    yinz_runtime::envs::set_session_name(yinz_types::internal_ids::SessionName::new(
        source.session_name(),
    ));
    palette_query(&source, "Kill all other sessions…");
    source.wait_until("kill targets and consequences", |s| {
        s.contains(destination.session_name())
            && s.contains("Stops their processes")
            && s.contains("Cancel")
    });
    source.send_stdin(&keys::ENTER);
    source.wait_until("kill cancelled", |s| {
        s.contains("> Kill all other sessions…")
    });
    assert!(!other.is_exited());
    source.send_stdin(&keys::ENTER);
    source.wait_until("kill confirmation reopened", |s| {
        s.contains(destination.session_name())
            && s.contains("Stops their processes")
            && s.contains("Esc Back")
    });
    source.send_stdin(b"\x1b[B\r");
    destination.wait_for_exit();
    source.wait_until("kill completed", |s| {
        !s.contains("Esc Close") && !s.contains("Esc Back") && !s.contains("Working…")
    });
    assert!(!shell.is_exited(), "bulk kill excludes the current session");

    let saved = yinz_runtime::consts::YINZ_SESSION_INFO_CACHE_DIR.join(destination.session_name());
    assert!(saved.exists());
    palette_query(&source, "Delete all saved sessions…");
    source.wait_until("delete targets and consequences", |s| {
        s.contains(destination.session_name())
            && s.contains("Permanently removes saved state")
            && s.contains("Cancel")
    });
    source.send_stdin(&keys::ENTER);
    source.wait_until("deletion cancelled", |s| {
        s.contains("> Delete all saved sessions…")
    });
    assert!(saved.exists());
    source.send_stdin(&keys::ENTER);
    source.wait_until("delete confirmation reopened", |s| {
        s.contains(destination.session_name())
            && s.contains("Permanently removes saved state")
            && s.contains("Esc Back")
    });
    source.send_stdin(b"\x1b[B\r");
    source.wait_until("deletion acknowledged", |s| {
        // The pending view replaces Esc Back with Working… Esc Cancel before deletion runs.
        !s.contains("Esc Close") && !s.contains("Esc Back") && !s.contains("Working…")
    });
    assert!(!saved.exists());
    source.quit();
}

#[test]
fn session_palette_disconnect_targets_preserve_watchers() {
    let mut host = TestRunner::new(TERMINAL_SIZE)
        .with_config(
            "load_plugins clear-defaults=true { \"yinz:session-palette\" { shared true; }; }",
        )
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let shell = host.expect_pty_spawn();
    shell.output(PROMPT);
    host.wait_until("host ready", |s| s.contains("$"));
    let watcher = host.attach_watcher(TERMINAL_SIZE);
    watcher.wait_until("watcher connected", |s| s.contains("$"));
    palette_query(&host, "Disconnect other clients");
    host.wait_until("watchers excluded", |s| {
        s.contains("No other clients are connected")
    });
    host.send_stdin(&keys::ESC);
    let client = host.attach_client(TERMINAL_SIZE);
    client.wait_until("ordinary client connected", |s| s.contains("$"));
    palette_query(&host, "Disconnect other clients");
    host.wait_until("exact ordinary target", |s| {
        s.contains("Disconnect clients 3?") && s.contains("Cancel")
    });
    host.send_stdin(b"\x1b[B\r");
    host.wait_until("disconnect completed", |s| {
        !s.contains("Esc Back") && !s.contains("Esc Close")
    });
    client.wait_until_raw_output("ordinary client kicked", |bytes| {
        String::from_utf8_lossy(bytes).contains("Disconnected by host")
    });
    client.quit();
    assert!(!shell.is_exited());
    shell.output(b"\r\nwatcher-survived\r\n");
    watcher.wait_until("watcher remains connected", |s| {
        s.contains("watcher-survived")
    });
    watcher.quit();
    host.quit();
}

fn configuration_query(yinz: &yinz_integration_tests::TestSession, query: &str) {
    palette_query(yinz, "Configuration");
    yinz.wait_until("configuration submenu", |screen| {
        screen.contains("simplified ui…")
    });
    yinz.send_stdin(query.as_bytes());
    yinz.wait_until("configuration entry", |screen| {
        screen.contains(&format!("› {query}…"))
    });
    yinz.send_stdin(&keys::ENTER);
}

#[test]
fn configuration_palette_session_draft_can_be_saved_unchanged_permanently() {
    let config_dir = tempfile::tempdir().unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(config_dir.path().into())
        .with_config("pane_animations false\npane_frames true\npane_frame_style \"full\"\n")
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(PROMPT);
    yinz.wait_until("shell ready without a permission prompt", |screen| {
        screen.contains("$") && !screen.contains("Deny")
    });
    let second = yinz.attach_client(TERMINAL_SIZE);
    second.wait_until("second client sees the framed pane", |screen| {
        screen.contains("$") && screen.contains("┌")
    });
    configuration_query(&yinz, "pane frames");
    yinz.wait_until("configuration value editor", |screen| {
        screen.contains("Apply change")
    });
    yinz.send_stdin(b"\x1b[C");
    yinz.send_stdin(&keys::ENTER);
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("explicit session destination", |screen| {
        screen.contains("Apply to session") && screen.contains("Apply change")
    });
    assert!(!config_dir.path().join("config.d/yinz-managed.kdl").exists());
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("server acknowledged configuration edit", |screen| {
        screen.contains("Applied to this session")
    });
    second.wait_until("session edit reaches the second client", |screen| {
        screen.contains("$") && !screen.contains("┌")
    });
    let later = yinz.attach_client(TERMINAL_SIZE);
    later.wait_until("later client inherits the session edit", |screen| {
        screen.contains("$") && !screen.contains("┌")
    });
    assert!(!config_dir.path().join("config.d/yinz-managed.kdl").exists());
    configuration_query(&yinz, "pane frames");
    yinz.wait_until("effective value is disabled", |screen| {
        screen.contains("Disabled") && screen.contains("Apply change")
    });
    yinz.send_stdin(b"\x1b[B\x1b[B\x1b[B\x1b[B\r");
    yinz.wait_until("session override details", |screen| {
        screen.contains("Session override: Yes") && !screen.contains("Apply change")
    });
    yinz.send_stdin(DO);
    yinz.wait_until("details closed", |screen| {
        !screen.contains("Esc Back") && !screen.contains("Esc Close") && screen.contains("$")
    });
    configuration_query(&yinz, "pane frames");
    yinz.wait_until("unchanged session value ready to save", |screen| {
        screen.contains("Disabled") && screen.contains("Apply change")
    });
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("choose permanent destination", |screen| {
        screen.contains("Apply change")
    });
    yinz.send_stdin(b"\x1b[C");
    yinz.send_stdin(&keys::ENTER);
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("unchanged override saved", |screen| {
        screen.contains("Saved permanently")
    });
    let saved = yinz_config::config::Config::load(None, Some(config_dir.path()), false).unwrap();
    assert_eq!(saved.options.pane_frames, Some(false));
    assert_eq!(
        saved.options.pane_animations,
        yinz_config::config::Config::from_default_assets()
            .unwrap()
            .options
            .pane_animations
    );
    let managed =
        std::fs::read_to_string(config_dir.path().join("config.d/yinz-managed.kdl")).unwrap();
    assert!(managed.contains("pane_frames"));
    assert!(!managed.contains("pane_animations"));
    later.quit();
    second.quit();
    yinz.quit();
}

#[test]
fn configuration_palette_locked_default_routes_later_client_input_in_locked_mode() {
    let config_dir = tempfile::tempdir().unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(config_dir.path().into())
        .with_config("pane_animations false\ndefault_mode \"normal\"\n")
        .with_layout(LayoutInfo::Stringified(r#"layout { pane; pane size=2 borderless=true { plugin location="yinz:status-bar"; }; }"#.into()))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(PROMPT);
    yinz.wait_until("shell ready", |screen| screen.contains("$"));
    configuration_query(&yinz, "default mode");
    yinz.wait_until("default mode editor", |screen| {
        screen.contains("Apply change")
    });
    yinz.send_stdin(b"\x1b[C");
    yinz.send_stdin(&keys::ENTER);
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("session destination", |screen| {
        screen.contains("Apply change")
    });
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("locked default applied", |screen| {
        screen.contains("Applied to this session")
    });
    let later = yinz.attach_client(TERMINAL_SIZE);
    later.wait_until("later client displays Locked", |screen| {
        screen.contains("LOCKED")
    });
    later.send_stdin(&keys::ctrl('q'));
    later.send_stdin(b"still-locked");
    shell.wait_for_stdin("Ctrl q is input rather than Quit", |bytes| {
        bytes.ends_with(b"\x11still-locked")
    });
    assert!(!shell.is_exited());
    assert!(!config_dir.path().join("config.d/yinz-managed.kdl").exists());
    later.quit();
    yinz.quit();
}
