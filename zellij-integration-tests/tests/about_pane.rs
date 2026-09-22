#![cfg(unix)]

use zellij_integration_tests::{
    keys, FakePtyHandle, Size, TestRunner, TestSession, PROMPT, TERMINAL_SIZE,
};

const PIN_MARKER: &str = "PIN [ ]";

fn start_zellij_with_about(plugin_settings: &str) -> (TestSession, FakePtyHandle) {
    let zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(
            r#"
keybinds {
    normal {
        bind "Ctrl b" {
            LaunchOrFocusPlugin "zellij:about" { floating true; PLUGIN_SETTINGS }
        }
    }
}
"#
            .replace("PLUGIN_SETTINGS", plugin_settings)
            .as_str(),
        )
        .start();
    let terminal = zellij.expect_pty_spawn();
    terminal.output(PROMPT);
    zellij.wait_until("terminal ready", |grid| grid.contains("$ "));
    zellij.send_stdin(&keys::ctrl('b'));
    (zellij, terminal)
}

#[test]
fn hidden_about_stays_hidden_after_terminal_resize() {
    let (mut zellij, terminal) = start_zellij_with_about("");
    zellij.wait_until("About floating pane shown", |grid_snapshot| {
        grid_snapshot.contains(PIN_MARKER) && grid_snapshot.contains("Nested Sessions")
    });
    zellij.send_stdin(&keys::alt('f'));
    zellij.wait_until("floating panes hidden", |grid_snapshot| {
        !grid_snapshot.contains(PIN_MARKER) && grid_snapshot.status_bar_appears()
    });

    zellij.resize(Size {
        cols: 140,
        rows: 30,
    });
    terminal.wait_for_size(
        "terminal pane resized to the larger display",
        |cols, _rows| cols > 120,
    );
    zellij.wait_until("display settled at the new size", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
    });

    zellij.send_stdin(&keys::ctrl('p'));
    zellij.wait_until("pane mode entered after the resize", |grid_snapshot| {
        grid_snapshot
            .lines()
            .last()
            .is_some_and(|last_line| last_line.contains("Fullscreen"))
    });
    zellij.send_stdin(&keys::ESC);
    let settled = zellij.wait_until("normal mode restored after the resize", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
    });

    assert!(
        !settled.contains(PIN_MARKER),
        "hidden About floating pane must stay hidden after a terminal resize\n{}",
        settled.text,
    );
    zellij.quit();
}

#[test]
fn startup_without_tip_setting_has_no_tip_pane() {
    assert_startup_has_no_tip_pane("");
}

#[test]
fn legacy_tip_setting_cannot_enable_a_startup_pane() {
    assert_startup_has_no_tip_pane("show_startup_tips true");
}

fn assert_startup_has_no_tip_pane(config: &str) {
    let mut zellij = TestRunner::new(TERMINAL_SIZE).with_config(config).start();
    let terminal = zellij.expect_pty_spawn();
    terminal.output(PROMPT);
    let ready = zellij.wait_until("startup ready", |grid| {
        grid.contains("$ ") && grid.status_bar_appears()
    });
    assert!(!ready.contains(PIN_MARKER), "{}", ready.text);
    assert!(!ready.contains("About Zellij"), "{}", ready.text);
    zellij.quit();
}

#[test]
fn about_has_no_tips_browser() {
    assert_about_has_no_tips_browser("");
}

#[test]
fn legacy_plugin_setting_cannot_enable_tips() {
    assert_about_has_no_tips_browser("is_startup_tip true;");
}

fn assert_about_has_no_tips_browser(plugin_settings: &str) {
    let (mut zellij, _terminal) = start_zellij_with_about(plugin_settings);
    let about = zellij.wait_until("About content ready", |grid| {
        grid.contains("Nested Sessions") && grid.contains("Dismiss")
    });
    assert!(!about.contains("Usage Tips"), "{}", about.text);
    zellij.send_stdin(b"?");
    // Navigation is a content barrier after the question mark: it must still
    // select the first About item, rather than changing to a tip page.
    zellij.send_stdin(b"\x1b[B");
    zellij.send_stdin(&keys::ENTER);
    zellij.wait_until("About navigation still works", |grid| {
        grid.contains("Nested Sessions") && grid.contains("Go back")
    });
    zellij.quit();
}
