use crate::tests::plugin_with_source;
use serde_json::json;
use std::collections::BTreeMap;

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
