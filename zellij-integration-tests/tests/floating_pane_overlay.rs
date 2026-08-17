#![cfg(unix)]

//! The `overlay` flag on `FloatingPaneCoordinates` (position a floating pane against the
//! whole session surface, above global panes) used to be reachable only by the two
//! built-in plugins that called the low-level floating-open shims directly
//! (`hocus-ask`, `file-finder`; see `zellij-integration-tests/tests/hocus_ask.rs` and
//! `file_finder.rs`). It is now threaded through the `zellij action` CLI surface as
//! well, so an ordinary floating pane opened with `--overlay` gets the same treatment.
//!
//! This test exercises that generic path with a plain command pane (not a plugin), and
//! guards the invariant the plugin-specific tests already cover for their own panes:
//! an overlay pane must still be gone from the screen once its tab is left, and back
//! once its tab is focused again.

use zellij_integration_tests::{keys, LayoutInfo, TestRunner, TestSession, PROMPT, TERMINAL_SIZE};
use zellij_utils::cli::CliAction;

/// Keep the session to just the shell pane: the default background plugins would
/// otherwise be spawning work of their own while the test watches for pty spawns.
const NO_BACKGROUND_PLUGINS: &str = "load_plugins clear-defaults=true {\n}\n";

const GLOBAL_AND_SHELL: &str = r#"layout {
    global_panes {
        pane edge="left" size="50%" focus=true
    }
    pane
}
"#;

/// Open a plain command pane as an overlay, positioned over the global pane's strip.
fn open_overlay_pane(zellij: &TestSession) -> zellij_integration_tests::FakePtyHandle {
    zellij.run_cli_action(CliAction::NewPane {
        direction: None,
        command: vec!["overlay-cmd".to_string()],
        remote_host: None,
        plugin: None,
        cwd: None,
        floating: true,
        in_place: false,
        close_replaced_pane: false,
        pane_id: None,
        name: None,
        close_on_exit: false,
        start_suspended: false,
        configuration: None,
        skip_plugin_cache: false,
        x: Some("0%".to_string()),
        y: Some("0%".to_string()),
        width: Some("20%".to_string()),
        height: Some("20%".to_string()),
        pinned: None,
        overlay: Some(true),
        stacked: false,
        blocking: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        tab_id: None,
        global: false,
        edge: None,
        size: None,
    });
    zellij.expect_pty_spawn()
}

/// A plain floating pane opened with `--overlay` is placed against the whole surface, so
/// its cells land on the global pane, exactly like the finder/popup overlays. It must not
/// keep rendering once its tab is left, and must come back once its tab is focused again.
#[test]
fn an_overlay_floating_pane_is_gone_from_another_tab() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(NO_BACKGROUND_PLUGINS)
        .with_layout(LayoutInfo::Stringified(GLOBAL_AND_SHELL.to_owned()))
        .start();
    let global = zellij.expect_pty_spawn();
    global.output(b"GLOBAL");
    let shell = zellij.expect_pty_spawn();
    shell.output(b"first tab shell");
    shell.output(PROMPT);
    zellij.wait_until("the global pane and shell load", |snapshot| {
        snapshot.contains("GLOBAL") && snapshot.contains("first tab shell")
    });

    // Tab mode, then `n` for a new tab - which spawns a shell of its own.
    zellij.send_stdin(&keys::ctrl('t'));
    zellij.send_stdin(b"n");
    let second_shell = zellij.expect_pty_spawn();
    second_shell.output(b"second tab shell");
    second_shell.output(PROMPT);
    zellij.wait_until("the second tab is up", |snapshot| {
        snapshot.contains("second tab shell")
    });
    zellij.send_stdin(&keys::ctrl('t'));
    zellij.send_stdin(b"h");
    zellij.wait_until("the first tab is back", |snapshot| {
        snapshot.contains("first tab shell")
    });

    let overlay = open_overlay_pane(&zellij);
    overlay.output(b"OVERLAY-CONTENT");
    zellij.wait_until(
        "the overlay pane is drawing over the global pane",
        |snapshot| snapshot.contains("OVERLAY-CONTENT"),
    );

    zellij.run_cli_action(CliAction::GoToTab { index: 2 });
    zellij.wait_until("the second tab is in view again", |snapshot| {
        snapshot.contains("second tab shell")
    });

    let snapshot = zellij.snapshot();
    assert!(
        !snapshot.contains("OVERLAY-CONTENT"),
        "the overlay pane belongs to the tab that was left behind, got:\n{snapshot}"
    );

    zellij.run_cli_action(CliAction::GoToTab { index: 1 });
    zellij.wait_until("the overlay pane is back with its tab", |snapshot| {
        snapshot.contains("OVERLAY-CONTENT")
    });

    zellij.quit();
}
