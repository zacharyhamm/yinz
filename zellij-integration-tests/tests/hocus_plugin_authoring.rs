#![cfg(unix)]

//! Authoring a Lua plugin end to end: a real session, a real `sh`, a real file on disk, and a
//! real Lua plugin loaded from it.
//!
//! The unit tests cover both halves and both pass with the wiring cut. They build an argv from a
//! folder without running it, and they render a command result without having asked for one — so
//! a tool that never reaches the host, or reaches it with a cwd the host rejects, looks exactly
//! like a working one.
//!
//! It also proves the part no unit test can: that a wasm plugin, which cannot open a file in a
//! config dir at all, still gets a plugin written there and then running.

use std::fs;

use zellij_integration_tests::{keys, LayoutInfo, TestRunner, TestSession, TERMINAL_SIZE};

/// `shared true` because hocus warns without it, and the warning would be the first thing on
/// screen. The store is loaded because hocus expects a snapshot from it.
fn hocus_config(user_plugin_dir: &str) -> String {
    format!(
        r#"user_plugin_dir "{user_plugin_dir}"
plugins {{
  hocus location="zellij:hocus" {{
    shared true
  }}
  hocus-store location="zellij:hocus-store"
}}
load_plugins clear-defaults=true {{
  "hocus-store"
}}
"#
    )
}

const HOCUS_LAYOUT: &str = r#"layout {
    pane {
        plugin location="hocus"
    }
}
"#;

/// Put a line into hocus and send it. Slash commands run the tool inline — no API key involved —
/// which is what makes the result readable from the screen.
///
/// Pasted rather than typed: a long line arrives as one `PastedText` event instead of a burst of
/// keystrokes that outruns the render.
fn submit(zellij: &TestSession, line: &str) {
    submit_awaiting(zellij, line, &line[line.len().saturating_sub(20)..]);
}

/// The same, for a line long enough to wrap in the input box — where the tail is split across
/// two rows and so is never one contiguous run in the snapshot. `marker` is something on the
/// line that is not near a wrap.
fn submit_awaiting(zellij: &TestSession, line: &str, marker: &str) {
    zellij.send_stdin(&keys::BRACKETED_PASTE_START);
    zellij.send_stdin(line.as_bytes());
    zellij.send_stdin(&keys::BRACKETED_PASTE_END);
    zellij.wait_until("the line is in the input box", |grid_snapshot| {
        grid_snapshot.contains(marker)
    });
    zellij.send_stdin(&keys::ENTER);
}

/// Ctrl-l. The result of a call echoes its arguments, so the query has to be off the screen
/// entirely for the next answer to be what puts it back.
fn clear(zellij: &TestSession, gone: &str) {
    zellij.send_stdin(&[0x0c]);
    zellij.wait_until("the transcript is cleared", |grid_snapshot| {
        !grid_snapshot.contains(gone)
    });
}

#[test]
fn hocus_writes_a_lua_plugin_and_loads_it() {
    // The folder hocus will be told about, empty to begin with: the write is what creates it,
    // which is the behaviour worth proving as much as the write itself.
    let plugins = tempfile::tempdir().expect("a temp dir");
    let plugin_dir = plugins.path().join("plugins");
    let plugin_dir_string = plugin_dir.display().to_string();

    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(&hocus_config(&plugin_dir_string))
        .with_layout(LayoutInfo::Stringified(HOCUS_LAYOUT.to_owned()))
        .start();

    zellij.wait_until("hocus asks for permissions", |grid_snapshot| {
        grid_snapshot.contains("Allow? (y/n)")
    });
    zellij.send_stdin(b"y");
    zellij.wait_until("the prompt is dismissed", |grid_snapshot| {
        !grid_snapshot.contains("Allow? (y/n)")
    });

    // The docs are compiled in, so they answer with nothing on disk at all.
    submit(&zellij, r#"/plugin_docs {"section": "quickstart"}"#);
    // The tail of the section rather than its head: the transcript is pinned to the bottom, so
    // the last lines of a long result are the ones on screen.
    zellij.wait_until("the guide reaches the model", |grid_snapshot| {
        grid_snapshot.contains("anything else is wasm")
    });
    clear(&zellij, "anything else is wasm");

    // Nothing there yet, and the folder does not even exist.
    submit(&zellij, "/plugin_list");
    zellij.wait_until("the folder is empty", |grid_snapshot| {
        grid_snapshot.contains("no plugins yet")
    });
    clear(&zellij, "no plugins yet");

    // A name that climbs out of the folder is refused before anything runs.
    submit(
        &zellij,
        r#"/plugin_write {"name": "../escape", "source": "return {}"}"#,
    );
    zellij.wait_until("a name that is a path is refused", |grid_snapshot| {
        grid_snapshot.contains("not a plugin name") || grid_snapshot.contains("single file name")
    });
    clear(&zellij, "plugin name");

    // The write itself.
    //
    // No confirmation appears here even though `plugin_write` is in the confirmation table:
    // typing a slash command *is* the confirmation, so `run_manual` goes straight to
    // `run_head`. The prompt the model would get is covered by the unit tests instead.
    let source =
        "local M = {}\\nfunction M.render() return \\\"probe is running\\\" end\\nreturn M";
    submit_awaiting(
        &zellij,
        &format!(r#"/plugin_write {{"name": "probe", "source": "{source}"}}"#),
        "M.render",
    );
    zellij.wait_until("the plugin is written", |grid_snapshot| {
        grid_snapshot.contains("call plugin_reload")
    });

    // On disk, in the folder the write had to create.
    let written = plugin_dir.join("probe.lua");
    assert!(
        written.exists(),
        "expected the plugin at {}",
        written.display()
    );
    let contents = fs::read_to_string(&written).expect("the plugin is readable");
    assert!(contents.contains("probe is running"), "{contents}");
    assert!(contents.ends_with('\n'), "exactly one trailing newline");

    clear(&zellij, "call plugin_reload");

    // Listed by name rather than by path, since a name is what the tools take back.
    submit(&zellij, "/plugin_list");
    zellij.wait_until("the plugin is listed", |grid_snapshot| {
        grid_snapshot.contains("probe.lua")
    });
    clear(&zellij, "probe.lua");

    // Read back through the host, into a plugin that cannot open the file itself.
    submit(&zellij, r#"/plugin_read {"name": "probe"}"#);
    zellij.wait_until("the plugin is read back", |grid_snapshot| {
        grid_snapshot.contains("probe is running")
    });
    clear(&zellij, "probe is running");

    // Writing it again without `overwrite` fails on `set -C` rather than truncating it.
    submit(
        &zellij,
        r#"/plugin_write {"name": "probe", "source": "return {}"}"#,
    );
    zellij.wait_until("the existing plugin is not clobbered", |grid_snapshot| {
        grid_snapshot.contains("already exists")
    });
    let after = fs::read_to_string(&written).expect("the plugin is still readable");
    assert!(
        after.contains("probe is running"),
        "the original must survive a refused overwrite, got: {after}"
    );
    clear(&zellij, "already exists");

    // And finally the whole point: the file becomes a running plugin. `plugin_reload` starts one
    // that is not already running, so this opens it in a pane of its own.
    submit(&zellij, r#"/plugin_reload {"name": "probe"}"#);
    zellij.wait_until("the plugin is loaded and renders", |grid_snapshot| {
        grid_snapshot.contains("probe is running")
    });

    zellij.quit();
}
