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

/// Open an ordinary floating pane - no `--overlay` - in the tab's own area.
fn open_plain_floating_pane(zellij: &TestSession) -> zellij_integration_tests::FakePtyHandle {
    zellij.run_cli_action(CliAction::NewPane {
        direction: None,
        command: vec!["floating-cmd".to_string()],
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
        x: Some("20".to_string()),
        y: Some("5".to_string()),
        width: Some("30".to_string()),
        height: Some("6".to_string()),
        pinned: None,
        overlay: None,
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

/// SGR mouse reports. Coordinates are 1-based; a release is the same button with a lowercase
/// terminator, and a motion with the left button held sets the motion bit.
fn sgr_press(column: usize, line: usize) -> Vec<u8> {
    format!("\u{1b}[<0;{column};{line}M").into_bytes()
}
fn sgr_left_motion(column: usize, line: usize) -> Vec<u8> {
    format!("\u{1b}[<32;{column};{line}M").into_bytes()
}
fn sgr_release(column: usize, line: usize) -> Vec<u8> {
    format!("\u{1b}[<0;{column};{line}m").into_bytes()
}

/// The 0-based (column, row) of `needle` in the snapshot.
fn position_of(snapshot: &zellij_integration_tests::GridSnapshot, needle: &str) -> (usize, usize) {
    snapshot
        .text
        .lines()
        .enumerate()
        .find_map(|(row, line)| {
            line.find(needle)
                .map(|byte_index| (line[..byte_index].chars().count(), row))
        })
        .unwrap_or_else(|| panic!("{needle} is not on the screen:\n{}", snapshot.text))
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

/// A tab is laid out over what the global panes leave it, so a floating pane dragged with the mouse
/// used to be clamped to that area and stopped dead at the global pane's edge. No `--overlay` here:
/// an ordinary floating pane floats above the global panes just as it does above the tiled ones.
#[test]
fn a_plain_floating_pane_can_be_dragged_over_a_global_pane_with_the_mouse() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!("{NO_BACKGROUND_PLUGINS}mouse_mode true\n"))
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

    let floating = open_plain_floating_pane(&zellij);
    floating.output(b"DRAGME");
    let snapshot = zellij.wait_until("the floating pane is up", |snapshot| {
        snapshot.contains("DRAGME")
    });

    // the global pane takes the left half of a 120 column terminal
    let global_pane_ends_at = TERMINAL_SIZE.cols / 2;
    let (column, row) = position_of(&snapshot, "DRAGME");
    assert!(
        column >= global_pane_ends_at,
        "the pane starts out in the tab, right of the global pane, but it is at column {column}"
    );

    // press its top frame and drag left, past the tab's edge and onto the global pane
    let drag_to = column - 40;
    zellij.send_stdin(&sgr_press(column + 1, row));
    zellij.send_stdin(&sgr_left_motion(drag_to + 1, row));
    zellij.send_stdin(&sgr_release(drag_to + 1, row));

    zellij.wait_until("the pane is drawn over the global pane", |snapshot| {
        snapshot.contains("DRAGME") && position_of(snapshot, "DRAGME").0 < global_pane_ends_at
    });

    zellij.quit();
}

/// Open `quit-confirmation` as a plain floating *plugin* pane - no `--overlay`, no `modal`
/// configuration. Like every popup plugin in this fork it turns itself into an overlay from its
/// own `load()`, which is the two-stage birth the reveal bug lives in.
fn open_self_upgrading_popup(zellij: &TestSession) {
    zellij.run_cli_action(CliAction::NewPane {
        direction: None,
        command: vec![],
        remote_host: None,
        plugin: Some("zellij:quit-confirmation".to_string()),
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
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        overlay: None,
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
}

/// An overlay is not a floating pane, so putting one on screen must not put the user's hidden
/// floating panes on screen with it. This is the end-to-end shape of the "new agent" modal
/// revealing whatever was behind it.
///
/// The popup is launched here the way a config written before `overlay`/`modal` existed launches
/// one - as an ordinary floating pane, which reveals the layer - and only becomes an overlay once
/// the plugin is running. The layer has to go back down with it, however many panes are on it.
#[test]
fn a_self_upgrading_popup_does_not_reveal_a_hidden_floating_pane() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(NO_BACKGROUND_PLUGINS)
        .start();
    let shell = zellij.expect_pty_spawn();
    shell.output(b"tiled shell");
    shell.output(PROMPT);
    zellij.wait_until("the shell loads", |snapshot| {
        snapshot.contains("tiled shell")
    });

    let floating = open_plain_floating_pane(&zellij);
    floating.output(b"HIDDEN-FLOATING");
    zellij.wait_until("the floating pane is up", |snapshot| {
        snapshot.contains("HIDDEN-FLOATING")
    });

    zellij.run_cli_action(CliAction::ToggleFloatingPanes { tab_id: None });
    zellij.wait_until("the floating layer is down", |snapshot| {
        !snapshot.contains("HIDDEN-FLOATING")
    });

    open_self_upgrading_popup(&zellij);
    zellij.wait_until("the popup is up", |snapshot| {
        snapshot.contains("Quit Zellij?")
    });

    let snapshot = zellij.snapshot();
    assert!(
        !snapshot.contains("HIDDEN-FLOATING"),
        "the floating pane the user hid must stay hidden under the popup, got:\n{snapshot}"
    );

    zellij.quit();
}

/// The other half of `a_self_upgrading_popup_does_not_reveal_a_hidden_floating_pane`: closing the
/// popup has to put the floating layer back the way it found it. Opening the popup took the layer
/// down (it was already down here, but the popup's own arrival raised and re-hid it), so a close
/// that restores the layer would put the user's hidden pane back on screen without being asked.
#[test]
fn closing_a_popup_leaves_the_hidden_floating_layer_hidden() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(NO_BACKGROUND_PLUGINS)
        .start();
    let shell = zellij.expect_pty_spawn();
    shell.output(b"tiled shell");
    shell.output(PROMPT);
    zellij.wait_until("the shell loads", |snapshot| {
        snapshot.contains("tiled shell")
    });

    let floating = open_plain_floating_pane(&zellij);
    floating.output(b"HIDDEN-FLOATING");
    zellij.wait_until("the floating pane is up", |snapshot| {
        snapshot.contains("HIDDEN-FLOATING")
    });

    zellij.run_cli_action(CliAction::ToggleFloatingPanes { tab_id: None });
    zellij.wait_until("the floating layer is down", |snapshot| {
        !snapshot.contains("HIDDEN-FLOATING")
    });

    open_self_upgrading_popup(&zellij);
    zellij.wait_until("the popup is up", |snapshot| {
        snapshot.contains("Quit Zellij?")
    });

    zellij.send_stdin(&keys::ESC);
    zellij.wait_until("the popup is gone", |snapshot| {
        !snapshot.contains("Quit Zellij?")
    });

    let snapshot = zellij.snapshot();
    assert!(
        !snapshot.contains("HIDDEN-FLOATING"),
        "closing the popup must leave the floating layer as it found it, got:\n{snapshot}"
    );
    assert!(
        snapshot.contains("tiled shell"),
        "and the tiled pane underneath is back, got:\n{snapshot}"
    );

    zellij.quit();
}

/// An overlay covers the global panes as well as the tab, so while one is up the client's focus
/// is taken off whichever global pane held it and parked (`Screen::activate_overlay`). Closing the
/// overlay has to hand that focus back, or the user is left on a tab pane they never asked for.
#[test]
fn closing_a_popup_hands_focus_back_to_the_global_pane_that_had_it() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(NO_BACKGROUND_PLUGINS)
        .with_layout(LayoutInfo::Stringified(GLOBAL_AND_SHELL.to_string()))
        .start();
    let global = zellij.expect_pty_spawn();
    let shell = zellij.expect_pty_spawn();
    global.output(b"GLOBAL-PANE");
    global.output(PROMPT);
    shell.output(b"tiled shell");
    shell.output(PROMPT);
    zellij.wait_until("both panes load", |snapshot| {
        snapshot.contains("GLOBAL-PANE") && snapshot.contains("tiled shell")
    });

    // the layout focuses the global pane; prove it owns the keyboard before the popup opens
    global.output(b"before-popup");
    zellij.wait_until("the global pane has focus", |snapshot| {
        snapshot.contains("before-popup")
    });

    open_self_upgrading_popup(&zellij);
    zellij.wait_until("the popup is up", |snapshot| {
        snapshot.contains("Quit Zellij?")
    });

    zellij.send_stdin(&keys::ESC);
    zellij.wait_until("the popup is gone", |snapshot| {
        !snapshot.contains("Quit Zellij?")
    });

    // typing now must reach the global pane again, not the tab
    zellij.send_stdin(&keys::key('x'));
    global.wait_for_stdin(
        "the global pane has the keyboard back after the popup closes",
        |bytes| bytes.contains(&b'x'),
    );

    zellij.quit();
}
