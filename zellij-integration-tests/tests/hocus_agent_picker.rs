#![cfg(unix)]

//! The one thing hocus cannot prove to itself: where the agent picker lands on
//! the screen.
//!
//! `strip_rows` and `picker_key` are unit-tested down to the row text, but they
//! say nothing about layout — a picker that drew over the conversation, or off
//! the bottom of the pane, would pass every one of those tests. Only a real
//! session with a real grid to read says where the rows actually went.

use zellij_integration_tests::{keys, LayoutInfo, TestRunner, TestSession, TERMINAL_SIZE};

/// hocus with the daemon that finds agents for it, as the real config has it.
///
/// `zellij:` rather than `file:` for the same reason `hocus_store` gives: hocus
/// relies on being a builtin for the permissions it never asks for.
const HOCUS_CONFIG: &str = r#"plugins {
  hocus location="zellij:hocus" {
    shared true
  }
  hocus-agentd location="zellij:hocus-agentd"
}
load_plugins clear-defaults=true {
  "hocus-agentd"
}
"#;

/// hocus above a pane named after an agent, which is what detection reads when
/// a pane's command names nothing — here, a shell.
const HOCUS_LAYOUT: &str = r#"layout {
    pane focus=true {
        plugin location="hocus"
    }
    pane name="claude" size="20%"
}
"#;

/// Put a line into hocus and send it. Pasted rather than typed for the reason
/// `hocus_store` gives: a typed burst outruns the render.
fn submit(zellij: &TestSession, line: &str) {
    zellij.send_stdin(&keys::BRACKETED_PASTE_START);
    zellij.send_stdin(line.as_bytes());
    zellij.send_stdin(&keys::BRACKETED_PASTE_END);
    zellij.wait_until("the line is in the input box", |grid_snapshot| {
        grid_snapshot.contains(&line[line.len().saturating_sub(20)..])
    });
    zellij.send_stdin(&keys::ENTER);
}

#[test]
fn the_agent_picker_takes_the_top_half_and_leaves_the_conversation_below() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_CONFIG)
        .with_layout(LayoutInfo::Stringified(HOCUS_LAYOUT.to_owned()))
        .start();
    let agent = zellij.expect_pty_spawn();
    agent.output(b"$ ");

    zellij.wait_until("hocus asks for permissions", |grid_snapshot| {
        grid_snapshot.contains("Allow? (y/n)")
    });
    zellij.send_stdin(&keys::key('y'));
    zellij.wait_until("the prompt is dismissed", |grid_snapshot| {
        !grid_snapshot.contains("Allow? (y/n)")
    });

    // Something in the transcript to look for underneath the picker. A slash
    // command runs the tool inline, so no API key is involved.
    submit(
        &zellij,
        r#"/project_assign {"name": "marker", "root": "/work/marker"}"#,
    );
    zellij.wait_until("the transcript has something in it", |grid_snapshot| {
        grid_snapshot.contains("marker")
    });

    zellij.send_stdin(&keys::ctrl('r'));
    zellij.wait_until("the agent picker is open", |grid_snapshot| {
        grid_snapshot.contains("<Enter> focus")
    });

    let screen = zellij.snapshot();
    let row = |needle: &str| {
        screen
            .row_of_line(needle)
            .unwrap_or_else(|| panic!("{needle:?} is on screen:\n{}", screen.text))
    };
    // The pane runs from its title to the help line, which is its last row.
    let title = row("hocus — ");
    let help = row("<Enter> focus");
    let pane_rows = help - title + 1;

    // Nothing here owns a project and a pane the harness fakes reports no
    // folder to read a branch out of, so both headers stand in for what they
    // could not name — and the agent sits under them either way.
    assert_eq!(row("no project"), title + 1);
    assert_eq!(row("  no branch"), title + 2);
    assert_eq!(row(">   0  claude  running"), title + 3);

    // The rule closes the picker region, which is the top half of the pane.
    let rule = row("────");
    assert_eq!(
        rule - title,
        pane_rows / 2 - 1,
        "the picker takes the top half of {pane_rows} rows:\n{}",
        screen.text,
    );
    // And the conversation is below it rather than replaced by it.
    assert!((rule..help).contains(&row("marker")));
    let input = screen.text.lines().nth(help - 1).unwrap_or_default();
    assert!(
        input.starts_with("> "),
        "the input box is drawn above the help line, visible but inert: {input:?}",
    );

    // And Esc gives the whole pane back to the conversation.
    zellij.send_stdin(&keys::ESC);
    zellij.wait_until("the picker is gone", |grid_snapshot| {
        !grid_snapshot.contains("<Enter> focus")
    });
    zellij.wait_until("the conversation is still there", |grid_snapshot| {
        grid_snapshot.contains("marker")
    });
    zellij.quit();
}
