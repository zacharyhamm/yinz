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
                    bind "Ctrl b" { SwitchToMode "Tmux"; }
                }
                normal { bind "Ctrl p" { SwitchToMode "Pane"; }; }
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
    yinz.wait_until("regular route wins over shorter Tmux route", |snapshot| {
        snapshot.contains("> Fullscreen") && snapshot.contains("Ctrl g → Ctrl p → f")
    });

    // No keyboard input or terminal output after opening and filtering the palette.
    for (kdl, hint, old_hint) in [
        (
            r#"keybinds { pane { unbind "f"; bind "x" { ToggleFocusFullscreen; }; }; }"#,
            "Ctrl g → Ctrl p → x",
            "Ctrl g → Ctrl p → f",
        ),
        (
            r#"keybinds { normal { unbind "Ctrl p"; bind "Ctrl o" { SwitchToMode "Pane"; }; }; }"#,
            "Ctrl g → Ctrl o → x",
            "Ctrl g → Ctrl p → x",
        ),
        (
            r#"keybinds { pane { unbind "x"; }; }"#,
            "Ctrl b → z",
            "Ctrl g → Ctrl o → x",
        ),
        (r#"keybinds { tmux { unbind "z"; }; }"#, "", "Ctrl b → z"),
        (
            r#"keybinds { locked { bind "Alt f" { ToggleFocusFullscreen; }; }; }"#,
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
        snapshot.contains("Close palette-child") && snapshot.contains("Cancel")
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
fn command_palette_focuses_a_registered_control_agent_across_tabs_and_removes_it_live() {
    use yinz_ipc::ipc::{ClientToServerMsg, IpcRecv, IpcSenderWithContext, ServerToClientMsg};
    use yinz_types::agent::control::{AgentOutcome, AgentRequest};
    use yinz_types::pane_id::PaneId;
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}\npane_animations false\nsession_serialization true")
        .with_layout(LayoutInfo::Stringified("layout { tab name=\"original\" focus=true { pane; }; tab name=\"controller\" { pane; }; }".into()))
        .start();
    let original = yinz.expect_pty_spawn();
    let controller = yinz.expect_pty_spawn();
    original.output(b"originalprompt$ ");
    controller.output(b"controllerprompt$ ");
    yinz.wait_until("original tab is ready", |snapshot| {
        snapshot.contains("originalprompt$")
    });
    let pane = PaneId::Terminal(controller.terminal_id());
    assert_eq!(
        yinz.run_cli_action(CliAction::AgentReport {
            pane_id: Some(pane.to_string()),
            kind: Some("codex".into()),
            state: Some("idle".into()),
            detail: None,
            cwd: None,
            hook_json: false,
            clear: false,
            run_id: None,
        }),
        0
    );
    let session_name = yinz.session_name().to_owned();
    let register = |enabled| {
        let socket =
            yinz_ipc::consts::ipc_connect(&yinz_runtime::consts::YINZ_SOCK_DIR.join(&session_name))
                .unwrap();
        let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(socket);
        let mut receiver = sender.get_receiver::<ServerToClientMsg>().unwrap();
        receiver
            .set_recv_timeout(Some(std::time::Duration::from_secs(30)))
            .unwrap();
        sender
            .send_client_msg(ClientToServerMsg::AgentControl(
                AgentRequest::SetControlAgent {
                    pane_id: pane,
                    enabled,
                },
            ))
            .unwrap();
        let response = loop {
            match receiver.recv_server_msg() {
                IpcRecv::Msg(ServerToClientMsg::AgentControl(response), _) => break response,
                IpcRecv::Msg(ServerToClientMsg::UnblockInputThread, _) => {},
                IpcRecv::Msg(other, _) => panic!("expected control-agent reply, got {other:?}"),
                _ => panic!("control-agent reply connection ended or timed out"),
            }
        };
        assert_eq!(response.outcome, AgentOutcome::Ok);
        sender
            .send_client_msg(ClientToServerMsg::ClientExited)
            .unwrap();
    };
    register(true);
    yinz.send_stdin(DO);
    yinz.wait_until("palette opened", |snapshot| {
        snapshot.contains("Switch session")
    });
    yinz.send_stdin(b"Focus control agent");
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("control-agent picker opened", |snapshot| {
        snapshot.contains("Focus control agent") && snapshot.contains(&pane.to_string())
    });
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("controller tab focused", |snapshot| {
        snapshot.contains("controllerprompt$") && !snapshot.contains("Esc Close")
    });
    yinz.send_stdin(b"control-probe");
    controller.wait_for_stdin("input reaches the controller", |bytes| {
        bytes
            .windows(b"control-probe".len())
            .any(|part| part == b"control-probe")
    });
    yinz.send_stdin(DO);
    yinz.wait_until("palette reopened", |snapshot| {
        snapshot.contains("Switch session")
    });
    yinz.send_stdin(b"Focus control agent");
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("control-agent picker reopened", |snapshot| {
        snapshot.contains(&pane.to_string()) && snapshot.contains("Focus control agent")
    });
    assert_eq!(
        yinz.run_cli_action(CliAction::AgentReport {
            pane_id: Some(pane.to_string()),
            kind: Some("codex".into()),
            state: None,
            detail: None,
            cwd: None,
            hook_json: false,
            clear: true,
            run_id: None,
        }),
        0
    );
    yinz.wait_until("cleared agent disappears without input", |snapshot| {
        snapshot.contains("No control agents")
    });
    assert_eq!(
        yinz.run_cli_action(CliAction::AgentReport {
            pane_id: Some(pane.to_string()),
            kind: Some("codex".into()),
            state: Some("idle".into()),
            detail: None,
            cwd: None,
            hook_json: false,
            clear: false,
            run_id: None,
        }),
        0
    );
    register(true);
    yinz.save_session();
    yinz.wait_for_serialized_session();
    let checkpoint = yinz_runtime::session_checkpoint::SessionCheckpoint::read(
        &yinz_runtime::consts::YINZ_SESSION_INFO_CACHE_DIR.join(&session_name),
    )
    .unwrap();
    let metadata = std::fs::read_to_string(checkpoint.metadata_path()).unwrap();
    assert!(metadata.contains("control_agents"));
    yinz.quit();
    yinz.resurrect(TERMINAL_SIZE);
    for _ in 0..2 {
        yinz.expect_pty_spawn().output(b"restoredprompt$ ");
    }
    yinz.wait_until("restored session accepts input", |snapshot| {
        snapshot.contains("restoredprompt$")
    });
    palette_query(&yinz, "Focus control agent");
    yinz.wait_until("resurrection does not restore registrations", |snapshot| {
        snapshot.contains("No control agents")
    });
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
