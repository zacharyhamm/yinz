#![cfg(unix)]

//! The one thing hocus cannot prove to itself: where the agent strip lands on
//! the screen.
//!
//! `strip_rows` and `picker_key` are unit-tested down to the row text, but they
//! say nothing about layout — a picker that drew over the conversation, or off
//! the bottom of the pane, would pass every one of those tests. Only a real
//! session with a real grid to read says where the rows actually went.

use zellij_integration_tests::test_env::write_git_repository;
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

/// The same, plus the store — which is what actually holds projects.
///
/// The daemon finds the checkout an agent is in; only the store turns one into
/// a project and pushes the registry back. Without it every agent groups under
/// the header that names nothing, however well detection worked.
const HOCUS_PROJECT_CONFIG: &str = r#"plugins {
  hocus location="zellij:hocus" {
    shared true
  }
  hocus-agentd location="zellij:hocus-agentd"
  hocus-store location="zellij:hocus-store"
}
load_plugins clear-defaults=true {
  "hocus-agentd"
  "hocus-store"
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

/// The whole chain, for the case that used to break it: pty records the folder
/// a pane starts in → screen carries it on the manifest → the daemon probes it
/// → the store makes a project out of it → the strip groups the agent under it.
///
/// The agent never changes directory, which is exactly what used to leave it
/// under "no project" for good: the initial folder was recorded silently and the
/// ticker only reports a folder that *differs* from the recorded one, so nothing
/// ever said where the pane was.
#[test]
fn an_agent_that_never_leaves_its_folder_lands_under_its_project() {
    let repository = write_git_repository("hocus-project", "trunk");
    let layout = format!(
        r#"layout {{
    pane focus=true {{
        plugin location="hocus"
    }}
    pane name="claude" cwd="{}" size="20%"
}}
"#,
        repository.display()
    );
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_PROJECT_CONFIG)
        .with_layout(LayoutInfo::Stringified(layout))
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

    // The project is named for the folder the checkout is in, and the branch is
    // the one it has out. Both arrive in the permanent strip without focusing it.
    zellij.wait_until("the agent is under its project", |grid_snapshot| {
        grid_snapshot.contains("hocus-project") && grid_snapshot.contains("trunk")
    });

    let unfocused = zellij.snapshot();
    assert!(
        !unfocused.text.contains(">   0  claude"),
        "the strip has no row cursor before Ctrl-r:\n{}",
        unfocused.text,
    );

    zellij.send_stdin(&keys::ctrl('r'));
    zellij.wait_until("the agent strip is focused", |grid_snapshot| {
        grid_snapshot.contains(">   0  claude")
    });

    let screen = zellij.snapshot();
    let row = |needle: &str| {
        screen
            .row_of_line(needle)
            .unwrap_or_else(|| panic!("{needle:?} is on screen:\n{}", screen.text))
    };
    let title = row("hocus — ");
    assert_eq!(row("hocus-project"), title + 1, "the project header");
    assert_eq!(row("  trunk"), title + 2, "the branch header under it");
    assert_eq!(row(">   0  claude"), title + 3);
    assert!(
        !screen.text.contains("no project"),
        "nothing fell back to the header that names nothing:\n{}",
        screen.text,
    );
    zellij.quit();
}

#[test]
fn the_agent_strip_always_takes_the_top_third_and_leaves_the_conversation_below() {
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
        grid_snapshot.contains("assigned tab 0 to project marker")
    });

    zellij.wait_until("the permanent agent strip is populated", |grid_snapshot| {
        grid_snapshot.contains("0  claude  running")
    });

    let screen = zellij.snapshot();
    let row = |needle: &str| {
        screen
            .row_of_line(needle)
            .unwrap_or_else(|| panic!("{needle:?} is on screen:\n{}", screen.text))
    };
    let title = row("hocus — ");
    let pane_bottom = screen
        .text
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains("────"))
        .nth(1)
        .map(|(row, _)| row)
        .unwrap_or_else(|| panic!("pane boundary is on screen:\n{}", screen.text));
    let pane_rows = pane_bottom - title;

    // Nothing here owns a project and a pane the harness fakes reports no
    // folder to read a branch out of, so both headers stand in for what they
    // could not name — and the agent sits under them either way.
    assert_eq!(row("no project"), title + 1);
    assert_eq!(row("  no branch"), title + 2);
    assert_eq!(row("    0  claude  running"), title + 3);

    // The rule closes the permanent region, which is the top third of the pane.
    let rule = row("────");
    assert_eq!(
        rule - title,
        pane_rows / 3 - 1,
        "the strip takes the top third of {pane_rows} rows:\n{}",
        screen.text,
    );
    // And the conversation is below it rather than replaced by it.
    assert!((rule..pane_bottom).contains(&row("assigned tab 0 to project marker")));
    assert!(
        screen
            .text
            .lines()
            .skip(rule + 1)
            .take(pane_bottom - rule - 1)
            .any(|line| line.starts_with("> ")),
        "the input box is drawn below the strip:\n{}",
        screen.text,
    );

    zellij.send_stdin(&keys::ctrl('r'));
    zellij.wait_until("the strip is focused", |grid_snapshot| {
        grid_snapshot.contains(">   0  claude")
    });

    // Esc returns input focus to chat without removing the strip.
    zellij.send_stdin(&keys::ESC);
    zellij.wait_until("the strip is blurred", |grid_snapshot| {
        grid_snapshot.contains("    0  claude") && !grid_snapshot.contains(">   0  claude")
    });
    zellij.wait_until("the conversation is still there", |grid_snapshot| {
        grid_snapshot.contains("marker")
    });
    zellij.quit();
}
