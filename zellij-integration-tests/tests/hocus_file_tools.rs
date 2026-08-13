#![cfg(unix)]

//! The file tools end to end: a real session, a real project, a real `rg`.
//!
//! The unit tests cover both halves and both pass with the wiring cut. They
//! build the argv from a project root without running it, and they render a
//! command result without having asked for one — so a tool that never reaches
//! the host, or reaches it with a cwd the host rejects, looks exactly like a
//! working one. Only a session that actually shells out tells them apart.
//!
//! It also proves the part no unit test can: that a wasm plugin, which cannot
//! open a file under a project root at all, still gets that file's contents
//! back.

use std::fs;

use zellij_integration_tests::{keys, LayoutInfo, TestRunner, TestSession, TERMINAL_SIZE};

/// `shared true` because hocus warns without it, and the warning would be the
/// first thing on screen. The store is loaded because hocus expects a snapshot
/// from it; without one the project registry is never pushed back.
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

const HOCUS_LAYOUT: &str = r#"layout {
    pane {
        plugin location="hocus"
    }
}
"#;

/// Put a line into hocus and send it. Slash commands run the tool inline — no
/// API key involved — which is what makes the result readable from the screen.
///
/// Pasted rather than typed: a long line arrives as one `PastedText` event
/// instead of a burst of keystrokes that outruns the render.
fn submit(zellij: &TestSession, line: &str) {
    zellij.send_stdin(&keys::BRACKETED_PASTE_START);
    zellij.send_stdin(line.as_bytes());
    zellij.send_stdin(&keys::BRACKETED_PASTE_END);
    zellij.wait_until("the line is in the input box", |grid_snapshot| {
        grid_snapshot.contains(&line[line.len().saturating_sub(20)..])
    });
    zellij.send_stdin(&keys::ENTER);
}

/// Ctrl-l. The result of a call echoes its arguments, so the query has to be
/// off the screen entirely for the next answer to be what puts it back.
fn clear(zellij: &TestSession, gone: &str) {
    zellij.send_stdin(&[0x0c]);
    zellij.wait_until("the transcript is cleared", |grid_snapshot| {
        !grid_snapshot.contains(gone)
    });
}

#[test]
fn the_file_tools_read_the_active_project_and_nothing_else() {
    // A project of our own rather than the repository: small enough that rg
    // answers at once, and with a file outside it to search for by name.
    //
    // Under `/tmp`, which is load-bearing. The host rewrites a *cwd* under
    // `/tmp` — or `/host`, `/data`, `/cache` — into the plugin's own folder of
    // that name, so a tool that ran its command in the project root would run it
    // in a folder that does not exist and report rg as missing. Arguments are
    // passed through untouched, which is why every path in the argv is absolute
    // and the cwd is `.`.
    let project = tempfile::tempdir().expect("a temp dir");
    let root = project.path().canonicalize().expect("a real path");
    fs::create_dir(root.join("src")).expect("a src dir");
    fs::write(
        root.join("src").join("widget.rs"),
        "fn assemble_widget() {\n    // the needle\n}\n",
    )
    .expect("a source file");

    let outside = tempfile::tempdir().expect("a second temp dir");
    let outside_root = outside.path().canonicalize().expect("a real path");
    fs::write(outside_root.join("secret.txt"), "the needle\n").expect("a file outside");

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

    // Without a project the tools refuse, which is the whole restriction.
    submit(&zellij, r#"/find_files {"pattern": "widget"}"#);
    zellij.wait_until("a file tool needs a project", |grid_snapshot| {
        grid_snapshot.contains("no active project")
    });
    clear(&zellij, "no active project");

    submit(
        &zellij,
        &format!(
            r#"/project_assign {{"name": "p", "root": "{}"}}"#,
            root.display()
        ),
    );
    zellij.wait_until("the project is assigned", |grid_snapshot| {
        grid_snapshot.contains("assigned tab")
    });
    clear(&zellij, "assigned tab");

    // Found by a bare name, which is only a glob because we make it one.
    submit(&zellij, r#"/find_files {"pattern": "widget"}"#);
    zellij.wait_until("the file is found by name", |grid_snapshot| {
        grid_snapshot.contains("widget.rs")
    });
    clear(&zellij, "widget.rs");

    // The contents reach a plugin that cannot open the file itself.
    submit(&zellij, r#"/search_files {"query": "assemble_widget"}"#);
    zellij.wait_until("the line is found by its contents", |grid_snapshot| {
        grid_snapshot.contains("fn assemble_widget")
    });
    clear(&zellij, "fn assemble_widget");

    submit(&zellij, r#"/read_file {"path": "src/widget.rs"}"#);
    zellij.wait_until("the file is read", |grid_snapshot| {
        grid_snapshot.contains("the needle")
    });
    clear(&zellij, "the needle");

    // The same needle, in a file the model names outright, from a folder that
    // is not the project. Refused before anything runs.
    submit(
        &zellij,
        &format!(
            r#"/read_file {{"path": "{}/secret.txt"}}"#,
            outside_root.display()
        ),
    );
    zellij.wait_until("a path outside the project is refused", |grid_snapshot| {
        grid_snapshot.contains("must be inside project p")
    });

    zellij.quit();
}
