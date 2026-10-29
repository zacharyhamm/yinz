use crate::tests::plugin_with_source;
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn project_store_modules_compile_in_the_confined_graph() {
    let _ = plugin_with_source("project-store", "(require \"./project-store/ops.scm\" \"./project-store/import.scm\" \"./project-store/arrangement.scm\")", json!({}), BTreeMap::new(), "");
}

#[test]
fn archive_listing_uses_shared_pagination_and_permission_checked_fetches() {
    let header = json!({"id":"first", "pane_id":7, "kind":"claude", "started_at_ms":1});
    let rows = json!({"columns":["header", "host_id", "yinz_session_name"],
        "rows":[[{"Text":header.to_string()}, {"Text":"host-a"}, {"Text":"main"}]],
        "truncated":true, "next_cursor":null});
    let (mut runtime, host) = plugin_with_source(
        "project-store",
        r#"(require "./shared/archive.scm")
        (define (list-runs filter limit offset) (archive-list-runs filter limit offset))"#,
        json!({}),
        BTreeMap::from([("DataFetch".into(), rows)]),
        "",
    );
    let page = runtime
        .invoke("list-runs", vec![json!({}), json!(50), json!(7)])
        .unwrap();
    assert_eq!(page["more"], true);
    assert_eq!(page["scanned"], 1);
    assert_eq!(page["headers"][0]["host_id"], "host-a");
    assert_eq!(page["headers"][0]["yinz_session_name"], "main");
    {
        let host = host.lock().unwrap();
        assert_eq!(host.commands.len(), 1);
        let request = &host.commands[0]["DataFetch"]["request"];
        assert_eq!(request["namespace"], "shared:yinz");
        assert_eq!(request["entity"], "agent_run");
        assert_eq!(request["limit"], 51);
        assert_eq!(request["offset"], 7);
        assert_eq!(
            request["select"],
            json!(["header", "host_id", "yinz_session_name"])
        );
    }

    host.lock()
        .unwrap()
        .failures
        .insert("DataFetch".into(), "permission denied".into());
    assert!(runtime
        .invoke("list-runs", vec![json!({}), json!(50), json!(8)])
        .is_err());
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
