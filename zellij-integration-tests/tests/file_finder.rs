#![cfg(unix)]

//! Lifecycle coverage for the `file-finder` plugin.
//!
//! The plugin's pure logic is unit-tested inside the crate. What cannot be tested there
//! is the part welded to the host: which events actually arrive, and whether the pane
//! handoff and teardown really happen. Those are the bugs that only show up when the
//! thing runs, so they are tested here.
//!
//! No real `fzf` is involved. The finder pane is a fake pty, so the test plays fzf's
//! part: it writes a selection to the file the generated script names, then reports the
//! command as exited.

use std::path::{Path, PathBuf};
use zellij_integration_tests::{
    keys, FakePtyHandle, LayoutInfo, TestRunner, TestSession, PROMPT, TERMINAL_SIZE,
};
use zellij_utils::input::command::TerminalAction;

const FINDER_TITLE: &str = "Find file";

/// Keep the session to just the shell pane: the default background plugins would
/// otherwise be spawning work of their own while the test watches for pty spawns.
const NO_BACKGROUND_PLUGINS: &str = "load_plugins clear-defaults=true {\n}\n";

const GLOBAL_AND_SHELL: &str = r#"layout {
    global_panes {
        pane edge="left" size="50%" focus=true
    }
    pane
}
"#;

fn start() -> (TestSession, FakePtyHandle) {
    let zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(NO_BACKGROUND_PLUGINS)
        .start();
    let shell = zellij.expect_pty_spawn();
    shell.output(PROMPT);
    zellij.wait_for_app_load();
    (zellij, shell)
}

/// The argv a fake pty was spawned with.
fn spawned_command(pane: &FakePtyHandle) -> (String, Vec<String>) {
    match pane.terminal_action() {
        Some(TerminalAction::RunCommand(run_command)) => (
            run_command.command.to_string_lossy().to_string(),
            run_command.args.clone(),
        ),
        other => panic!("expected a command pane, got {:?}", other),
    }
}

/// Recover the selection-file path out of the script the plugin generated, which is
/// where it will look for whatever the user picked.
fn selection_file(script: &str) -> PathBuf {
    let tail = script
        .rsplit_once("esac > '")
        .expect("the script should redirect into a quoted selection file")
        .1;
    let path = tail
        .split_once('\'')
        .expect("the redirect target should be closed")
        .0;
    PathBuf::from(path)
}

/// Open the finder and return its pane plus the file it expects a selection in.
fn open_finder(zellij: &TestSession) -> (FakePtyHandle, PathBuf) {
    zellij.send_stdin(&keys::alt('e'));
    let finder = zellij.expect_pty_spawn();

    let (command, args) = spawned_command(&finder);
    assert_eq!(command, "/bin/sh", "the finder should run through a shell");
    assert_eq!(args[0], "-c");
    let script = &args[1];
    assert!(
        script.contains("fzf --prompt"),
        "the script should invoke fzf: {script}"
    );
    assert!(
        script.contains("git rev-parse --show-toplevel"),
        "the script should scope itself to the project: {script}"
    );

    zellij.wait_until("the finder pane is shown", |grid_snapshot| {
        grid_snapshot.contains(FINDER_TITLE)
    });

    (finder, selection_file(script))
}

/// Wait for the plugin to have unlinked its selection file.
///
/// Not a bare `assert!`: every wait before this one watches the *screen*, and the plugin
/// unlinks after an async `close_pane_with_id`, so the repaint that satisfies those waits
/// can beat the unlink. Poll on the same bounded-loop pattern the re-trigger test below
/// uses for the same reason.
fn assert_cleaned_up(selection_file: &Path, what: &str) {
    for _ in 0..50 {
        if !selection_file.exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("{what}: {} is still there", selection_file.display());
}

/// Play fzf: leave a chosen path behind and exit successfully.
fn choose(finder: &FakePtyHandle, selection_file: &PathBuf, chosen: &str) {
    std::fs::write(selection_file, format!("{}\n", chosen)).expect("selection file written");
    finder.exit(Some(0));
}

#[test]
fn a_chosen_file_opens_in_the_finder_pane_and_closes_with_the_viewer() {
    // A configured opener keeps the test off whatever $EDITOR the machine happens to
    // have, and exercises the per-file-type branch at the same time.
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!(
            "{NO_BACKGROUND_PLUGINS}plugins {{\n  file-finder location=\"zellij:file-finder\" {{\n    open_rs \"my-viewer\"\n  }}\n}}\n"
        ))
        .start();
    let shell = zellij.expect_pty_spawn();
    shell.output(PROMPT);
    zellij.wait_for_app_load();

    let (finder, selection_file) = open_finder(&zellij);
    let finder_pane_id = finder.terminal_id();
    choose(&finder, &selection_file, "/tmp/project/src/main.rs");

    // The viewer replaces the finder rather than opening alongside it.
    let viewer = zellij.expect_pty_spawn();
    let (command, args) = spawned_command(&viewer);
    assert_eq!(command, "my-viewer");
    assert_eq!(args, vec!["/tmp/project/src/main.rs".to_owned()]);
    assert_ne!(
        viewer.terminal_id(),
        finder_pane_id,
        "the viewer should be a new pane in the finder's slot"
    );
    zellij.wait_until("the finder pane is gone", |grid_snapshot| {
        !grid_snapshot.contains(FINDER_TITLE)
    });

    // Quitting the viewer takes the floating pane with it and leaves nothing behind.
    viewer.exit(Some(0));
    zellij.wait_until(
        "the overlay is gone once the viewer quits",
        |grid_snapshot| !grid_snapshot.contains("my-viewer") && grid_snapshot.status_bar_appears(),
    );

    assert_cleaned_up(
        &selection_file,
        "the plugin should clean up its selection file",
    );
    zellij.quit();
}

#[test]
fn cancelling_the_finder_dismisses_the_overlay() {
    let (mut zellij, _shell) = start();
    let (finder, selection_file) = open_finder(&zellij);

    // fzf exits non-zero on Esc and writes nothing.
    finder.exit(Some(130));

    zellij.wait_until("the overlay is dismissed", |grid_snapshot| {
        !grid_snapshot.contains(FINDER_TITLE) && grid_snapshot.status_bar_appears()
    });
    assert_cleaned_up(&selection_file, "a cancel should leave no selection file");
    zellij.quit();
}

#[test]
fn finder_from_a_global_pane_covers_it_and_returns_focus() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(NO_BACKGROUND_PLUGINS)
        .with_layout(LayoutInfo::Stringified(GLOBAL_AND_SHELL.to_owned()))
        .start();
    let global = zellij.expect_pty_spawn();
    global.output(b"GLOBAL");
    let shell = zellij.expect_pty_spawn();
    shell.output(PROMPT);
    zellij.wait_until("the global pane and shell load", |snapshot| {
        snapshot.contains("GLOBAL") && snapshot.contains("$")
    });
    zellij.send_stdin(&keys::alt('h'));
    for _ in 0..50 {
        zellij.send_stdin(b"g");
        if global.stdin_bytes().contains(&b'g') {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(global.stdin_bytes().contains(&b'g'), "global pane focused");

    let (finder, _selection_file) = open_finder(&zellij);
    let title_column = zellij
        .snapshot()
        .lines()
        .into_iter()
        .find_map(|line| {
            line.find(FINDER_TITLE)
                .map(|byte_index| line[..byte_index].chars().count())
        })
        .expect("finder title rendered");
    assert!(title_column < TERMINAL_SIZE.cols / 2);

    let mut focused = false;
    for _ in 0..50 {
        zellij.send_stdin(b"q");
        if finder.stdin_bytes().contains(&b'q') {
            focused = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(focused, "the finder should take keyboard focus");
    finder.exit(Some(130));
    zellij.wait_until("the finder closes", |snapshot| {
        !snapshot.contains(FINDER_TITLE)
    });

    let mut restored = false;
    for _ in 0..50 {
        zellij.send_stdin(b"x");
        if global.stdin_bytes().contains(&b'x') {
            restored = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        restored,
        "closing the finder should restore global-pane focus"
    );
    zellij.quit();
}

/// Regression test for the re-trigger guard.
///
/// Two earlier versions of that guard were silently dead. The first watched
/// `Event::Visible`, which reports *tab* visibility and only reaches plugins in
/// `tiled_panes` — never a hidden one. The second looked for its own pane in the
/// `PaneManifest`, where `Tab::pane_infos` hardcodes `is_focused = false` for suppressed
/// panes, so it always looked unfocused. Both left a second keypress focusing the
/// plugin's own empty pane, where the user's typing vanished.
///
/// Focus does land on the plugin for a moment — `LaunchOrFocusPlugin` puts it there and
/// the plugin bounces it back on the next `PaneUpdate`. Keystrokes issued inside that
/// window are genuinely lost, which no person types fast enough to notice but a test
/// does, so this re-offers the keystroke until it lands rather than asserting on a
/// single instant.
#[test]
fn pressing_the_keybinding_again_returns_focus_to_the_open_finder() {
    let (mut zellij, _shell) = start();
    let (finder, _selection_file) = open_finder(&zellij);

    zellij.send_stdin(&keys::alt('e'));

    let mut reached_the_finder = false;
    for _ in 0..50 {
        zellij.send_stdin(b"q");
        if !finder.stdin_bytes().is_empty() {
            reached_the_finder = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    assert!(
        reached_the_finder,
        "after a re-trigger, typing should reach the finder rather than the hidden plugin"
    );

    finder.exit(Some(130));
    zellij.quit();
}
