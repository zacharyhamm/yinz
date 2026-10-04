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
//! So the write comes from outside hocus entirely — a `yinz pipe` into
//! hocus-store, the way a script would — and the read is hocus's own tool,
//! printed on the grid. A store reload sits between them, because the thing
//! this file used to guard was a reloaded store silently losing its
//! subscribers, and a reload is still the moment most likely to break the tie.

use yinz_cli::args::CliAction;
use yinz_integration_tests::{keys, LayoutInfo, TestRunner, TestSession, TERMINAL_SIZE};

/// The store loaded in the background, as the real config has it.
///
/// `yinz:` rather than `file:` even though the two point at the same wasm:
/// hocus deliberately does not ask for `MessageAndLaunchOtherPlugins`, relying
/// on a builtin being granted it implicitly, and a `file:` load is not a
/// builtin — every pipe to the store would be denied, and so would the data
/// store itself. `shared true` because hocus warns without it, and the warning
/// would be the first thing on screen.
const HOCUS_CONFIG: &str = r#"plugins {
  hocus location="yinz:hocus" {
    shared true
  }
  hocus-store location="yinz:hocus-store"
  hocus-agentd location="yinz:hocus-agentd"
}
load_plugins clear-defaults=true {
  "hocus-store"
  "hocus-agentd"
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
fn submit(yinz: &TestSession, line: &str) {
    yinz.send_stdin(&keys::BRACKETED_PASTE_START);
    yinz.send_stdin(line.as_bytes());
    yinz.send_stdin(&keys::BRACKETED_PASTE_END);
    yinz.wait_until("the line is in the input box", |grid_snapshot| {
        grid_snapshot.contains(&line[line.len().saturating_sub(20)..])
    });
    yinz.send_stdin(&keys::ENTER);
}

/// A write to the store from outside every plugin, as `yinz pipe` would.
///
/// Deliberately no `plugin` argument: a bare `yinz:hocus-store` url carries
/// none of the alias' configuration, so it would miss the running instance and
/// launch a second one. The host resolves one existing store instance.
fn pipe_to_store(yinz: &mut TestSession, payload: &str) {
    let mut mutation: serde_json::Value = serde_json::from_str(payload).unwrap();
    mutation["structured_reply"] = true.into();
    let (status, output) =
        yinz.run_cli_action_reporting(store_pipe("store_mutate", Some(mutation.to_string())));
    assert_eq!(status, 0, "{output}");
    let reply: serde_json::Value = serde_json::from_str(&output).expect("store mutation reply");
    assert_eq!(reply["ok"], true, "{reply}");
}

fn store_pipe(name: &str, payload: Option<String>) -> CliAction {
    CliAction::Pipe {
        name: Some(name.to_owned()),
        payload,
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
    }
}

#[test]
fn closed_tab_assignments_stay_removed_after_daemon_reload_with_and_without_chat() {
    use serde_json::json;
    use yinz_types::{
        cli::DataRequest,
        internal_ids::SessionName,
        plugin_data::{DataRows, DataValue, FetchRequest},
    };

    for chat in [false, true] {
        let mut yinz = TestRunner::new(TERMINAL_SIZE)
            .with_config(HOCUS_CONFIG)
            .with_layout(LayoutInfo::Stringified(
                if chat {
                    r#"layout { pane { plugin location="hocus"; }; new_tab_template { pane; }; }"#
                } else {
                    "layout { pane; }"
                }
                .into(),
            ))
            .start();
        if chat {
            yinz.wait_until("hocus asks for permissions", |grid| {
                grid.contains("Allow? (y/n)")
            });
            yinz.send_stdin(b"y");
            yinz.wait_until("hocus is ready", |grid| !grid.contains("Allow? (y/n)"));
        } else {
            yinz.expect_pty_spawn().output(b"session ready");
            yinz.wait_until("session is ready", |grid| grid.contains("session ready"));
        }
        pipe_to_store(
            &mut yinz,
            r#"{"op":"project_remember","args":{"name":"work","root":"/work"}}"#,
        );
        let session = SessionName::new(yinz.session_name());
        let operation = |op, args| {
            yinz_client::store_client::request(
                &session,
                yinz_cli::notes_todos::pipe_action(
                    "agent_action",
                    json!({"op":op,"args":args,"structured_reply":true}),
                ),
                None,
            )
            .unwrap()
        };
        let assignments = || -> DataRows {
            serde_json::from_value(
                yinz_client::store_client::data_request(
                    &session,
                    DataRequest::Fetch(
                        FetchRequest::entity("agent_assignment").in_namespace("shared:hocus"),
                    ),
                )
                .unwrap(),
            )
            .unwrap()
        };
        operation("project_assign", json!({"project":"work","tab_id":0}));
        let opened = operation("project_open", json!({"project":"work"}));
        let tab = opened["tab_id"].as_u64().unwrap();
        yinz.expect_pty_spawn().output(b"PROJECT OPEN");
        yinz.wait_until("the new project tab is rendered", |grid| {
            grid.contains("PROJECT OPEN")
        });
        // This requires the daemon to have observed the new tab before closing it.
        operation("project_assign", json!({"project":"work","tab_id":tab}));
        assert_eq!(assignments().rows.len(), 2);
        assert_eq!(yinz.run_cli_action(CliAction::CloseTabById { id: tab }), 0);
        operation("context", json!({}));
        let rows = assignments();
        assert_eq!(rows.rows.len(), 1, "chat loaded: {chat}; {rows:?}");
        assert_eq!(rows.get(&rows.rows[0], "tab_id"), Some(&DataValue::Int(0)));
        assert_eq!(
            yinz.run_cli_action(CliAction::StartOrReloadPlugin {
                url: "yinz:hocus-agentd".into(),
                configuration: None
            }),
            0
        );
        operation("context", json!({}));
        assert_eq!(
            assignments().rows.len(),
            1,
            "reload must not resurrect the closed tab"
        );
        yinz.quit();
    }
}

#[test]
fn store_pipes_ignore_eof_but_reject_invalid_payloads() {
    use serde_json::json;

    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_CONFIG)
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let terminal = yinz.expect_pty_spawn();
    terminal.output(b"session ready");
    yinz.wait_until("session ready for store pipes", |grid| {
        grid.contains("session ready")
    });

    for (name, request, parse_error) in [
        (
            "store_mutate",
            r#"{"op":"project_remember","args":{"name":"p","root":"/work/p"},"structured_reply":true}"#,
            "could not parse mutation",
        ),
        (
            "store_query",
            r#"{"op":"project_list","args":{},"structured_reply":true}"#,
            "could not parse query",
        ),
    ] {
        for explicit_payload in [false, true] {
            let (payload, stdin) = if explicit_payload {
                (Some(request.to_owned()), String::new())
            } else {
                (None, format!("{request}\n"))
            };
            let (status, output) =
                yinz.run_cli_action_reporting_with_stdin(store_pipe(name, payload), Some(&stdin));
            assert_eq!(status, 0, "{name}: {output}");
            assert_eq!(output.lines().count(), 1, "{name}: {output}");
            let reply: serde_json::Value = serde_json::from_str(&output).unwrap();
            assert_eq!(reply["ok"], true, "{name}: {reply}");
        }
        for payload in ["", "not json"] {
            let (_, output) = yinz.run_cli_action_reporting_with_stdin(
                store_pipe(name, Some(payload.to_owned())),
                Some(""),
            );
            assert_eq!(output, format!("{parse_error}\n"));
        }
    }
    for args in [json!([]), json!("bad"), json!(42), json!(true)] {
        for structured in [false, true] {
            let request = json!({
                "op": "project_list", "args": args,
                "structured_reply": structured, "request_id": "invalid-args",
            });
            let (_, output) =
                yinz.run_cli_action_reporting(store_pipe("store_query", Some(request.to_string())));
            if structured {
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&output).unwrap(),
                    json!({"ok": false, "error": "query args must be an object"}),
                );
            } else {
                assert_eq!(output, "query args must be an object\n");
            }
        }
    }
    for (pattern, ok) in [("[", false), ("(?i)panic|ERROR", true)] {
        let request = json!({"op":"agent_history_search", "args":{"query":pattern,"regex":true}, "structured_reply":true});
        let (_, output) =
            yinz.run_cli_action_reporting(store_pipe("store_query", Some(request.to_string())));
        let reply: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(reply["ok"], ok, "{reply}");
        if !ok {
            assert!(reply["error"].as_str().unwrap().contains("invalid regex"));
        }
    }
    // Invalid requests must leave the plugin able to answer valid requests.
    for args in [None, Some(json!(null)), Some(json!({}))] {
        let mut request = json!({"op": "project_list", "structured_reply": true});
        if let Some(args) = args {
            request["args"] = args;
        }
        let (status, output) =
            yinz.run_cli_action_reporting(store_pipe("store_query", Some(request.to_string())));
        assert_eq!(status, 0, "{output}");
        let reply: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(reply["ok"], true, "{reply}");
    }
    yinz.quit();
}

#[test]
fn a_write_from_the_cli_is_visible_in_hocus() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_CONFIG)
        .with_layout(LayoutInfo::Stringified(HOCUS_LAYOUT.to_owned()))
        .start();

    yinz.wait_until("hocus asks for permissions", |grid_snapshot| {
        grid_snapshot.contains("Allow? (y/n)")
    });
    yinz.send_stdin(b"y");
    yinz.wait_until("the prompt is dismissed", |grid_snapshot| {
        !grid_snapshot.contains("Allow? (y/n)")
    });

    // Every todo belongs to a project, so there has to be one first. Written
    // through hocus, so the project rows are the ones hocus itself wrote.
    submit(&yinz, r#"/project_assign {"name": "p", "root": "/work/p"}"#);
    yinz.wait_until("the background project assignment completed", |grid| {
        grid.contains("assigned tab 0 to project p")
    });
    submit(&yinz, r#"/todo_add {"project": "p", "text": "before"}"#);
    yinz.wait_until("the first todo is written", |grid_snapshot| {
        grid_snapshot.contains("Added 1 in p.")
    });

    // The reload. A brand new `State` on the other side — which now holds
    // nothing, so what this really checks is that a fresh instance finds the
    // rows the old one wrote rather than declaring itself an empty store.
    yinz.run_cli_action(CliAction::StartOrReloadPlugin {
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
        &mut yinz,
        r#"{"op":"todo_add","args":{"project":"p","text":"from the cli"}}"#,
    );

    // Clear the transcript first: nothing about the write above has been on
    // this screen, but "before" has, and an empty grid makes the read below the
    // only thing that can put any todo text back.
    yinz.send_stdin(&[0x0c]); // Ctrl-l
    yinz.wait_until("the transcript is cleared", |grid_snapshot| {
        !grid_snapshot.contains("before")
    });

    submit(&yinz, r#"/todo_list {"project": "p"}"#);
    yinz.wait_until(
        "hocus reads the row the CLI wrote through hocus-store",
        |grid_snapshot| grid_snapshot.contains("from the cli"),
    );
    // And the row written before the reload is still there, so the list is the
    // accumulated state rather than whatever the last writer happened to hold.
    yinz.wait_until("and the row written before the reload", |grid_snapshot| {
        grid_snapshot.contains("before")
    });
    yinz.quit();
}

/// The same tie for the notes, which are three entities rather than one: a
/// note the CLI writes through hocus-store, with a tag and a link to a todo
/// hocus wrote, comes back whole from hocus's own tools.
#[test]
fn a_note_written_from_the_cli_is_listed_and_read_in_hocus() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(HOCUS_CONFIG)
        .with_layout(LayoutInfo::Stringified(HOCUS_LAYOUT.to_owned()))
        .start();

    yinz.wait_until("hocus asks for permissions", |grid_snapshot| {
        grid_snapshot.contains("Allow? (y/n)")
    });
    yinz.send_stdin(b"y");
    yinz.wait_until("the prompt is dismissed", |grid_snapshot| {
        !grid_snapshot.contains("Allow? (y/n)")
    });

    submit(&yinz, r#"/project_assign {"name": "p", "root": "/work/p"}"#);
    yinz.wait_until("the background project assignment completed", |grid| {
        grid.contains("assigned tab 0 to project p")
    });
    // A todo for the note to reference, written by hocus so its id is the
    // first the store hands out.
    submit(&yinz, r#"/todo_add {"project": "p", "text": "the item"}"#);
    yinz.wait_until("the todo is written", |grid_snapshot| {
        grid_snapshot.contains("Added 1 in p.")
    });

    pipe_to_store(
        &mut yinz,
        r#"{"op":"note_add","args":{"project":"p","title":"from the cli","body":"the whole body","tags":["cli"],"todo_ids":[1]}}"#,
    );

    yinz.send_stdin(&[0x0c]); // Ctrl-l
    yinz.wait_until("the transcript is cleared", |grid_snapshot| {
        !grid_snapshot.contains("the item")
    });

    submit(&yinz, r#"/note_list {"project": "p"}"#);
    yinz.wait_until(
        "hocus lists the note the CLI wrote through hocus-store",
        |grid_snapshot| grid_snapshot.contains("from the cli"),
    );
    yinz.wait_until("with its tag and its link", |grid_snapshot| {
        grid_snapshot.contains("#cli") && grid_snapshot.contains("todos 1")
    });
    // The body is not in a listing; reading the note is what shows it.
    assert!(!yinz.snapshot().contains("the whole body"));
    submit(&yinz, r#"/note_get {"id": 1}"#);
    yinz.wait_until("and reads its body", |grid_snapshot| {
        grid_snapshot.contains("the whole body")
    });
    // The other direction: the todo listing names the note.
    submit(&yinz, r#"/todo_list {"project": "p"}"#);
    yinz.wait_until("and the todo names the note", |grid_snapshot| {
        grid_snapshot.contains("notes 1")
    });
    yinz.quit();
}

/// The production one-request client talks to a real server and the wasm store.
#[test]
fn named_notes_todos_share_hocus_data_and_survive_reload() {
    use serde_json::{json, Value};
    use yinz_cli::notes_todos::NotesTodos;
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(&HOCUS_CONFIG.replace(
            "hocus-store location=\"yinz:hocus-store\"",
            "hocus-store location=\"yinz:hocus-store\" { test_configuration \"retained\"; }",
        ))
        .with_layout(LayoutInfo::Stringified(HOCUS_LAYOUT.to_owned()))
        .start();
    yinz.wait_until("hocus asks for permissions", |grid| {
        grid.contains("Allow? (y/n)")
    });
    yinz.send_stdin(b"y");
    yinz.wait_until("permissions accepted", |grid| {
        !grid.contains("Allow? (y/n)")
    });
    // The store alias is deliberately nonshared. Both clients get an instance.
    let second_client = yinz.attach_client(TERMINAL_SIZE);
    second_client.wait_until("second client sees hocus", |grid| {
        grid.contains("Ask something.")
    });
    let request = |command: NotesTodos| -> Value {
        let action = command.action(&mut std::io::empty()).unwrap();
        yinz_client::store_client::request(&yinz.session_name().into(), action, None).unwrap()
    };
    request(NotesTodos::ProjectRemember {
        name: "named".into(),
        root: "/work/named".into(),
        worktree_dir: None,
        firecracker_image: None,
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
    submit(&yinz, r#"/todo_list {"project":"named"}"#);
    yinz.wait_until("hocus sees the named command task", |grid| {
        grid.contains("named command task")
    });
    // Reload through the existing acknowledged CLI route. A following request is
    // delivered to the reloaded store, including while its wasm is loading.
    yinz.run_cli_action(CliAction::StartOrReloadPlugin {
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
        yinz_client::store_client::request(&yinz.session_name().into(), missing, None).unwrap_err();
    assert!(error.to_string().contains("no note"));
    second_client.quit();
    yinz.quit();
}

#[test]
fn named_notes_todos_fail_when_the_store_is_missing_without_launching_it() {
    use yinz_cli::notes_todos::NotesTodos;
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified("layout { pane; }".into()))
        .start();
    let terminal = yinz.expect_pty_spawn();
    terminal.output(b"session ready");
    yinz.wait_until("session ready for CLI requests", |grid| {
        grid.contains("session ready")
    });
    let action = NotesTodos::ProjectList
        .action(&mut std::io::empty())
        .unwrap();
    let started = std::time::Instant::now();
    let error =
        yinz_client::store_client::request(&yinz.session_name().into(), action, None).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("No running hocus-store instance"),
        "{error:#}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    yinz.quit();
}
