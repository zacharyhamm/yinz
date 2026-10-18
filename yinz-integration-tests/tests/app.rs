#![cfg(unix)]

use insta::assert_snapshot;
use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, col, keys, normalized,
    split_right_and_wait_for_prompt, Size, TestRunner, PROMPT, TERMINAL_SIZE,
};

const CHANGED_KEYS_CONFIG: &str = r#"
keybinds clear-defaults=true {
    normal {
        bind "F1" { SwitchToMode "Locked"; }
        bind "F2" { SwitchToMode "Pane"; }
        bind "F3" { SwitchToMode "Tab"; }
        bind "F4" { SwitchToMode "Resize"; }
        bind "F5" { SwitchToMode "Move"; }
        bind "F6" { SwitchToMode "Scroll"; }
        bind "Alt F7" { SwitchToMode "Session"; }
        bind "Ctrl F8" { Quit; }
    }
}
"#;

#[test]
fn exit_yinz() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE).start();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('q'));
    yinz.wait_until("quit confirmation appeared", |grid_snapshot| {
        grid_snapshot.contains("Quit Yinz?")
            && grid_snapshot.contains("Are you sure you want to quit?")
            && grid_snapshot.contains("Cancel")
    });

    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("quit confirmation was cancelled", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && !grid_snapshot.contains("Quit Yinz?")
    });

    yinz.send_stdin(&keys::ctrl('q'));
    yinz.wait_until("quit confirmation reappeared", |grid_snapshot| {
        grid_snapshot.contains("Quit Yinz?")
    });
    yinz.send_stdin(&keys::TAB);
    yinz.send_stdin(&keys::ENTER);

    yinz.wait_until("yinz said goodbye", |grid_snapshot| {
        !grid_snapshot.status_bar_appears() && grid_snapshot.contains("Bye from Yinz!")
    });
    yinz.wait_for_exit();
}

#[test]
fn resize_terminal_window() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE).start();
    let first_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    let right_terminal = split_right_and_wait_for_prompt(&yinz);

    yinz.resize(Size {
        cols: 100,
        rows: 24,
    });
    right_terminal.wait_for_size("right terminal reacted to window resize", |cols, _| {
        cols < (TERMINAL_SIZE.cols / 2) as u16
    });
    first_terminal.wait_for_size("left terminal reacted to window resize", |cols, _| {
        cols < (TERMINAL_SIZE.cols / 2) as u16
    });

    // The split went through Pane mode, and the status bar is a plugin that renders the mode
    // on its own time: the panes can be drawn at the new size while it still shows `PANE`.
    // So wait for its Normal-mode line, not for any status bar at all - at this width that
    // line is the modes' bare letters, and a mode's name on it is the Pane mode still.
    let grid_snapshot =
        yinz.wait_until("app re-rendered at the new window size", |grid_snapshot| {
            grid_snapshot.contains("Ctrl +")
                && !grid_snapshot.contains("PANE")
                && grid_snapshot.tab_bar_appears()
                && grid_snapshot.cursor_is_at(col(52).row(2))
        });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn resize_re_queries_pixel_sizes() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE).start();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.wait_until_raw_output("startup pixel size query", |bytes| {
        count_pixel_size_queries(bytes) == 1
    });

    yinz.resize(Size {
        cols: 100,
        rows: 24,
    });

    yinz.wait_until_raw_output("pixel size re-query after resize", |bytes| {
        count_pixel_size_queries(bytes) >= 2
    });
    yinz.quit();
}

fn count_pixel_size_queries(client_stdout_bytes: &[u8]) -> usize {
    let pixel_size_query = b"\x1b[14t\x1b[16t";
    client_stdout_bytes
        .windows(pixel_size_query.len())
        .filter(|window| window == pixel_size_query)
        .count()
}

#[test]
fn status_bar_loads_custom_keybindings() {
    let yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(CHANGED_KEYS_CONFIG)
        .start();
    let terminal = yinz.expect_pty_spawn();
    terminal.output(PROMPT);

    let grid_snapshot = yinz.wait_until(
        "status bar reflects the custom keybindings",
        |grid_snapshot| {
            grid_snapshot.cursor_is_at(col(2).row(1))
                && grid_snapshot.contains("LOCK")
                && grid_snapshot.contains("[0] Tab #1")
        },
    );
    assert_snapshot!(normalized(&grid_snapshot));
}

#[test]
fn use_custom_layout_with_relative_path() {
    let layout_dir = format!(
        "{}/../src/tests/fixtures/config-dirs/e2e-upside-down/layouts",
        env!("CARGO_MANIFEST_DIR")
    );
    let config = format!(
        "layout_dir \"{}\"\ndefault_layout \"upside-down\"\n",
        layout_dir
    );
    let mut yinz = TestRunner::new(TERMINAL_SIZE).with_config(&config).start();
    let terminal = yinz.expect_pty_spawn();
    terminal.output(PROMPT);

    let grid_snapshot = yinz.wait_until("upside-down layout loaded from disk", |grid_snapshot| {
        grid_snapshot.tab_bar_appears()
            && grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("Yinz (test")
            && grid_snapshot.cursor.is_some()
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn edit_scrollback() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE).start();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    terminal.output(b"SCROLLBACK_MARKER_ONE\r\nSCROLLBACK_MARKER_TWO\r\n");
    yinz.wait_until("scrollback content rendered", |grid_snapshot| {
        grid_snapshot.contains("SCROLLBACK_MARKER_TWO")
    });

    yinz.send_stdin(&keys::ctrl('s'));
    yinz.send_stdin(&keys::key('e'));
    yinz.expect_pty_spawn();

    let grid_snapshot = yinz.wait_until("scrollback editor pane open", |grid_snapshot| {
        grid_snapshot.contains("EDITING SCROLLBACK")
            && grid_snapshot.contains("SCROLLBACK_MARKER_ONE")
            && grid_snapshot.contains("SCROLLBACK_MARKER_TWO")
            && grid_snapshot.status_bar_appears()
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}
