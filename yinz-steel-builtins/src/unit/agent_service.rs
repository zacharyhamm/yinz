use crate::tests::plugin_with_source;
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn service_modules_compile_in_the_confined_graph() {
    let (_, _) = plugin_with_source("agent-service", "(require \"./agent-service/state.scm\") (require \"./agent-service/probes.scm\") (require \"./agent-service/messages.scm\") (require \"./agent-service/runs.scm\") (require \"./agent-service/observe.scm\") (require \"./agent-service/summaries.scm\") (require \"./agent-service/completions.scm\") (require \"./agent-service/operations.scm\")", json!({}), BTreeMap::new(), "");
}

#[test]
fn publishing_and_persistence_keep_long_histories_and_other_live_agents() {
    let (mut runtime, host) = plugin_with_source(
        "agent-service",
        r#"
        (require "./agent-service/state.scm" "./agent-service/messages.scm"
                 "./agent-service/runs.scm" "./agent-service/probes.scm")
        (define original #f)
        (define (seed run other)
          (set! original run)
          (as-set! "host" "history-host") (as-set! "session" "history-session")
          (as-set! "panes" '()) (as-set! "subscribers" '(42))
          (as-map-set! "runs" 7 run) (as-map-set! "runs" 8 other)
          (as-map-set! "persisted" (hash-ref run "id") 8000))
        (define (append-and-persist)
          (for-each (lambda (pane)
            (define run (append-entry (as-map-get "runs" pane #f) "result" "new result" #f json-null 90000))
            (as-map-set! "runs" pane run) (as-map-set! "dirty-runs" (hash-ref run "id") run)) '(7 8))
          (as-publish! #t) (as-call "persist"))
        (define (retained?)
          (equal? (take (hash-ref (as-map-get "runs" 7 #f) "entries") 8000) (hash-ref original "entries")))
        (define (persisted) (as-get "persisted"))
        (define (dirty-count) (length (as-keys (as-get "dirty-runs"))))
        "#,
        json!({}),
        BTreeMap::new(),
        "",
    );
    let entries: Vec<_> = (1..=8_000).map(|revision| json!({
        "revision":revision, "event_id":null, "created_at_ms":revision,
        "kind":if revision == 1 { "prompt" } else { "tool_result" },
        "text":if revision == 1 { "Earlier prompt title".into() } else { format!("tool output {revision}") },
    })).collect();
    let mut run = json!({
        "id":"claude:long", "pane_id":7, "kind":"claude", "session_id":"long",
        "host_id":"history-host", "yinz_session_name":"history-session", "started_at_ms":1000,
        "ended_at_ms":null, "outcome":"active", "revision":8000, "entries":entries,
    });
    let mut other = json!({
        "id":"claude:other", "pane_id":8, "kind":"claude", "session_id":"other",
        "host_id":"history-host", "yinz_session_name":"history-session", "started_at_ms":1000,
        "ended_at_ms":null, "outcome":"active", "revision":0, "entries":[],
    });
    runtime
        .invoke("seed", vec![run.clone(), other.clone()])
        .unwrap();
    for added in 1..=2 {
        host.lock().unwrap().commands.clear();
        runtime.invoke("append-and-persist", vec![]).unwrap();
        assert_eq!(runtime.invoke("retained?", vec![]).unwrap(), true);
        assert_eq!(runtime.invoke("dirty-count", vec![]).unwrap(), 0);
        assert_eq!(
            runtime.invoke("persisted", vec![]).unwrap(),
            json!({"claude:long":8000 + added, "claude:other":added})
        );
        let commands = host.lock().unwrap().commands.clone();
        let snapshot = commands
            .iter()
            .find_map(|command| command.get("MessageToPlugin"))
            .unwrap();
        assert_eq!(snapshot["message_name"], "agent_snapshot");
        let snapshot: serde_json::Value =
            serde_json::from_str(snapshot["message_payload"].as_str().unwrap()).unwrap();
        assert_eq!(snapshot["runs"].as_array().unwrap().len(), 2);
        assert_eq!(snapshot["runs"][0]["title"], "Earlier prompt title");
        let writes: Vec<_> = commands
            .iter()
            .filter_map(|command| command.get("DataApply"))
            .collect();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0]["namespace"], "shared:yinz");
        let mutations = writes[0]["mutations"].as_array().unwrap();
        assert_eq!(mutations.len(), 4);
        for (run, revision) in [(&mut run, 8000 + added), (&mut other, added)] {
            let entry = json!({"revision":revision, "event_id":null, "kind":"result", "text":"new result", "created_at_ms":90000});
            run["revision"] = json!(revision);
            run["entries"].as_array_mut().unwrap().push(entry.clone());
            for (op, args) in [
                ("RunMutation", json!([run])),
                ("EntryMutation", json!([run["id"], run["host_id"], entry])),
            ] {
                let expected = runtime
                    .invoke("yinz-agent-model", vec![json!("agents"), json!(op), args])
                    .unwrap();
                assert!(mutations.contains(&expected));
            }
            assert_eq!(
                snapshot["runs"][if run["pane_id"] == 7 { 0 } else { 1 }]["revision"],
                revision
            );
        }
    }
}
