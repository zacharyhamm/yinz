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

use zellij_cli::args::CliAction;
use zellij_integration_tests::{keys, LayoutInfo, TestRunner, TestSession, TERMINAL_SIZE};

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
/// launch a second one. The host resolves one existing store instance.
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

/// The same tie for the notes, which are three entities rather than one: a
/// note the CLI writes through hocus-store, with a tag and a link to a todo
/// hocus wrote, comes back whole from hocus's own tools.
#[test]
fn a_note_written_from_the_cli_is_listed_and_read_in_hocus() {
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

    submit(
        &zellij,
        r#"/project_assign {"name": "p", "root": "/work/p"}"#,
    );
    // A todo for the note to reference, written by hocus so its id is the
    // first the store hands out.
    submit(&zellij, r#"/todo_add {"project": "p", "text": "the item"}"#);
    zellij.wait_until("the todo is written", |grid_snapshot| {
        grid_snapshot.contains("the item")
    });

    pipe_to_store(
        &mut zellij,
        r#"{"op":"note_add","args":{"project":"p","title":"from the cli","body":"the whole body","tags":["cli"],"todo_ids":[1]}}"#,
    );

    zellij.send_stdin(&[0x0c]); // Ctrl-l
    zellij.wait_until("the transcript is cleared", |grid_snapshot| {
        !grid_snapshot.contains("the item")
    });

    submit(&zellij, r#"/note_list {"project": "p"}"#);
    zellij.wait_until(
        "hocus lists the note the CLI wrote through hocus-store",
        |grid_snapshot| grid_snapshot.contains("from the cli"),
    );
    zellij.wait_until("with its tag and its link", |grid_snapshot| {
        grid_snapshot.contains("#cli") && grid_snapshot.contains("todos 1")
    });
    // The body is not in a listing; reading the note is what shows it.
    assert!(!zellij.snapshot().contains("the whole body"));
    submit(&zellij, r#"/note_get {"id": 1}"#);
    zellij.wait_until("and reads its body", |grid_snapshot| {
        grid_snapshot.contains("the whole body")
    });
    // The other direction: the todo listing names the note.
    submit(&zellij, r#"/todo_list {"project": "p"}"#);
    zellij.wait_until("and the todo names the note", |grid_snapshot| {
        grid_snapshot.contains("notes 1")
    });
    zellij.quit();
}

/// The production one-request client talks to a real server and the wasm store.
#[test]
fn named_notes_todos_share_hocus_data_and_survive_reload() {
    use serde_json::{json, Value};
    use zellij_cli::notes_todos::NotesTodos;
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(&HOCUS_CONFIG.replace(
            "hocus-store location=\"zellij:hocus-store\"",
            "hocus-store location=\"zellij:hocus-store\" { test_configuration \"retained\"; }",
        ))
        .with_layout(LayoutInfo::Stringified(HOCUS_LAYOUT.to_owned()))
        .start();
    zellij.wait_until("hocus asks for permissions", |grid| {
        grid.contains("Allow? (y/n)")
    });
    zellij.send_stdin(b"y");
    zellij.wait_until("permissions accepted", |grid| {
        !grid.contains("Allow? (y/n)")
    });
    // The store alias is deliberately nonshared. Both clients get an instance.
    let second_client = zellij.attach_client(TERMINAL_SIZE);
    second_client.wait_until("second client sees hocus", |grid| {
        grid.contains("Ask something.")
    });
    let request = |command: NotesTodos| -> Value {
        let action = command.action(&mut std::io::empty()).unwrap();
        zellij_client::store_client::request(&zellij.session_name().into(), action).unwrap()
    };
    request(NotesTodos::ProjectRemember {
        name: "named".into(),
        root: "/work/named".into(),
    });
    let item = request(NotesTodos::TodoAdd {
        text: "named command task".into(),
        project: "named".into(),
        parent_id: None,
    });
    let id = item["id"].as_u64().unwrap();
    let note = request(NotesTodos::NoteAdd {
        title: "named command finding".into(),
        project: Some("named".into()),
        global: false,
        body: Some("first line\n\nsecond line".into()),
        body_file: None,
        tags: vec!["design".into()],
        author: Some("codex".into()),
        todo_ids: vec![id],
    });
    let note_id = note["id"].as_u64().unwrap();
    assert_eq!(note["body"], "first line\n\nsecond line");
    let list = request(NotesTodos::TodoList {
        project: "named".into(),
        include_done: true,
    });
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(list["items"][0]["notes"], json!([note_id]));
    let notes = request(NotesTodos::NoteList {
        project: Some("named".into()),
        global: false,
        include_archived: false,
        tag: None,
        todo_id: None,
    });
    assert_eq!(notes["notes"].as_array().unwrap().len(), 1);
    submit(&zellij, r#"/todo_list {"project":"named"}"#);
    zellij.wait_until("hocus sees the named command task", |grid| {
        grid.contains("named command task")
    });
    // Reload through the existing acknowledged CLI route. A following request is
    // delivered to the reloaded store, including while its wasm is loading.
    zellij.run_cli_action(CliAction::StartOrReloadPlugin {
        url: "hocus-store".into(),
        configuration: None,
    });
    request(NotesTodos::TodoAdd {
        text: "one request during reload".into(),
        project: "named".into(),
        parent_id: None,
    });
    let reloaded_list = request(NotesTodos::TodoList {
        project: "named".into(),
        include_done: true,
    });
    assert_eq!(reloaded_list["items"].as_array().unwrap().len(), 2);
    let note_after = request(NotesTodos::NoteGet { id: note_id });
    assert_eq!(note_after["body"], note["body"]);
    assert_eq!(note_after["todo_ids"], json!([id]));
    let missing = NotesTodos::NoteGet { id: 999999 }
        .action(&mut std::io::empty())
        .unwrap();
    let error =
        zellij_client::store_client::request(&zellij.session_name().into(), missing).unwrap_err();
    assert!(error.to_string().contains("no note"));
    second_client.quit();
    zellij.quit();
}

#[test]
fn named_notes_todos_fail_when_the_store_is_missing_without_launching_it() {
    use zellij_cli::notes_todos::NotesTodos;
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let terminal = zellij.expect_pty_spawn();
    terminal.output(b"session ready");
    zellij.wait_until("session ready for CLI requests", |grid| {
        grid.contains("session ready")
    });
    let action = NotesTodos::ProjectList
        .action(&mut std::io::empty())
        .unwrap();
    let started = std::time::Instant::now();
    let error =
        zellij_client::store_client::request(&zellij.session_name().into(), action).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("No running hocus-store instance"),
        "{error:#}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    zellij.quit();
}
