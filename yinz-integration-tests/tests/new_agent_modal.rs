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

use yinz_integration_tests::{keys, LayoutInfo, Size, TestRunner, TERMINAL_SIZE};

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
  hocus-new-agent location="yinz:hocus-new-agent"
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
  hocus-new-agent location="yinz:hocus-new-agent"
}
load_plugins clear-defaults=true {
}
"#;

const SHELL_ONLY: &str = r#"layout {
    pane
}
"#;

#[test]
fn duplicate_agent_never_shows_the_spawn_form() {
    duplicate_agent_shortcut(true);
}

#[test]
fn duplicate_of_a_shell_notifies_without_showing_the_spawn_form() {
    duplicate_agent_shortcut(false);
}

fn duplicate_agent_shortcut(is_agent: bool) {
    const CONFIG: &str = r#"
agent_pane_min_columns 0
agent_pane_min_rows 0
keybinds {
  normal {
    bind "Ctrl d" {
      LaunchOrFocusPlugin "hocus-duplicate-agent" {
        floating true
        overlay true
        modal true
        move_to_focused_tab true
      }
    }
  }
}
plugins {
  hocus-duplicate-agent location="yinz:hocus-new-agent" { duplicate_focused true; }
  hocus-agentd location="yinz:hocus-agentd"
}
load_plugins clear-defaults=true { "hocus-agentd"; }
"#;
    let layout = if is_agent {
        "layout { pane command=\"claude\"; }"
    } else {
        SHELL_ONLY
    };
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(CONFIG)
        .with_layout(LayoutInfo::Stringified(layout.to_owned()))
        .start();
    let source = yinz.expect_pty_spawn();
    source.output(b"SOURCE READY\r\n");
    yinz.wait_until("the source pane is ready", |snapshot| {
        snapshot.contains("SOURCE READY")
    });
    for attempt in 1..=if is_agent { 2 } else { 1 } {
        yinz.send_stdin(&keys::ctrl('d'));
        if is_agent {
            let agent = yinz.expect_pty_spawn();
            assert_eq!(agent.run_command().command.to_str(), Some("env"));
            assert_eq!(
                &agent.run_command().args[..2],
                ["YINZ_CLAUDE_INLINE_HOOKS=1", "claude"]
            );
            let marker = format!("DUPLICATE {attempt} READY");
            agent.output(format!("{marker}\r\n").as_bytes());
            yinz.wait_until("the duplicate is visible", |snapshot| {
                snapshot.contains(&marker)
            });
            let input = format!("duplicate-{attempt}");
            yinz.send_stdin(input.as_bytes());
            agent.wait_for_stdin("the duplicate has focus", |bytes| {
                bytes
                    .windows(input.len())
                    .any(|window| window == input.as_bytes())
            });
        } else {
            yinz.wait_until("duplication reports a non-agent pane", |snapshot| {
                snapshot.contains("the focused pane is not an agent")
            });
            let input = format!("source-{attempt}");
            yinz.send_stdin(input.as_bytes());
            source.wait_for_stdin("the source keeps focus", |bytes| {
                bytes
                    .windows(input.len())
                    .any(|window| window == input.as_bytes())
            });
        }
        assert!(!yinz.snapshot().contains("Create agent"));
        assert!(
            !String::from_utf8_lossy(&yinz.raw_bytes()).contains("Create agent"),
            "the form must never flash during duplication"
        );
    }
    yinz.quit();
}

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

/// The other way the frame used to move: opening a list scrolled the form.
///
/// A dropdown is an overlay - it covers the rows under it and takes none of its own -
/// so the form has no reason to move when one opens. It used to anyway: the viewport
/// scrolled a row to reserve space for the list, which put the select at the top of the
/// form and took the field above it off screen.
#[test]
fn opening_a_dropdown_leaves_the_rest_of_the_form_where_it_was() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(NEW_AGENT_CONFIG)
        .with_layout(LayoutInfo::Stringified(SHELL_ONLY.to_owned()))
        .start();
    let _shell = yinz.expect_pty_spawn();

    yinz.send_stdin(&keys::ctrl('b'));
    let snapshot = yinz.wait_until("the new agent modal is up", |snapshot| {
        snapshot.contains("Create and switch branch")
            && snapshot.contains("[x]")
            && snapshot.contains("<Enter> open")
    });
    let title = position_of(&snapshot, "New agent");
    let project = position_of(&snapshot, "Project");
    let (column, row) = position_of(&snapshot, "Agent:");

    yinz.send_stdin(&sgr_press(column + 1, row + 1));
    yinz.send_stdin(&sgr_release(column + 1, row + 1));

    // An option that is only on screen while the list is open.
    let snapshot = yinz.wait_until("the agent list is open", |snapshot| {
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

    yinz.quit();
}

#[test]
fn a_click_on_a_form_widget_acts_on_the_widget_under_the_pointer() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(NEW_AGENT_CONFIG)
        .with_layout(LayoutInfo::Stringified(SHELL_ONLY.to_owned()))
        .start();
    let _shell = yinz.expect_pty_spawn();

    yinz.send_stdin(&keys::ctrl('b'));
    let snapshot = yinz.wait_until("the new agent modal is up", |snapshot| {
        snapshot.contains("Create and switch branch")
            && snapshot.contains("[x]")
            && snapshot.contains("<Enter> open")
    });
    // The dialog's own title, on the row that used to be scrolled off the top.
    assert!(
        snapshot.contains("New agent"),
        "the modal's title is missing, so the frame scrolled:\n{}",
        snapshot.text
    );

    let (column, row) = position_of(&snapshot, "Create and switch branch");
    yinz.send_stdin(&sgr_press(column + 1, row + 1));
    yinz.send_stdin(&sgr_release(column + 1, row + 1));

    let snapshot = yinz.wait_until("the checkbox under the pointer toggled", |snapshot| {
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

    yinz.quit();
}

/// The modal is an overlay, so it drags by its frame like any other - but it is the one overlay
/// that re-asserts its own placement on every `TabUpdate`, and a drag *is* a `TabUpdate`: moving
/// it reported session state, the plugin answered by asking for `x: None, y: 20%` again, and the
/// modal snapped back to where it started. It read as a popup that could not be moved at all.
#[test]
fn the_new_agent_modal_can_be_dragged_by_its_frame() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(NEW_AGENT_OVERLAY_CONFIG)
        .with_layout(LayoutInfo::Stringified(SHELL_ONLY.to_owned()))
        .start();
    let _shell = yinz.expect_pty_spawn();

    yinz.send_stdin(&keys::ctrl('b'));
    let snapshot = yinz.wait_until("the new agent modal is up", |snapshot| {
        snapshot.contains("Use worktree") && snapshot.contains("[x]")
    });
    let (column, row) = position_of(&snapshot, "New agent");

    // its top frame is the row above its title; carry it left and up
    let drag_to_column = column - 20;
    let drag_to_row = row - 1;
    yinz.send_stdin(&sgr_press(column + 1, row));
    yinz.send_stdin(&sgr_left_motion(drag_to_column + 1, drag_to_row));
    yinz.send_stdin(&sgr_release(drag_to_column + 1, drag_to_row));

    let snapshot = yinz.wait_until("the modal is where it was dropped", |snapshot| {
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
    yinz.send_stdin(&keys::ctrl('b'));
    let snapshot = yinz.snapshot();
    assert_eq!(
        position_of(&snapshot, "New agent"),
        (drag_to_column, drag_to_row),
        "the modal went back to where it was opened:\n{}",
        snapshot.text
    );

    yinz.quit();
}

/// Changing a VM launch back to local preserves the image field for later without
/// making that unused setting block the local agent.
#[test]
fn switching_back_to_local_after_typing_a_vm_image_starts_a_local_agent() {
    // A tall terminal lets this check the modal's requested size rather than scrolling.
    let mut yinz = TestRunner::new(Size {
        cols: 120,
        rows: 40,
    })
    .with_config(NEW_AGENT_CONFIG)
    .with_layout(LayoutInfo::Stringified(SHELL_ONLY.to_owned()))
    .start();
    let _shell = yinz.expect_pty_spawn();

    yinz.send_stdin(&keys::ctrl('b'));
    let snapshot = yinz.wait_until("the new agent modal is up", |snapshot| {
        snapshot.contains("Firecracker image")
            && snapshot.contains("[x]")
            && snapshot.contains("<Enter> open")
    });
    assert!(
        snapshot.contains("Run in Firecracker"),
        "the VM switch is missing from the modal:\n{}",
        snapshot.text
    );

    // Off, so the branch fields are not the first thing the submit complains about.
    let (column, row) = position_of(&snapshot, "Create and switch branch");
    yinz.send_stdin(&sgr_press(column + 1, row + 1));
    yinz.send_stdin(&sgr_release(column + 1, row + 1));
    let snapshot = yinz.wait_until("the branch checkbox is off", |snapshot| {
        snapshot
            .text
            .lines()
            .any(|line| line.contains("Create and switch branch") && line.contains("[ ]"))
    });

    let (column, row) = position_of(&snapshot, "Run in Firecracker");
    yinz.send_stdin(&sgr_press(column + 1, row + 1));
    yinz.send_stdin(&sgr_release(column + 1, row + 1));
    let snapshot = yinz.wait_until("the VM checkbox is on", |snapshot| {
        snapshot
            .text
            .lines()
            .any(|line| line.contains("Run in Firecracker") && line.contains("[x]"))
    });

    // A click into a text field is enough to start typing in it.
    let (column, row) = position_of(&snapshot, "Firecracker image");
    yinz.send_stdin(&sgr_press(column + 1, row + 1));
    yinz.send_stdin(&sgr_release(column + 1, row + 1));
    for character in "rust".chars() {
        yinz.send_stdin(&keys::key(character));
    }
    let snapshot = yinz.wait_until("the image was typed into its field", |snapshot| {
        snapshot
            .text
            .lines()
            .any(|line| line.contains("Firecracker image") && line.contains("rust"))
    });

    let (column, row) = position_of(&snapshot, "Run in Firecracker");
    yinz.send_stdin(&sgr_press(column + 1, row + 1));
    yinz.send_stdin(&sgr_release(column + 1, row + 1));
    let snapshot = yinz.wait_until("the VM checkbox is off", |snapshot| {
        snapshot
            .text
            .lines()
            .any(|line| line.contains("Run in Firecracker") && line.contains("[ ]"))
    });
    assert!(snapshot
        .text
        .lines()
        .any(|line| line.contains("Firecracker image") && line.contains("rust")));

    let (column, row) = position_of(&snapshot, "Create agent");
    yinz.send_stdin(&sgr_press(column + 1, row + 1));
    yinz.send_stdin(&sgr_release(column + 1, row + 1));
    let agent = yinz.expect_pty_spawn();
    let Some(yinz_types::input::command::TerminalAction::RunCommand(command)) =
        agent.terminal_action()
    else {
        panic!("the modal must start an agent command");
    };
    assert_eq!(command.target, yinz_types::command::PaneTarget::Local);
    agent.output(b"LOCAL AGENT\r\n");
    yinz.wait_until("the modal closes onto the local agent", |snapshot| {
        snapshot.contains("LOCAL AGENT") && !snapshot.contains("Create agent")
    });

    yinz.quit();
}

#[test]
fn normal_fork_shortcut_starts_a_new_pane_and_focuses_it() {
    normal_agent_shortcut("fork_focused", "codex");
}

#[test]
fn normal_duplicate_shortcut_starts_a_new_pane_and_focuses_it() {
    normal_agent_shortcut("duplicate_focused", "claude");
}

#[test]
fn normal_codex_duplicate_shortcut_starts_a_new_pane_and_focuses_it() {
    normal_agent_shortcut("duplicate_focused", "codex");
}

fn normal_agent_shortcut(action: &str, kind: &str) {
    let source_kind = if action == "fork_focused" {
        "claude"
    } else {
        kind
    };
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!(
            r#"
agent_pane_min_columns 0
agent_pane_min_rows 0
keybinds {{
  normal {{
    bind "Ctrl b" {{
      LaunchOrFocusPlugin "hocus-new-agent" {{ floating true; {action} true; }}
    }}
  }}
}}
plugins {{ hocus-new-agent location="yinz:hocus-new-agent"; }}
load_plugins clear-defaults=true {{ "yinz:hocus-agentd"; }}
"#
        ))
        .with_layout(LayoutInfo::Stringified(format!(
            "layout {{ pane command=\"{source_kind}\" cwd=\"/host/topic\"; }}"
        )))
        .start();
    let original = yinz.expect_pty_spawn();
    original.output(b"ORIGINAL AGENT\r\n");
    yinz.wait_until("the original agent is running", |s| {
        s.contains("ORIGINAL AGENT")
    });
    yinz.send_stdin(&keys::ctrl('b'));
    if action == "fork_focused" {
        yinz.wait_until("the normal fork picker", |s| {
            s.contains("forking claude in topic") && s.contains("Codex")
        });
        yinz.send_stdin(&keys::TAB);
        yinz.send_stdin(&keys::ENTER);
    }
    let new = yinz.expect_pty_spawn();
    assert_ne!(new.terminal_id(), original.terminal_id());
    let Some(yinz_types::input::command::TerminalAction::RunCommand(command)) =
        new.terminal_action()
    else {
        panic!("the shortcut must start a command");
    };
    if kind == "codex" {
        assert_eq!(command.command, std::path::PathBuf::from("yinz"));
        assert_eq!(command.args, ["hocus", "codex", "--command", "codex", "--"]);
    } else if kind == "claude" {
        assert_eq!(command.command, std::path::PathBuf::from("env"));
        assert_eq!(&command.args[..2], ["YINZ_CLAUDE_INLINE_HOOKS=1", "claude"]);
    } else {
        assert_eq!(command.command, std::path::PathBuf::from(kind));
    }
    assert_eq!(
        yinz_types::agent::kind_from_program_and_args(&command.command, &command.args).as_deref(),
        Some(kind)
    );
    assert_eq!(
        command.cwd.as_deref(),
        Some(std::path::Path::new("/host/topic"))
    );
    assert!(!original.is_exited());
    new.output(b"NEW AGENT\r\n");
    yinz.wait_until("the picker closes onto the new pane", |s| {
        s.contains("NEW AGENT") && !s.contains("Fork agent")
    });
    yinz.send_stdin(b"NEW INPUT");
    new.wait_for_stdin("the new pane receives focus", |s| s.ends_with(b"NEW INPUT"));
    assert!(!original.stdin_bytes().ends_with(b"NEW INPUT"));
    yinz.quit();
}
