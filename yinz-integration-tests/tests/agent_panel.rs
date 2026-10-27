#![cfg(unix)]

use yinz_cli::args::CliAction;
use yinz_integration_tests::{keys, LayoutInfo, Size, TestRunner, TestSession, TERMINAL_SIZE};
use yinz_types::{data::PaneListEntry, input::actions::Action};

const CONFIG: &str = r#"
mouse_mode true
keybinds {
 normal {
  bind "Ctrl x" { MessagePlugin { name "agent_strip_reposition"; }; }
  bind "Ctrl g" { LaunchOrFocusPlugin "new-agent" { floating true; overlay true; modal true; move_to_focused_tab true; }; }
  bind "Ctrl t" { NewTab; }
  bind "Ctrl b" {
   LaunchOrFocusPlugin "agents" { floating true; overlay true; awaits_placement true; move_to_focused_tab true; }
  }
 }
}
plugins {
  agents location="yinz:agents" { shared true; }
  agent-service location="yinz:agent-service" { shared true; summaries false; }
  project-store location="yinz:project-store" { shared true; }
  new-agent location="yinz:new-agent"
}
load_plugins clear-defaults=true { "agent-service"; "project-store"; }
"#;

#[test]
fn panel_tabs_open_and_dismiss_without_chat() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(CONFIG)
        .with_layout(LayoutInfo::Stringified(
            "layout { pane command=\"claude\"; }".into(),
        ))
        .start();
    let agent = yinz.expect_pty_spawn();
    agent.output(b"AGENT READY\r\n");
    yinz.wait_until("the initial terminal is rendered", |s| {
        s.contains("AGENT READY")
    });
    yinz.send_stdin(&keys::ctrl('b'));
    let initial = yinz.wait_until("the agents panel receives the service snapshot", |s| {
        s.contains("Agents") && s.contains("CLA")
    });
    assert!(!initial.contains("Hocus"));
    assert_eq!(panel(&yinz).pane_info.pane_tabs.unwrap().tabs.len(), 1);
    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("the panel is dismissed", |s| {
        !s.contains("Agents") && !s.contains("Companion")
    });
    yinz.quit();
}

#[test]
fn panel_reports_missing_local_quota_without_blocking_dismissal() {
    let mut yinz = TestRunner::new(Size {
        cols: 120,
        rows: 40,
    })
    .with_config(CONFIG)
    .with_layout(LayoutInfo::Stringified(
        "layout { pane command=\"claude\"; }".into(),
    ))
    .start();
    let agent = yinz.expect_pty_spawn();
    agent.output(b"AGENT READY\r\n");
    yinz.wait_until("the initial terminal is rendered", |s| {
        s.contains("AGENT READY")
    });
    yinz.send_stdin(&keys::ctrl('b'));
    // The harness has an empty, isolated Claude config; this exercises the real host reply.
    let quota = yinz.wait_until("the panel explains missing local provider quotas", |s| {
        s.contains("Agents") && s.contains("CLA") && s.contains("Unavailable")
    });
    assert!(!quota.contains("% used"));
    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("quota details dismiss with the panel", |s| {
        !s.contains("Agents") && !s.contains("Unavailable") && s.contains("AGENT READY")
    });
    yinz.quit();
}

#[test]
fn open_agent_panel_repaints_its_background_when_the_theme_changes() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!(
            "{CONFIG}\npane_animations false\ntheme \"gruvbox-light\""
        ))
        .with_layout(LayoutInfo::Stringified(
            "layout { pane command=\"claude\"; }".into(),
        ))
        .start();
    let agent = yinz.expect_pty_spawn();
    agent.output(b"AGENT READY\r\n");
    yinz.wait_until("the initial terminal is rendered", |s| {
        s.contains("AGENT READY")
    });
    yinz.send_stdin(&keys::ctrl('e'));
    yinz.send_stdin(b"a");
    yinz.wait_until("the agents panel receives the service snapshot", |s| {
        s.contains("Agents") && s.contains("CLA")
    });
    let pane = panel(&yinz).pane_info;
    let x = pane.pane_content_x.get();
    let y = pane.pane_content_y.get();
    let columns = pane.pane_content_columns.get();
    let rows = pane.pane_content_rows.get();
    let themes = yinz_config::assets::get_default_themes();
    for name in ["gruvbox-light", "gruvbox-dark", "dracula", "gruvbox-light"] {
        let (code, error) = yinz.run_actions_reporting(vec![Action::Reconfigure {
            config: format!("theme \"{name}\""),
            write_to_disk: false,
        }]);
        assert_eq!(code, 0, "{error}");
        let colors = themes.get_theme(name).unwrap().palette;
        yinz.wait_until(&format!("the whole agents panel uses {name}"), |s| {
            let lines = s.lines();
            let Some(selected) = (y..y + rows)
                .find(|&row| lines[row].chars().nth(x) == Some('┃') && lines[row].contains("CLA"))
            else {
                return false;
            };
            let list_columns = lines[selected]
                .chars()
                .skip(x)
                .take(columns)
                .position(|ch| ch == '│')
                .unwrap_or(columns);
            (y..y + rows).all(|row| {
                (x..x + columns).all(|col| {
                    let expected =
                        if lines[row].chars().nth(x) == Some('┃') && col < x + list_columns {
                            colors.text_selected
                        } else {
                            colors.text_unselected
                        };
                    s.cell_style(col, row).background == Some(expected.background.into())
                })
            }) && s.cell_foreground(x, selected) == Some(colors.text_selected.base.into())
        });
    }
    yinz.quit();
}

fn click_text(yinz: &mut TestSession, text: &str) {
    let snapshot = yinz.snapshot();
    let (column, row) = snapshot
        .text
        .lines()
        .enumerate()
        .find_map(|(row, line)| {
            line.find(text)
                .map(|index| (line[..index].chars().count() + 1, row + 1))
        })
        .unwrap_or_else(|| panic!("{text} is missing:\n{}", snapshot.text));
    yinz.send_stdin(format!("\x1b[<0;{column};{row}M").as_bytes());
    yinz.send_stdin(format!("\x1b[<0;{column};{row}m").as_bytes());
}

#[test]
fn clicking_a_terminal_dismisses_each_companion_panel() {
    companion_panel_dismisses(false);
}

#[test]
fn creating_an_agent_dismisses_each_companion_panel() {
    companion_panel_dismisses(true);
}

fn companion_config() -> (tempfile::TempDir, String) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("companion.scm");
    std::fs::write(
        &path,
        r#"(yinz-declare-pane) (define (render rows columns) "companion content")"#,
    )
    .unwrap();
    let config = format!(
        "{CONFIG}\noverlay_tab_plugins {{ agents label=\"Agents\"; \"file:{}\" label=\"Companion\"; }}", path.display()
    );
    (directory, config)
}

fn companion_panel_dismisses(create_agent: bool) {
    let (_directory, config) = companion_config();
    let (index, name, content) = (1, "Companion", "companion content");
    let mut yinz = TestRunner::new(Size {
        cols: 120,
        rows: 40,
    })
    .with_config(&config)
    .with_layout(LayoutInfo::Stringified(
        "layout { pane command=\"claude\"; }".into(),
    ))
    .start();
    let agent = yinz.expect_pty_spawn();
    agent.output(b"AGENT READY\r\n");
    yinz.wait_until("the initial terminal is rendered", |s| {
        s.contains("AGENT READY")
    });
    yinz.send_stdin(&keys::ctrl('b'));
    yinz.wait_until("the panel tab bar is ready", |s| {
        s.contains("Agents") && s.contains("Companion")
    });
    click_text(&mut yinz, name);
    yinz.wait_until(&format!("{name} is active"), |s| {
        s.text.to_lowercase().contains(&content.to_lowercase())
            && panel(&yinz).pane_info.pane_tabs.unwrap().active == index
    });

    if create_agent {
        yinz.send_stdin(&keys::ctrl('g'));
        yinz.wait_until("the new agent modal is ready", |s| {
            s.contains("Create agent") && s.contains("Create and switch branch")
        });
        click_text(&mut yinz, "Create and switch branch");
        yinz.wait_until("branch creation is unchecked", |s| {
            s.text
                .lines()
                .any(|line| line.contains("Create and switch branch") && line.contains("[ ]"))
        });
        click_text(&mut yinz, "Create agent");
        let agent = yinz.expect_pty_spawn();
        agent.output(b"NEW AGENT\r\n");
        yinz.wait_until(
            "the new agent receives focus and the panel is dismissed",
            |s| s.contains("NEW AGENT") && !s.contains("Create agent") && !s.contains("Companion"),
        );
    } else {
        // The terminal's top-left corner is outside the panel's padded rectangle.
        yinz.send_stdin(b"\x1b[<0;1;1M");
        yinz.send_stdin(b"\x1b[<0;1;1m");
        yinz.wait_until("terminal focus dismisses the companion panel", |s| {
            !s.contains("Agents") && !s.contains("Companion")
        });
    }

    yinz.send_stdin(&keys::ctrl('b'));
    yinz.wait_until("the dismissed panel can be reopened", |s| {
        s.contains("Agents") && s.contains("Companion")
    });
    assert!(
        panel(&yinz).pane_info.is_focused,
        "reopening returns keyboard focus to the panel"
    );
    yinz.quit();
}

fn panel(yinz: &TestSession) -> PaneListEntry {
    let (code, output) = yinz.run_cli_action_reporting(CliAction::ListPanes {
        tab: true,
        command: false,
        state: true,
        geometry: true,
        all: true,
        json: true,
    });
    assert_eq!(code, 0, "{output}");
    serde_json::from_str::<Vec<PaneListEntry>>(&output)
        .unwrap()
        .into_iter()
        .find(|pane| {
            pane.pane_info
                .pane_tabs
                .as_ref()
                .is_some_and(|tabs| tabs.pane_id == pane.pane_info.id)
        })
        .expect("project panel")
}

#[test]
fn panel_placement_drag_tab_follow_and_modal_return_work_without_chat() {
    let (_directory, config) = companion_config();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(&config)
        .with_layout(LayoutInfo::Stringified(
            "layout { pane command=\"claude\"; }".into(),
        ))
        .start();
    let agent = yinz.expect_pty_spawn();
    agent.output(b"AGENT READY\r\n");
    yinz.wait_until("the initial terminal is rendered", |s| {
        s.contains("AGENT READY")
    });
    yinz.send_stdin(&keys::ctrl('b'));
    yinz.wait_until("the project panel is placed", |s| {
        s.contains("Agents") && s.contains("CLA")
    });
    let first = panel(&yinz).pane_info;
    assert_eq!(
        (
            first.pane_x.get(),
            first.pane_y.get(),
            first.pane_columns.get(),
            first.pane_rows.get()
        ),
        (3, 6, 72, 15)
    );
    yinz.send_stdin(&keys::ctrl('x'));
    yinz.wait_until("the panel moves to the top-left corner", |s| {
        s.contains("Agents") && panel(&yinz).pane_info.pane_y.get() == 3
    });
    // Drag the top frame five columns to the right and two rows down.
    for (button, column, row, end) in [(0, 9, 4, 'M'), (32, 14, 6, 'M'), (0, 14, 6, 'm')] {
        yinz.send_stdin(format!("\x1b[<{button};{column};{row}{end}").as_bytes());
    }
    yinz.wait_until("the panel stays where it was dragged", |s| {
        s.contains("Agents") && {
            let p = panel(&yinz).pane_info;
            (p.pane_x.get(), p.pane_y.get()) == (8, 5)
        }
    });
    yinz.resize(Size {
        rows: 30,
        cols: 140,
    });
    yinz.wait_until("resize preserves the dragged position", |s| {
        s.contains("Agents") && {
            let p = panel(&yinz).pane_info;
            (
                p.pane_x.get(),
                p.pane_y.get(),
                p.pane_columns.get(),
                p.pane_rows.get(),
            ) == (8, 5, 84, 19)
        }
    });
    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("the dragged panel is dismissed", |s| {
        !s.contains("Agents") && !s.contains("Companion")
    });
    yinz.send_stdin(&keys::ctrl('x'));
    yinz.send_stdin(&keys::ctrl('b'));
    yinz.wait_until(
        "the panel reopens after the hidden reposition message",
        |s| s.contains("Agents") && s.contains("CLA"),
    );
    let reopened = panel(&yinz).pane_info;
    assert_eq!(
        (
            reopened.pane_x.get(),
            reopened.pane_y.get(),
            reopened.pane_columns.get(),
            reopened.pane_rows.get(),
        ),
        (8, 5, 84, 19),
        "repositioning while hidden preserves the dragged placement"
    );
    yinz.send_stdin(&keys::ctrl('g'));
    yinz.wait_until("a modal opens above the panel", |s| {
        s.contains("Create agent")
    });
    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("modal cancellation leaves the panel up", |s| {
        !s.contains("Create agent") && s.contains("Companion")
    });
    yinz.send_stdin(b"w");
    yinz.wait_until("the modal returned keyboard focus to Agents", |s| {
        s.contains("Waiting for you")
    });
    yinz.send_stdin(b"c");
    // Keep a companion active through placement, resize, and the session-tab move.
    yinz.send_stdin(&keys::SHIFT_TAB);
    yinz.send_stdin(b"\x1b[C");
    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("Companion is the active panel child", |s| {
        s.contains("companion content") && panel(&yinz).pane_info.pane_tabs.unwrap().active == 1
    });
    yinz.send_stdin(&keys::ctrl('x'));
    yinz.wait_until("the companion moves to the top-right corner", |s| {
        let p = panel(&yinz).pane_info;
        s.contains("companion content") && (p.pane_x.get(), p.pane_y.get()) == (53, 3)
    });
    yinz.resize(Size {
        rows: 36,
        cols: 160,
    });
    yinz.wait_until("the companion follows the resized surface", |s| {
        let p = panel(&yinz).pane_info;
        s.contains("companion content")
            && (
                p.pane_x.get(),
                p.pane_y.get(),
                p.pane_columns.get(),
                p.pane_rows.get(),
            ) == (61, 3, 96, 23)
    });
    yinz.send_stdin(&keys::ctrl('t'));
    let shell = yinz.expect_pty_spawn();
    shell.output(b"SECOND TAB");
    yinz.wait_until("the panel follows the new tab", |s| {
        s.contains("SECOND TAB") && s.contains("companion content")
    });
    assert_eq!(panel(&yinz).tab_position.get(), 1);
    yinz.quit();
}
