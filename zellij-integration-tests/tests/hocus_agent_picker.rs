#![cfg(unix)]

//! The one thing hocus cannot prove to itself: where the agent list lands on
//! the screen.
//!
//! `strip_rows` and the hocus-agents selection logic are unit-tested down to
//! the row text, but they say nothing about layout — the list is a real pane
//! now, spawned by hocus as a split of its own global strip, and only a real
//! session with a real grid to read says the split happened, the rows went to
//! the right pane, and hiding the sidebar takes both panes with it.

use zellij_integration_tests::test_env::write_git_repository;
use zellij_integration_tests::{keys, LayoutInfo, TestRunner, TestSession, TERMINAL_SIZE};

/// hocus with the daemon that finds agents for it, as the real config has it,
/// plus the agents pane hocus spawns beside itself and a binding to reveal the
/// sidebar after it has been hidden.
///
/// `zellij:` rather than `file:` for the same reason `hocus_store` gives: hocus
/// relies on being a builtin for the permissions it never asks for.
const HOCUS_CONFIG: &str = r#"keybinds {
  normal {
    bind "Ctrl b" {
      LaunchOrFocusPlugin "hocus" {
        global true
        edge "left"
        size "50%"
      }
    }
  }
}
plugins {
  hocus location="zellij:hocus" {
    shared true
  }
  hocus-agents location="zellij:hocus-agents" {
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
  hocus-agents location="zellij:hocus-agents" {
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

/// hocus as the left global sidebar it really runs as, beside a pane named
/// after an agent — which is what detection reads when a pane's command names
/// nothing. hocus spawns the agents pane above itself on load.
const HOCUS_LAYOUT: &str = r#"layout {
    global_panes {
        pane edge="left" size="50%" focus=true {
            plugin location="hocus"
        }
    }
    pane name="claude"
}
"#;

/// Put a line into hocus and send it. Pasted rather than typed for the reason
/// `hocus_store` gives: a typed burst outruns the render.
///
/// The wait watches the line's head rather than its tail: the sidebar is half
/// the screen wide, so a long line wraps and its tail spans two grid rows. A
/// bracketed paste arrives whole, so the head proves as much as the tail did.
fn submit(zellij: &TestSession, line: &str) {
    zellij.send_stdin(&keys::BRACKETED_PASTE_START);
    zellij.send_stdin(line.as_bytes());
    zellij.send_stdin(&keys::BRACKETED_PASTE_END);
    zellij.wait_until("the line is in the input box", |grid_snapshot| {
        grid_snapshot.contains(&line[..line.len().min(20)])
    });
    zellij.send_stdin(&keys::ENTER);
}

/// The whole chain, for the case that used to break it: pty records the folder
/// a pane starts in → screen carries it on the manifest → the daemon probes it
/// → the store makes a project out of it → hocus formats the strip → the
/// hocus-agents pane draws the agent under its project.
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
    global_panes {{
        pane edge="left" size="50%" focus=true {{
            plugin location="hocus"
        }}
    }}
    pane name="claude" cwd="{}"
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
    // the one it has out. Both arrive in the agents pane hocus spawned above
    // itself, without anyone focusing anything.
    zellij.wait_until("the agent is under its project", |grid_snapshot| {
        grid_snapshot.contains("hocus-project") && grid_snapshot.contains("trunk")
    });

    let screen = zellij.snapshot();
    let row = |needle: &str| {
        screen
            .row_of_line(needle)
            .unwrap_or_else(|| panic!("{needle:?} is on screen:\n{}", screen.text))
    };
    // The agents pane sits above the conversation in the same strip: headers,
    // then the agent row with the list's selection on it, then the chat title.
    let project = row("hocus-project");
    assert_eq!(row("  trunk"), project + 1, "the branch header under it");
    assert_eq!(row(">   0  claude"), project + 2, "the selected agent row");
    assert!(
        row("hocus — ") > project + 2,
        "the conversation is below the agent list:\n{}",
        screen.text,
    );
    assert!(
        !screen.text.contains("no project"),
        "nothing fell back to the header that names nothing:\n{}",
        screen.text,
    );
    zellij.quit();
}

/// The agent list is a real pane above the conversation, and the sidebar hides
/// and returns as one unit.
#[test]
fn the_agent_list_is_its_own_pane_and_the_sidebar_hides_as_one_strip() {
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

    // Something in the transcript to look for underneath the agent list. A
    // slash command runs the tool inline, so no API key is involved.
    submit(
        &zellij,
        r#"/project_assign {"name": "marker", "root": "/work/marker"}"#,
    );
    zellij.wait_until("the transcript has something in it", |grid_snapshot| {
        grid_snapshot.contains("assigned tab 0 to project marker")
    });

    zellij.wait_until("the agents pane is populated", |grid_snapshot| {
        grid_snapshot.contains("0  claude  running")
    });

    let screen = zellij.snapshot();
    let row = |needle: &str| {
        screen
            .row_of_line(needle)
            .unwrap_or_else(|| panic!("{needle:?} is on screen:\n{}", screen.text))
    };

    // Nothing here owns a project and a pane the harness fakes reports no
    // folder to read a branch out of, so both headers stand in for what they
    // could not name — and the agent sits under them either way, in its own
    // pane above the conversation.
    let no_project = row("no project");
    assert_eq!(row("  no branch"), no_project + 1);
    assert_eq!(row(">   0  claude  running"), no_project + 2);
    let title = row("hocus — ");
    assert!(
        title > no_project + 2,
        "the conversation pane sits below the agent list:\n{}",
        screen.text,
    );
    assert!(
        (title..screen.text.lines().count()).contains(&row("assigned tab 0 to project marker")),
        "the transcript is in the conversation pane:\n{}",
        screen.text,
    );

    // Esc in the conversation hides the whole sidebar strip: the agent list
    // goes with the chat, because they are one sidebar.
    zellij.send_stdin(&keys::ESC);
    zellij.wait_until("the sidebar is hidden", |grid_snapshot| {
        !grid_snapshot.contains("hocus — ") && !grid_snapshot.contains("0  claude  running")
    });

    // And the launch binding brings the whole strip back, both panes intact.
    zellij.send_stdin(&keys::ctrl('b'));
    zellij.wait_until("the sidebar is back", |grid_snapshot| {
        grid_snapshot.contains("hocus — ") && grid_snapshot.contains("0  claude  running")
    });
    zellij.wait_until(
        "the conversation survived the round trip",
        |grid_snapshot| grid_snapshot.contains("assigned tab 0 to project marker"),
    );
    zellij.quit();
}
