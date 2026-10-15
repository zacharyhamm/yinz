use super::{
    data_parity::DataHost,
    runtime_parity::{default_reply, Pair, Responder},
};
use agent_proto::store;
use prost::Message;
use std::{collections::BTreeMap, sync::Arc};
use yinz_plugin_api::plugin_command::*;
use yinz_types::data::*;

fn host(populated: bool) -> (Arc<DataHost>, Responder) {
    let data = DataHost::new("todos-parity");
    let fallback: Responder = Arc::new(|command| match command {
        PluginCommand::PaneTabs(..) => Some(
            ProtobufPaneTabsResponse {
                json: "{\"Ok\":null}".into(),
            }
            .encode_to_vec(),
        ),
        _ => default_reply(command),
    });
    let responder = data.responder(fallback);
    for def in store::shared_defs() {
        responder(&PluginCommand::DataDefineEntity {
            namespace: Some(store::SHARED.into()),
            def,
            allow_destructive: false,
        })
        .unwrap();
    }
    if populated {
        data.apply(
            store::SHARED,
            &[Mutation::put("project")
                .set("name", "work")
                .set("root", "/work")
                .build()],
        );
        data.apply(
            store::SHARED,
            &[Mutation::put("todo")
                .set("root", "/work")
                .set("text", "**Parent** with a long description and 雪 text")
                .set("done", false)
                .build()],
        );
        data.apply(
            store::SHARED,
            &[
                Mutation::put("todo")
                    .set("root", "/work")
                    .set("text", "child with `code`\nand a second line")
                    .set("done", false)
                    .set("parent_id", 1u64)
                    .build(),
                Mutation::put("todo")
                    .set("root", "/work")
                    .set("text", "Another top-level item")
                    .set("done", false)
                    .build(),
                Mutation::put("todo")
                    .set("root", "/work")
                    .set("text", "Finished item")
                    .set("done", true)
                    .build(),
            ],
        );
    }
    (data, responder)
}
fn pair(populated: bool) -> (Pair, Arc<DataHost>, Arc<DataHost>) {
    let (wasm, wh) = host(populated);
    let (steel, sh) = host(populated);
    (
        Pair::with_hosts("todos", BTreeMap::new(), wh, sh, None),
        wasm,
        steel,
    )
}
#[test]
fn todos_actual_runtimes_match_empty_registry_and_key_sheet() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "todos_actual_runtimes_match_empty_registry_and_key_sheet",
    );
    let (mut pair, _, _) = pair(false);
    pair.viewports();
    for key in [
        "a", "p", "Enter", "?", "j", "PageDown", "PageUp", "Esc", "Ctrl r",
    ] {
        pair.key(key);
        pair.viewports();
    }
}
#[test]
fn todos_actual_runtimes_match_tree_edits_drafts_completion_and_delete() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "todos_actual_runtimes_match_tree_edits_drafts_completion_and_delete",
    );
    let (mut pair, _, _) = pair(true);
    pair.viewports();
    for key in ["Down", "Up", "v", "v", "Space", "n", "e"] {
        pair.key(key);
        pair.viewports();
    }
    pair.event(Event::PastedText(" edited".into()));
    pair.viewports();
    for key in ["Esc", "e", "Enter", "a"] {
        pair.key(key);
        pair.viewports();
    }
    pair.event(Event::PastedText("new todo\n\twith paste\n".into()));
    pair.viewports();
    for key in ["Enter", "Up", "Space", "v", "d", "n", "d", "y"] {
        pair.key(key);
        pair.viewports();
    }
}
fn mutate_both(wasm: &DataHost, steel: &DataHost, mutations: &[Mutation]) {
    assert_eq!(
        wasm.apply(store::SHARED, mutations),
        steel.apply(store::SHARED, mutations)
    );
}
fn deliver(pair: &mut Pair, wasm: &DataHost, steel: &DataHost) {
    let events = wasm.changed();
    assert_eq!(events, steel.changed());
    for event in events {
        pair.event(event);
    }
}
#[test]
fn todos_actual_runtimes_preserve_conflicts_until_overwrite_was_visible() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "todos_actual_runtimes_preserve_conflicts_until_overwrite_was_visible",
    );
    let (mut pair, wasm, steel) = pair(true);
    pair.key("e");
    pair.event(Event::PastedText(" mine".into()));
    mutate_both(
        &wasm,
        &steel,
        &[Mutation::update("todo", Pred::eq("id", 1u64))
            .set("text", "the other writer's words")
            .build()],
    );
    pair.key("Enter");
    pair.render(1, 8);
    let waiting = pair.key("Enter");
    assert!(!waiting
        .iter()
        .any(|command| command.get("DataApply").is_some()));
    pair.render(24, 100);
    let overwrite = pair.key("Enter");
    assert!(overwrite
        .iter()
        .any(|command| command.get("DataApply").is_some()));
    pair.viewports();
    deliver(&mut pair, &wasm, &steel);
    pair.key("Space");
    mutate_both(
        &wasm,
        &steel,
        &[Mutation::put("todo")
            .set("root", "/work")
            .set("text", "arrived while confirmation was open")
            .set("done", false)
            .build()],
    );
    pair.key("y");
    pair.viewports();
}
#[test]
fn todos_actual_runtimes_match_move_watches_project_drafts_and_orphaned_input() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "todos_actual_runtimes_match_move_watches_project_drafts_and_orphaned_input",
    );
    let (mut pair, wasm, steel) = pair(true);
    pair.key("Down");
    pair.key("m");
    pair.viewports();
    pair.key("Down");
    pair.key("Enter");
    pair.viewports();
    pair.key("m");
    pair.key("t");
    pair.viewports();
    pair.key("a");
    pair.event(Event::PastedText("draft for work".into()));
    pair.key("Esc");
    mutate_both(
        &wasm,
        &steel,
        &[Mutation::put("project")
            .set("name", "another")
            .set("root", "/another")
            .build()],
    );
    deliver(&mut pair, &wasm, &steel);
    pair.key("p");
    pair.key("Up");
    pair.key("Enter");
    pair.viewports();
    pair.key("a");
    pair.event(Event::PastedText("another project's draft".into()));
    pair.key("Esc");
    pair.key("p");
    pair.key("Down");
    pair.key("Enter");
    pair.key("a");
    pair.viewports();
    pair.key("Esc");
    pair.key("e");
    pair.event(Event::PastedText(" keep these edits".into()));
    mutate_both(
        &wasm,
        &steel,
        &[Mutation::delete("todo", Pred::eq("id", 1u64))],
    );
    deliver(&mut pair, &wasm, &steel);
    pair.viewports();
    pair.key("Enter");
    pair.viewports();
}
#[test]
fn todos_actual_runtimes_match_agent_launch_focus_ring_mouse_and_hidden_drafts() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "todos_actual_runtimes_match_agent_launch_focus_ring_mouse_and_hidden_drafts",
    );
    use serde_json::json;
    let (mut pair, _, _) = pair(true);
    let tabs = serde_json::from_value(json!({"id":7,"pane_id":{"Plugin":1},"active":0,"bar_focused":[],"tabs":[{"descriptor":{"label":"Todos","location":"yinz:todos"},"plugin_id":1,"state":"Ready"}]})).unwrap();
    pair.event(Event::PaneTabsUpdate(vec![tabs]));
    pair.render(16, 70);
    pair.event(Event::Mouse(Mouse::LeftClick(3, 10)));
    let launch = pair.key("n");
    assert!(launch
        .iter()
        .any(|command| command.get("LoadNewPlugin").is_some()));
    for key in [
        "Ctrl o",
        "?",
        "Tab",
        "Right",
        "Enter",
        "Shift Tab",
        "Shift Tab",
    ] {
        pair.key(key);
        pair.render(16, 70);
    }
    pair.pipe(PipeMessage::new(
        PipeSource::Plugin(yinz_types::internal_ids::PluginId::new(2)),
        "agent_strip_take",
        &None,
        &Some(BTreeMap::from([("stop".into(), "history".into())])),
        false,
    ));
    pair.key("a");
    pair.event(Event::PastedText("draft".into()));
    pair.key("Alt Enter");
    pair.event(Event::PastedText("continued".into()));
    pair.viewports();
    pair.event(Event::Visible(false));
    pair.viewports();
    pair.key("a");
    pair.viewports();
    pair.key("Esc");
    pair.event(Event::Mouse(Mouse::ScrollDown(2)));
    pair.viewports();
    pair.event(Event::Mouse(Mouse::ScrollUp(2)));
    pair.viewports();
}
