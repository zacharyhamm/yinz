#![cfg(unix)]

//! An overlay - a pane placed against the whole session surface, above a tab's own panes, rather
//! than on a tab's floating layer - used to be reachable only by the built-in plugins that called
//! the low-level floating-open shims directly (`file-finder`; see
//! `yinz-integration-tests/tests/file_finder.rs`). It is a placement on the
//! `yinz action` CLI surface now: `--overlay`, or `--modal` for one that keeps focus.
//!
//! This file exercises that generic path with a plain command pane (not a plugin), and guards the
//! invariants the plugin-specific tests already cover for their own panes: an overlay is gone from
//! the screen once its tab is left and back once its tab is focused again, and putting one on
//! screen never reveals the floating panes the user had hidden.

use yinz_cli::args::CliAction;
use yinz_integration_tests::{keys, LayoutInfo, TestRunner, TestSession, PROMPT, TERMINAL_SIZE};
use yinz_types::internal_ids::TabNumber;

/// Keep the session to just the shell pane: the default background plugins would
/// otherwise be spawning work of their own while the test watches for pty spawns.
const NO_BACKGROUND_PLUGINS: &str = "load_plugins clear-defaults=true {\n}\n";

const TWO_TILED_PANES: &str = r#"layout {
    pane split_direction="vertical" {
        pane focus=true
        pane
    }
}
"#;

/// Open a plain command pane as an overlay, in the bottom-right corner so that it covers none
/// of the shell text the tests wait for (a shell prints at the top-left of its pane).
fn open_overlay_pane(yinz: &TestSession) -> yinz_integration_tests::FakePtyHandle {
    yinz.run_cli_action(CliAction::NewPane {
        direction: None,
        command: vec!["overlay-cmd".to_string()],
        remote_host: None,
        firecracker: None,
        firecracker_image: None,
        firecracker_worktree: None,
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
        configuration_json: None,
        skip_plugin_cache: false,
        x: Some("75%".to_string()),
        y: Some("75%".to_string()),
        width: Some("20%".to_string()),
        height: Some("20%".to_string()),
        pinned: None,
        overlay: Some(true),
        modal: None,
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
    });
    yinz.expect_pty_spawn()
}

/// Open a plain command pane as a *modal* overlay - `--modal` rather than `--overlay`.
fn open_modal_pane(yinz: &TestSession) -> yinz_integration_tests::FakePtyHandle {
    yinz.run_cli_action(CliAction::NewPane {
        direction: None,
        command: vec!["modal-cmd".to_string()],
        remote_host: None,
        firecracker: None,
        firecracker_image: None,
        firecracker_worktree: None,
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
        configuration_json: None,
        skip_plugin_cache: false,
        x: Some("0%".to_string()),
        y: Some("0%".to_string()),
        width: Some("20%".to_string()),
        height: Some("20%".to_string()),
        pinned: None,
        overlay: None,
        modal: Some(true),
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
    });
    yinz.expect_pty_spawn()
}

/// Open an ordinary floating pane - no `--overlay` - in the tab's own area.
fn open_plain_floating_pane(yinz: &TestSession) -> yinz_integration_tests::FakePtyHandle {
    yinz.run_cli_action(CliAction::NewPane {
        direction: None,
        command: vec!["floating-cmd".to_string()],
        remote_host: None,
        firecracker: None,
        firecracker_image: None,
        firecracker_worktree: None,
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
        configuration_json: None,
        skip_plugin_cache: false,
        x: Some("20".to_string()),
        y: Some("5".to_string()),
        width: Some("30".to_string()),
        height: Some("6".to_string()),
        pinned: None,
        overlay: None,
        modal: None,
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
    });
    yinz.expect_pty_spawn()
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
fn position_of(snapshot: &yinz_integration_tests::GridSnapshot, needle: &str) -> (usize, usize) {
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
/// its cells land on the pane beside it, exactly like the finder/popup overlays. It must not
/// keep rendering once its tab is left, and must come back once its tab is focused again.
#[test]
fn an_overlay_floating_pane_is_gone_from_another_tab() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(NO_BACKGROUND_PLUGINS)
        .with_layout(LayoutInfo::Stringified(TWO_TILED_PANES.to_owned()))
        .start();
    let left = yinz.expect_pty_spawn();
    left.output(b"LEFT-PANE");
    let shell = yinz.expect_pty_spawn();
    shell.output(b"first tab shell");
    shell.output(PROMPT);
    yinz.wait_until("both panes load", |snapshot| {
        snapshot.contains("LEFT-PANE") && snapshot.contains("first tab shell")
    });

    // Tab mode, then `n` for a new tab - which applies the same two-pane layout.
    yinz.send_stdin(&keys::ctrl('t'));
    yinz.send_stdin(b"n");
    let second_left = yinz.expect_pty_spawn();
    second_left.output(b"second tab left");
    let second_shell = yinz.expect_pty_spawn();
    second_shell.output(b"second tab shell");
    second_shell.output(PROMPT);
    yinz.wait_until("the second tab is up", |snapshot| {
        snapshot.contains("second tab shell")
    });
    yinz.send_stdin(&keys::ctrl('t'));
    yinz.send_stdin(b"h");
    yinz.wait_until("the first tab is back", |snapshot| {
        snapshot.contains("first tab shell")
    });

    let overlay = open_overlay_pane(&yinz);
    overlay.output(b"OVERLAY-CONTENT");
    yinz.wait_until(
        "the overlay pane is drawing over the pane beside it",
        |snapshot| snapshot.contains("OVERLAY-CONTENT"),
    );

    yinz.run_cli_action(CliAction::GoToTab {
        index: TabNumber::new(2),
    });
    yinz.wait_until("the second tab is in view again", |snapshot| {
        snapshot.contains("second tab shell")
    });

    let snapshot = yinz.snapshot();
    assert!(
        !snapshot.contains("OVERLAY-CONTENT"),
        "the overlay pane belongs to the tab that was left behind, got:\n{snapshot}"
    );

    yinz.run_cli_action(CliAction::GoToTab {
        index: TabNumber::new(1),
    });
    yinz.wait_until("the overlay pane is back with its tab", |snapshot| {
        snapshot.contains("OVERLAY-CONTENT")
    });

    yinz.quit();
}

/// Open `quit-confirmation` the way `Action::Quit` does: born a modal on the session surface,
/// with the geometry the plugin then keeps for itself. It is never on the tab's floating layer.
fn open_popup(yinz: &TestSession) {
    yinz.run_cli_action(CliAction::NewPane {
        direction: None,
        command: vec![],
        remote_host: None,
        firecracker: None,
        firecracker_image: None,
        firecracker_worktree: None,
        plugin: Some("yinz:quit-confirmation".to_string()),
        cwd: None,
        floating: true,
        in_place: false,
        close_replaced_pane: false,
        pane_id: None,
        name: None,
        close_on_exit: false,
        start_suspended: false,
        configuration: None,
        configuration_json: None,
        skip_plugin_cache: false,
        x: Some("30%".to_string()),
        y: Some("30%".to_string()),
        width: Some("40%".to_string()),
        height: Some("8".to_string()),
        pinned: None,
        overlay: None,
        modal: Some(true),
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
    });
}

/// The same drag on a *plugin* modal, which is what the popups in this fork actually are.
///
/// Worth its own test rather than trusting the command-pane one: the unit suite's plugin panes are
/// mocks that never render, so the whole plugin-output path is untested there - which is how an
/// earlier round of this work shipped popups that drew an empty box. And a modal is the sharper
/// case: it must keep the keyboard across the drag, so it is still the thing `Esc` closes when the
/// pointer lets go.
#[test]
fn a_modal_plugin_popup_can_be_dragged_with_the_mouse() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!("{NO_BACKGROUND_PLUGINS}mouse_mode true\n"))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(b"tiled shell");
    shell.output(PROMPT);
    yinz.wait_until("the shell loads", |snapshot| {
        snapshot.contains("tiled shell")
    });

    open_popup(&yinz);
    let snapshot = yinz.wait_until("the popup is up", |snapshot| {
        snapshot.contains("Quit Yinz?")
    });
    let (column, row) = position_of(&snapshot, "Quit Yinz?");

    // SGR reports are 1-based, so passing the 0-based content row lands one row higher - on the
    // popup's top frame. The floating drag above aims the same way.
    let drag_to_row = row + 3;
    yinz.send_stdin(&sgr_press(column + 1, row));
    yinz.send_stdin(&sgr_left_motion(column + 1, drag_to_row));
    yinz.send_stdin(&sgr_release(column + 1, drag_to_row));

    yinz.wait_until("the popup has moved down", |snapshot| {
        snapshot.contains("Quit Yinz?") && position_of(snapshot, "Quit Yinz?").1 > row
    });

    // a drag never moves the keyboard, so the modal is still what Esc closes
    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("the popup is still the focused modal", |snapshot| {
        !snapshot.contains("Quit Yinz?")
    });

    yinz.quit();
}

/// An overlay is not a floating pane, so putting one on screen must not put the user's hidden
/// floating panes on screen with it. This is the end-to-end shape of the "new agent" modal
/// revealing whatever was behind it.
///
/// The reveal used to come from the popup's own birth: it opened as an ordinary floating pane,
/// which raises the layer, and only became an overlay once the plugin was running. A pane born an
/// overlay never touches the layer, so there is nothing to put back - but the guarantee the user
/// sees is the same one, and it is what this asserts.
#[test]
fn a_popup_does_not_reveal_a_hidden_floating_pane() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(NO_BACKGROUND_PLUGINS)
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(b"tiled shell");
    shell.output(PROMPT);
    yinz.wait_until("the shell loads", |snapshot| {
        snapshot.contains("tiled shell")
    });

    let floating = open_plain_floating_pane(&yinz);
    floating.output(b"HIDDEN-FLOATING");
    yinz.wait_until("the floating pane is up", |snapshot| {
        snapshot.contains("HIDDEN-FLOATING")
    });

    yinz.run_cli_action(CliAction::ToggleFloatingPanes { tab_id: None });
    yinz.wait_until("the floating layer is down", |snapshot| {
        !snapshot.contains("HIDDEN-FLOATING")
    });

    open_popup(&yinz);
    yinz.wait_until("the popup is up", |snapshot| {
        snapshot.contains("Quit Yinz?")
    });

    let snapshot = yinz.snapshot();
    assert!(
        !snapshot.contains("HIDDEN-FLOATING"),
        "the floating pane the user hid must stay hidden under the popup, got:\n{snapshot}"
    );

    yinz.quit();
}

/// The other half of `a_popup_does_not_reveal_a_hidden_floating_pane`: closing the popup has to
/// leave the floating layer where it found it. The popup was never on that layer, so a close that
/// raised it would be putting the user's hidden pane back on screen without being asked.
#[test]
fn closing_a_popup_leaves_the_hidden_floating_layer_hidden() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(NO_BACKGROUND_PLUGINS)
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(b"tiled shell");
    shell.output(PROMPT);
    yinz.wait_until("the shell loads", |snapshot| {
        snapshot.contains("tiled shell")
    });

    let floating = open_plain_floating_pane(&yinz);
    floating.output(b"HIDDEN-FLOATING");
    yinz.wait_until("the floating pane is up", |snapshot| {
        snapshot.contains("HIDDEN-FLOATING")
    });

    yinz.run_cli_action(CliAction::ToggleFloatingPanes { tab_id: None });
    yinz.wait_until("the floating layer is down", |snapshot| {
        !snapshot.contains("HIDDEN-FLOATING")
    });

    open_popup(&yinz);
    yinz.wait_until("the popup is up", |snapshot| {
        snapshot.contains("Quit Yinz?")
    });

    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("the popup is gone", |snapshot| {
        !snapshot.contains("Quit Yinz?")
    });

    let snapshot = yinz.snapshot();
    assert!(
        !snapshot.contains("HIDDEN-FLOATING"),
        "closing the popup must leave the floating layer as it found it, got:\n{snapshot}"
    );
    assert!(
        snapshot.contains("tiled shell"),
        "and the tiled pane underneath is back, got:\n{snapshot}"
    );

    yinz.quit();
}

/// `--modal` is `--overlay` plus "do not take focus from it". It reaches the tab as an overlay
/// placement rather than as a coordinates flag, so it works for a plain command pane - modal-ness
/// used to be spelled only in a plugin's own configuration, which a terminal pane does not have.
#[test]
fn a_modal_command_pane_is_placed_on_the_surface_and_keeps_focus() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(NO_BACKGROUND_PLUGINS)
        .with_layout(LayoutInfo::Stringified(TWO_TILED_PANES.to_string()))
        .start();
    let left = yinz.expect_pty_spawn();
    let shell = yinz.expect_pty_spawn();
    left.output(b"LEFT-PANE");
    left.output(PROMPT);
    shell.output(b"tiled shell");
    shell.output(PROMPT);
    yinz.wait_until("both panes load", |snapshot| {
        snapshot.contains("LEFT-PANE") && snapshot.contains("tiled shell")
    });

    let modal = open_modal_pane(&yinz);
    modal.output(b"MODAL-PANE");
    yinz.wait_until("the modal is up", |snapshot| {
        snapshot.contains("MODAL-PANE")
    });

    // it is on the surface, not the tab: it starts left of where the tab's own area begins
    let snapshot = yinz.snapshot();
    let (_, column) = position_of(&snapshot, "MODAL-PANE");
    assert!(
        column < TERMINAL_SIZE.cols / 2,
        "a surface-placed pane at x=0% starts left of the tab area, which begins at 50%; \
         got column {column} in:\n{snapshot}"
    );

    // and it holds the keyboard
    yinz.send_stdin(&keys::key('x'));
    modal.wait_for_stdin("the modal has the keyboard", |bytes| bytes.contains(&b'x'));

    yinz.quit();
}

/// An overlay belongs to a tab, but it is drawn against the whole session surface and holds
/// state no other pane does - agent keeps a whole conversation in one. Closing the tab it
/// happens to be sitting on used to take it down with the tab, because a tab's teardown names
/// every pane in it. It has to move to the tab the user lands on instead.
#[test]
fn an_overlay_floating_pane_moves_to_another_tab_when_its_own_tab_closes() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(NO_BACKGROUND_PLUGINS)
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(b"first tab shell");
    shell.output(PROMPT);
    yinz.wait_until("the first tab's shell loads", |snapshot| {
        snapshot.contains("first tab shell")
    });

    yinz.send_stdin(&keys::ctrl('t'));
    yinz.send_stdin(b"n");
    let second_shell = yinz.expect_pty_spawn();
    second_shell.output(b"second tab shell");
    second_shell.output(PROMPT);
    yinz.wait_until("the second tab is up", |snapshot| {
        snapshot.contains("second tab shell")
    });

    let overlay = open_overlay_pane(&yinz);
    overlay.output(b"OVERLAY-CONTENT");
    yinz.wait_until("the overlay is up on the second tab", |snapshot| {
        snapshot.contains("OVERLAY-CONTENT")
    });

    yinz.run_cli_action(CliAction::CloseTab { tab_id: None });

    yinz.wait_until("the first tab is back", |snapshot| {
        snapshot.contains("first tab shell")
    });
    let snapshot = yinz.snapshot();
    assert!(
        snapshot.contains("OVERLAY-CONTENT"),
        "the overlay must follow the user to the tab they landed on rather than closing with \
         its own, got:\n{snapshot}"
    );

    yinz.quit();
}
