use super::runtime_parity::Pair;
use serde_json::json;
use std::collections::BTreeMap;
use yinz_types::{
    data::{
        Event, KeyWithModifier, PaneInfo, PaneManifest, PipeMessage, PluginInfo, SessionInfo,
        TabInfo,
    },
    internal_ids::{PluginId, TabPosition},
    pane_id::PaneId,
};

fn inventory() -> Event {
    let session = SessionInfo {
        is_current_session: true,
        tabs: vec![TabInfo {
            position: TabPosition::new(0),
            name: "Development".into(),
            ..Default::default()
        }],
        plugins: BTreeMap::from([
            (
                PluginId::new(3),
                PluginInfo {
                    location: "yinz:strider".into(),
                    configuration: BTreeMap::new(),
                },
            ),
            (
                PluginId::new(20),
                PluginInfo {
                    location: "file:/work/status.scm".into(),
                    configuration: BTreeMap::from([
                        ("theme".into(), "dark".into()),
                        ("display".into(), "compact".into()),
                    ]),
                },
            ),
        ]),
        ..Default::default()
    };
    Event::SessionUpdate(vec![session], vec![])
}
fn text(pair: &mut Pair, value: &str) {
    for ch in value.chars() {
        pair.event(
            serde_json::from_value(
                json!({"Key":{"bare_key":{"Char":ch.to_string()},"key_modifiers":[]}}),
            )
            .unwrap(),
        );
    }
}
fn editor_viewports(pair: &mut Pair) {
    // The original truncator panics below five available characters for nonempty fields.
    for (rows, cols) in [(30, 120), (30, 100), (20, 60), (12, 34)] {
        pair.render(rows, cols);
    }
}

#[test]
fn actual_wasm_and_steel_plugin_manager_inventory_search_and_actions() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "actual_wasm_and_steel_plugin_manager_inventory_search_and_actions",
    );
    let mut pair = Pair::new("plugin-manager", BTreeMap::new());
    pair.mode(false);
    pair.event(inventory());
    pair.event(Event::PaneUpdate(PaneManifest {
        panes: std::collections::HashMap::from([(
            TabPosition::new(0),
            vec![PaneInfo {
                id: PaneId::Plugin(PluginId::new(20)),
                ..Default::default()
            }],
        )]),
    }));
    pair.viewports();
    pair.key("Down");
    pair.key("Down");
    pair.key("Right");
    pair.viewports();
    pair.key("Enter");
    pair.key("Tab");
    pair.key("Delete");
    pair.key("Left");
    pair.viewports();
    text(&mut pair, "sts");
    pair.key("Down");
    pair.key("Right");
    pair.viewports();
    pair.key("Enter");
    pair.key("Up");
    pair.key("Backspace");
    pair.viewports();
    pair.key("Esc");
    pair.key("Esc");
}

#[test]
fn actual_wasm_and_steel_plugin_manager_new_configuration_and_load() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "actual_wasm_and_steel_plugin_manager_new_configuration_and_load",
    );
    let mut pair = Pair::new("plugin-manager", BTreeMap::new());
    pair.mode(false);
    pair.key("Ctrl a");
    pair.viewports();
    pair.key("Down");
    pair.key("Up");
    text(&mut pair, "file:/work/plugin.scmx");
    pair.key("Backspace");
    editor_viewports(&mut pair);
    pair.key("Tab");
    text(&mut pair, "theme");
    editor_viewports(&mut pair);
    pair.key("Tab");
    text(&mut pair, "dark");
    editor_viewports(&mut pair);
    pair.key("Tab");
    text(&mut pair, "other");
    pair.key("Tab");
    text(&mut pair, "value");
    pair.key("Esc");
    editor_viewports(&mut pair);
    pair.key("Down");
    pair.key("Tab");
    text(&mut pair, "!");
    pair.key("Enter");
    pair.key("Delete");
    pair.key("Ctrl l");
    editor_viewports(&mut pair);
    pair.key("Enter");
    pair.viewports();
}

#[test]
fn actual_wasm_and_steel_plugin_manager_picker_request_and_reply() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "actual_wasm_and_steel_plugin_manager_picker_request_and_reply",
    );
    let mut pair = Pair::new("plugin-manager", BTreeMap::new());
    pair.key("Ctrl a");
    let key = Event::Key("Ctrl f".parse::<KeyWithModifier>().unwrap());
    assert_eq!(pair.wasm.update(&key), pair.steel.update(&key));
    let mut original = pair.wasm.take_commands();
    let mut scheme = pair.steel.take_commands();
    assert_eq!(original.len(), 1);
    assert_eq!(scheme.len(), 1);
    let expected = original[0]["MessageToPlugin"]["message_args"]["request_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let actual = scheme[0]["MessageToPlugin"]["message_args"]["request_id"]
        .as_str()
        .unwrap()
        .to_owned();
    for (command, id) in [(&mut original[0], &expected), (&mut scheme[0], &actual)] {
        assert_eq!(uuid::Uuid::parse_str(id).unwrap().get_version_num(), 4);
        assert_eq!(
            command["MessageToPlugin"]["plugin_config"]["request_id"],
            *id
        );
        command["MessageToPlugin"]["message_args"]["request_id"] = json!("correlated-request");
        command["MessageToPlugin"]["plugin_config"]["request_id"] = json!("correlated-request");
    }
    assert_eq!(scheme, original);
    let reply = |id: &str| -> PipeMessage {
        serde_json::from_value(json!({"source":{"Plugin":55},"name":"filepicker_result","payload":"/tmp/chosen.scm","args":{"request_id":id},"is_private":true})).unwrap()
    };
    assert_eq!(
        pair.wasm.pipe(&reply(&expected)),
        pair.steel.pipe(&reply(&actual))
    );
    pair.commands();
    editor_viewports(&mut pair);
    pair.key("Enter");
}

#[test]
fn actual_wasm_and_steel_plugin_manager_inventory_refresh_and_long_lists() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "actual_wasm_and_steel_plugin_manager_inventory_refresh_and_long_lists",
    );
    let mut pair = Pair::new("plugin-manager", BTreeMap::new());
    let mut session = SessionInfo {
        is_current_session: true,
        ..Default::default()
    };
    for index in 1..30 {
        session.plugins.insert(
            PluginId::new(index),
            PluginInfo {
                location: format!("file:/work/plugin{index:02}.scm"),
                ..Default::default()
            },
        );
    }
    pair.event(Event::SessionUpdate(vec![session], vec![]));
    for _ in 0..15 {
        pair.key("Down");
    }
    pair.viewports();
    text(&mut pair, "plg12");
    pair.key("Down");
    pair.viewports();
    pair.event(inventory());
    pair.viewports();
    pair.key("Esc");
    pair.key("Up");
    pair.viewports();
}

#[test]
fn plugin_manager_tiny_field_width_does_not_repeat_the_legacy_truncation_trap() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "plugin_manager_tiny_field_width_does_not_repeat_the_legacy_truncation_trap",
    );
    let mut pair = Pair::new("plugin-manager", BTreeMap::new());
    pair.key("Ctrl a");
    text(&mut pair, "file:/work/plugin.scm");
    // The original byte-range replacement exceeds the string length when less than
    // five columns remain. The Scheme port clamps the range and remains usable.
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pair.wasm.render(10, 4))).is_err()
    );
    assert!(pair.steel.render(10, 4).len() < 10000);
    assert!(pair.steel.take_commands().is_empty());
}
