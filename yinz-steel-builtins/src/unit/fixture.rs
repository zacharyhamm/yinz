//! The integration fixture lives outside the production embedded source tree.
use crate::tests::{plugin_with_source, Host};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use yinz_steel::Runtime;

const SOURCE: &str = include_str!("../../fixtures/fixture-plugin-for-tests.scm");

fn fixture() -> (Runtime, Arc<Mutex<Host>>) {
    plugin_with_source(
        "fixture-plugin-for-tests",
        SOURCE,
        json!({}),
        BTreeMap::new(),
        "",
    )
}

fn key(runtime: &mut Runtime, character: char, modifiers: &[&str]) {
    assert!(runtime
        .update(json!({"Key": {
            "bare_key": {"Char": character.to_string()}, "key_modifiers": modifiers,
        }}))
        .unwrap());
}

fn pipe(runtime: &mut Runtime, source: Value, name: &str, payload: Option<&str>) -> bool {
    runtime
        .pipe(json!({"source": source, "name": name, "payload": payload,
        "args": {}, "is_private": false}))
        .unwrap()
}

#[test]
fn fixture_is_excluded_from_production_and_requests_filesystem_authority() {
    assert!(crate::fixture_source("fixture-plugin-for-tests").is_none());
    let (_, host) = fixture();
    let host = host.lock().unwrap();
    assert!(host.commands[0]["RequestPluginPermissions"]
        .as_array()
        .unwrap()
        .contains(&json!("WriteFiles")));
    assert_eq!(host.commands.last(), Some(&json!("WatchFilesystem")));
    assert!(host.commands[1]["Subscribe"]
        .as_array()
        .unwrap()
        .contains(&json!("BeforeClose")));
}

#[test]
fn fixture_key_matrix_emits_valid_commands_with_the_original_arguments() {
    let (mut runtime, host) = fixture();
    // Every mapped key runs through the typed PluginCommand decoder in the fake host.
    for (modifiers, characters) in [
        (vec![], "abcdefghijklmnopqrstuvwxyz0123456789"),
        (vec!["Ctrl"], "abcdefghijklmnopqrstuvwx1234567890"),
        (vec!["Alt"], "abcdefghijklmnopqrstuvwxy"),
        (vec!["Super"], "abcdefghi"),
        (vec!["Ctrl", "Alt"], "lm"),
    ] {
        for character in characters.chars() {
            let before = host.lock().unwrap().commands.len();
            key(&mut runtime, character, &modifiers);
            assert_eq!(
                host.lock().unwrap().commands.len(),
                before + 1,
                "key {modifiers:?} {character}"
            );
        }
    }
    key(&mut runtime, '8', &["Ctrl"]);
    assert_eq!(
        host.lock().unwrap().commands.last(),
        Some(&json!({"FsWrite": {
            "path":"/host/hi-from-plugin.txt", "contents":"Hi there!", "append":false,"create_dirs":false,
        }}))
    );
    key(&mut runtime, 'g', &["Super"]);
    let host = host.lock().unwrap();
    let highlights = &host.commands.last().unwrap()["SetPaneRegexHighlights"];
    assert_eq!(highlights[0], json!({"Terminal":1}));
    assert_eq!(highlights[1][0]["pattern"], "test_pattern");
    assert_eq!(highlights[1][0]["italic"], true);
}

#[test]
fn fixture_special_keys_preserve_reply_and_error_rendering() {
    let (mut runtime, host) = fixture();
    for (name, value) in [
        ("GenerateRandomName", json!("sample-name")),
        ("GetLayoutDir", json!("/layouts")),
        ("DumpLayout", json!("layout { pane; }")),
        (
            "GetFocusedPaneInfo",
            json!({"tab_id":2,"pane_id":{"Terminal":3}}),
        ),
        (
            "DumpSessionLayout",
            json!({"layout":"layout","metadata":null}),
        ),
        ("ParseLayout", json!({"tabs":[{}]})),
        ("GetSessionEnvironmentVariables", json!({"VAR":"value"})),
    ] {
        host.lock().unwrap().replies.insert(name.into(), value);
    }
    for (character, expected) in [
        ('a', "Generated name"),
        ('b', "Layout dump success: layout { pane; }"),
        ('d', "Got layout folder"),
        ('f', "Session layout: 6 tabs, metadata: null"),
        ('g', "Tab 0 layout: 6 bytes, metadata: null"),
        ('h', "Parse success"),
        ('j', "Got environment variables:"),
        ('l', "open_command_pane: None"),
    ] {
        key(&mut runtime, character, &["Shift", "Ctrl"]);
        assert!(
            runtime.render(10, 80).unwrap().contains(expected),
            "key {character}"
        );
    }
    key(&mut runtime, 'e', &["Ctrl", "Shift"]);
    assert!(runtime
        .take_output()
        .unwrap()
        .contains("Focused pane: tab=2"));
    host.lock()
        .unwrap()
        .failures
        .insert("DumpLayout".into(), "not found".into());
    key(&mut runtime, 'c', &["Ctrl", "Shift"]);
    assert!(runtime
        .render(10, 80)
        .unwrap()
        .contains("Layout dump error: "));
    host.lock()
        .unwrap()
        .failures
        .insert("GenerateRandomName".into(), "denied".into());
    key(&mut runtime, 'a', &["Ctrl", "Shift"]);
    assert_eq!(runtime.render(10, 80).unwrap(), "Error: got empty name\n");
    // An ordinary denied action stays live, and the next key still runs.
    host.lock()
        .unwrap()
        .failures
        .insert("OpenCommandPane".into(), "denied".into());
    key(&mut runtime, 'm', &["Ctrl"]);
    key(&mut runtime, 'l', &["Ctrl", "Shift"]);
    assert!(runtime
        .render(10, 80)
        .unwrap()
        .contains("open_command_pane: None"));
}

#[test]
fn fixture_layout_and_configuration_keys_keep_their_distinct_operations() {
    let (mut runtime, host) = fixture();
    for (character, expected) in [
        ('a', "Save layout success"),
        ('b', "Save layout success"),
        ('c', "Save layout with overwrite success"),
        ('d', "Save invalid layout success"),
        ('e', "Rename layout success"),
        ('f', "Rename nonexistent layout success"),
        ('g', "Delete layout success"),
        ('h', "Delete nonexistent layout success"),
        ('i', "Path traversal save success"),
        ('j', "Edit layout command sent"),
        ('k', "Override layout command sent"),
    ] {
        key(&mut runtime, character, &["Ctrl", "Alt"]);
        assert_eq!(runtime.render(10, 80).unwrap(), format!("{expected}\n"));
    }
    runtime
        .update(json!({"PluginConfigurationChanged":{"changed":"yes"}}))
        .unwrap();
    key(&mut runtime, 'z', &["Ctrl"]);
    assert_eq!(
        host.lock().unwrap().commands.last(),
        Some(&json!({"GoToTabName":"{\"changed\":\"yes\"}"}))
    );
    key(&mut runtime, 'z', &["Alt"]);
    assert_eq!(
        host.lock().unwrap().commands.last(),
        Some(&json!("ListClients"))
    );
}

#[test]
fn fixture_workers_keep_state_and_reply_through_the_parent_event() {
    let (mut runtime, host) = fixture();
    runtime.update(json!("SystemClipboardFailure")).unwrap();
    assert_eq!(
        host.lock().unwrap().commands.last().unwrap()["PostMessageTo"]["worker_name"],
        "test"
    );
    assert_eq!(
        runtime.invoke("yinz-worker-names", vec![]).unwrap(),
        json!(["test"])
    );
    runtime
        .invoke("yinz-worker-bind", vec![json!("test")])
        .unwrap();
    for count in 1..=2 {
        runtime
            .invoke("yinz-worker-message", vec![json!("ping"), json!("payload")])
            .unwrap();
        let expected = format!("payload, received {count} messages");
        assert_eq!(
            host.lock().unwrap().commands.last().unwrap()["PostMessageToPlugin"]["payload"],
            expected
        );
        runtime
            .update(json!({"CustomMessage":["pong",expected]}))
            .unwrap();
        assert!(runtime
            .render(10, 80)
            .unwrap()
            .contains(&format!("Payload from worker: \"{expected}\"")));
    }
}

#[test]
fn fixture_pipes_route_sources_queue_commands_and_one_shot_frames() {
    let (mut runtime, host) = fixture();
    for (source, id) in [
        (json!({"Cli":"pipe-a"}), "pipe-a"),
        (json!({"Plugin":7}), "7"),
        (json!("Keybind"), "keybind"),
    ] {
        assert!(pipe(
            &mut runtime,
            source.clone(),
            "message_name",
            Some("message_payload")
        ));
        assert_eq!(
            host.lock().unwrap().commands.last(),
            Some(&json!({"UnblockCliPipeInput":id}))
        );
        pipe(&mut runtime, source, "message_name_block", None);
        assert_eq!(runtime.render(12, 50).unwrap(), format!("{id} blocked\n"));
    }
    pipe(&mut runtime, json!("Keybind"), "pipe_output", None);
    assert_eq!(
        host.lock().unwrap().commands.last(),
        Some(&json!({"CliPipeOutput":["pipe_output","this_is_my_output"]}))
    );
    pipe(
        &mut runtime,
        json!("Keybind"),
        "pipe_message_to_plugin",
        None,
    );
    assert_eq!(
        host.lock().unwrap().commands.last().unwrap()["MessageToPlugin"]["message_payload"],
        "my_cool_payload"
    );
    pipe(
        &mut runtime,
        json!("Keybind"),
        "message_to_plugin",
        Some("reply"),
    );
    assert_eq!(
        runtime.render(12, 50).unwrap(),
        "Payload from self: \"reply\"\n"
    );
    assert!(runtime
        .render(12, 50)
        .unwrap()
        .contains("Rows: 12, Cols: 50, Received events:"));
    runtime.update(json!("BeforeClose")).unwrap();
    assert_eq!(
        host.lock().unwrap().commands.last(),
        Some(&json!({"HighlightAndUnhighlightPanes":[[{"Terminal":1}],[{"Plugin":1}]]}))
    );
    assert!(runtime.pipe(json!({"source":"Keybind","name":"panic_while_handling_pipe","payload":null,"args":{},"is_private":false})).is_err());
}

#[test]
fn fixture_data_roundtrip_guards_revisions_and_closes_the_watch() {
    let commands = Arc::new(Mutex::new(Vec::<Value>::new()));
    let recorded = commands.clone();
    let mut runtime = Runtime::new(move |command: Value| {
        serde_json::from_value::<yinz_types::data::PluginCommand>(command.clone()).unwrap();
        recorded.lock().unwrap().push(command.clone());
        Ok(if command.get("DataDefineEntity").is_some() { json!({"created":true}) }
        else if command.get("DataWatch").is_some() { json!({"watch_id":9}) }
        else if command.get("DataFetch").is_some() {
            let request = &command["DataFetch"]["request"];
            if request["select"] == json!(["uid","rev"]) {
                json!({"rows":[[{"Text":"fixture-a"},{"Int":4}]]})
            } else if request["search"] == "rebase" {
                json!({"rows":[[{"Text":"fixing a rebase conflict"}]]})
            } else { json!({"rows":[[{"Text":"fixing a rebase conflict"}],[{"Text":"writing the changelog"}]]}) }
        } else if command.get("DataCount").is_some() { json!(2) }
        else { Value::Null })
    }).unwrap();
    runtime.eval(SOURCE).unwrap();
    runtime.load(json!({})).unwrap();
    key(&mut runtime, 'k', &["Ctrl", "Shift"]);
    assert_eq!(runtime.render(10, 80).unwrap(), "Data store: created true, rows [\"fixing a rebase conflict\", \"writing the changelog\"], search hits 1, count 2, watch true\n");
    let commands = commands.lock().unwrap();
    let batch = &commands
        .iter()
        .find(|command| command.get("DataApply").is_some())
        .unwrap()["DataApply"]["mutations"];
    assert_eq!(batch[0]["Put"]["expect_rev"], 4);
    assert_eq!(batch[1]["Put"]["expect_rev"], 0);
    assert_eq!(
        commands.last(),
        Some(&json!({"DataUnwatch":{"watch_id":9}}))
    );
}
