#![cfg(unix)]

//! The one thing hocus and hocus-store cannot prove between themselves: that a
//! store which reloads is still pushing snapshots afterwards.
//!
//! Both halves have unit tests, and both pass with the wiring cut — the store
//! answers writes directly, so a subscriber list that silently emptied looks
//! exactly like a working store right up until someone reads through hocus's
//! cached copy. Only a real session with a real reload in it tells them apart.

use zellij_integration_tests::{keys, LayoutInfo, TestRunner, TestSession, TERMINAL_SIZE};
use zellij_utils::cli::CliAction;

/// The store loaded in the background, as the real config has it.
///
/// `zellij:` rather than `file:` even though the two point at the same wasm:
/// hocus deliberately does not ask for `MessageAndLaunchOtherPlugins`, relying
/// on a builtin being granted it implicitly, and a `file:` load is not a
/// builtin — every pipe to the store would be denied. `shared true` because
/// hocus warns without it, and the warning would be the first thing on screen.
const HOCUS_CONFIG: &str = r#"plugins {
  hocus location="zellij:hocus" {
    shared true
  }
  hocus-store location="zellij:hocus-store"
}
load_plugins clear-defaults=true {
  "hocus-store"
}
"#;

/// hocus in a pane rather than in `load_plugins`, because the whole assertion
/// is made by reading its transcript off the grid.
const HOCUS_LAYOUT: &str = r#"layout {
    pane {
        plugin location="hocus"
    }
}
"#;

/// Put a line into hocus and send it. Slash commands run the tool inline — no
/// API key involved — which is what makes hocus's cached copy readable from the
/// screen at all.
///
/// Pasted rather than typed: a line this long arrives as one `PastedText`
/// event instead of forty-odd keystrokes, each of which repaints the whole
/// transcript. Typed, the burst outruns the render and the line arrives half
/// finished.
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
fn the_store_is_still_heard_from_after_it_reloads() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_CONFIG)
        .with_layout(LayoutInfo::Stringified(HOCUS_LAYOUT.to_owned()))
        .start();

    zellij.wait_until("hocus asks for permissions", |grid_snapshot| {
        grid_snapshot.contains("Allow? (y/n)")
    });
    zellij.send_stdin(b"y");
    zellij.wait_until("the prompt is dismissed", |grid_snapshot| {
        !grid_snapshot.contains("Allow? (y/n)")
    });

    // Every todo belongs to a project, so there has to be one first.
    submit(
        &zellij,
        r#"/project_assign {"name": "p", "root": "/work/p"}"#,
    );
    submit(&zellij, r#"/todo_add {"project": "p", "text": "before"}"#);
    zellij.wait_until("the first todo goes through the store", |grid_snapshot| {
        grid_snapshot.contains("before")
    });

    // The reload. Same plugin id on the other side, and a brand new `State`
    // whose subscriber list is empty — which hocus is not told and, since the
    // store only ever speaks when something changes, cannot infer.
    zellij.run_cli_action(CliAction::StartOrReloadPlugin {
        url: "hocus-store".to_owned(),
        configuration: None,
    });

    // A reload is asynchronous and finishing it is not observable from here —
    // a background plugin has no pane to render a loading indication into. The
    // wait guards against the write landing on the *old* instance, which would
    // leave the cache correct for the wrong reason and pass either way. It can
    // only ever cause a false pass, never a false failure, which is the safe
    // direction for the one thing this test exists to catch.
    std::thread::sleep(std::time::Duration::from_secs(2));
    submit(&zellij, r#"/todo_add {"project": "p", "text": "after"}"#);
    zellij.wait_until("the second todo is written", |grid_snapshot| {
        grid_snapshot.contains("after")
    });

    // Clear the transcript before reading the list back: the write's own
    // result echoes the text, so "after" has to be off the screen entirely for
    // the list to be the thing that puts it back.
    zellij.send_stdin(&[0x0c]); // Ctrl-l
    zellij.wait_until("the transcript is cleared", |grid_snapshot| {
        !grid_snapshot.contains("after")
    });

    submit(&zellij, r#"/todo_list {"project": "p"}"#);
    zellij.wait_until(
        "hocus's cached copy has the change made after the reload",
        |grid_snapshot| grid_snapshot.contains("after"),
    );
    zellij.quit();
}
