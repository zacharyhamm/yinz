#![cfg(unix)]

use yinz_integration_tests::{keys, LayoutInfo, TestRunner, PROMPT, TERMINAL_SIZE};

#[test]
fn command_palette_receives_original_keys_before_write_bindings() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(
            r#"
            load_plugins clear-defaults=true {}
            keybinds {
                normal {
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
    yinz.send_stdin(&keys::ctrl('d'));
    yinz.wait_until("palette opened", |snapshot| {
        snapshot.contains("Open settings")
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
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(PROMPT);
    yinz.wait_until("shell ready", |snapshot| snapshot.contains("$"));
    let before = shell.stdin_bytes();
    yinz.send_stdin(&keys::ctrl('d'));
    yinz.wait_until("palette opened", |snapshot| {
        snapshot.contains("Open settings")
    });
    // Ctrl+p normally enters Pane mode; it must stay inside the palette.
    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(b"\x1b[200~New\npane\x1b[201~");
    yinz.wait_until("paste filtered", |snapshot| {
        snapshot.contains("> New pane") && !snapshot.contains("New tab")
    });
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("dummy selection acknowledged", |snapshot| {
        snapshot.contains("Selected: New pane")
    });
    assert_eq!(shell.stdin_bytes(), before);
    yinz.send_stdin(&keys::ctrl('d'));
    yinz.wait_until("toggle closed palette", |snapshot| {
        !snapshot.contains("Selected:") && !snapshot.contains("Esc Close")
    });
    yinz.send_stdin(b"still focused");
    shell.wait_for_stdin("typing reaches original pane", |bytes| {
        bytes.ends_with(b"still focused")
    });
    yinz.send_stdin(&keys::ctrl('d'));
    yinz.wait_until("reopened palette is empty", |snapshot| {
        snapshot.contains("Open settings") && snapshot.contains("New tab")
    });
    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("Escape closes palette", |snapshot| {
        !snapshot.contains("Open settings")
    });
    yinz.send_stdin(&keys::ctrl('g'));
    yinz.send_stdin(&keys::ctrl('d'));
    shell.wait_for_stdin("Ctrl+d still reaches the pane in Locked mode", |bytes| {
        bytes.ends_with(&[4])
    });
    yinz.quit();
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
                keybinds {{
                    {mode} {{
                        bind "Ctrl d" {{ ToggleCommandPalette; }}
                    }}
                }}
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
        yinz.send_stdin(&keys::ctrl('d'));
        yinz.wait_until("palette opened", |snapshot| {
            snapshot.contains("Open settings")
        });
        yinz.send_stdin(&keys::ESC);
        let before = yinz.wait_until("baseline after dismissal", |snapshot| {
            !snapshot.contains("Esc Close") && snapshot.tab_bar_appears()
        });
        let stdin_before = shell.stdin_bytes();
        yinz.send_stdin(&keys::ctrl('d'));
        yinz.wait_until("palette reopened", |snapshot| {
            snapshot.contains("Open settings")
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
        // Check the fallback again after dismissal, including ignored Pane-mode paste.
        yinz.send_stdin("\x1b[200~é\x1b[201~".as_bytes());
        if mode != "pane" {
            yinz.wait_until("rename/search paste resumes", |snapshot| {
                snapshot.contains("baseé")
            });
        }
        yinz.send_stdin(&keys::ctrl('d'));
        yinz.wait_until("palette still opens after fallback", |snapshot| {
            snapshot.contains("Open settings")
        });
        yinz.send_stdin(&keys::ESC);
        let after_fallback = yinz.wait_until("fallback settled", |snapshot| {
            !snapshot.contains("Esc Close") && snapshot.tab_bar_appears()
        });
        if mode == "pane" {
            yinz_integration_tests::assert_same_rendered_grid(
                &after_fallback,
                &before,
                "Pane-mode paste stays ignored",
            );
        }
        assert_eq!(shell.stdin_bytes(), stdin_before, "{mode}");
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
