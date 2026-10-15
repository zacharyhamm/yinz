use crate::tests::plugin_with_replies;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use yinz_steel::Runtime;
use yinz_types::{
    data::{Event, PaneId, PaneInfo, PaneManifest},
    internal_ids::{PluginId, TabPosition, TerminalId},
};

fn todos() -> Runtime {
    let (mut runtime, _) = plugin_with_replies(
        "todos",
        json!({}),
        BTreeMap::from([
            (
                "GetPluginIds".into(),
                json!({"plugin_id":42,"client_id":1,"initial_cwd":"/repo"}),
            ),
            (
                "DataWatch".into(),
                json!({"watch_id":1,"rows":{"columns":[],"rows":[],"truncated":false}}),
            ),
        ]),
    );
    runtime
        .eval(
            r#"
            (put "root" "/repo")
            (put "list" (hash "items" (list
              (hash "id" 1 "text" "parent" "rev" 3 "children" (list
                (hash "id" 2 "text" "child" "rev" 4))))))
            (put "selected" 1)
        "#,
        )
        .unwrap();
    runtime
}

fn pane(id: PaneId, is_focused: bool, is_suppressed: bool) -> PaneInfo {
    PaneInfo {
        id,
        is_focused,
        is_suppressed,
        ..PaneInfo::default()
    }
}

fn panes(runtime: &mut Runtime, panes: Vec<PaneInfo>) -> bool {
    runtime
        .update(
            serde_json::to_value(Event::PaneUpdate(PaneManifest {
                panes: [(TabPosition::new(0), panes)].into(),
            }))
            .unwrap(),
        )
        .unwrap()
}

fn own_pane(runtime: &mut Runtime, focused: bool, suppressed: bool) -> bool {
    panes(
        runtime,
        vec![pane(PaneId::Plugin(PluginId::new(42)), focused, suppressed)],
    )
}

fn key(runtime: &mut Runtime, character: &str) {
    runtime
        .update(json!({"Key":{"bare_key":{"Char":character},"key_modifiers":[]}}))
        .unwrap();
}

fn get(runtime: &mut Runtime, name: &str) -> Value {
    runtime.invoke("get", vec![json!(name)]).unwrap()
}

#[test]
fn todos_pane_updates_track_focus_and_ignore_other_pane_identities() {
    let mut runtime = todos();
    assert!(!own_pane(&mut runtime, true, false));
    assert_eq!(get(&mut runtime, "pane-shown"), true);
    assert_eq!(get(&mut runtime, "keyboard"), true);

    for id in [
        PaneId::Terminal(TerminalId::new(42)),
        PaneId::Plugin(PluginId::new(43)),
    ] {
        assert!(!panes(&mut runtime, vec![pane(id, false, true)]));
        assert_eq!(get(&mut runtime, "pane-shown"), true);
        assert_eq!(get(&mut runtime, "keyboard"), true);
    }
    assert!(panes(
        &mut runtime,
        vec![
            pane(PaneId::Terminal(TerminalId::new(42)), true, true),
            pane(PaneId::Plugin(PluginId::new(43)), true, true),
            pane(PaneId::Plugin(PluginId::new(42)), false, false),
        ]
    ));
    assert_eq!(get(&mut runtime, "keyboard"), false);
    assert_eq!(get(&mut runtime, "pane-shown"), true);
    assert!(!own_pane(&mut runtime, false, false));
    assert!(own_pane(&mut runtime, true, false));
    assert_eq!(get(&mut runtime, "keyboard"), true);
    assert!(!own_pane(&mut runtime, true, false));
}

#[test]
fn todos_suppression_closes_editing_and_restores_the_draft_on_reopen() {
    let mut runtime = todos();
    own_pane(&mut runtime, true, false);
    key(&mut runtime, "e");
    runtime.update(json!({"PastedText":" draft"})).unwrap();
    assert_eq!(
        runtime.invoke("mget", vec![json!("kind")]).unwrap(),
        "input"
    );
    assert_eq!(
        runtime.eval("(length (get \"drafts\"))").unwrap().values,
        ["0"]
    );
    own_pane(&mut runtime, true, true);
    assert_eq!(
        runtime.invoke("mget", vec![json!("kind")]).unwrap(),
        "normal"
    );
    assert_eq!(get(&mut runtime, "pane-shown"), false);
    assert_eq!(get(&mut runtime, "keyboard"), false);
    assert_eq!(
        runtime.eval("(length (get \"drafts\"))").unwrap().values,
        ["1"]
    );

    own_pane(&mut runtime, true, false);
    key(&mut runtime, "e");
    runtime
        .eval("(define restored (yinz-ui-value (mget \"input\") \"text\"))")
        .unwrap();
    assert_eq!(runtime.value("restored").unwrap(), "parent draft");
    assert_eq!(
        runtime.eval("(length (get \"drafts\"))").unwrap().values,
        ["0"]
    );
}

#[test]
fn todos_suppression_clears_confirmations_and_visit_state() {
    for (character, confirmation) in [("d", "delete"), (" ", "complete")] {
        let mut runtime = todos();
        own_pane(&mut runtime, true, false);
        key(&mut runtime, character);
        assert_eq!(
            runtime.invoke("mget", vec![json!("kind")]).unwrap(),
            confirmation
        );
        runtime.eval("(put \"tabs-focused\" #t) (put \"bar-focused\" '(1)) (put \"sheet\" 0) (set-note \"notice\")").unwrap();
        own_pane(&mut runtime, true, true);
        assert_eq!(
            runtime.invoke("mget", vec![json!("kind")]).unwrap(),
            "normal"
        );
        for name in ["tabs-focused", "sheet", "note"] {
            assert_eq!(get(&mut runtime, name), false);
        }
        assert_eq!(get(&mut runtime, "bar-focused"), json!([]));
    }
}
