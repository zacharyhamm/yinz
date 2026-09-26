#![cfg(unix)]

//! The one thing hocus cannot prove to itself: where the agent list lands on
//! the screen.
//!
//! `strip_rows` and the hocus-agents selection logic are unit-tested down to
//! the row text, but they say nothing about layout — the list is a real pane
//! now, opened as an overlay beside hocus, and only a real
//! session with a real grid to read says the split happened and the rows went
//! to the right pane.

use std::io::{Read, Write};
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
///
/// The pane is an agent by *command*, not by title. A plain `pane name="claude"`
/// is spawned as the default shell, and the harness reports that shell as the
/// pane's process the moment it produces output — at which point `detect_kind`
/// lets the shell outrank the title and the list empties again. Title-only
/// detection is a guess by design, and is unit-tested where it lives.
const NO_SIDEBAR_LAYOUT: &str = r#"layout {
    pane command="claude"
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
        grid_snapshot.contains("Ask something.") && grid_snapshot.contains(" Agents │")
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
    // is running, which is the path detection uses in earnest. Title-only
    // detection is not covered end to end — see `NO_SIDEBAR_LAYOUT` for why it
    // cannot be — only in the unit tests beside `detect_kind`.
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
            && grid_snapshot.contains("Ask something.")
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
    let members = panel_members(&zellij);
    assert_eq!(members[0]["pane_y"], members[1]["pane_y"]);
    assert_eq!(members[0]["multipane"], members[1]["multipane"]);
    assert!(
        !screen.text.contains("no project"),
        "nothing fell back to the header that names nothing:\n{}",
        screen.text,
    );
    zellij.quit();
}

#[test]
fn a_todo_launches_an_agent_with_its_project_and_task_context() {
    use zellij_cli::notes_todos::NotesTodos;
    use zellij_integration_tests::Size;

    let repository = write_git_repository("todo-agent", "main")
        .canonicalize()
        .unwrap();
    let mut zellij = TestRunner::new(Size {
        cols: 120,
        rows: 50,
    })
    .with_config(&format!("mouse_mode true\n{HOCUS_PROJECT_CONFIG}"))
    .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
    .start();
    let shell = zellij.expect_pty_spawn();
    shell.output(b"ready");
    zellij.wait_until("the session is ready", |grid| grid.contains("ready"));
    let request = |command: NotesTodos| {
        let action = command.action(&mut std::io::empty()).unwrap();
        zellij_client::store_client::request(&zellij.session_name().into(), action).unwrap()
    };
    request(NotesTodos::ProjectRemember {
        name: "todo-agent".into(),
        root: repository.to_string_lossy().into_owned(),
    });
    let parent = request(NotesTodos::TodoAdd {
        project: "todo-agent".into(),
        text: "Improve the build".into(),
        parent_id: None,
    });
    let task = "Keep $HOME literal\nand test the build";
    let child = request(NotesTodos::TodoAdd {
        project: "todo-agent".into(),
        text: task.into(),
        parent_id: parent["id"].as_u64(),
    });

    open_sidebar(&zellij);
    zellij.resize(Size {
        cols: 120,
        rows: 50,
    });
    zellij.send_stdin(&keys::SHIFT_TAB);
    zellij.send_stdin(b"\x1b[C");
    zellij.send_stdin(&keys::ENTER);
    zellij.wait_until("the todo panel is open", |grid| {
        grid.contains("Improve the build") && grid.contains("Keep $HOME literal")
    });
    zellij.send_stdin(b"jn");
    zellij.wait_until("the task opens the agent launcher", |grid| {
        grid.contains("New agent") && grid.contains("Prompt (optional)")
    });

    // Keep this test in the remembered checkout. Worktree creation has its own tests.
    let click = |label: &str| {
        let snapshot = zellij.snapshot();
        let (row, line) = snapshot
            .text
            .lines()
            .enumerate()
            .find(|(_, line)| line.contains(label))
            .unwrap_or_else(|| panic!("{label} is missing:\n{}", snapshot.text));
        let column = line[..line.find(label).unwrap()].chars().count();
        zellij.send_stdin(format!("\x1b[<0;{};{}M", column + 1, row + 1).as_bytes());
        zellij.send_stdin(format!("\x1b[<0;{};{}m", column + 1, row + 1).as_bytes());
    };
    click("Create and switch branch");
    zellij.wait_until("branch creation is disabled", |grid| {
        grid.text
            .lines()
            .any(|line| line.contains("Create and switch branch") && line.contains("[ ]"))
    });
    click("Create agent");
    let agent = zellij.expect_pty_spawn();
    let command = agent.run_command();
    assert_eq!(command.command.to_string_lossy(), "zellij");
    assert_eq!(
        &command.args[..5],
        ["hocus", "codex", "--command", "codex", "--"]
    );
    assert_eq!(
        command.cwd.as_ref().unwrap().canonicalize().unwrap(),
        repository.canonicalize().unwrap()
    );
    let prompt = command
        .args
        .last()
        .expect("the task is passed as one argument");
    assert!(prompt.contains(task), "{prompt}");
    assert!(prompt.contains("Improve the build"), "{prompt}");
    let todos = request(NotesTodos::TodoList {
        project: "todo-agent".into(),
        include_done: true,
    });
    let item = &todos["items"][0]["children"][0];
    assert_eq!(item["id"], child["id"]);
    assert_eq!(item["done"], false, "launching does not complete the task");
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
            && grid_snapshot.contains("Ask something.")
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

/// The agent list and the conversation are two panes drawn as one panel: side by side, and with
/// one visibility between them. Esc puts the whole thing away and the launch binding brings the
/// whole thing back.
#[test]
fn the_agent_list_and_conversation_are_one_panel() {
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
        grid_snapshot.contains("0  claude")
    });

    let screen = zellij.snapshot();
    let row = |needle: &str| {
        screen
            .row_of_line(needle)
            .unwrap_or_else(|| panic!("{needle:?} is on screen:\n{}", screen.text))
    };

    // Nothing here owns a project and a pane the harness fakes reports no
    // folder to read a branch out of, so both headers stand in for what they
    // could not name — and the agent sits under them either way, in the list
    // that is the panel's other half.
    let no_project = row("no project");
    assert_eq!(row("  no branch"), no_project + 1);
    // The row and nothing after it: the status cell is `running` until the daemon
    // has polled the pane's screen, then `busy` or `idle`, and this test is about
    // which panes are on screen rather than what the agent is doing.
    assert_eq!(row(">     0  claude"), no_project + 2);
    let members = panel_members(&zellij);
    assert_eq!(members[0]["pane_y"], members[1]["pane_y"]);
    assert_eq!(members[0]["multipane"], members[1]["multipane"]);
    assert!(screen.text.contains("assigned tab 0 to project"));

    // Esc in either half puts the whole panel away: the two hide together, so neither the
    // conversation's title nor the list's rows are left standing on their own.
    zellij.send_stdin(&keys::ESC);
    zellij.wait_until("the panel is put away", |grid_snapshot| {
        !grid_snapshot.contains("hocus — ") && !grid_snapshot.contains("0  claude")
    });

    // And the launch binding brings the whole panel back, both halves together.
    zellij.send_stdin(&keys::ctrl('b'));
    zellij.wait_until("both halves are back", |grid_snapshot| {
        grid_snapshot.contains("hocus — ") && grid_snapshot.contains("0  claude")
    });
    zellij.wait_until(
        "the conversation survived the round trip",
        |grid_snapshot| {
            grid_snapshot.contains("assigned tab 0 to project") && grid_snapshot.contains("marker")
        },
    );
    zellij.quit();
}

/// The panel is one focus ring: the agent list, its history half, the chat tab bar and the box
/// you type in, walked by Tab and Shift+Tab.
///
/// This is the one part of the ring no unit test can reach. The two halves are separate plugin
/// processes and neither can focus itself into the other - the list holds no id for the
/// conversation, so it asks (`AGENT_STRIP_YIELD`) and hocus answers - so a step across the seam
/// is two plugins and the server's focus, all of which only exist in a running session.
///
/// The cursor is the tell. hocus draws one in its input box and the agent list draws none, and
/// only the focused pane's reaches the grid, so a cursor on screen means the keys are in the
/// conversation and no cursor means they are in the list.
#[test]
fn tab_walks_the_keyboard_across_the_panel() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_CONFIG)
        .with_layout(LayoutInfo::Stringified(NO_SIDEBAR_LAYOUT.to_owned()))
        .start();
    let agent = zellij.expect_pty_spawn();
    agent.output(b"$ ");

    open_sidebar(&zellij);
    zellij.send_stdin(&keys::ctrl('b'));
    zellij.wait_until("the conversation has the keys", |grid_snapshot| {
        grid_snapshot.contains("hocus — ") && grid_snapshot.cursor.is_some()
    });
    zellij.wait_until("the agents pane is populated", |grid_snapshot| {
        grid_snapshot.contains("0  claude")
    });

    // Forward out of an empty box. Nothing to complete, so Tab is the ring's: hocus focuses the
    // list and sends it the half to land in, and the conversation's cursor goes with the keys.
    zellij.send_stdin(&keys::TAB);
    zellij.wait_until("the agent list has the keys", |grid_snapshot| {
        grid_snapshot.cursor.is_none()
            && grid_snapshot.contains("Ask something.")
            && grid_snapshot.contains("0  claude")
    });

    // And backward off the list's first stop is the way back - the crossing that has to travel
    // as a request, the list having no id to focus us with.
    zellij.send_stdin(&keys::SHIFT_TAB);
    zellij.wait_until("the box has the keys again", |grid_snapshot| {
        grid_snapshot.cursor.is_some() && grid_snapshot.contains("0  claude")
    });

    // The panel is still whole through all of it: a ring step is a focus, never a hide.
    let screen = zellij.snapshot();
    assert!(
        screen.text.contains("hocus — ") && screen.text.contains(" Agents │"),
        "both halves are still up:\n{}",
        screen.text
    );
    zellij.quit();
}

/// `Ctrl+p f` on the panel, through the real client, the real keybinding and the real grid.
///
/// The panel is two overlays linked into one, and the keystroke is aimed at whichever half has
/// the keyboard - so this is also the test that a linked pair is expanded *as a panel*: both
/// halves grow and the divider keeps its share. The tab bar and the status bar keep their rows,
/// exactly as they do when a tiled pane goes fullscreen; `Ctrl+p Shift+f` is the one that covers
/// them.
///
/// The default layout rather than `NO_SIDEBAR_LAYOUT`, because the chrome is half of what this
/// test is about - and no agent has to be found for a panel to be expanded.
#[test]
fn the_panel_fills_the_viewport_on_ctrl_p_f_and_comes_back() {
    let zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_CONFIG)
        .start();
    zellij.wait_for_app_load();

    // Keep the viewport fixed while remembering the panel's home. The shared
    // opener sends asynchronous resizes, which can move it after this snapshot.
    zellij.send_stdin(&keys::ctrl('b'));
    zellij.wait_until("hocus asks for permissions", |snapshot| {
        snapshot.contains("Allow? (y/n)")
    });
    zellij.send_stdin(&keys::key('y'));
    zellij.wait_until("the conversation overlay is placed", |snapshot| {
        snapshot.contains("hocus — ")
    });
    zellij.send_stdin(&keys::ctrl('a'));

    /// Where the agent list's first row sits. The list is the panel's left half, so this is the
    /// first content cell, one row and column inside its frame even when expanded.
    fn list_corner(grid_snapshot: &GridSnapshot) -> Option<(usize, usize)> {
        grid_snapshot
            .text
            .lines()
            .enumerate()
            .find_map(|(row, line)| {
                line.find(" Agents │")
                    .map(|byte_index| (row, line[..byte_index].chars().count()))
            })
    }

    let initial = zellij.wait_until("the agent list is on screen", |snapshot| {
        list_corner(snapshot).is_some_and(|(_, column)| column == 4)
            && snapshot.contains("Ask something.")
    });
    let home = list_corner(&initial).expect("the agent list is on screen");
    assert!(
        home.0 > 1 && home.1 > 0,
        "the panel starts in a corner rather than filling the screen, got {home:?}"
    );

    zellij.send_stdin(&keys::ctrl('p'));
    zellij.wait_until("pane mode entered", |grid_snapshot| {
        grid_snapshot.contains("Fullscreen")
    });
    zellij.send_stdin(&keys::key('f'));

    let expanded = zellij.wait_until("the panel fills the viewport", |grid_snapshot| {
        list_corner(grid_snapshot) == Some((2, 1))
    });
    assert!(
        expanded.contains("Ask something."),
        "both halves are still drawn, the divider having kept its share:\n{}",
        expanded.text
    );
    assert!(
        expanded.row_of_line("Tab #1") == Some(0),
        "and the tab bar keeps its row, as it does for a fullscreen tiled pane:\n{}",
        expanded.text
    );
    assert!(
        expanded
            .text
            .lines()
            .last()
            .is_some_and(|last| last.contains("PANE")),
        "as does the status bar:\n{}",
        expanded.text
    );

    zellij.send_stdin(&keys::ctrl('p'));
    zellij.wait_until("pane mode entered again", |grid_snapshot| {
        grid_snapshot.contains("Fullscreen")
    });
    zellij.send_stdin(&keys::key('f'));

    zellij.wait_until(
        &format!("the panel is back in its corner {home:?}"),
        |grid_snapshot| list_corner(grid_snapshot) == Some(home),
    );
}

/// The other keystroke, which covers the chrome as well - the difference between the two being
/// the whole reason the plain one stops at the viewport.
#[test]
fn ctrl_p_shift_f_covers_the_tab_bar_too() {
    let zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_CONFIG)
        .start();
    zellij.wait_for_app_load();

    open_sidebar(&zellij);
    zellij.wait_until("the tab bar is on screen to begin with", |grid_snapshot| {
        grid_snapshot.row_of_line("Tab #1") == Some(0)
    });

    zellij.send_stdin(&keys::ctrl('p'));
    zellij.wait_until("pane mode entered", |grid_snapshot| {
        grid_snapshot.contains("Fullscreen")
    });
    zellij.send_stdin(&keys::key('F'));

    zellij.wait_until("the panel covers the tab bar", |grid_snapshot| {
        grid_snapshot.row_of_line("Tab #1").is_none() && grid_snapshot.contains("no agents")
    });
}

#[test]
fn vm_tab_focuses_a_pane_in_another_session_and_dismisses_the_panel() {
    use std::os::unix::fs::PermissionsExt;
    let mut remote = TestRunner::new(TERMINAL_SIZE)
        .skip_concurrency_slot()
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified(
            r#"layout { pane command="sh" firecracker_vm="api" name="vm-focus-target"; }"#.into(),
        ))
        .start();
    let terminal = remote.expect_pty_spawn();
    remote.wait_until("remote VM pane registered", |screen| {
        screen.cursor.is_some()
    });
    terminal.output(b"remote-vm-focus-proof\r\n$ ");
    remote.wait_until("remote VM pane rendered", |screen| {
        screen.contains("remote-vm-focus-proof")
    });
    let dir = tempfile::tempdir().unwrap();
    let stub = dir.path().join("zellij");
    let listing = serde_json::json!({"vms": [{"session": remote.session_name(), "name": "api", "state": "running", "vm_id": "physical-api", "panes": [{"id": terminal.terminal_id().get(), "title": "vm-focus-target", "agent": false}]}], "recoveries": []});
    std::fs::write(&stub, format!("#!/bin/sh\nprintf '%s' '{listing}'\n")).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(std::process::Command::new(&stub)
        .output()
        .unwrap()
        .status
        .success());
    let config = HOCUS_CONFIG.replace(
        "hocus location=\"zellij:hocus\" {",
        &format!(
            "hocus location=\"zellij:hocus\" {{\n    zellij_path \"{}\"",
            stub.display()
        ),
    );
    let mut source = TestRunner::new(TERMINAL_SIZE)
        .skip_concurrency_slot()
        .with_config(&config)
        .with_layout(LayoutInfo::Stringified(NO_SIDEBAR_LAYOUT.into()))
        .start();
    source.expect_pty_spawn().output(b"$ ");
    open_sidebar(&source);
    source.send_stdin(&keys::SHIFT_TAB);
    source.send_stdin(b"\x1b[C\x1b[C");
    source.wait_until(
        "the VM tab has the remote VM and associated pane",
        |screen| screen.contains(" VMs │") && screen.contains("vm-focus-target"),
    );
    source.send_stdin(&keys::TAB);
    source.send_stdin(&keys::ENTER);
    source.send_stdin(&keys::ENTER);
    let destination = source.take_session_switch();
    assert_eq!(
        destination.name.as_ref().map(|name| name.as_str()),
        Some(remote.session_name())
    );
    assert_eq!(
        destination.pane_id,
        Some(zellij_types::data::PaneId::Terminal(terminal.terminal_id()))
    );
    assert!(!source.snapshot().contains("hocus — "));
    let client = remote.attach_client_with_focus(TERMINAL_SIZE, destination.pane_id);
    client.wait_until("the destination pane is visible", |screen| {
        screen.contains("remote-vm-focus-proof")
    });
    client.send_stdin(b"vm-focus-input-proof");
    terminal.wait_for_stdin("input reached the pane in the other session", |bytes| {
        bytes
            .windows(b"vm-focus-input-proof".len())
            .any(|window| window == b"vm-focus-input-proof")
    });
    client.quit();
    source.quit();
    remote.quit();
}

#[test]
fn swamp_issue_details_load_after_moving_the_panel_to_another_tab() {
    let _root = zellij_integration_tests::test_env::init();
    // The Lab API, served for as long as the test runs. No `auth.json` exists in the
    // test's own HOME, so the panel calls it anonymously.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    std::env::set_var(
        "SWAMP_CLUB_URL",
        format!("http://{}", listener.local_addr().unwrap()),
    );
    std::thread::spawn(move || {
        for socket in listener.incoming() {
            let Ok(mut socket) = socket else { continue };
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") && socket.read_exact(&mut byte).is_ok() {
                request.push(byte[0]);
            }
            let request = String::from_utf8_lossy(&request);
            let path = request.split_whitespace().nth(1).unwrap_or_default();
            let issue = r#"{"id":"i-42","number":42,"title":"List proof","type":"bug","status":"open","authorId":"u","authorUsername":"tester","source":"swamp","body":"Issue detail proof"}"#;
            let body = if path.starts_with("/api/v1/lab/issues?") {
                format!(r#"{{"issues":[{issue}],"total":1}}"#)
            } else if path == "/api/v1/lab/issues/42" {
                format!(
                    r#"{{"issue":{issue},"audience":{{"visibility":"public","reasons":[]}},"comments":[]}}"#
                )
            } else {
                r#"{"error":"not found"}"#.to_owned()
            };
            let status = if body.starts_with(r#"{"error""#) {
                "404 Not Found"
            } else {
                "200 OK"
            };
            let _ = socket.write_all(
                format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        }
    });
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_CONFIG)
        .start();
    zellij_integration_tests::claim_first_terminal_and_wait_for_prompt(&zellij);
    open_sidebar(&zellij);
    zellij.send_stdin(&keys::SHIFT_TAB);
    for _ in 0..3 {
        zellij.send_stdin(b"\x1b[C");
    }
    zellij.wait_until("permission requested", |grid| grid.contains("Allow?"));
    zellij.send_stdin(&keys::key('y'));
    zellij.wait_until("issue list loaded", |grid| grid.contains("List proof"));
    zellij.send_stdin(&keys::ctrl('t'));
    zellij.send_stdin(&keys::key('n'));
    zellij.expect_pty_spawn().output(b"new-tab-proof\r\n$ ");
    zellij.wait_until("new tab opened", |grid| grid.contains("new-tab-proof"));
    zellij.send_stdin(&keys::ctrl('b'));
    zellij.wait_until("hocus moved to the new tab", |grid| {
        grid.contains("hocus — ")
    });
    zellij.send_stdin(&keys::ctrl('a'));
    zellij.wait_until("issue list reopened", |grid| grid.contains("List proof"));
    zellij.send_stdin(b"\x1b[B");
    zellij.send_stdin(&keys::ENTER);
    zellij.wait_until("issue details loaded", |grid| {
        grid.contains("Issue detail proof")
    });
    zellij.quit();
}

fn panel_members(zellij: &TestSession) -> Vec<serde_json::Value> {
    let (code, output) = zellij.run_cli_action_reporting(zellij_cli::args::CliAction::ListPanes {
        tab: true,
        command: false,
        state: true,
        geometry: true,
        all: true,
        json: true,
    });
    assert_eq!(code, 0, "{output}");
    serde_json::from_str::<Vec<serde_json::Value>>(&output)
        .unwrap()
        .into_iter()
        .filter(|pane| pane.get("multipane").is_some())
        .collect()
}

fn multipane_startup_and_lifecycle(first: char) {
    use zellij_types::data::PermissionType;
    use zellij_types::multipane::MultipaneInfo;
    // Pregrant only the builtin's existing permission list, so startup focus is observable.
    zellij_integration_tests::test_env::init();
    let cache_path = zellij_runtime::consts::ZELLIJ_PLUGIN_PERMISSIONS_CACHE.clone();
    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    let mut cache =
        zellij_config::permission::PermissionCache::from_path_or_default(Some(cache_path));
    cache.cache(
        zellij_runtime::plugin_location::parse_plugin_location("zellij:hocus", None)
            .unwrap()
            .to_string(),
        vec![
            PermissionType::WebAccess,
            PermissionType::ReadSessionEnvironmentVariables,
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::OpenTerminalsOrPlugins,
            PermissionType::RunCommands,
            PermissionType::ReadPaneContents,
            PermissionType::WriteToStdin,
        ],
    );
    cache.write_to_file().unwrap();
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_CONFIG)
        .with_layout(LayoutInfo::Stringified(NO_SIDEBAR_LAYOUT.into()))
        .start();
    let shell = zellij.expect_pty_spawn();
    shell.output(b"BASE AGENT");
    zellij.send_stdin(&keys::ctrl(first));
    zellij.wait_until("both multipane children load", |grid| {
        grid.contains("Ask something.") && grid.contains(" Agents │")
    });
    zellij.wait_until("requested child retains startup focus", |_| {
        panel_members(&zellij).iter().any(|pane| {
            pane["is_focused"] == true
                && pane["plugin_url"]
                    == if first == 'a' {
                        "hocus-agents"
                    } else {
                        "hocus"
                    }
        })
    });
    let members = panel_members(&zellij);
    assert_eq!(members.len(), 2);
    assert_eq!(members[0]["multipane"], members[1]["multipane"]);
    let info: MultipaneInfo = serde_json::from_value(members[0]["multipane"].clone()).unwrap();
    let focused = members
        .iter()
        .find(|pane| pane["is_focused"] == true)
        .unwrap_or_else(|| {
            panic!(
                "no focused child: {members:#?}\n{}",
                format!("{:?}", zellij.snapshot())
            )
        });
    assert_eq!(
        focused["plugin_url"],
        if first == 'a' {
            "hocus-agents"
        } else {
            "hocus"
        }
    );
    // Drag only the divider. Coordinates in the SGR mouse protocol are one-based.
    let divider = info.x + 1 + (info.columns - 3) * usize::from(info.split.ratio) / 10_000;
    let row = info.y + 3;
    for (button, column, suffix) in [
        (0, divider + 1, 'M'),
        (32, divider + 8, 'M'),
        (0, divider + 8, 'm'),
    ] {
        zellij.send_stdin(format!("\x1b[<{button};{column};{row}{suffix}").as_bytes());
    }
    zellij.wait_until("divider share is stored by the host", |_| {
        panel_members(&zellij)
            .first()
            .is_some_and(|pane| pane["multipane"]["split"]["ratio"] != info.split.ratio)
    });
    let dragged: MultipaneInfo =
        serde_json::from_value(panel_members(&zellij)[0]["multipane"].clone()).unwrap();
    assert_eq!(
        (dragged.x, dragged.y, dragged.columns, dragged.rows),
        (info.x, info.y, info.columns, info.rows)
    );
    for _ in 0..3 {
        zellij.send_stdin(&keys::ESC);
        zellij.send_stdin(&keys::ctrl('b'));
    }
    zellij.wait_until("rapid reveal keeps the whole container visible", |grid| {
        grid.contains("Ask something.") && grid.contains(" Agents │")
    });
    let members = panel_members(&zellij);
    assert!(members
        .iter()
        .all(|pane| pane["is_suppressed"] == false && pane["multipane"]["id"] == info.id.0));
    zellij.send_stdin(&[keys::ctrl('t'), keys::key('n')].concat());
    let new_shell = zellij.expect_pty_spawn();
    new_shell.output(b"NEXT TAB");
    zellij.wait_until("one transfer moves both children to the new tab", |_| {
        let members = panel_members(&zellij);
        members.len() == 2
            && members
                .iter()
                .all(|pane| pane["tab_position"] == 1 && pane["multipane"]["id"] == info.id.0)
    });
    for size in [
        zellij_types::pane_size::Size { rows: 12, cols: 50 },
        TERMINAL_SIZE,
    ] {
        zellij.resize(size);
        zellij.wait_until("both children survive a terminal resize", |_| {
            let members = panel_members(&zellij);
            let outer = members.first().map(|pane| &pane["multipane"]);
            let resized = outer.is_some_and(|outer| {
                if size == TERMINAL_SIZE {
                    outer["columns"] == info.columns && outer["rows"] == info.rows
                } else {
                    outer["columns"].as_u64().unwrap() <= size.cols as u64
                        && outer["rows"].as_u64().unwrap() <= size.rows as u64
                }
            });
            resized
                && members.len() == 2
                && members.iter().all(|pane| {
                    pane["multipane"]["id"] == info.id.0
                        && pane["multipane"]["split"]["ratio"] == dragged.split.ratio
                })
        });
    }
    let owner = panel_members(&zellij)
        .into_iter()
        .find(|pane| pane["plugin_url"] == "hocus")
        .unwrap();
    zellij.run_cli_action(zellij_cli::args::CliAction::FocusPaneId {
        pane_id: format!("plugin_{}", owner["id"]),
    });
    zellij.send_stdin(b"reload-marker");
    zellij.wait_until("input reaches the child before reload", |grid| {
        grid.contains("reload-marker")
    });
    zellij.run_cli_action(zellij_cli::args::CliAction::StartOrReloadPlugin {
        url: "hocus".into(),
        configuration: None,
    });
    zellij.wait_until(
        "reload retains both slots and the dragged divider",
        |grid| grid.contains("Ask something.") && grid.contains(" Agents │"),
    );
    let members = panel_members(&zellij);
    assert_eq!(members.len(), 2);
    assert!(members
        .iter()
        .all(|pane| pane["multipane"]["id"] == info.id.0
            && pane["multipane"]["split"]["ratio"] == dragged.split.ratio));
    zellij.quit();
}

#[test]
fn hocus_multipane_chat_first_keeps_ownership_through_its_lifecycle() {
    multipane_startup_and_lifecycle('b');
}
#[test]
fn hocus_multipane_agents_first_keeps_ownership_through_its_lifecycle() {
    multipane_startup_and_lifecycle('a');
}
