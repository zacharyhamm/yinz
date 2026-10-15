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
use yinz_integration_tests::{keys, FakePtyHandle, TestRunner, TestSession, PROMPT, TERMINAL_SIZE};
use yinz_types::input::command::TerminalAction;

const FINDER_TITLE: &str = "Find file";

/// Keep the session to just the shell pane: the default background plugins would
/// otherwise be spawning work of their own while the test watches for pty spawns.
const NO_BACKGROUND_PLUGINS: &str = "load_plugins clear-defaults=true {\n}\n";

fn start() -> (TestSession, FakePtyHandle) {
    let yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(NO_BACKGROUND_PLUGINS)
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(PROMPT);
    yinz.wait_for_app_load();
    (yinz, shell)
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
fn open_finder(yinz: &TestSession) -> (FakePtyHandle, PathBuf) {
    yinz.send_stdin(&keys::alt('e'));
    let finder = yinz.expect_pty_spawn();

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

    yinz.wait_until("the finder pane is shown", |grid_snapshot| {
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
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(&format!(
            "{NO_BACKGROUND_PLUGINS}plugins {{\n  file-finder location=\"yinz:file-finder\" {{\n    open_rs \"my-viewer\"\n  }}\n}}\n"
        ))
        .start();
    let shell = yinz.expect_pty_spawn();
    shell.output(PROMPT);
    yinz.wait_for_app_load();

    let (finder, selection_file) = open_finder(&yinz);
    let finder_pane_id = finder.terminal_id();
    choose(&finder, &selection_file, "/tmp/project/src/main.rs");

    // The viewer replaces the finder rather than opening alongside it.
    let viewer = yinz.expect_pty_spawn();
    let (command, args) = spawned_command(&viewer);
    assert_eq!(command, "my-viewer");
    assert_eq!(args, vec!["/tmp/project/src/main.rs".to_owned()]);
    assert_ne!(
        viewer.terminal_id(),
        finder_pane_id,
        "the viewer should be a new pane in the finder's slot"
    );
    yinz.wait_until("the finder pane is gone", |grid_snapshot| {
        !grid_snapshot.contains(FINDER_TITLE)
    });

    // Quitting the viewer takes the floating pane with it and leaves nothing behind.
    viewer.exit(Some(0));
    yinz.wait_until(
        "the overlay is gone once the viewer quits",
        |grid_snapshot| !grid_snapshot.contains("my-viewer") && grid_snapshot.status_bar_appears(),
    );

    assert_cleaned_up(
        &selection_file,
        "the plugin should clean up its selection file",
    );
    yinz.quit();
}

#[test]
fn cancelling_the_finder_dismisses_the_overlay() {
    let (mut yinz, _shell) = start();
    let (finder, selection_file) = open_finder(&yinz);

    // fzf exits non-zero on Esc and writes nothing.
    finder.exit(Some(130));

    yinz.wait_until("the overlay is dismissed", |grid_snapshot| {
        !grid_snapshot.contains(FINDER_TITLE) && grid_snapshot.status_bar_appears()
    });
    assert_cleaned_up(&selection_file, "a cancel should leave no selection file");
    yinz.quit();
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
    let (mut yinz, _shell) = start();
    let (finder, _selection_file) = open_finder(&yinz);

    yinz.send_stdin(&keys::alt('e'));

    let mut reached_the_finder = false;
    for _ in 0..50 {
        yinz.send_stdin(b"q");
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
    yinz.quit();
}

#[path = "unit/file_finder/host_paths.rs"]
mod host_paths;

#[path = "unit/file_finder/structured_openers.rs"]
mod structured_openers;

#[path = "unit/file_finder/pick.rs"]
mod pick;

#[path = "unit/file_finder/lifecycle.rs"]
mod lifecycle;
