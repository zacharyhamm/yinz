#![cfg(unix)]

//! The one thing hocus cannot prove to itself: where the agent list lands on
//! the screen.
//!
//! `strip_rows` and the hocus-agents selection logic are unit-tested down to
//! the row text, but they say nothing about layout — the list is a real pane
//! now, opened as an overlay beside hocus, and only a real
//! session with a real grid to read says the split happened and the rows went
//! to the right pane.

use std::time::{Duration, Instant};
use zellij_integration_tests::test_env::{write_git_repository, write_git_worktree};
use zellij_integration_tests::{
    keys, GridSnapshot, LayoutInfo, TestRunner, TestSession, TERMINAL_SIZE,
};

/// hocus with the daemon that finds agents for it, plus bindings for the two
/// independent panes.
///
/// `zellij:` rather than `file:` for the same reason `hocus_store` gives: hocus
/// relies on being a builtin for the permissions it never asks for.
const HOCUS_CONFIG: &str = r#"keybinds {
  normal {
    bind "Ctrl b" {
      LaunchOrFocusPlugin "hocus" {
        floating true
        overlay true
        awaits_placement true
        move_to_focused_tab true
      }
    }
    bind "Ctrl a" {
      LaunchOrFocusPlugin "hocus-agents" {
        floating true
        overlay true
        awaits_placement true
        move_to_focused_tab true
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

/// No sidebar at all to begin with: the binding has to build the whole strip.
const NO_SIDEBAR_LAYOUT: &str = r#"layout {
    pane name="claude"
}
"#;

/// The same, plus the store — which is what actually holds projects.
///
/// The daemon finds the checkout an agent is in; only the store turns one into
/// a project and pushes the registry back. Without it every agent groups under
/// the header that names nothing, however well detection worked.
const HOCUS_PROJECT_CONFIG: &str = r#"keybinds {
  normal {
    bind "Ctrl b" {
      LaunchOrFocusPlugin "hocus" {
        floating true
        overlay true
        awaits_placement true
        move_to_focused_tab true
      }
    }
    bind "Ctrl a" {
      LaunchOrFocusPlugin "hocus-agents" {
        floating true
        overlay true
        awaits_placement true
        move_to_focused_tab true
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
  hocus-store location="zellij:hocus-store"
}
load_plugins clear-defaults=true {
  "hocus-agentd"
  "hocus-store"
}
"#;

fn wait_with_resizes(zellij: &TestSession, what: &str, predicate: impl Fn(&GridSnapshot) -> bool) {
    let resizer = zellij.resize_sender();
    let wider = std::cell::Cell::new(false);
    let last_resize = std::cell::Cell::new(Instant::now() - Duration::from_secs(1));
    zellij.wait_until(what, |grid_snapshot| {
        if predicate(grid_snapshot) {
            return true;
        }
        if last_resize.get().elapsed() < Duration::from_millis(250) {
            return false;
        }
        let mut size = TERMINAL_SIZE;
        if wider.replace(!wider.get()) {
            size.cols += 1;
        }
        let _ = resizer.resize(size);
        last_resize.set(Instant::now());
        false
    });
}

fn open_sidebar(zellij: &TestSession) {
    zellij.send_stdin(&keys::ctrl('b'));
    wait_with_resizes(zellij, "hocus asks for permissions", |grid_snapshot| {
        grid_snapshot.contains("Allow? (y/n)")
    });
    zellij.send_stdin(&keys::key('y'));
    wait_with_resizes(
        zellij,
        "the conversation overlay is placed",
        |grid_snapshot| grid_snapshot.contains("hocus — "),
    );

    zellij.send_stdin(&keys::ctrl('a'));
    wait_with_resizes(zellij, "both overlay panes are placed", |grid_snapshot| {
        grid_snapshot.contains("hocus — ") && grid_snapshot.contains("agents")
    });
}

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
    // Deliberately *not* named after an agent: this pane is detected by what it
    // is running, which is the path detection uses in earnest. The title path is
    // covered by the test below.
    let layout = format!(
        r#"layout {{
    pane cwd="{}"
}}
"#,
        repository.display()
    );
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_PROJECT_CONFIG)
        .with_layout(LayoutInfo::Stringified(layout))
        .start();
    let agent = zellij.expect_pty_spawn();
    agent.run_foreground(&["claude"]);
    agent.output(b"$ ");

    open_sidebar(&zellij);

    // The project is named for the folder the checkout is in, and the branch is
    // the one it has out. Both arrive in the agents pane hocus spawned beside
    // itself, without anyone focusing anything.
    zellij.wait_until("the agent is under its project", |grid_snapshot| {
        grid_snapshot.contains("hocus-project")
            && grid_snapshot.contains("trunk")
            && grid_snapshot.contains("hocus — ")
    });

    let screen = zellij.snapshot();
    let row = |needle: &str| {
        screen
            .row_of_line(needle)
            .unwrap_or_else(|| panic!("{needle:?} is on screen:\n{}", screen.text))
    };
    // The agents pane sits beside the conversation in the same strip: headers,
    // then the agent row with the list's selection on it.
    let project = row("hocus-project");
    assert_eq!(row("  trunk"), project + 1, "the branch header under it");
    assert_eq!(
        row(">     0  claude"),
        project + 2,
        "the selected agent row"
    );
    assert_eq!(
        row("hocus — "),
        row("agents ─"),
        "the panes are side by side"
    );
    assert!(
        !screen.text.contains("no project"),
        "nothing fell back to the header that names nothing:\n{}",
        screen.text,
    );
    zellij.quit();
}

/// The wrong-branch bug, end to end with a real `git worktree`.
///
/// Two agents in one repository: one in the primary checkout on `trunk`, one in
/// a worktree nested inside it on `topic`. Discovery makes no second project out
/// of the nested worktree — the primary's root already contains its folder — so
/// both agents sit under one project header and the branch headers are the only
/// thing telling them apart. Getting the branch wrong therefore does not merely
/// mislabel a row, it files an agent under another agent's work.
///
/// The negative assertion is the point: pane 1 must never appear under `trunk`.
#[test]
fn an_agent_in_a_nested_worktree_shows_its_own_branch() {
    let repository = write_git_repository("hocus-nested", "trunk");
    let worktree = write_git_worktree(&repository, "topic", "topic");
    let layout = format!(
        r#"layout {{
    pane cwd="{}"
    pane cwd="{}"
}}
"#,
        repository.display(),
        worktree.display(),
    );
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_PROJECT_CONFIG)
        .with_layout(LayoutInfo::Stringified(layout))
        .start();
    let primary_agent = zellij.expect_pty_spawn();
    primary_agent.run_foreground(&["claude"]);
    primary_agent.output(b"$ ");
    let worktree_agent = zellij.expect_pty_spawn();
    worktree_agent.run_foreground(&["claude"]);
    worktree_agent.output(b"$ ");

    open_sidebar(&zellij);

    // The *indented* headers, and both agent rows. A bare `contains("topic")`
    // is not enough: the project takes its name from whichever checkout is
    // discovered first, so "topic" can already be on screen as the project
    // header while this pane is still waiting under "no branch" for its probe.
    zellij.wait_until("both branch headers are drawn", |grid_snapshot| {
        grid_snapshot.contains("  topic")
            && grid_snapshot.contains("  trunk")
            && grid_snapshot.contains("0  claude")
            && grid_snapshot.contains("1  claude")
            && grid_snapshot.contains("hocus — ")
            && !grid_snapshot.contains("no branch")
            && !grid_snapshot.contains("no project")
    });

    let screen = zellij.snapshot();
    let row = |needle: &str| {
        screen
            .row_of_line(needle)
            .unwrap_or_else(|| panic!("{needle:?} is on screen:\n{}", screen.text))
    };
    // One project, then the two branches with their own agent under each. The
    // headers sort by branch name, so `topic` comes after `trunk`.
    // A branch header per checkout, with that checkout's agent directly beneath
    // it and nothing else between them. The headers sort by branch name, so
    // `topic` comes before `trunk`.
    //
    // The project's *name* is deliberately not asserted: it is taken from
    // whichever of the two checkouts is discovered first, and the probes race.
    // Both agents landing under one project is what matters, and the absence of
    // the header that names nothing says it.
    let topic = row("  topic");
    let trunk = row("  trunk");
    assert_eq!(
        row("  1  claude"),
        topic + 1,
        "the worktree's agent sits under its own branch:\n{}",
        screen.text,
    );
    assert_eq!(
        trunk,
        topic + 2,
        "one agent row between the two branch headers:\n{}",
        screen.text,
    );
    assert_eq!(
        row("  0  claude"),
        trunk + 1,
        "and the primary's agent under the primary's branch:\n{}",
        screen.text,
    );
    assert!(
        !screen.text.contains("no project"),
        "both agents found their project:\n{}",
        screen.text,
    );
    zellij.quit();
}

/// A sidebar beside two unnamed panes — nothing about either says "agent" until
/// something runs in it.
const UNNAMED_PANES_LAYOUT: &str = r#"layout {
    pane
    pane
}
"#;

/// The reported symptom, end to end: an agent that shells out to a tool used to
/// vanish from the list until the tool finished, because the only thing anyone
/// looked at was the *foreground* command.
///
/// The whole chain is real here — `tcgetpgrp` and sysinfo faked at the OS
/// boundary, `CommandChanged` off the pty ticker, the daemon's latch, hocus's
/// strip, the agents pane drawing it.
///
/// The second pane is what makes the negative assertion mean anything. "The row
/// is still there" proves nothing if nothing has looked yet, so the second agent
/// starts *after* the first shells out: both changes travel the same ticker and
/// the same snapshot, so by the time the second is on screen everything the
/// daemon heard about the first has already been applied.
#[test]
fn an_agent_that_shells_out_stays_in_the_list_until_it_is_really_gone() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_CONFIG)
        .with_layout(LayoutInfo::Stringified(UNNAMED_PANES_LAYOUT.to_owned()))
        .start();
    let first = zellij.expect_pty_spawn();
    let second = zellij.expect_pty_spawn();
    first.output(b"$ ");
    second.output(b"$ ");
    open_sidebar(&zellij);

    first.run_foreground(&["claude"]);
    zellij.wait_until("the agent is listed", |grid_snapshot| {
        grid_snapshot.contains("0  claude")
    });

    // The agent runs a tool. It is the same agent doing the same work, and
    // nothing on screen announces the change — so the second pane is the clock.
    // Driving it through two further states guarantees the tick that carried the
    // first pane's change has been all the way through the pipeline by the time
    // we look, even if the two panes' changes straddled a ticker boundary.
    first.run_foreground(&["git", "status"]);
    second.run_foreground(&["aider"]);
    zellij.wait_until("the second agent is listed", |grid_snapshot| {
        grid_snapshot.contains("1  aider")
    });
    second.run_foreground(&["gemini"]);
    zellij.wait_until("and the pipeline has turned over again", |grid_snapshot| {
        grid_snapshot.contains("1  gemini")
    });

    let screen = zellij.snapshot();
    assert!(
        screen.text.contains("0  claude"),
        "an agent that shelled out must not leave the list:\n{}",
        screen.text,
    );

    // Now it really exits, and the pane is back at its own prompt.
    first.return_to_shell();
    zellij.wait_until("the finished agent is gone", |grid_snapshot| {
        !grid_snapshot.contains("0  claude")
    });
    let screen = zellij.snapshot();
    assert!(
        screen.text.contains("1  gemini"),
        "and only that one went:\n{}",
        screen.text,
    );
    zellij.quit();
}

/// A pane launched *as* a command is its own foreground process, so the host
/// reports its argv with `is_foreground` false. Reading that flag as "back at a
/// prompt" would unclassify every agent started from a layout — and nothing but
/// this test would say so.
#[test]
fn a_pane_launched_as_a_command_is_an_agent_without_a_title() {
    let layout = r#"layout {
    pane command="claude"
}
"#;
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_CONFIG)
        .with_layout(LayoutInfo::Stringified(layout.to_owned()))
        .start();
    let agent = zellij.expect_pty_spawn();
    agent.output(b"$ ");
    open_sidebar(&zellij);

    zellij.wait_until("the agent is listed", |grid_snapshot| {
        grid_snapshot.contains("0  claude  running")
    });
    zellij.quit();
}

/// The agent list is a real pane beside the conversation, and either pane can
/// hide without taking the other with it.
#[test]
fn the_agent_list_and_conversation_are_independent_overlay_panes() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_CONFIG)
        .with_layout(LayoutInfo::Stringified(NO_SIDEBAR_LAYOUT.to_owned()))
        .start();
    let agent = zellij.expect_pty_spawn();
    agent.output(b"$ ");

    open_sidebar(&zellij);
    zellij.send_stdin(&keys::ctrl('b'));
    zellij.wait_until("the conversation is ready", |grid_snapshot| {
        grid_snapshot.contains("hocus — ") && grid_snapshot.cursor.is_some()
    });

    // Something in the transcript to look for beside the agent list. A
    // slash command runs the tool inline, so no API key is involved.
    submit(
        &zellij,
        r#"/project_assign {"name": "marker", "root": "/work/marker"}"#,
    );
    zellij.wait_until("the transcript has something in it", |grid_snapshot| {
        grid_snapshot.contains("assigned tab 0 to project") && grid_snapshot.contains("marker")
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
    // pane beside the conversation.
    let no_project = row("no project");
    assert_eq!(row("  no branch"), no_project + 1);
    assert_eq!(row(">     0  claude  running"), no_project + 2);
    let title = row("hocus — ");
    assert_eq!(title, row("agents ─"), "the panes are side by side");
    assert!(screen.text.contains("assigned tab 0 to project"));

    // Esc in the conversation hides only that pane; the list is independent.
    zellij.send_stdin(&keys::ESC);
    zellij.wait_until("the conversation is hidden", |grid_snapshot| {
        !grid_snapshot.contains("hocus — ") && grid_snapshot.contains("0  claude  running")
    });

    // And the launch binding brings the conversation back beside the intact list.
    zellij.send_stdin(&keys::ctrl('b'));
    zellij.wait_until("the sidebar is back", |grid_snapshot| {
        grid_snapshot.contains("hocus — ") && grid_snapshot.contains("0  claude  running")
    });
    zellij.wait_until(
        "the conversation survived the round trip",
        |grid_snapshot| {
            grid_snapshot.contains("assigned tab 0 to project") && grid_snapshot.contains("marker")
        },
    );
    zellij.quit();
}
