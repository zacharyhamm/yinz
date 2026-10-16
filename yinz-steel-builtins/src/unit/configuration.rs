use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::palette_tests::Palette;

fn node(name: &str, arguments: Value, children: Vec<Value>) -> Value {
    json!({"name": name, "arguments": arguments, "properties": {}, "children": children})
}

fn empty(name: &str) -> Value {
    node(name, json!([]), vec![])
}

fn snapshot() -> Value {
    let setting = node("mouse_mode", json!([true]), vec![]);
    json!({
        "action_properties": {}, "action_children": {}, "section_timing": {}, "sections": [], "templates": {},
        "revision": "revision-one", "notice": "Applied to this session", "overrides": [], "actions": [],
        "catalog": [{"name": "mouse_mode", "label": "mouse mode", "kind": "boolean", "choices": ["Enabled", "Disabled"],
            "minimum": null, "maximum": null, "help": "Mouse", "timing": "Applies to this session"}],
        "saved": [setting], "clients": {"1": [setting]},
    })
}

fn with_section(name: &str, root: Value) -> Value {
    let mut state = snapshot();
    state["sections"] = json!([name]);
    state["saved"] = json!([root]);
    state["clients"] = json!({"1": [root]});
    state
}

struct Editor {
    palette: Palette,
    view: Value,
}

impl Editor {
    fn new(state: Value, root: &str) -> Self {
        let palette = Palette::for_plugin(
            "configuration-palette",
            BTreeMap::from([
                ("InspectConfiguration".into(), state),
                ("EditConfiguration".into(), snapshot()),
            ]),
        );
        let mut result = Self {
            palette,
            view: Value::Null,
        };
        result
            .execute(result.palette.root("configuration"), Value::Null)
            .unwrap();
        result.palette.roots = result.view["actions"].as_array().unwrap().clone();
        result.choose(root);
        result
    }

    fn execute(&mut self, handle: Value, values: Value) -> Result<(), String> {
        let result = self.palette.execute(handle, values)?;
        self.view = if result["type"] == "view" {
            result["view"].clone()
        } else {
            result
        };
        if self.view["type"] == "actions" && !self.view["source"].is_null() {
            self.view["actions"] = json!(self.palette.discover(self.view["source"].clone(), ""));
        }
        Ok(())
    }

    fn choose(&mut self, id: &str) {
        self.choose_with_values(id, json!({}));
    }

    fn choose_with_values(&mut self, id: &str, values: Value) {
        let handle = self.view["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == id)
            .unwrap_or_else(|| panic!("missing action {id} in {}", self.view))["handle"]
            .clone();
        self.execute(handle, values).unwrap();
    }

    fn submit(&mut self, values: Value) {
        self.execute(self.view["handle"].clone(), values).unwrap();
    }

    fn writes(&self) -> Vec<Value> {
        self.palette
            .host
            .lock()
            .unwrap()
            .commands
            .iter()
            .filter_map(|command| command.get("EditConfiguration").cloned())
            .collect()
    }
}

#[test]
fn configuration_scalar_drafts_submit_value_and_destination_together() {
    let mut editor = Editor::new(snapshot(), "mouse_mode");
    assert_eq!(editor.view["title"], "mouse mode");
    assert_eq!(editor.view["description"], "Mouse");
    assert!(editor.writes().is_empty());
    assert_eq!(editor.view["fields"][0]["initialValue"], "Enabled");
    assert_eq!(editor.view["fields"][1]["initialValue"], "Apply to session");
    assert_eq!(editor.view["submitLabel"], "Apply change");
    editor.submit(json!({"value": "Disabled", "destination": "Apply to session"}));
    assert_eq!(editor.view["type"], "complete");
    assert_eq!(
        editor.writes(),
        [
            json!({"revision": "revision-one", "destination": "session", "reset": [],
        "patches": [{"path": ["mouse_mode"], "node": node("mouse_mode", json!([false]), vec![])}]})
        ]
    );
}

#[test]
fn configuration_ordered_binding_actions_keep_the_selected_order_in_the_draft() {
    let binding = node(
        "bind",
        json!(["Ctrl x"]),
        vec![empty("Copy"), empty("Quit")],
    );
    let root = node(
        "keybinds",
        json!([]),
        vec![node("normal", json!([]), vec![binding])],
    );
    let mut editor = Editor::new(with_section("keybinds", root), "keybinds");
    for choice in [
        "entry:0", "children", "entry:0", "children", "entry:0", "down",
    ] {
        editor.choose(choice);
    }
    assert!(editor.writes().is_empty());
    editor.choose("save");
    editor.submit(json!({"destination": "Apply to session"}));
    let writes = editor.writes();
    let children = &writes[0]["patches"][0]["node"]["children"];
    assert_eq!(children[0]["name"], "Quit");
    assert_eq!(children[1]["name"], "Copy");
}

#[test]
fn configuration_discovery_selects_repeated_groups_by_position() {
    let groups = ["normal", "locked"].map(|mode| {
        node(
            "shared_among",
            json!([mode]),
            vec![node(
                "bind",
                json!(["Ctrl a"]),
                vec![empty(if mode == "normal" { "Quit" } else { "Copy" })],
            )],
        )
    });
    let state = with_section("keybinds", node("keybinds", json!([]), groups.to_vec()));
    for id in [
        "keybinds:1:shared_among",
        "keybinds:1:shared_among:0:bind:[\"Ctrl a\"]",
    ] {
        let mut editor = Editor::new(state.clone(), id);
        if id.contains("bind:") {
            editor.choose("children");
            assert!(editor.view["actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["label"].as_str().unwrap().contains("Copy")));
        } else {
            editor.choose("edit");
            assert!(editor.view["fields"]
                .as_array()
                .unwrap()
                .iter()
                .any(|field| field["initialValue"] == "locked"));
        }
    }
}

#[test]
fn configuration_discovery_selects_second_separator_and_rejects_changed_revision() {
    let root = node(
        "context_menu",
        json!([]),
        vec![empty("separator"), empty("item"), empty("separator")],
    );
    let mut state = with_section("context_menu", root);
    let mut editor = Editor::new(state.clone(), "context_menu:2:separator");
    let original = editor.palette.root("context_menu:2:separator");
    editor.choose("remove");
    editor.submit(json!({}));
    editor.choose("save");
    editor.submit(json!({"destination": "Apply to session"}));
    assert_eq!(
        editor.writes()[0],
        json!({"revision": "revision-one", "destination": "session", "reset": [],
        "patches": [{"path": ["context_menu"], "node": node("context_menu", json!([]), vec![empty("separator"), empty("item")])}]})
    );
    state["revision"] = json!("revision-two");
    editor.palette.stub("InspectConfiguration", state);
    assert!(editor
        .execute(original, Value::Null)
        .unwrap_err()
        .contains("Configuration changed"));
}

#[test]
fn configuration_reload_retains_patches_and_permanent_scalar_reconciles_saved_and_effective() {
    for permanent in [false, true] {
        let effective = node("mouse_mode", json!([false]), vec![]);
        let unrelated = node("pane_frames", json!([false]), vec![]);
        let mut state = snapshot();
        state["clients"] = json!({"1": [effective, unrelated]});
        state["saved"]
            .as_array_mut()
            .unwrap()
            .push(node("pane_frames", json!([true]), vec![]));
        let mut editor = Editor::new(state, "mouse_mode");
        let mut fresh = snapshot();
        fresh["revision"] = json!("revision-two");
        editor.palette.stub("InspectConfiguration", fresh);
        editor.choose_with_values("reload", json!({"value": "Disabled"}));
        editor.submit(json!({}));
        editor.submit(
            json!({"destination": if permanent {"Save permanently"} else {"Apply to session"}}),
        );
        assert_eq!(
            editor.writes()[0],
            json!({"revision": "revision-two", "destination": if permanent {"permanent"} else {"session"}, "reset": [],
            "patches": if permanent {json!([{"path": ["mouse_mode"], "node": effective}])} else {json!([])}})
        );
    }
    for (saved, effective) in [(true, false), (false, true)] {
        let mut state = snapshot();
        state["saved"][0]["arguments"] = json!([saved]);
        state["clients"]["1"][0]["arguments"] = json!([effective]);
        let mut editor = Editor::new(state, "mouse_mode");
        editor.submit(json!({"value": "Enabled", "destination": "Save permanently"}));
        assert_eq!(
            editor.writes()[0]["patches"],
            json!([{"path": ["mouse_mode"], "node": node("mouse_mode", json!([true]), vec![])}])
        );
    }
}

fn binding(chords: &[&str], action: &str) -> Value {
    node("bind", json!(chords), vec![empty(action)])
}

#[test]
fn configuration_binding_replacement_and_edit_preserve_unrelated_session_overrides() {
    for add in [false, true] {
        let occupied = binding(
            if add {
                &["Ctrl b", "Ctrl c"]
            } else {
                &["Ctrl b"]
            },
            "Quit",
        );
        let root = |first| {
            node(
                "keybinds",
                json!([]),
                vec![node("normal", json!([]), vec![first, occupied.clone()])],
            )
        };
        let mut state = with_section("keybinds", root(binding(&["Ctrl a"], "Quit")));
        state["clients"] = json!({"1": [root(binding(&["Ctrl a"], "Copy"))]});
        state["templates"] = json!({"keybinds/*": [binding(&["Ctrl b"], "Copy")]});
        let mut editor = Editor::new(state, "keybinds");
        for choice in ["entry:0", "children"] {
            editor.choose(choice);
        }
        if add {
            for choice in ["add", "template:0"] {
                editor.choose(choice);
            }
            editor.submit(json!({"name": "bind", "arg:0": "Ctrl b"}));
            assert_eq!(editor.view["confirmation"], true);
            editor.submit(json!({}));
        } else {
            for choice in ["entry:1", "children", "entry:0", "edit"] {
                editor.choose(choice);
            }
            editor.submit(json!({"name": "Copy"}));
        }
        editor.choose("save");
        editor.submit(json!({"destination": "Save permanently"}));
        let patches = if add {
            json!([
                {"path": ["keybinds", "normal", "bind:[\"Ctrl b\",\"Ctrl c\"]"], "node": null},
                {"path": ["keybinds", "normal", "bind:[\"Ctrl c\"]"], "node": binding(&["Ctrl c"], "Quit")},
                {"path": ["keybinds", "normal", "bind:[\"Ctrl b\"]"], "node": binding(&["Ctrl b"], "Copy")},
            ])
        } else {
            json!([{ "path": ["keybinds", "normal", "bind:[\"Ctrl b\"]"], "node": binding(&["Ctrl b"], "Copy") }])
        };
        assert_eq!(
            editor.writes()[0],
            json!({"revision": "revision-one", "destination": "permanent", "reset": [], "patches": patches})
        );
    }
}

#[test]
fn configuration_scalar_reset_unset_and_startup_destination_preserve_host_timing() {
    let mut state = snapshot();
    state["catalog"][0]["timing"] = json!("On next session start");
    let mut editor = Editor::new(state.clone(), "mouse_mode");
    editor.choose("reset");
    assert_eq!(
        editor.view["fields"][0]["options"],
        json!(["Save permanently"])
    );
    editor.submit(json!({"destination": "Save permanently"}));
    assert_eq!(editor.writes()[0]["reset"], json!([["mouse_mode"]]));
    let mut editor = Editor::new(state, "mouse_mode");
    editor.choose("unset");
    assert_eq!(editor.view["confirmation"], true);
    editor.submit(json!({}));
    editor.submit(json!({"destination": "Save permanently"}));
    assert_eq!(
        editor.writes()[0]["patches"],
        json!([{"path": ["mouse_mode"], "node": null}])
    );
}

#[test]
fn configuration_number_fields_validate_integer_syntax_and_safe_range() {
    let mut state = snapshot();
    state["catalog"][0]["kind"] = json!("number");
    state["catalog"][0]["choices"] = json!([]);
    state["catalog"][0]["minimum"] = json!(0);
    state["catalog"][0]["maximum"] = json!(100);
    state["saved"][0]["arguments"] = json!([5]);
    state["clients"]["1"][0]["arguments"] = json!([5]);
    let mut editor = Editor::new(state, "mouse_mode");
    assert_eq!(editor.view["fields"][0]["type"], "input");
    assert_eq!(editor.view["fields"][0]["options"]["maximum"], 100);
    let handle = editor.view["handle"].clone();
    for invalid in ["", "1.5", " 3 ", "+3", "9007199254740992", "雪"] {
        assert!(
            editor
                .execute(handle.clone(), json!({"value": invalid}))
                .is_err(),
            "{invalid}"
        );
    }
    editor
        .execute(
            handle,
            json!({"value": "12", "destination": "Apply to session"}),
        )
        .unwrap();
    assert_eq!(
        editor.writes()[0]["patches"][0]["node"]["arguments"],
        json!([12])
    );
}

#[test]
fn configuration_submenu_excludes_theme_settings_and_sections() {
    let mut state = with_section("themes", node("themes", json!([]), vec![empty("custom")]));
    for name in [
        "theme",
        "theme_dark",
        "theme_light",
        "theme_dir",
        "explicit_theme_hue",
        "agent_theme_mapping",
    ] {
        let mut setting = state["catalog"][0].clone();
        setting["name"] = json!(name);
        state["catalog"].as_array_mut().unwrap().push(setting);
    }
    let editor = Editor::new(state, "mouse_mode");
    assert_eq!(
        editor
            .palette
            .roots
            .iter()
            .map(|action| action["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["mouse_mode"]
    );
    assert!(editor.writes().is_empty());
}

#[test]
fn configuration_explanations_cover_scalar_fields_and_keep_timing_in_details() {
    for (kind, value, choices) in [
        ("boolean", json!(true), json!([])),
        ("choice", json!("normal"), json!(["normal", "locked"])),
        ("number", json!(5), json!([])),
        ("file", json!("/bin/sh"), json!([])),
        ("directory", json!("/tmp"), json!([])),
    ] {
        let mut state = snapshot();
        state["catalog"][0]["kind"] = json!(kind);
        state["catalog"][0]["choices"] = choices;
        state["saved"][0]["arguments"] = json!([value]);
        state["clients"]["1"][0]["arguments"] = json!([value]);
        let mut editor = Editor::new(state, "mouse_mode");
        assert_eq!(editor.palette.roots[0]["label"], "mouse mode…");
        assert_eq!(editor.palette.roots[0]["description"], "");
        assert_eq!(editor.view["description"], "Mouse", "{kind}");
        editor.choose("state");
        let details = editor.view["text"].as_str().unwrap();
        assert!(details.contains("Applies to this session"));
        assert!(details.contains("Client 1"));
        assert!(editor.writes().is_empty());
    }
}

#[test]
fn configuration_sections_explain_themselves_in_nested_lists_and_forms() {
    for section in [
        "keybinds",
        "env",
        "plugins",
        "load_plugins",
        "hooks",
        "context_menu",
        "ui",
    ] {
        let root = node(section, json!([]), vec![empty("example")]);
        let mut editor = Editor::new(with_section(section, root), section);
        let description = editor.view["description"].clone();
        assert!(!description.as_str().unwrap().is_empty(), "{section}");
        editor.choose("entry:0");
        assert_eq!(editor.view["description"], description);
        editor.choose("edit");
        assert_eq!(editor.view["description"], description);
        assert!(editor.writes().is_empty());
    }
}
