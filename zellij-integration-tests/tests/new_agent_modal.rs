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

use zellij_integration_tests::{keys, LayoutInfo, Size, TestRunner, TERMINAL_SIZE};

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

/// The modal as the shipped keybinding actually opens it: an overlay, and a modal one - the
/// collection whose frame the mouse can drag.
const NEW_AGENT_OVERLAY_CONFIG: &str = r#"mouse_mode true
keybinds {
  normal {
    bind "Ctrl b" {
      LaunchOrFocusPlugin "hocus-new-agent" {
        floating true
        overlay true
        modal true
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

/// The other way the frame used to move: opening a list scrolled the form.
///
/// A dropdown is an overlay - it covers the rows under it and takes none of its own -
/// so the form has no reason to move when one opens. It used to anyway: the viewport
/// scrolled a row to reserve space for the list, which put the select at the top of the
/// form and took the field above it off screen.
#[test]
fn opening_a_dropdown_leaves_the_rest_of_the_form_where_it_was() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(NEW_AGENT_CONFIG)
        .with_layout(LayoutInfo::Stringified(SHELL_ONLY.to_owned()))
        .start();
    let _shell = zellij.expect_pty_spawn();

    zellij.send_stdin(&keys::ctrl('b'));
    let snapshot = zellij.wait_until("the new agent modal is up", |snapshot| {
        snapshot.contains("Create and switch branch") && snapshot.contains("[x]")
    });
    let title = position_of(&snapshot, "New agent");
    let project = position_of(&snapshot, "Project");
    let (column, row) = position_of(&snapshot, "Agent:");

    zellij.send_stdin(&sgr_press(column + 1, row + 1));
    zellij.send_stdin(&sgr_release(column + 1, row + 1));

    // An option that is only on screen while the list is open.
    let snapshot = zellij.wait_until("the agent list is open", |snapshot| {
        snapshot.contains("Cursor")
    });
    assert_eq!(
        position_of(&snapshot, "Project"),
        project,
        "the field above the select moved when the list opened:\n{}",
        snapshot.text
    );
    assert_eq!(
        position_of(&snapshot, "Agent:"),
        (column, row),
        "the select itself moved when its list opened:\n{}",
        snapshot.text
    );
    assert_eq!(
        position_of(&snapshot, "New agent"),
        title,
        "the dialog's title moved when the list opened:\n{}",
        snapshot.text
    );

    zellij.quit();
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
        snapshot.contains("Create and switch branch") && snapshot.contains("[x]")
    });
    // The dialog's own title, on the row that used to be scrolled off the top.
    assert!(
        snapshot.contains("New agent"),
        "the modal's title is missing, so the frame scrolled:\n{}",
        snapshot.text
    );

    let (column, row) = position_of(&snapshot, "Create and switch branch");
    zellij.send_stdin(&sgr_press(column + 1, row + 1));
    zellij.send_stdin(&sgr_release(column + 1, row + 1));

    let snapshot = zellij.wait_until("the checkbox under the pointer toggled", |snapshot| {
        snapshot
            .text
            .lines()
            .any(|line| line.contains("Create and switch branch") && line.contains("[ ]"))
    });
    assert!(
        snapshot.contains("New agent"),
        "the modal's title is missing, so the frame scrolled:\n{}",
        snapshot.text
    );

    zellij.quit();
}

/// The modal is an overlay, so it drags by its frame like any other - but it is the one overlay
/// that re-asserts its own placement on every `TabUpdate`, and a drag *is* a `TabUpdate`: moving
/// it reported session state, the plugin answered by asking for `x: None, y: 20%` again, and the
/// modal snapped back to where it started. It read as a popup that could not be moved at all.
#[test]
fn the_new_agent_modal_can_be_dragged_by_its_frame() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(NEW_AGENT_OVERLAY_CONFIG)
        .with_layout(LayoutInfo::Stringified(SHELL_ONLY.to_owned()))
        .start();
    let _shell = zellij.expect_pty_spawn();

    zellij.send_stdin(&keys::ctrl('b'));
    let snapshot = zellij.wait_until("the new agent modal is up", |snapshot| {
        snapshot.contains("Use worktree") && snapshot.contains("[x]")
    });
    let (column, row) = position_of(&snapshot, "New agent");

    // its top frame is the row above its title; carry it left and up
    let drag_to_column = column - 20;
    let drag_to_row = row - 1;
    zellij.send_stdin(&sgr_press(column + 1, row));
    zellij.send_stdin(&sgr_left_motion(drag_to_column + 1, drag_to_row));
    zellij.send_stdin(&sgr_release(drag_to_column + 1, drag_to_row));

    let snapshot = zellij.wait_until("the modal is where it was dropped", |snapshot| {
        snapshot.contains("New agent")
            && position_of(snapshot, "New agent") == (drag_to_column, drag_to_row)
    });
    assert!(
        snapshot.contains("Use worktree"),
        "the whole dialog moved with the frame, not just its title:\n{}",
        snapshot.text
    );

    // and it stays there: the plugin gets its `TabUpdate` for the drag either way, and the
    // position it would ask for is no longer the one that wins
    zellij.send_stdin(&keys::ctrl('b'));
    let snapshot = zellij.snapshot();
    assert_eq!(
        position_of(&snapshot, "New agent"),
        (drag_to_column, drag_to_row),
        "the modal went back to where it was opened:\n{}",
        snapshot.text
    );

    zellij.quit();
}

/// The two Firecracker rows, and the check that pairs them: an image names what a
/// VM boots from, so one typed without a VM is refused in the modal's status row
/// rather than reaching the host as a pane that cannot start.
#[test]
fn an_image_without_a_vm_is_refused_in_the_modals_status_row() {
    // Tall enough for the whole dialog: in the default 24 rows the form scrolls, and
    // the button and the status row this test reads sit below the viewport.
    let mut zellij = TestRunner::new(Size {
        cols: 120,
        rows: 40,
    })
    .with_config(NEW_AGENT_CONFIG)
    .with_layout(LayoutInfo::Stringified(SHELL_ONLY.to_owned()))
    .start();
    let _shell = zellij.expect_pty_spawn();

    zellij.send_stdin(&keys::ctrl('b'));
    let snapshot = zellij.wait_until("the new agent modal is up", |snapshot| {
        snapshot.contains("Firecracker image") && snapshot.contains("[x]")
    });
    assert!(
        snapshot.contains("Firecracker VM"),
        "the VM row is missing from the modal:\n{}",
        snapshot.text
    );

    // Off, so the branch fields are not the first thing the submit complains about.
    let (column, row) = position_of(&snapshot, "Create and switch branch");
    zellij.send_stdin(&sgr_press(column + 1, row + 1));
    zellij.send_stdin(&sgr_release(column + 1, row + 1));
    zellij.wait_until("the branch checkbox is off", |snapshot| {
        snapshot
            .text
            .lines()
            .any(|line| line.contains("Create and switch branch") && line.contains("[ ]"))
    });

    // A click into a text field is enough to start typing in it.
    let (column, row) = position_of(&snapshot, "Firecracker image");
    zellij.send_stdin(&sgr_press(column + 1, row + 1));
    zellij.send_stdin(&sgr_release(column + 1, row + 1));
    for character in "rust".chars() {
        zellij.send_stdin(&keys::key(character));
    }
    let snapshot = zellij.wait_until("the image was typed into its field", |snapshot| {
        snapshot
            .text
            .lines()
            .any(|line| line.contains("Firecracker image") && line.contains("rust"))
    });

    let (column, row) = position_of(&snapshot, "Create agent");
    zellij.send_stdin(&sgr_press(column + 1, row + 1));
    zellij.send_stdin(&sgr_release(column + 1, row + 1));
    zellij.wait_until("the modal refused the image", |snapshot| {
        snapshot.contains("firecracker_image requires a Firecracker VM")
    });

    zellij.quit();
}
