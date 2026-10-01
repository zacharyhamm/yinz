#![cfg(unix)]

use yinz_integration_tests::{keys, Size, TestRunner, PROMPT, TERMINAL_SIZE};

const PIN_MARKER: &str = "PIN [ ]";

#[test]
fn hidden_configuration_stays_hidden_after_terminal_resize() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(
            r#"
keybinds {
    normal {
        bind "Ctrl b" {
            LaunchOrFocusPlugin "yinz:configuration" { floating true; }
        }
    }
}
"#,
        )
        .start();
    let terminal = yinz.expect_pty_spawn();
    terminal.output(PROMPT);
    yinz.wait_until("terminal ready", |grid| grid.contains("$ "));
    yinz.send_stdin(&keys::ctrl('b'));
    yinz.wait_until("Configuration floating pane shown", |grid_snapshot| {
        grid_snapshot.contains(PIN_MARKER) && grid_snapshot.contains("Configuration")
    });
    yinz.send_stdin(&keys::alt('f'));
    yinz.wait_until("floating panes hidden", |grid_snapshot| {
        !grid_snapshot.contains(PIN_MARKER) && grid_snapshot.status_bar_appears()
    });

    yinz.resize(Size {
        cols: 140,
        rows: 30,
    });
    terminal.wait_for_size(
        "terminal pane resized to the larger display",
        |cols, _rows| cols > 120,
    );
    yinz.wait_until("display settled at the new size", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
    });

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.wait_until("pane mode entered after the resize", |grid_snapshot| {
        grid_snapshot
            .lines()
            .last()
            .is_some_and(|last_line| last_line.contains("Fullscreen"))
    });
    yinz.send_stdin(&keys::ESC);
    let settled = yinz.wait_until("normal mode restored after the resize", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
    });

    assert!(
        !settled.contains(PIN_MARKER),
        "hidden Configuration floating pane must stay hidden after a terminal resize\n{}",
        settled.text,
    );
    yinz.quit();
}

#[test]
fn startup_has_no_unexpected_floating_pane() {
    assert_startup_has_no_floating_pane("");
}

#[test]
fn legacy_startup_options_cannot_enable_a_floating_pane() {
    for config in [
        "show_startup_tips true",
        "show_release_notes true",
        "show_release_notes false",
    ] {
        assert_startup_has_no_floating_pane(config);
    }
}

fn assert_startup_has_no_floating_pane(config: &str) {
    let mut yinz = TestRunner::new(TERMINAL_SIZE).with_config(config).start();
    let terminal = yinz.expect_pty_spawn();
    terminal.output(PROMPT);
    let ready = yinz.wait_until("startup ready", |grid| {
        grid.contains("$ ") && grid.status_bar_appears()
    });
    assert!(!ready.contains(PIN_MARKER), "{}", ready.text);
    yinz.quit();
}
