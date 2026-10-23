use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use serde_json::{json, Value};
use yinz_steel::Runtime;

use crate::tests::{plugin_with_replies, Host};

fn live(name: &str, active: bool) -> Value {
    json!({
        "name": name, "is_current_session": active, "connected_clients": 1,
        "creation_time": {"secs": 12, "nanos": 0},
        "tabs": [{"tab_id": 7, "position": 0, "name": "shell"}],
        "panes": {"panes": {"0": [{"id": 4, "is_plugin": false, "is_selectable": true,
            "is_suppressed": false, "title": "shell", "exited": false, "exit_status": null}]}},
        "available_layouts": [{"BuiltIn": "default"}, {"File": ["custom", yinz_types::data::LayoutMetadata::default()]}],
    })
}

fn state() -> Value {
    json!({"live_sessions": [live("current", true), live("other", false)], "resurrectable_sessions": [["saved", {"secs": 20, "nanos": 0}]]})
}

pub(super) struct Palette {
    pub(super) runtime: Runtime,
    pub(super) host: Arc<Mutex<Host>>,
    pub(super) scope: Value,
    pub(super) roots: Vec<Value>,
    context: Value,
}

impl Palette {
    fn new(state: Value) -> Self {
        let mut entries = state["live_sessions"].as_array().unwrap().iter().map(|session| json!({
            "destination": "CurrentServer", "name": session["name"], "current": session["is_current_session"],
            "saved": false, "status": if session["is_current_session"] == true { "Current session" } else { "Running" },
        })).collect::<Vec<_>>();
        entries.extend(state["resurrectable_sessions"].as_array().unwrap().iter().map(|saved| json!({
            "destination": "CurrentServer", "name": saved[0], "current": false, "saved": true, "status": "Saved",
        })));
        let replies = BTreeMap::from([
            (
                "GetSessionInventory".into(),
                json!({"current_machine": "Local", "entries": entries}),
            ),
            ("SwitchSessionDestination".into(), json!("Ready")),
            ("GetSessionList".into(), state),
            ("SaveSession".into(), json!(true)),
            ("KillSessionsAndReply".into(), json!(true)),
            ("DeleteDeadSessionAndReply".into(), json!(true)),
            ("GetDisconnectTargets".into(), json!([2])),
            ("CurrentSessionLastSavedTime".into(), json!(1000)),
            ("ListNotifications".into(), json!([])),
        ]);
        Self::for_plugin("session-palette", replies)
    }

    pub(super) fn for_plugin(tag: &str, replies: BTreeMap<String, Value>) -> Self {
        let (runtime, host) = plugin_with_replies(tag, json!({}), replies);
        let registration = host
            .lock()
            .unwrap()
            .commands
            .iter()
            .find_map(|command| {
                let value = &command["CommandPalette"];
                (value["type"] == "register").then(|| value["registration"].clone())
            })
            .unwrap();
        let mut result = Self {
            runtime,
            host,
            roots: vec![],
            scope: json!({"registration": registration, "opening": 1, "view": 0, "revision": 1}),
            context: json!({"clientId": 1, "paneId": {"Terminal": 4}, "tabId": 7, "currentProject": null}),
        };
        result.roots = result.discover(Value::Null, "");
        result
    }

    pub(super) fn stub(&self, name: &str, value: Value) {
        self.host.lock().unwrap().replies.insert(name.into(), value);
    }

    fn reply(&self) -> Result<Value, String> {
        let host = self.host.lock().unwrap();
        let reply = host
            .commands
            .iter()
            .rev()
            .find_map(|command| {
                let value = &command["CommandPalette"];
                (value["type"] == "reply").then_some(&value["result"])
            })
            .unwrap();
        match reply.get("Err") {
            Some(error) => Err(error.as_str().unwrap().to_owned()),
            None => Ok(reply["Ok"].clone()),
        }
    }

    fn send(&mut self, mut message: Value) {
        self.scope["revision"] = json!(self.scope["revision"].as_u64().unwrap() + 1);
        message["scope"] = self.scope.clone();
        message["context"] = self.context.clone();
        self.runtime.palette(message).unwrap();
    }

    pub(super) fn discover(&mut self, source: Value, query: &str) -> Vec<Value> {
        self.send(json!({"type": "discover", "source": source, "query": query}));
        let reply = self.reply().unwrap();
        assert_eq!(reply["type"], "actions");
        reply["actions"].as_array().unwrap().clone()
    }

    pub(super) fn execute(&mut self, handle: Value, values: Value) -> Result<Value, String> {
        self.send(json!({"type": "execute", "handle": handle, "values": values}));
        let result = self.reply()?;
        if result["type"] == "view" {
            self.scope["view"] = json!(self.scope["view"].as_u64().unwrap() + 1);
        }
        Ok(result)
    }

    pub(super) fn root(&self, id: &str) -> Value {
        self.roots.iter().find(|action| action["id"] == id).unwrap()["handle"].clone()
    }

    fn open(&mut self, id: &str) -> Value {
        let result = self.execute(self.root(id), Value::Null).unwrap();
        assert_eq!(result["type"], "view");
        result["view"].clone()
    }

    fn issued(&self, command: Value) -> bool {
        self.host.lock().unwrap().commands.contains(&command)
    }
}

#[test]
fn plugin_palette_loads_from_context_cwd_and_stops_on_lookup_failure() {
    for from_file in [false, true] {
        let mut h = Palette::for_plugin("plugin-palette", BTreeMap::new());
        h.context["paneId"] = json!({"Terminal": 27});
        let mut form = h.open("load");
        if from_file {
            form = h
                .execute(form["actions"][1]["handle"].clone(), json!({}))
                .unwrap()["view"]
                .clone();
        }
        h.host
            .lock()
            .unwrap()
            .failures
            .insert("GetPaneCwd".into(), "pane disappeared".into());
        assert!(h
            .execute(form["handle"].clone(), json!({"location": "./plugin.scm"}))
            .unwrap_err()
            .contains("pane disappeared"));
        assert!(!h
            .host
            .lock()
            .unwrap()
            .commands
            .iter()
            .any(|command| command.get("LoadNewPlugin").is_some()));
        h.host.lock().unwrap().failures.clear();
        for placement in ["Foreground", "Background"] {
            h.stub("GetPaneCwd", json!("/changed/雪"));
            h.execute(
                form["handle"].clone(),
                json!({"location": "./plugin.scm", "placement": placement}),
            )
            .unwrap();
            assert!(h.issued(json!({"GetPaneCwd": {"pane_id": {"Terminal": 27}}})));
            assert!(h.issued(json!({"LoadNewPlugin": {
                "url": if from_file { "file:./plugin.scm" } else { "./plugin.scm" },
                "config": {}, "load_in_background": placement == "Background",
                "skip_plugin_cache": true, "cwd": "/changed/雪"
            }})));
        }
    }
    let mut h = Palette::for_plugin("plugin-palette", BTreeMap::new());
    h.context["paneId"] = json!({"Plugin": 27});
    let form = h.open("load");
    h.execute(form["handle"].clone(), json!({"location": "example"}))
        .unwrap();
    assert!(!h
        .host
        .lock()
        .unwrap()
        .commands
        .iter()
        .any(|command| command.get("GetPaneCwd").is_some()));
    assert!(h.issued(json!({"LoadNewPlugin": {
        "url": "example", "config": {}, "load_in_background": false,
        "skip_plugin_cache": true, "cwd": null
    }})));
}

#[test]
fn plugin_palette_blank_keys_remain_visible_and_keep_their_values() {
    let mut h = Palette::for_plugin(
        "plugin-palette",
        BTreeMap::from([("GetPaneCwd".into(), json!("/chosen"))]),
    );
    let form = h.open("load");
    let configuration = h
        .execute(form["actions"][0]["handle"].clone(), json!({}))
        .unwrap()["view"]
        .clone();
    let add = h.discover(configuration["source"].clone(), "")[0]["handle"].clone();
    let add = h.execute(add, Value::Null).unwrap()["view"]["handle"].clone();
    let keys = [
        String::new(),
        " \t\n".into(),
        "\u{2003}".into(),
        format!("{}hidden", " ".repeat(180)),
    ];
    let mut expected = BTreeMap::new();
    for key in &keys {
        expected.insert(key.clone(), format!("value for {key}"));
        h.execute(add.clone(), json!({"key": key, "value": expected[key]}))
            .unwrap();
    }
    h.execute(form["handle"].clone(), json!({"location": "example"}))
        .unwrap();
    assert!(h.issued(json!({"LoadNewPlugin": {
        "url": "example", "config": expected, "load_in_background": false,
        "skip_plugin_cache": true, "cwd": "/chosen"
    }})));
    for key in keys {
        // Revisit the original unfiltered source after every edit and removal.
        let actions = h.discover(configuration["source"].clone(), "");
        let id = serde_json::to_string(&[&key]).unwrap();
        let entry = actions.iter().find(|action| action["id"] == id).unwrap();
        assert_eq!(
            display_text(&entry["label"]),
            if key.is_empty() {
                "(empty key)"
            } else {
                "(blank key)"
            }
        );
        let edit = h.execute(entry["handle"].clone(), Value::Null).unwrap()["view"].clone();
        assert_eq!(edit["fields"][0]["initialValue"], key);
        assert_eq!(edit["fields"][1]["initialValue"], expected[&key]);
        h.execute(
            edit["handle"].clone(),
            json!({"key": key, "value": "edited\nvalue"}),
        )
        .unwrap();
        expected.insert(key.clone(), "edited\nvalue".into());
        h.execute(form["handle"].clone(), json!({"location": "example"}))
            .unwrap();
        assert!(h.issued(json!({"LoadNewPlugin": {
            "url": "example", "config": expected, "load_in_background": false,
            "skip_plugin_cache": true, "cwd": "/chosen"
        }})));
        let actions = h.discover(configuration["source"].clone(), "");
        let entry = actions.iter().find(|action| action["id"] == id).unwrap();
        let edit = h.execute(entry["handle"].clone(), Value::Null).unwrap()["view"].clone();
        assert_eq!(edit["fields"][1]["initialValue"], "edited\nvalue");
        h.execute(edit["actions"][0]["handle"].clone(), Value::Null)
            .unwrap();
        expected.remove(&key);
        assert_eq!(
            h.discover(configuration["source"].clone(), "").len(),
            expected.len() + 1
        );
    }
    h.execute(form["handle"].clone(), json!({"location": "example"}))
        .unwrap();
    assert!(h.issued(json!({"LoadNewPlugin": {
        "url": "example", "config": {}, "load_in_background": false,
        "skip_plugin_cache": true, "cwd": "/chosen"
    }})));
}

#[test]
fn session_palette_registers_all_actions_and_separates_live_from_saved() {
    let mut h = Palette::new(state());
    assert_eq!(h.roots.len(), 14);
    let view = h.open("switch-session");
    let actions = h.discover(view["source"].clone(), "");
    assert_eq!(
        actions
            .iter()
            .map(|action| (
                json!(display_text(&action["label"])),
                json!(display_text(&action["description"]))
            ))
            .collect::<Vec<_>>(),
        [
            (json!("Local / current"), json!("Current session")),
            (json!("Local / other"), json!("Running")),
            (json!("Local / saved"), json!("Saved"))
        ]
    );
    h.execute(actions[2]["handle"].clone(), Value::Null)
        .unwrap();
    assert!(h.host.lock().unwrap().commands.iter().any(|command| command
        .pointer("/SwitchSessionDestination/Prepare/target")
        == Some(&json!({"name":"saved","destination":"CurrentServer","saved":true}))));
}

#[test]
fn session_palette_pages_after_searching_full_inventory_and_preserves_query() {
    let mut h = Palette::new(
        json!({"live_sessions": (0..205).map(|i| live(&format!("session-{i:03}"), i == 0)).collect::<Vec<_>>(), "resurrectable_sessions": []}),
    );
    let view = h.open("find-pane");
    let first = h.discover(view["source"].clone(), "");
    assert_eq!(first.len(), 99);
    let next = h
        .execute(first.last().unwrap()["handle"].clone(), Value::Null)
        .unwrap();
    let second = h.discover(next["view"]["source"].clone(), "");
    assert!(display_text(&second[0]["description"]).contains("session-098"));
    let found = h.discover(view["source"].clone(), "session-204");
    assert_eq!(found.len(), 1);
    h.execute(found[0]["handle"].clone(), Value::Null).unwrap();
    assert!(h.issued(json!({"SwitchSession": {"name": "session-204", "tab_position": 0, "pane_id": [4, false], "layout": null, "cwd": null}})));
    let view = h.open("switch-session");
    let first = h.discover(view["source"].clone(), "session-1");
    let next = h
        .execute(first.last().unwrap()["handle"].clone(), Value::Null)
        .unwrap();
    assert_eq!(next["view"]["initialQuery"], "session-1");
    let cleared = h.discover(next["view"]["source"].clone(), "");
    assert_eq!(display_text(&cleared[0]["label"]), "Local / session-000");
}

#[test]
fn session_palette_validates_creation_and_offers_collisions_and_configuration() {
    let mut h = Palette::new(state());
    let form = h.open("new-session");
    for (name, label) in [
        ("saved", "Resurrect this session"),
        ("other", "Switch to this session"),
    ] {
        let collision = h
            .execute(form["handle"].clone(), json!({"name": name}))
            .unwrap();
        assert_eq!(collision["view"]["actions"][0]["label"], label);
    }
    for name in [
        "bad/name",
        ".",
        "..",
        "pty-broker-v1.sock",
        "line\nname",
        " ",
    ] {
        assert!(
            h.execute(form["handle"].clone(), json!({"name": name}))
                .is_err(),
            "{name}"
        );
    }
    let configured = h
        .execute(
            form["actions"][0]["handle"].clone(),
            json!({"name": "chosen"}),
        )
        .unwrap();
    let configured = &configured["view"];
    assert_eq!(configured["fields"][0]["initialValue"], "chosen");
    assert_eq!(
        configured["fields"][1]["options"],
        json!(["Default", "custom (custom)", "default (built-in)"])
    );
    h.execute(
        configured["handle"].clone(),
        json!({"name": "chosen", "layout": "custom (custom)", "directory": ""}),
    )
    .unwrap();
    assert!(h.issued(json!({"SwitchSession": {"name": "chosen", "tab_position": null, "pane_id": null, "layout": {"File": ["custom", yinz_types::data::LayoutMetadata::default()]}, "cwd": null}})));
    h.execute(form["handle"].clone(), json!({"name": ""}))
        .unwrap();
    assert!(h.issued(json!({"SwitchSession": {"name": null, "tab_position": null, "pane_id": null, "layout": null, "cwd": null}})));
    h.stub(
        "GetPluginIds",
        json!({"plugin_id": 42, "client_id": 1, "max_session_name_bytes": 4}),
    );
    assert!(h
        .execute(form["handle"].clone(), json!({"name": "雪雪"}))
        .unwrap_err()
        .contains("4 bytes"));
}

#[test]
fn session_palette_reconfirms_changed_destructive_targets() {
    let mut h = Palette::new(state());
    let form = h.open("kill-others");
    assert_eq!(form["confirmation"], true);
    assert!(form["title"].as_str().unwrap().contains("other"));
    assert!(!form["title"].as_str().unwrap().contains("current"));
    h.stub("GetSessionList", json!({"live_sessions": [live("current", true), live("new", false)], "resurrectable_sessions": []}));
    let changed = h.execute(form["handle"].clone(), json!({})).unwrap();
    assert!(changed["view"]["title"]
        .as_str()
        .unwrap()
        .contains("confirm again"));
    assert!(!h.issued(json!({"KillSessionsAndReply": ["other"]})));
    assert!(!h.issued(json!({"KillSessionsAndReply": ["new"]})));
    h.execute(changed["view"]["handle"].clone(), json!({}))
        .unwrap();
    assert!(h.issued(json!({"KillSessionsAndReply": ["new"]})));
}

#[test]
fn session_palette_revalidates_destinations_and_distinguishes_pane_kinds() {
    let mut session = live("other", false);
    let mut plugin = session["panes"]["panes"]["0"][0].clone();
    plugin["is_plugin"] = json!(true);
    session["panes"]["panes"]["0"]
        .as_array_mut()
        .unwrap()
        .push(plugin);
    let mut h = Palette::new(
        json!({"live_sessions": [live("current", true), session], "resurrectable_sessions": []}),
    );
    let view = h.open("find-pane");
    let actions = h.discover(view["source"].clone(), "other");
    assert_eq!(actions.len(), 2);
    assert_ne!(actions[0]["id"], actions[1]["id"]);
    h.execute(actions[1]["handle"].clone(), Value::Null)
        .unwrap();
    assert!(h.issued(json!({"SwitchSession": {"name": "other", "tab_position": 0, "pane_id": [4, true], "layout": null, "cwd": null}})));
    session["panes"]["panes"]["0"] = json!([]);
    h.stub(
        "GetSessionList",
        json!({"live_sessions": [session], "resurrectable_sessions": []}),
    );
    assert!(h
        .execute(actions[0]["handle"].clone(), Value::Null)
        .unwrap_err()
        .contains("Pane no longer exists"));
    let view = h.open("find-tab");
    let actions = h.discover(view["source"].clone(), "other");
    h.stub(
        "GetSessionList",
        json!({"live_sessions": [live("current", true)], "resurrectable_sessions": []}),
    );
    assert!(h
        .execute(actions[0]["handle"].clone(), Value::Null)
        .unwrap_err()
        .contains("Session no longer exists"));
}

#[test]
fn session_palette_notifications_search_full_messages_but_bound_previews() {
    let mut h = Palette::new(state());
    h.stub("SaveSession", json!(false));
    assert!(h.execute(h.root("save"), Value::Null).is_err());
    h.stub(
        "ListNotifications",
        json!([
            {"id": 1, "received_at_unix_ms": 1, "message": "old"},
            {"id": 2, "received_at_unix_ms": 2, "message": "new\nfull message"},
        ]),
    );
    let view = h.open("notifications");
    let choices = h.discover(view["source"].clone(), "");
    assert_eq!(
        choices.iter().map(|item| &item["id"]).collect::<Vec<_>>(),
        [&json!("2"), &json!("1")]
    );
    let detail = h
        .execute(choices[0]["handle"].clone(), Value::Null)
        .unwrap();
    assert_eq!(display_text(&detail["view"]["text"]), "new\nfull message");
    assert_eq!(h.discover(view["source"].clone(), "full message").len(), 1);
    let message = format!("{}searchable ending", "🔔 long notification ".repeat(220));
    h.stub(
        "ListNotifications",
        json!((0..205)
            .map(|id| json!({"id": id, "received_at_unix_ms": id, "message": message}))
            .collect::<Vec<_>>()),
    );
    let choices = h.discover(view["source"].clone(), "searchable ending");
    assert_eq!(choices.len(), 99);
    assert!(serde_json::to_vec(&choices).unwrap().len() < 100_000);
    let detail = h
        .execute(choices[0]["handle"].clone(), Value::Null)
        .unwrap();
    assert_eq!(display_text(&detail["view"]["text"]), message);
}

#[test]
fn session_palette_rename_save_resurrect_delete_details_and_focus_use_typed_commands() {
    let mut h = Palette::new(state());
    let rename = h.open("rename");
    h.execute(rename["handle"].clone(), json!({"name": "renamed"}))
        .unwrap();
    assert!(h.issued(json!({"RenameSession": "renamed"})));
    h.execute(h.root("save"), Value::Null).unwrap();
    let resurrect = h.open("resurrect");
    assert_eq!(
        h.discover(resurrect["source"].clone(), "")[0]["label"],
        "saved"
    );
    let details = h.open("details");
    let choices = h.discover(details["source"].clone(), "current");
    let detail = h
        .execute(choices[0]["handle"].clone(), Value::Null)
        .unwrap();
    assert!(display_text(&detail["view"]["text"]).contains("Last save: 1s ago"));
    let deletion = h.open("delete-all");
    assert_eq!(deletion["confirmation"], true);
    h.execute(deletion["handle"].clone(), json!({})).unwrap();
    assert!(h.issued(json!({"DeleteDeadSessionAndReply": "saved"})));
    let single = h.open("delete");
    assert_eq!(h.discover(single["source"].clone(), "").len(), 1);
    let kill = h.open("kill");
    assert_eq!(h.discover(kill["source"].clone(), "").len(), 2);
    let panes = h.open("find-pane");
    let current = h.discover(panes["source"].clone(), "current");
    h.execute(current[0]["handle"].clone(), Value::Null)
        .unwrap();
    assert!(h.issued(json!({"FocusTerminalPane": [4, true, false]})));
}

#[test]
fn session_palette_directory_picker_preserves_values_on_selection_and_cancel() {
    let mut h = Palette::new(state());
    let form = h.open("new-session");
    let configured = h
        .execute(
            form["actions"][0]["handle"].clone(),
            json!({"name": "kept"}),
        )
        .unwrap();
    let browse = configured["view"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "browse:directory")
        .unwrap()["handle"]
        .clone();
    for selected in [json!("/chosen"), Value::Null] {
        h.send(json!({"type": "execute", "handle": browse, "values": {"name": "edited", "layout": "custom (custom)", "directory": "/original"}}));
        let pick = h
            .host
            .lock()
            .unwrap()
            .commands
            .iter()
            .rev()
            .find_map(|command| command.get("Pick").and_then(|pick| pick.get("Start")))
            .unwrap()
            .clone();
        assert_eq!(
            pick["options"],
            json!({"kind": "directory", "roots": ["/original"]})
        );
        h.runtime.update(json!({"PickResult": {"request_id": pick["request_id"], "result": {"Ok": selected}}})).unwrap();
        let reply = h.reply().unwrap();
        assert_eq!(
            reply,
            json!({"type": "fields", "values": {"name": "edited", "layout": "custom (custom)", "directory": selected.as_str().unwrap_or("/original")}})
        );
        let host = h.host.lock().unwrap();
        let transitions = host
            .commands
            .iter()
            .filter_map(|command| {
                let value = &command["CommandPalette"];
                (value["type"] == "suspend").then_some(value["suspended"].clone())
            })
            .collect::<Vec<_>>();
        assert!(transitions.ends_with(&[json!(true), json!(false)]));
    }
}

#[test]
fn session_palette_disconnect_rechecks_targets_and_released_handles_cannot_mutate() {
    let mut h = Palette::new(state());
    let form = h.open("disconnect");
    h.stub("GetDisconnectTargets", json!([3, 2]));
    let changed = h.execute(form["handle"].clone(), json!({})).unwrap();
    let changed = &changed["view"];
    assert!(changed["title"].as_str().unwrap().contains("clients 2, 3?"));
    assert!(!h.issued(json!("DisconnectOtherClients")));
    for failed in [json!([]), json!({"error": "query failed"})] {
        h.stub("GetDisconnectTargets", failed);
        assert!(h.execute(changed["handle"].clone(), json!({})).is_err());
        assert!(!h.issued(json!("DisconnectOtherClients")));
    }
    h.stub("GetDisconnectTargets", json!([3, 2]));
    h.execute(changed["handle"].clone(), json!({})).unwrap();
    assert!(h.issued(json!("DisconnectOtherClients")));
    h.runtime
        .palette(json!({"type": "release", "handles": [form["handle"]]}))
        .unwrap();
    assert!(h
        .execute(form["handle"].clone(), json!({}))
        .unwrap_err()
        .contains("no longer available"));
    h.stub("GetDisconnectTargets", json!([4]));
    assert!(h.open("disconnect")["title"]
        .as_str()
        .unwrap()
        .contains("clients 4?"));
}

#[test]
fn session_palette_scopes_callbacks_to_client_and_opening_and_cancels_pending_picks() {
    let mut h = Palette::new(state());
    let form = h.open("new-session");
    h.scope["opening"] = json!(2);
    assert!(h
        .execute(form["handle"].clone(), json!({"name": "unexpected"}))
        .is_err());
    h.scope["opening"] = json!(1);
    h.stub(
        "GetPluginIds",
        json!({"plugin_id": 42, "client_id": 2, "max_session_name_bytes": 80}),
    );
    assert!(h
        .execute(form["handle"].clone(), json!({"name": "unexpected"}))
        .is_err());
    h.stub(
        "GetPluginIds",
        json!({"plugin_id": 42, "client_id": 1, "max_session_name_bytes": 80}),
    );
    let configured = h
        .execute(form["actions"][0]["handle"].clone(), json!({}))
        .unwrap();
    let browse = configured["view"]["actions"][0]["handle"].clone();
    h.send(json!({"type": "execute", "handle": browse, "values": {"directory": "/tmp"}}));
    let pick = h
        .host
        .lock()
        .unwrap()
        .commands
        .iter()
        .rev()
        .find_map(|command| command.get("Pick").and_then(|pick| pick.get("Start")))
        .unwrap()
        .clone();
    h.runtime
        .palette(json!({"type": "dismiss", "opening": 1}))
        .unwrap();
    assert!(
        h.issued(json!({"Pick": {"Cancel": {"request_id": pick["request_id"], "timeout": false}}}))
    );
    let count = h.host.lock().unwrap().commands.len();
    h.runtime
        .update(
            json!({"PickResult": {"request_id": pick["request_id"], "result": {"Ok": "/late"}}}),
        )
        .unwrap();
    assert_eq!(h.host.lock().unwrap().commands.len(), count);
    assert!(h.execute(browse, json!({})).is_err());
}

#[test]
fn session_palette_failed_picker_unsuspends_the_form_and_clears_pending_state() {
    let mut h = Palette::new(state());
    let form = h.open("new-session");
    let configured = h
        .execute(form["actions"][0]["handle"].clone(), json!({}))
        .unwrap();
    h.host
        .lock()
        .unwrap()
        .failures
        .insert("Pick".into(), "picker unavailable".into());
    let error = h
        .execute(
            configured["view"]["actions"][0]["handle"].clone(),
            json!({"directory": "/tmp"}),
        )
        .unwrap_err();
    assert!(error.contains("picker unavailable"));
    let host = h.host.lock().unwrap();
    let transitions = host
        .commands
        .iter()
        .filter_map(|command| {
            let value = &command["CommandPalette"];
            (value["type"] == "suspend").then_some(value["suspended"].clone())
        })
        .collect::<Vec<_>>();
    assert_eq!(transitions, [json!(true), json!(false)]);
    drop(host);
    h.runtime
        .palette(json!({"type": "dismiss", "opening": 1}))
        .unwrap();
}

#[test]
fn session_palette_searches_displayed_tab_names_and_keeps_tab_identity() {
    let mut session = live("current", true);
    session["tabs"][0]["name"] = json!("🔔 [4] yinz:topic #2");
    let mut h = Palette::new(json!({"live_sessions": [session], "resurrectable_sessions": []}));
    let view = h.open("find-tab");
    let actions = h.discover(view["source"].clone(), "yinz:topic");
    assert_eq!(actions.len(), 1);
    assert!(display_text(&actions[0]["label"]).contains("🔔 [4] yinz:topic #2"));
    h.execute(actions[0]["handle"].clone(), Value::Null)
        .unwrap();
    assert!(h.issued(json!({"GoToTabWithId": 7})));
}

fn display_text(value: &Value) -> String {
    yinz_tile_markdown::render(
        value.as_str().unwrap(),
        10000,
        yinz_tile_markdown::Options::compact(),
    )
    .into_iter()
    .map(|line| line.text)
    .collect::<Vec<_>>()
    .join("\n")
}

#[test]
fn session_palette_searches_hosts_and_keeps_distinct_names_during_incremental_discovery() {
    let mut h = Palette::new(state());
    let entries = json!([
        {"destination":"CurrentServer","name":"same","status":"Current session","current":true,"saved":false},
        {"destination":"Local","name":"same","status":"Running","current":false,"saved":false},
        {"destination":{"Remote":"host-b"},"name":null,"status":"loading…","current":false,"saved":false}
    ]);
    h.stub(
        "GetSessionInventory",
        json!({"current_machine":{"Remote":"host-a"},"entries":entries}),
    );
    let view = h.open("switch-session");
    let initial = h.discover(view["source"].clone(), "same");
    assert_eq!(initial.len(), 2);
    assert_ne!(initial[0]["id"], initial[1]["id"]);
    assert_eq!(display_text(&initial[0]["label"]), "Local / same");
    assert_eq!(display_text(&initial[1]["label"]), "host-a / same");
    let loading = h.discover(view["source"].clone(), "host-b");
    assert_eq!(loading.len(), 1);
    assert_eq!(loading[0]["description"], "loading…");
    let mut entries = entries.as_array().unwrap().clone();
    entries[2]["name"] = json!("same");
    entries[2]["status"] = json!("Running");
    h.stub(
        "GetSessionInventory",
        json!({"current_machine":{"Remote":"host-a"},"entries":entries}),
    );
    let refreshed = h.discover(view["source"].clone(), "same");
    assert_eq!(refreshed.len(), 3);
    assert_eq!(refreshed[0]["id"], initial[0]["id"]);
    let remote = h.discover(view["source"].clone(), "host-b same");
    assert_eq!(remote.len(), 1);
    h.execute(remote[0]["handle"].clone(), Value::Null).unwrap();
    assert!(h.host.lock().unwrap().commands.iter().any(|command| command
        .pointer("/SwitchSessionDestination/Prepare/target")
        == Some(&json!({"name":"same","destination":{"Remote":"host-b"},"saved":false}))));
}

#[test]
fn releasing_session_picker_cancels_its_client_inventory() {
    let mut h = Palette::new(state());
    let view = h.open("switch-session");
    h.discover(view["source"].clone(), "");
    let id = h
        .host
        .lock()
        .unwrap()
        .commands
        .iter()
        .rev()
        .find_map(|command| command.get("GetSessionInventory").cloned())
        .unwrap();
    h.runtime
        .palette(json!({"type":"release","handles":[view["source"]]}))
        .unwrap();
    assert!(h.issued(json!({"SwitchSessionDestination":{"Cancel":id}})));
}

#[test]
fn failed_deferred_session_switch_reports_the_error_and_cancels_preparation() {
    let mut h = Palette::new(state());
    h.stub("SwitchSessionDestination", json!("Pending"));
    h.host
        .lock()
        .unwrap()
        .failures
        .insert("SetTimeout".into(), "SSH unavailable".into());
    let view = h.open("switch-session");
    let rows = h.discover(view["source"].clone(), "other");
    h.send(json!({"type":"execute", "handle":rows[0]["handle"], "values":null}));
    h.runtime.invoke("yinz-async-poll", vec![]).unwrap();
    let error = h.reply().unwrap_err();
    assert!(error.contains("SSH unavailable"), "{error}");
    assert!(!error.contains("Type Mismatch"), "{error}");
    let host = h.host.lock().unwrap();
    let id = host
        .commands
        .iter()
        .find_map(|command| command.pointer("/SwitchSessionDestination/Prepare/id"))
        .unwrap();
    assert!(host
        .commands
        .contains(&json!({"SwitchSessionDestination":{"Cancel":id}})));
    assert!(!host.commands.iter().any(|command| command
        .pointer("/SwitchSessionDestination/Commit")
        .is_some()));
}
