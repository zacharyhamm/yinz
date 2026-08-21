#![cfg(unix)]

//! Where the new-agent modal's widgets really are on screen.
//!
//! The forms library records a rectangle for every widget as it draws it and hit-tests
//! clicks against those rectangles, so it can only be right if what the host draws lands
//! on the row the plugin printed it at. It used to not: the dialog padded its centred
//! rows out to its full width from an already-indented column, which ran past the pane,
//! wrapped, and - on the bottom row, where the help line sits whenever the dialog was
//! given exactly the rows it asked for - scrolled the whole frame up by one. Every widget
//! then answered to a click one row *below* itself, and the title scrolled off the top.

use zellij_integration_tests::{keys, LayoutInfo, TestRunner, TERMINAL_SIZE};

/// The modal as the shipped config opens it, minus every background plugin: a floating
/// plugin pane that sizes itself to the dialog, with the mouse on.
const NEW_AGENT_CONFIG: &str = r#"mouse_mode true
keybinds {
  normal {
    bind "Ctrl b" {
      LaunchOrFocusPlugin "hocus-new-agent" {
        floating true
        move_to_focused_tab true
      }
    }
  }
}
plugins {
  hocus-new-agent location="zellij:hocus-new-agent"
}
load_plugins clear-defaults=true {
}
"#;

const SHELL_ONLY: &str = r#"layout {
    pane
}
"#;

/// SGR mouse reports, 1-based, as `floating_pane_overlay.rs` sends them.
fn sgr_press(column: usize, line: usize) -> Vec<u8> {
    format!("\u{1b}[<0;{column};{line}M").into_bytes()
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

#[test]
fn a_click_on_a_form_widget_acts_on_the_widget_under_the_pointer() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(NEW_AGENT_CONFIG)
        .with_layout(LayoutInfo::Stringified(SHELL_ONLY.to_owned()))
        .start();
    let _shell = zellij.expect_pty_spawn();

    zellij.send_stdin(&keys::ctrl('b'));
    let snapshot = zellij.wait_until("the new agent modal is up", |snapshot| {
        snapshot.contains("Use worktree") && snapshot.contains("[x]")
    });
    // The dialog's own title, on the row that used to be scrolled off the top.
    assert!(
        snapshot.contains("New agent"),
        "the modal's title is missing, so the frame scrolled:\n{}",
        snapshot.text
    );

    let (column, row) = position_of(&snapshot, "Use worktree");
    zellij.send_stdin(&sgr_press(column + 1, row + 1));
    zellij.send_stdin(&sgr_release(column + 1, row + 1));

    let snapshot = zellij.wait_until("the checkbox under the pointer toggled", |snapshot| {
        snapshot
            .text
            .lines()
            .any(|line| line.contains("Use worktree") && line.contains("[ ]"))
    });
    assert!(
        snapshot.contains("New agent"),
        "the modal's title is missing, so the frame scrolled:\n{}",
        snapshot.text
    );

    zellij.quit();
}
