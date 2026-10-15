use crate::tests::{plugin_with_replies, Host};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use yinz_steel::Runtime;
use yinz_types::data::{Event, LayoutInfo, LayoutMetadata, TabMetadata};

fn plugin() -> (Runtime, Arc<Mutex<Host>>) {
    let metadata = LayoutMetadata {
        tabs: vec![TabMetadata {
            name: None,
            panes: vec![],
        }],
        ..Default::default()
    };
    let (mut runtime, host) = plugin_with_replies(
        "layout-manager",
        json!({}),
        BTreeMap::from([
            ("NewTabsWithLayoutInfo".into(), json!([7])),
            ("GenerateRandomName".into(), json!("generated")),
            (
                "DumpSessionLayout".into(),
                json!({"layout":"layout { pane; }","metadata":metadata}),
            ),
            ("SaveLayout".into(), json!(true)),
            ("RenameLayout".into(), json!(true)),
            ("DeleteLayout".into(), json!(true)),
        ]),
    );
    runtime
        .update(
            serde_json::to_value(Event::AvailableLayoutInfo(
                vec![
                    LayoutInfo::BuiltIn("compact".into()),
                    LayoutInfo::File("work.kdl".into(), metadata),
                ],
                vec![],
            ))
            .unwrap(),
        )
        .unwrap();
    runtime.render(24, 100).unwrap();
    (runtime, host)
}
fn key(runtime: &mut Runtime, key: &str) {
    runtime
        .update(serde_json::to_value(Event::Key(key.parse().unwrap())).unwrap())
        .unwrap();
}
fn kind(runtime: &mut Runtime) -> Value {
    runtime.invoke("get", vec![json!("kind")]).unwrap()
}
#[test]
fn layout_manager_search_open_and_rename_keep_host_state_authoritative() {
    let (mut runtime, host) = plugin();
    key(&mut runtime, "Enter");
    assert!(host
        .lock()
        .unwrap()
        .commands
        .contains(&json!({"RenameTabWithId":[7,"work.kdl"]})));
    key(&mut runtime, "Esc");
    key(&mut runtime, "r");
    runtime.update(json!({"PastedText":"copy"})).unwrap();
    for value in ["Enter", "Tab", "Enter"] {
        key(&mut runtime, value);
    }
    assert_eq!(kind(&mut runtime), "list");
    assert!(host.lock().unwrap().commands.contains(
        &json!({"RenameLayout":{"old_layout_name":"work.kdl","new_layout_name":"work.kdlcopy"}})
    ));
    assert_eq!(runtime.value("layouts").unwrap()[0]["name"], "work.kdlcopy");
}
#[test]
fn layout_manager_refused_delete_preserves_row_and_restores_screen() {
    let (mut runtime, host) = plugin();
    host.lock()
        .unwrap()
        .failures
        .insert("DeleteLayout".into(), "read-only file system".into());
    key(&mut runtime, "Esc");
    key(&mut runtime, "Delete");
    assert_eq!(kind(&mut runtime), "error");
    assert_eq!(
        runtime.value("layouts").unwrap().as_array().unwrap().len(),
        2
    );
    assert!(!runtime.render(24, 100).unwrap().is_empty());
    key(&mut runtime, "x");
    assert_eq!(kind(&mut runtime), "list");
}
#[test]
fn layout_manager_save_without_name_uses_host_generated_name_and_keeps_metadata() {
    let (mut runtime, host) = plugin();
    key(&mut runtime, "Esc");
    key(&mut runtime, "n");
    assert_eq!(kind(&mut runtime), "new");
    for value in ["Tab", "Tab", "Enter"] {
        key(&mut runtime, value);
    }
    assert_eq!(kind(&mut runtime), "list");
    assert!(host.lock().unwrap().commands.contains(&json!({"SaveLayout":{"layout_name":"generated","layout_kdl":"layout { pane; }","overwrite":false}})));
    assert!(runtime
        .value("layouts")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["name"] == "generated"
            && entry["metadata"]["tabs"].as_array().unwrap().len() == 1));
}
