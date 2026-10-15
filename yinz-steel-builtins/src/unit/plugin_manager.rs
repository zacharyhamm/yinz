use crate::tests::{plugin_with_replies, rendered_text, Host};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use yinz_steel::Runtime;
use yinz_types::{
    data::{Event, KeyWithModifier, PluginInfo, SessionInfo},
    internal_ids::PluginId,
};

fn plugin() -> (Runtime, Arc<Mutex<Host>>) {
    plugin_with_replies("plugin-manager", json!({}), BTreeMap::new())
}
fn key(runtime: &mut Runtime, value: &str) -> bool {
    runtime
        .update(
            serde_json::to_value(Event::Key(value.parse::<KeyWithModifier>().unwrap())).unwrap(),
        )
        .unwrap()
}
fn type_text(runtime: &mut Runtime, value: &str) {
    for ch in value.chars() {
        runtime
            .update(json!({"Key":{"bare_key":{"Char":ch.to_string()},"key_modifiers":[]}}))
            .unwrap();
    }
}
fn inventory(runtime: &mut Runtime) {
    let mut session = SessionInfo {
        is_current_session: true,
        ..Default::default()
    };
    session.plugins.insert(
        PluginId::new(20),
        PluginInfo {
            location: "file:/work/status.scm".into(),
            configuration: BTreeMap::from([("theme".into(), "dark".into())]),
        },
    );
    session.plugins.insert(
        PluginId::new(3),
        PluginInfo {
            location: "yinz:strider".into(),
            ..Default::default()
        },
    );
    runtime
        .update(serde_json::to_value(Event::SessionUpdate(vec![session], vec![])).unwrap())
        .unwrap();
}

#[test]
fn plugin_manager_sorts_inventory_and_routes_focus_reload_close() {
    let (mut runtime, host) = plugin();
    inventory(&mut runtime);
    assert!(key(&mut runtime, "Down"));
    assert!(!key(&mut runtime, "Enter"));
    assert!(!key(&mut runtime, "Tab"));
    assert!(!key(&mut runtime, "Delete"));
    let commands = host.lock().unwrap().commands.clone();
    assert!(commands.ends_with(&[
        json!({"FocusPluginPane":[3,true,false]}),
        json!({"ReloadPlugin":3}),
        json!({"ClosePluginPane":3})
    ]));
    key(&mut runtime, "Down");
    key(&mut runtime, "Right");
    let text = rendered_text(&runtime.render(30, 100).unwrap());
    assert!(text.contains("Configuration:") && text.contains("theme: dark"));
    key(&mut runtime, "Left");
    assert!(!rendered_text(&runtime.render(30, 100).unwrap()).contains("theme: dark"));
}

#[test]
fn plugin_manager_search_and_inventory_changes_clear_selection() {
    let (mut runtime, host) = plugin();
    inventory(&mut runtime);
    type_text(&mut runtime, "sts");
    key(&mut runtime, "Down");
    key(&mut runtime, "Enter");
    assert_eq!(
        host.lock().unwrap().commands.last(),
        Some(&json!({"FocusPluginPane":[20,true,false]}))
    );
    key(&mut runtime, "Esc");
    assert_eq!(runtime.value("search").unwrap(), "");
    inventory(&mut runtime);
    assert_eq!(runtime.value("selected").unwrap(), 0);
    runtime
        .update(
            serde_json::to_value(Event::SessionUpdate(
                vec![SessionInfo {
                    is_current_session: true,
                    ..Default::default()
                }],
                vec![],
            ))
            .unwrap(),
        )
        .unwrap();
    assert_eq!(runtime.value("selected").unwrap(), false);
}

#[test]
fn plugin_manager_new_plugin_edits_unicode_configuration_and_background() {
    let (mut runtime, host) = plugin();
    key(&mut runtime, "Ctrl a");
    type_text(&mut runtime, "file:/tmp/日本.scmx");
    key(&mut runtime, "Backspace");
    key(&mut runtime, "Tab");
    type_text(&mut runtime, "key");
    key(&mut runtime, "Tab");
    type_text(&mut runtime, "value");
    key(&mut runtime, "Enter");
    key(&mut runtime, "Ctrl l");
    let frame = rendered_text(&runtime.render(30, 100).unwrap());
    assert!(frame.contains("key: value"));
    key(&mut runtime, "Enter");
    assert_eq!(
        host.lock().unwrap().commands.last(),
        Some(
            &json!({"LoadNewPlugin":{"url":"file:/tmp/日本.scm","config":{"key":"value"},"load_in_background":true,"skip_plugin_cache":true}})
        )
    );
    assert_eq!(runtime.value("new?").unwrap(), false);
}

#[test]
fn plugin_manager_picker_replies_are_correlated_and_consumed_once() {
    let (mut runtime, host) = plugin();
    key(&mut runtime, "Ctrl a");
    assert!(!key(&mut runtime, "Ctrl f"));
    let command = host.lock().unwrap().commands.last().cloned().unwrap();
    let id = command["MessageToPlugin"]["message_args"]["request_id"]
        .as_str()
        .unwrap();
    let message = |id: &str, path: &str| json!({"source":{"Plugin":4},"name":"filepicker_result","payload":path,"args":{"request_id":id},"is_private":true});
    assert!(runtime.pipe(message("other", "/wrong.scm")).unwrap());
    assert_eq!(runtime.value("url").unwrap(), "");
    assert!(runtime.pipe(message(id, "/correct.scm")).unwrap());
    assert_eq!(runtime.value("url").unwrap(), "file:/correct.scm");
    runtime.pipe(message(id, "/stale.scm")).unwrap();
    assert_eq!(runtime.value("url").unwrap(), "file:/correct.scm");
}

#[test]
fn plugin_manager_configuration_navigation_deletion_and_cancel() {
    let (mut runtime, _) = plugin();
    key(&mut runtime, "Ctrl a");
    key(&mut runtime, "Tab");
    type_text(&mut runtime, "one");
    key(&mut runtime, "Tab");
    type_text(&mut runtime, "1");
    key(&mut runtime, "Tab");
    type_text(&mut runtime, "two");
    key(&mut runtime, "Tab");
    type_text(&mut runtime, "2");
    key(&mut runtime, "Esc");
    key(&mut runtime, "Down");
    key(&mut runtime, "Tab");
    type_text(&mut runtime, "!");
    key(&mut runtime, "Enter");
    assert_eq!(
        runtime.value("configuration").unwrap(),
        json!([["one!", "1"], ["two", "2"]])
    );
    key(&mut runtime, "Delete");
    assert_eq!(
        runtime.value("configuration").unwrap(),
        json!([["two", "2"]])
    );
    key(&mut runtime, "Esc");
    assert_eq!(runtime.value("new?").unwrap(), false);
}

#[test]
fn plugin_manager_all_viewports_and_empty_inventory_are_bounded() {
    let (mut runtime, _) = plugin();
    for screen in [false, true] {
        if screen {
            key(&mut runtime, "Ctrl a");
            type_text(&mut runtime, "file:/long/path/plugin.scm");
        }
        for rows in [0, 1, 6, 12, 30] {
            for cols in [0, 1, 4, 20, 60, 120] {
                let output = runtime.render(rows, cols).unwrap();
                assert!(output.len() < 10000);
            }
        }
    }
}
