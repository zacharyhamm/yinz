use crate::{keys, CellStyle, GridSnapshot, LayoutInfo, Size, TestRunner, TestSession};

fn selected(frame: &GridSnapshot, title: &str) -> bool {
    frame.text.lines().any(|line| {
        line.contains(title) && line.trim_start_matches([' ', '│']).starts_with(['>', '·'])
    })
}

fn bar_style(frame: &GridSnapshot) -> CellStyle {
    let (row, line) = frame
        .text
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains("Agents") && line.contains("Todos"))
        .expect("panel tab bar");
    let column = line[..line.find("Agents").unwrap()].chars().count();
    frame.cell_style(column, row)
}

fn wait_for_list(yinz: &TestSession, unfocused_bar: CellStyle) {
    yinz.wait_until("list focus and its keyboard hints return", |frame| {
        frame.contains("Enter focus")
            && !frame.contains("←/→ choose")
            && !frame.contains("▸")
            && bar_style(frame) == unfocused_bar
    });
}

fn wait_for_history(yinz: &TestSession, unfocused_bar: CellStyle) {
    yinz.wait_until("history focus marker and keyboard hints", |frame| {
        frame.contains("▸") && frame.contains("↑↓ event") && bar_style(frame) == unfocused_bar
    });
}

fn wait_for_bar(yinz: &TestSession, unfocused_bar: CellStyle) {
    yinz.wait_until("bar highlight and keyboard hints", |frame| {
        frame.contains("←/→ choose") && !frame.contains("▸") && bar_style(frame) != unfocused_bar
    });
}

fn cycle(history_visible: bool) {
    let height = if history_visible { 20 } else { 8 };
    let mut yinz = TestRunner::new(Size { rows: 40, cols: 120 })
        .with_config(&format!(
            r#"
keybinds {{
  normal {{
    bind "Ctrl b" {{ LaunchOrFocusPlugin "agents" {{ floating true; overlay true; awaits_placement true; }}; }}
  }}
}}
plugins {{
  agents location="yinz:agents" {{ shared true; width "100"; height "{height}"; }}
  agent-service location="yinz:agent-service" {{ shared true; summaries false; }}
}}
load_plugins clear-defaults=true {{ "agent-service"; }}
"#
        ))
        .with_layout(LayoutInfo::Stringified(
            r#"layout { pane command="claude"; pane command="codex"; }"#.into(),
        ))
        .start();
    let _claude = yinz.expect_pty_spawn();
    let _codex = yinz.expect_pty_spawn();
    yinz.send_stdin(&keys::ctrl('b'));
    let initial = yinz.wait_until("both agents appear in the panel", |frame| {
        frame.contains("Agents")
            && frame.contains("Todos")
            && frame.contains("claude")
            && frame.contains("codex")
            && frame.contains("Enter focus")
    });
    assert_eq!(initial.contains("nothing to show yet"), history_visible);
    let unfocused_bar = bar_style(&initial);
    yinz.send_stdin(b"\x1b[A");
    yinz.wait_until("first agent selected", |frame| selected(frame, "claude"));

    for _ in 0..2 {
        yinz.send_stdin(&keys::TAB);
        if history_visible {
            wait_for_history(&yinz, unfocused_bar);
            yinz.send_stdin(&keys::TAB);
        }
        wait_for_bar(&yinz, unfocused_bar);
        yinz.send_stdin(&keys::TAB);
        wait_for_list(&yinz, unfocused_bar);
        yinz.send_stdin(b"\x1b[B");
        yinz.wait_until(
            "Down moves through the list after leaving the bar",
            |frame| selected(frame, "codex"),
        );
        yinz.send_stdin(b"\x1b[A");
        yinz.wait_until("Up returns to the first agent", |frame| {
            selected(frame, "claude")
        });
    }

    for _ in 0..2 {
        yinz.send_stdin(&keys::SHIFT_TAB);
        wait_for_bar(&yinz, unfocused_bar);
        yinz.send_stdin(&keys::SHIFT_TAB);
        if history_visible {
            wait_for_history(&yinz, unfocused_bar);
            yinz.send_stdin(&keys::SHIFT_TAB);
        }
        wait_for_list(&yinz, unfocused_bar);
    }
    yinz.quit();
}

#[test]
fn agents_tab_cycle_with_history() {
    cycle(true);
}

#[test]
fn agents_tab_cycle_without_history() {
    cycle(false);
}
