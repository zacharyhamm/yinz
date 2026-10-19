use crate::tests::plugin_with_source;
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn shipped_note_attachments_guard_both_endpoints() {
    use std::sync::{Arc, Mutex};
    use yinz_steel::{modules::SourceRoot, Runtime};
    let batches = Arc::new(Mutex::new(Vec::new()));
    let recorded = batches.clone();
    let mut runtime = Runtime::new(move |command: serde_json::Value| {
        yinz_steel::binary::decode_command(command.clone()).unwrap();
        if let Some(request) = command.get("DataFetch") {
            let (columns, rows) = match request["request"]["entity"].as_str().unwrap() {
                "note" => (
                    json!(["id", "rev", "title"]),
                    json!([[{"Int":17}, {"Int":4}, {"Text":"note"}]]),
                ),
                "todo" => (json!(["id", "rev"]), json!([[{"Int":9}, {"Int":8}]])),
                _ => (json!([]), json!([])),
            };
            Ok(json!({"columns":columns,"rows":rows,"truncated":false,"next_cursor":null}))
        } else if let Some(request) = command.get("DataApply") {
            recorded.lock().unwrap().push(request["mutations"].clone());
            Ok(json!({"rows_written":1,"rows_deleted":0,"ids":[17],"per_mutation":[1]}))
        } else if command.get("DataCount").is_some() {
            Ok(json!(0))
        } else {
            Ok(json!(null))
        }
    })
    .unwrap();
    let mut root = SourceRoot::file(&crate::asset_dir().join("project-store.scm"), None).unwrap();
    runtime
        .eval_with_source_root("(require \"./project-store/notes.scm\")", &mut root)
        .unwrap();
    runtime.load(json!({})).unwrap();
    for (operation, args) in [
        ("note_link", json!({"note_id":17,"todo_id":9})),
        (
            "note_add",
            json!({"global":true,"title":"note","body":"body","tags":["tag"],"todo_ids":[9]}),
        ),
    ] {
        batches.lock().unwrap().clear();
        runtime
            .invoke("ps-note-mutate", vec![json!(operation), args])
            .unwrap();
        let batches = batches.lock().unwrap();
        let mutations = batches.last().unwrap().as_array().unwrap();
        for (entity, revisions) in [("note", json!([[17, 4]])), ("todo", json!([[9, 8]]))] {
            assert!(
                mutations.iter().any(|mutation| {
                    if entity == "note" && operation == "note_add" {
                        mutation["Update"]["entity"] == "note"
                            && mutation["Update"]["filter"] == json!({"Eq":["id",{"Int":17}]})
                            && mutation["Update"]["expect_rev"] == 4
                    } else {
                        mutation["AssertRows"]["entity"] == entity
                            && mutation["AssertRows"]["rows"] == revisions
                    }
                }),
                "{operation} did not guard {entity}: {mutations:?}"
            );
        }
        assert!(mutations
            .iter()
            .any(|mutation| mutation["Put"]["entity"] == "note_link"));
    }
}

#[test]
fn project_store_modules_compile_in_the_confined_graph() {
    let _ = plugin_with_source("project-store", "(require \"./project-store/ops.scm\" \"./project-store/import.scm\" \"./project-store/arrangement.scm\")", json!({}), BTreeMap::new(), "");
}

#[test]
fn arrangement_respects_provider_identity_and_explicit_project_roots() {
    let (mut runtime, _) = plugin_with_source(
        "project-store",
        r#"(require "./project-store/arrangement.scm")
        (define (arrange registry reply) (ps-arrange registry reply))"#,
        json!({}),
        BTreeMap::new(),
        "",
    );
    for (discovered, child_identity, grouped) in [
        (true, "child", false),
        (true, "parent", true),
        (false, "child", true),
    ] {
        let pane = |id, cwd| {
            json!({"pane_id":id,"title":"shell","cwd":cwd,"command":null,
                "status":"Running","tab":null})
        };
        let result = runtime
            .invoke(
                "arrange",
                vec![
                    json!({"work":{"name":"work","root":"/work",
                        "provider_identity":"parent","discovered":discovered}}),
                    json!({"panes":[pane(1, "/work"), pane(2, "/work/child")],
                        "repositories":{
                            "/work":{"root":"/work","provider_identity":"parent","display_name":"Work"},
                            "/work/child":{"root":"/work/child","provider_identity":child_identity,"display_name":"Child"}
                        }}),
                ],
            )
            .unwrap();
        assert_eq!(result[1], 0);
        let groups = result[0].as_array().unwrap();
        let parent = groups
            .iter()
            .find(|group| group["identity"] == "project:/work")
            .unwrap();
        assert_eq!(
            parent["pane_ids"],
            if grouped { json!([1, 2]) } else { json!([1]) }
        );
        assert_eq!(groups.len(), if grouped { 1 } else { 2 });
        if !grouped {
            let child = groups
                .iter()
                .find(|group| group["identity"] == "provider:child")
                .unwrap();
            assert_eq!(child["pane_ids"], json!([2]));
            assert_eq!(child["name"], "Child");
        }
    }
}
