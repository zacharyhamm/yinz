use crate::tests::{plugin_with_replies, Host};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

fn acting_client(host: &Arc<Mutex<Host>>, client: u16) {
    host.lock().unwrap().replies.insert(
        "GetPluginIds".into(),
        json!({"plugin_id": 42, "client_id": client}),
    );
}

#[test]
fn shared_plugin_tracks_acting_clients_and_owner_render_sweeps() {
    let (mut runtime, host) =
        plugin_with_replies("shared-test", json!({"shared":"true"}), BTreeMap::new());
    let first = runtime.render(10, 100).unwrap();
    assert!(
        first.contains("you are client 1\towner: 1\trendered for: [ClientId(1)]"),
        "{first}"
    );
    acting_client(&host, 10);
    assert!(runtime
        .render(10, 100)
        .unwrap()
        .contains("you are client 10\towner: 1"));
    for client in [10, 1, 10] {
        acting_client(&host, client);
        assert!(runtime
            .update(json!({"Key":{"bare_key":{"Char":"x"},"key_modifiers":[]}}))
            .unwrap());
    }
    acting_client(&host, 1);
    let next = runtime.render(10, 100).unwrap();
    assert!(
        next.contains("rendered for: [ClientId(1), ClientId(10)]"),
        "{next}"
    );
    assert!(
        next.contains("your presses: 1\ttotal: 3\tlast presser: 10"),
        "{next}"
    );
    assert!(next.contains("per client: 1 -> 1   10 -> 2"), "{next}");
    acting_client(&host, 10);
    assert!(runtime.render(10, 100).unwrap().contains("your presses: 2"));
    assert!(!runtime.update(json!({"Timer":0.1})).unwrap());
}

#[test]
fn shared_plugin_keys_use_the_existing_host_commands() {
    let (mut runtime, host) = plugin_with_replies("shared-test", json!({}), BTreeMap::new());
    for (bare, expected) in [
        (json!({"Char":"m"}), json!({"SwitchToMode":"Locked"})),
        (
            json!({"Char":"f"}),
            json!({"OpenTerminalFloating":[{"path":".","line_number":null,"cwd":null},null]}),
        ),
        (json!("Esc"), Value::String("CloseSelf".into())),
    ] {
        runtime
            .update(json!({"Key":{"bare_key":bare,"key_modifiers":[]}}))
            .unwrap();
        assert_eq!(host.lock().unwrap().commands.last(), Some(&expected));
    }
}
