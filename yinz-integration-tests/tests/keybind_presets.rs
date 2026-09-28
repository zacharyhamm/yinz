#![cfg(unix)]

use insta::assert_snapshot;
use yinz_cli::args::CliAction;
use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, keys, normalized, start_yinz, GridSnapshot,
    TestSession,
};
use yinz_types::data::InputMode;

const ARROW_DOWN: &[u8] = b"\x1b[B";

fn open_presets_screen(yinz: &TestSession) {
    yinz.wait_until("configuration plugin opened", |grid_snapshot| {
        grid_snapshot.contains("Configuration")
    });
    yinz.send_stdin(&keys::TAB);
    yinz.wait_until("presets screen opened", |grid_snapshot| {
        grid_snapshot.contains("1. Default") && grid_snapshot.contains("2. Unlock First")
    });
}

fn apply_selected_preset_and_close(
    yinz: &TestSession,
    times_to_move_down: usize,
    applied_marker: &str,
) {
    for _ in 0..times_to_move_down {
        yinz.send_stdin(ARROW_DOWN);
    }
    yinz.send_stdin(&keys::ENTER);
    let applied_marker = applied_marker.to_owned();
    yinz.wait_until("preset applied to the running session", |grid_snapshot| {
        grid_snapshot.contains(&applied_marker)
    });
    yinz.send_stdin(&keys::ctrl('c'));
    yinz.send_stdin(&keys::ctrl('c'));
    yinz.wait_until("configuration plugin closed", |grid_snapshot| {
        !grid_snapshot.contains("Configuration")
    });
}

fn wait_for_unlock_first_status_bar(yinz: &TestSession) -> GridSnapshot {
    yinz.wait_until(
        "status bar groups keys the unlock-first way",
        |grid_snapshot| grid_snapshot.contains("PANE") && !grid_snapshot.contains("Ctrl +"),
    )
}

fn wait_for_default_status_bar(yinz: &TestSession) -> GridSnapshot {
    yinz.wait_until("status bar groups keys the default way", |grid_snapshot| {
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
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('o'));
    yinz.send_stdin(&keys::key('c'));
    open_presets_screen(&yinz);
    apply_selected_preset_and_close(&yinz, 2, "UNLOCK");

    // Ctrl+g starts a timed prefix in Locked mode, and reconfiguration updates client
    // bindings asynchronously. Closing the configuration pane also updates passthrough
    // state, which can clear a pending prefix before it expires. This test observes preset
    // delivery: reach its view with an acknowledged mode change instead of the toggle.
    let (code, output) = yinz.run_cli_action_reporting(CliAction::SwitchMode {
        input_mode: InputMode::Normal,
    });
    assert_eq!(code, 0, "{output}");
    let grid_snapshot = wait_for_unlock_first_status_bar(&yinz);
    assert_snapshot!(normalized(&grid_snapshot));

    yinz.send_stdin(&keys::key('o'));
    yinz.send_stdin(&keys::key('c'));
    open_presets_screen(&yinz);
    apply_selected_preset_and_close(&yinz, 1, "Ctrl +");

    let grid_snapshot = wait_for_default_status_bar(&yinz);
    assert_snapshot!(normalized(&grid_snapshot));

    yinz.quit();
}
