#![cfg(unix)]

use insta::assert_snapshot;
use zellij_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, keys, normalized, start_zellij, GridSnapshot,
    TestSession,
};

const ARROW_DOWN: &[u8] = b"\x1b[B";

fn open_presets_screen(zellij: &TestSession) {
    zellij.wait_until("configuration plugin opened", |grid_snapshot| {
        grid_snapshot.contains("Configuration")
    });
    zellij.send_stdin(&keys::TAB);
    zellij.wait_until("presets screen opened", |grid_snapshot| {
        grid_snapshot.contains("1. Default") && grid_snapshot.contains("2. Unlock First")
    });
}

fn apply_selected_preset_and_close(
    zellij: &TestSession,
    times_to_move_down: usize,
    applied_marker: &str,
) {
    for _ in 0..times_to_move_down {
        zellij.send_stdin(ARROW_DOWN);
    }
    zellij.send_stdin(&keys::ENTER);
    let applied_marker = applied_marker.to_owned();
    zellij.wait_until("preset applied to the running session", |grid_snapshot| {
        grid_snapshot.contains(&applied_marker)
    });
    zellij.send_stdin(&keys::ctrl('c'));
    zellij.send_stdin(&keys::ctrl('c'));
    zellij.wait_until("configuration plugin closed", |grid_snapshot| {
        !grid_snapshot.contains("Configuration")
    });
}

fn wait_for_unlock_first_status_bar(zellij: &TestSession) -> GridSnapshot {
    zellij.wait_until(
        "status bar groups keys the unlock-first way",
        |grid_snapshot| grid_snapshot.contains("PANE") && !grid_snapshot.contains("Ctrl +"),
    )
}

fn wait_for_default_status_bar(zellij: &TestSession) -> GridSnapshot {
    zellij.wait_until("status bar groups keys the default way", |grid_snapshot| {
        grid_snapshot.contains("PANE") && grid_snapshot.contains("Ctrl +")
    })
}

/// Switching keybinding presets reconfigures the running session: the status bar must pick up
/// the new base mode and keybinds in the existing tab, without a new tab being opened to force a
/// fresh ModeUpdate.
///
/// This guards two races in the plugin host: the InitialKeybinds sent on reconfiguration used to
/// read the keybinds back from the plugin's env before the executor had updated it, and a
/// ModeUpdate that already named the new base mode used to have it overwritten with the plugin's
/// stale default mode.
#[test]
fn switching_keybind_presets_updates_the_status_bar_without_a_new_tab() {
    let mut zellij = start_zellij();
    claim_first_terminal_and_wait_for_prompt(&zellij);

    zellij.send_stdin(&keys::ctrl('o'));
    zellij.send_stdin(&keys::key('c'));
    open_presets_screen(&zellij);
    apply_selected_preset_and_close(&zellij, 2, "UNLOCK");

    // Ctrl+g starts a timed prefix in Locked mode. Closing the configuration pane also
    // updates passthrough state, which can clear that pending prefix before it expires.
    // This test observes preset delivery; use an acknowledged mode change to reach its view.
    zellij.run_cli_action(zellij_cli::args::CliAction::SwitchMode {
        input_mode: zellij_types::data::InputMode::Normal,
    });
    let grid_snapshot = wait_for_unlock_first_status_bar(&zellij);
    assert_snapshot!(normalized(&grid_snapshot));

    zellij.send_stdin(&keys::key('o'));
    zellij.send_stdin(&keys::key('c'));
    open_presets_screen(&zellij);
    apply_selected_preset_and_close(&zellij, 1, "Ctrl +");

    let grid_snapshot = wait_for_default_status_bar(&zellij);
    assert_snapshot!(normalized(&grid_snapshot));

    zellij.quit();
}
