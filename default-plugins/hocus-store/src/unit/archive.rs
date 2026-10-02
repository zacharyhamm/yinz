use super::*;
use yinz_tile::prelude::{Mutation, Pred};

#[test]
fn archive_replies_identify_sessions_and_hosts_with_reused_pane_ids() {
    let backend = store::Local::shared("hocus");
    backend.define_all(&store::shared_defs()).unwrap();
    let owners = [
        ("claude:first", "main", "host-a"),
        ("claude:second", "other", "host-a"),
        ("claude:third", "main", "host-b"),
    ];
    for (id, session, host_id) in owners {
        let run: agents::AgentRun = serde_json::from_value(json!({
            "id": id, "pane_id": 7, "yinz_session_name": session,
            "host_id": host_id, "kind": "claude", "started_at_ms": 1,
        }))
        .unwrap();
        backend.apply(agents::rows::run_mutations(&run)).unwrap();
        if id == "claude:second" {
            // Existing archived headers predate these fields; retain ownership from the row.
            let mut header = serde_json::to_value(run.header()).unwrap();
            header.as_object_mut().unwrap().remove("yinz_session_name");
            header.as_object_mut().unwrap().remove("host_id");
            backend
                .apply(vec![Mutation::update(
                    agents::rows::RUN_ENTITY,
                    Pred::eq("uid", id),
                )
                .set("header", header.to_string())
                .build()])
                .unwrap();
        }
    }
    let listed: Value =
        serde_json::from_str(&query(&backend, "agent_runs", &json!({})).unwrap()).unwrap();
    assert_eq!(listed["count"], owners.len());
    for (id, session, host_id) in owners {
        let header = listed["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|header| header["run_id"] == id)
            .unwrap();
        let detail: Value =
            serde_json::from_str(&query(&backend, "agent_run", &json!({"run_id": id})).unwrap())
                .unwrap();
        for reply in [header, &detail] {
            assert_eq!(reply["pane_id"], 7);
            assert_eq!(reply["session"], session);
            assert_eq!(reply["host_id"], host_id);
        }
    }
}

#[test]
fn run_pages_can_be_followed_with_structured_revision_cursors() {
    let backend = store::Local::shared("hocus");
    backend.define_all(&store::shared_defs()).unwrap();
    let run: agents::AgentRun = serde_json::from_value(json!({
        "id": "claude:paged", "pane_id": 7, "yinz_session_name": "main",
        "host_id": "host-a", "kind": "claude", "started_at_ms": 1,
    }))
    .unwrap();
    let run = agents::AgentRun {
        entries: (1..=3)
            .map(|revision| agents::RunEntry {
                revision,
                event_id: Some(format!("event-{revision}")),
                kind: agents::RunEntryKind::Result,
                text: format!("result {revision}"),
                created_at_ms: Some(revision as i64),
            })
            .collect(),
        revision: 3,
        ..run
    };
    backend.apply(agents::rows::run_mutations(&run)).unwrap();
    let first: Value = serde_json::from_str(
        &query(
            &backend,
            "agent_run",
            &json!({
                "run_id": run.id, "from_revision": 1, "limit": 2,
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(first["more"], true);
    assert_eq!(first["next_revision"], 3);
    assert_eq!(first["entries"].as_array().unwrap().len(), 2);
    let last: Value = serde_json::from_str(
        &query(
            &backend,
            "agent_run",
            &json!({
                "run_id": run.id, "from_revision": first["next_revision"], "limit": 2,
            }),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(last["entries"][0]["text"], "result 3");
    assert_eq!(last["more"], false);
    assert!(last["next_revision"].is_null());
}
