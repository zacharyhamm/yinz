#![cfg(unix)]

//! The one thing hocus and hocus-store cannot prove between themselves: that
//! they are looking at the same rows.
//!
//! Both halves have thorough unit tests, and both run against a real store
//! engine — but each against its *own* temporary one. Nothing in either crate
//! can tell a shared namespace from two private ones that happen to have the
//! same schema, because a private namespace is derived from the plugin URL and
//! there is no URL under `cargo test`. Only a real session has two plugins with
//! two identities and one database.
//!
//! So the write comes from outside hocus entirely — a `zellij pipe` into
//! hocus-store, the way a script would — and the read is hocus's own tool,
//! printed on the grid. A store reload sits between them, because the thing
//! this file used to guard was a reloaded store silently losing its
//! subscribers, and a reload is still the moment most likely to break the tie.

use zellij_integration_tests::{keys, LayoutInfo, TestRunner, TestSession, TERMINAL_SIZE};
use zellij_utils::cli::CliAction;

/// The store loaded in the background, as the real config has it.
///
/// `zellij:` rather than `file:` even though the two point at the same wasm:
/// hocus deliberately does not ask for `MessageAndLaunchOtherPlugins`, relying
/// on a builtin being granted it implicitly, and a `file:` load is not a
/// builtin — every pipe to the store would be denied, and so would the data
/// store itself. `shared true` because hocus warns without it, and the warning
/// would be the first thing on screen.
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
/// API key involved — which is what makes the store readable from the screen at
/// all.
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

/// A write to the store from outside every plugin, as `zellij pipe` would.
///
/// Deliberately no `plugin` argument: a bare `zellij:hocus-store` url carries
/// none of the alias' configuration, so it would miss the running instance and
/// launch a second one. A broadcast reaches whatever is already running, and
/// `store_mutate` is handled in exactly one place.
fn pipe_to_store(zellij: &mut TestSession, payload: &str) {
    zellij.run_cli_action(CliAction::Pipe {
        name: Some("store_mutate".to_owned()),
        payload: Some(payload.to_owned()),
        args: None,
        plugin: None,
        plugin_configuration: None,
        // Every one of these only matters when the pipe *launches* a plugin,
        // which this must never do — see above.
        force_launch_plugin: false,
        skip_plugin_cache: false,
        floating_plugin: None,
        in_place_plugin: None,
        plugin_cwd: None,
        plugin_title: None,
    });
}

#[test]
fn a_write_from_the_cli_is_visible_in_hocus() {
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

    // Every todo belongs to a project, so there has to be one first. Written
    // through hocus, so the project rows are the ones hocus itself wrote.
    submit(
        &zellij,
        r#"/project_assign {"name": "p", "root": "/work/p"}"#,
    );
    submit(&zellij, r#"/todo_add {"project": "p", "text": "before"}"#);
    zellij.wait_until("the first todo is written", |grid_snapshot| {
        grid_snapshot.contains("before")
    });

    // The reload. A brand new `State` on the other side — which now holds
    // nothing, so what this really checks is that a fresh instance finds the
    // rows the old one wrote rather than declaring itself an empty store.
    zellij.run_cli_action(CliAction::StartOrReloadPlugin {
        url: "hocus-store".to_owned(),
        configuration: None,
    });

    // A reload is asynchronous and finishing it is not observable from here — a
    // background plugin has no pane to render a loading indication into. The
    // wait guards against the write landing on the *old* instance, which would
    // pass for the wrong reason. It can only ever cause a false pass, never a
    // false failure, which is the safe direction.
    std::thread::sleep(std::time::Duration::from_secs(2));

    // The write nothing in either crate's tests can make: from outside both
    // plugins, into the store, addressed to neither.
    pipe_to_store(
        &mut zellij,
        r#"{"op":"todo_add","args":{"project":"p","text":"from the cli"}}"#,
    );

    // Clear the transcript first: nothing about the write above has been on
    // this screen, but "before" has, and an empty grid makes the read below the
    // only thing that can put any todo text back.
    zellij.send_stdin(&[0x0c]); // Ctrl-l
    zellij.wait_until("the transcript is cleared", |grid_snapshot| {
        !grid_snapshot.contains("before")
    });

    submit(&zellij, r#"/todo_list {"project": "p"}"#);
    zellij.wait_until(
        "hocus reads the row the CLI wrote through hocus-store",
        |grid_snapshot| grid_snapshot.contains("from the cli"),
    );
    // And the row written before the reload is still there, so the list is the
    // accumulated state rather than whatever the last writer happened to hold.
    zellij.wait_until("and the row written before the reload", |grid_snapshot| {
        grid_snapshot.contains("before")
    });
    zellij.quit();
}
