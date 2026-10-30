use crate::tests::plugin_with_source;
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn service_modules_compile_in_the_confined_graph() {
    let (_, _) = plugin_with_source("agent-service", "(require \"./agent-service/state.scm\") (require \"./agent-service/probes.scm\") (require \"./agent-service/messages.scm\") (require \"./agent-service/runs.scm\") (require \"./agent-service/observe.scm\") (require \"./agent-service/summaries.scm\") (require \"./agent-service/completions.scm\") (require \"./agent-service/operations.scm\")", json!({}), BTreeMap::new(), "");
}

#[test]
fn host_hook_receipt_times_are_published_without_retimestamping() {
    let (mut runtime, host) = plugin_with_source(
        "agent-service",
        r#"
        (require "./agent-service/state.scm" "./agent-service/observe.scm")
        (define (load _)
          (as-reset!) (as-set! "subscribers" '(42))
          (for-each (lambda (name) (as-register! name (lambda args (void))))
            '("record-event" "drain" "persist" "resolve-repositories" "adopt-arrived-runs"
              "finish-run" "publish-arrangement")))
        (define (rebuild manifest) (as-set! "manifest" manifest) (as-rebuild!))
        (define (hook id name now archive)
          (vector-set! test-clock 0 now)
          (define event (as-model "HookEvent" (yinz-json-string
              (hash "hook_event_name" name "session_id" "same-session" "archive_only" archive))))
          (as-call "hook" id "claude" (hash-ref event "Ok")))
        (define (stamp id) (as-field (as-pane id) "last_hook_at_ms" json-null))
        "#,
        json!({}),
        BTreeMap::new(),
        "(define test-clock (vector 0)) (define (yinz-wall-clock-ms) (vector-ref test-clock 0))",
    );
    let pane = |id| yinz_types::data::PaneInfo {
        id: yinz_types::pane_id::PaneId::from_legacy(id, false),
        title: "claude".to_owned(),
        terminal_command: Some("claude".to_owned()),
        ..Default::default()
    };
    let mut manifest = json!({"panes":{"0":[pane(7), pane(8)]}});
    runtime.invoke("rebuild", vec![manifest.clone()]).unwrap();
    assert_eq!(
        runtime.invoke("stamp", vec![json!(7)]).unwrap(),
        json!(null)
    );

    let published = || {
        host.lock()
            .unwrap()
            .commands
            .iter()
            .filter_map(|command| command.get("MessageToPlugin"))
            .filter(|message| message["message_name"] == "agent_snapshot")
            .map(|message| {
                serde_json::from_str::<serde_json::Value>(
                    message["message_payload"].as_str().unwrap(),
                )
                .unwrap()
            })
            .collect::<Vec<_>>()
    };
    for (id, name, now) in [
        (7, "PreCompact", 100),
        (8, "PreCompact", 200),
        // An identical hook still advances the timestamp and publishes a fresh snapshot.
        (7, "PreCompact", 300),
        // Worker lifecycle hooks do not change the parent's phase.
        (8, "SubagentStart", 400),
        (7, "SessionEnd", 500),
    ] {
        // The host delivers its manifest before the hook. The service clock can differ.
        manifest["panes"]["0"][if id == 7 { 0 } else { 1 }]["last_hook_at_ms"] = json!(now);
        runtime.invoke("rebuild", vec![manifest.clone()]).unwrap();
        runtime
            .invoke(
                "hook",
                vec![json!(id), json!(name), json!(now + 9000), json!(false)],
            )
            .unwrap();
        assert_eq!(runtime.invoke("stamp", vec![json!(id)]).unwrap(), now);
        let snapshots = published();
        let snapshot = snapshots.last().unwrap();
        let pane = snapshot["panes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|pane| pane["pane_id"] == id)
            .unwrap();
        assert_eq!(pane["last_hook_at_ms"], now);
    }
    assert_eq!(published().len(), 5);
    // Transcript replay and hooks for a pane that has closed do not change list ordering.
    for (id, archive) in [(7, true), (99, false)] {
        runtime
            .invoke(
                "hook",
                vec![json!(id), json!("PreCompact"), json!(600), json!(archive)],
            )
            .unwrap();
    }
    assert_eq!(published().len(), 5);
    assert_eq!(runtime.invoke("stamp", vec![json!(7)]).unwrap(), 500);

    let mut updated = manifest.clone();
    updated["panes"]["0"][0]["title"] = json!("new title");
    updated["panes"]["0"][0]["is_suppressed"] = json!(true);
    runtime.invoke("rebuild", vec![updated]).unwrap();
    assert_eq!(runtime.invoke("stamp", vec![json!(7)]).unwrap(), 500);
    assert_eq!(runtime.invoke("stamp", vec![json!(8)]).unwrap(), 400);
    // Removal and rerunning a pane must not give a new agent the previous agent's timestamp.
    runtime
        .invoke("rebuild", vec![json!({"panes":{"0":[pane(8)]}})])
        .unwrap();
    manifest["panes"]["0"][0]
        .as_object_mut()
        .unwrap()
        .remove("last_hook_at_ms");
    runtime.invoke("rebuild", vec![manifest.clone()]).unwrap();
    assert_eq!(
        runtime.invoke("stamp", vec![json!(7)]).unwrap(),
        json!(null)
    );
    // Old peer metadata has no recency; a held pane retains the host timestamp.
    manifest["panes"]["0"][1]["last_hook_at_ms"] = json!(400);
    let mut held = manifest.clone();
    held["panes"]["0"][1]["is_held"] = json!(true);
    runtime.invoke("rebuild", vec![held]).unwrap();
    assert_eq!(runtime.invoke("stamp", vec![json!(8)]).unwrap(), 400);
    manifest["panes"]["0"][1]["last_hook_at_ms"] = json!(null);
    runtime.invoke("rebuild", vec![manifest]).unwrap();
    assert_eq!(
        runtime.invoke("stamp", vec![json!(8)]).unwrap(),
        json!(null)
    );
}

#[test]
fn named_branch_repository_publishes_snapshots_and_supplies_summary_branch() {
    assert_repository_snapshot_and_summary(Some("main"));
}

#[test]
fn detached_repository_publishes_snapshots_and_supplies_null_summary_branch() {
    assert_repository_snapshot_and_summary(Some("HEAD"));
}

#[test]
fn ordinary_directory_publishes_snapshots_and_supplies_null_summary_branch() {
    assert_repository_snapshot_and_summary(None);
}

fn assert_repository_snapshot_and_summary(head: Option<&str>) {
    let source = format!(
        "{}\n{}",
        include_str!("../../assets/plugins/agent-service/summaries.scm")
            .replace("./state.scm", "./agent-service/state.scm"),
        r#"
        (require "./agent-service/probes.scm")
        (define (seed panes repository other)
          (as-set! "panes" panes) (as-set! "subscribers" '(42 43))
          (for-each (lambda (pane)
              (define id (hash-ref pane "pane_id"))
              (as-record-cwd! id (hash-ref pane "cwd"))
              (as-map-set! "repositories" (as-location id) (if (= id 9) other repository))) panes))
        (define (publish focused)
          (as-set! "focused" focused) (as-publish! #t))
        "#,
    );
    let (mut runtime, host) =
        plugin_with_source("agent-service", &source, json!({}), BTreeMap::new(), "");
    let mut repository = |head: Option<&str>| {
        if let Some(head) = head {
            let metadata = format!("/repo\n/repo/.git\n/repo/.git\n{head}\n");
            runtime
                .invoke(
                    "as-model",
                    vec![
                        json!("GitProbe"),
                        json!(0),
                        json!(metadata.as_bytes()),
                        json!([]),
                    ],
                )
                .unwrap()["Checkout"]
                .clone()
        } else {
            runtime
                .invoke(
                    "as-model",
                    vec![json!("RepositoryFolder"), json!("/repo"), json!(null)],
                )
                .unwrap()
        }
    };
    let other = repository(Some("other"));
    let repository = repository(head);
    let branch = head.filter(|head| *head != "HEAD");
    assert_eq!(
        repository.get("branch"),
        branch.map(|branch| json!(branch)).as_ref()
    );
    assert_eq!(repository.get("common_dir").is_some(), head.is_some());
    let panes: Vec<_> = [7, 8, 9]
        .into_iter()
        .map(|id| {
            json!({
                "pane_id":id, "title":"Claude", "command":"claude", "status":"Running",
                "cwd":if id == 9 { "/other" } else { "/repo" }, "tab":[1, "agents"], "agent":null,
            })
        })
        .collect();
    runtime
        .invoke("seed", vec![json!(panes), repository.clone(), other])
        .unwrap();
    let label = match head {
        Some("HEAD") => Some("repo"),
        Some(_) => Some("repo:main"),
        None => None,
    };
    for focused in [9, 7] {
        host.lock().unwrap().commands.clear();
        runtime.invoke("publish", vec![json!(focused)]).unwrap();
        let commands = host.lock().unwrap().commands.clone();
        let prefixes: Vec<_> = commands
            .iter()
            .filter_map(|command| command.get("SetPaneTitlePrefix"))
            .map(|prefix| {
                (
                    prefix["pane_id"]["Terminal"].clone(),
                    prefix["prefix"].clone(),
                )
            })
            .collect();
        // Labels name each pane's checkout, so moving focus must not resend them.
        let expected: Vec<_> = if focused == 9 {
            [7, 8]
                .into_iter()
                .filter_map(|id| label.map(|label| (json!(id), json!(label))))
                .chain([(json!(9), json!("repo:other"))])
                .collect()
        } else {
            vec![]
        };
        assert_eq!(prefixes, expected);
        let snapshots: Vec<_> = commands
            .iter()
            .filter_map(|command| command.get("MessageToPlugin"))
            .collect();
        assert_eq!(snapshots.len(), 2);
        for (snapshot, subscriber) in snapshots.iter().zip([42, 43]) {
            assert_eq!(snapshot["message_name"], "agent_snapshot");
            assert_eq!(snapshot["destination_plugin_id"], subscriber);
            let payload: serde_json::Value =
                serde_json::from_str(snapshot["message_payload"].as_str().unwrap()).unwrap();
            assert_eq!(payload["focused"], focused);
            assert_eq!(payload["panes"].as_array().unwrap().len(), 3);
            assert_eq!(payload["repositories"]["/repo"], repository);
        }
    }
    assert_eq!(
        runtime.invoke("branch", vec![json!(7)]).unwrap(),
        json!(branch)
    );
    assert_eq!(runtime.invoke("branch", vec![json!(9)]).unwrap(), "other");
    assert_eq!(
        runtime.invoke("branch", vec![json!(99)]).unwrap(),
        json!(null)
    );
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
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0]["namespace"], "shared:yinz");
        let mutations: Vec<_> = writes
            .iter()
            .flat_map(|write| write["mutations"].as_array().unwrap())
            .collect();
        assert_eq!(mutations.len(), 4);
        for (run, revision) in [(&mut run, 8000 + added), (&mut other, added)] {
            let entry = json!({"revision":revision, "event_id":null, "kind":"result", "text":"new result", "created_at_ms":90000});
            run["revision"] = json!(revision);
            run["entries"].as_array_mut().unwrap().push(entry.clone());
            for (op, args) in [
                ("RunMutation", json!([run, added - 1])),
                ("EntryMutation", json!([run["id"], run["host_id"], entry])),
            ] {
                let expected = runtime
                    .invoke("yinz-agent-model", vec![json!("agents"), json!(op), args])
                    .unwrap();
                assert!(mutations.contains(&&expected));
            }
            assert_eq!(
                snapshot["runs"][if run["pane_id"] == 7 { 0 } else { 1 }]["revision"],
                revision
            );
        }
    }
}

const RUN_STORE_PRELUDE: &str = r#"
    (define fail-writes #f)
    (define (write-failures enabled) (set! fail-writes enabled))
    (define stored #f) (define store-rev 0) (define attempts '())
    (define fail-read #f) (define race-retry #f) (define race-read #f) (define metadata-reads 0)
    (define (set-store run rev) (set! stored run) (set! store-rev rev))
    (define (faults read race) (set! fail-read read) (set! race-retry race) (set! metadata-reads 0))
    (define (commit-during-read run) (set! race-read run))
    (define (rows columns values) (hash "columns" columns "rows" values "truncated" #f "next_cursor" json-null))
    (define (entry-row entry)
      (map (lambda (key)
          (define value (hash-ref entry key))
          (cond ((json-null? value) "Null") ((number? value) (hash "Int" value)) (else (hash "Text" value))))
        '("revision" "event_id" "kind" "text" "created_at_ms")))
    (define (yinz-data-fetch namespace request)
      (when (and (equal? fail-read "entries") (equal? (hash-ref request "entity") "agent_run_entry")) (error "injected read failure"))
      (when (and race-read (equal? (hash-ref request "entity") "agent_run_entry"))
        (set-store race-read (+ store-rev 1)) (set! race-read #f))
      (if (equal? (hash-ref request "entity") "agent_run")
        (begin
          (set! metadata-reads (+ metadata-reads 1))
          (when (or (equal? fail-read "metadata") (and (equal? fail-read "validation") (= metadata-reads 2)))
            (error "injected metadata read failure"))
          (rows '("uid" "rev" "payload" "host_id" "yinz_session_name")
            (if (and (equal? fail-read "missing") (= metadata-reads 2)) '()
              (list (list (hash "Text" "claude:handoff") (hash "Int" store-rev)
                (hash "Text" (yinz-json-string (hash-insert stored "entries" '())))
                (hash "Text" (hash-ref stored "host_id")) (hash "Text" (hash-ref stored "yinz_session_name")))))))
        (rows '("run_id" "revision" "event_id" "kind" "text" "created_at_ms")
          (map (lambda (entry) (cons (hash "Text" "claude:handoff") (entry-row entry))) (hash-ref stored "entries")))))
    (define (yinz-data-fetch-all namespace request) (yinz-data-fetch namespace request))
    (define (matches-owner predicate)
      (cond ((hash-contains? predicate "And")
          (foldl (lambda (part matches) (and matches (matches-owner part))) #t (hash-ref predicate "And")))
        ((hash-contains? predicate "Or")
          (foldl (lambda (part matches) (or matches (matches-owner part))) #f (hash-ref predicate "Or")))
        (else
          (define eq (hash-ref predicate "Eq"))
          (equal? (hash-ref stored (if (equal? (car eq) "uid") "id" (car eq))) (hash-ref (cadr eq) "Text")))))
    (define (yinz-data-apply namespace mutations)
      (define owned (hash-contains? (car mutations) "Update"))
      (define put (hash-ref (car mutations) (if owned "Update" "Put")))
      (define expected (hash-ref put "expect_rev"))
      (set! attempts (append attempts (list expected)))
      (when fail-writes (error "injected write failure"))
      (when (and owned (not (matches-owner (hash-ref put "filter")))) (error "owner conflict"))
      (when (and race-retry (= expected store-rev))
        (set! race-retry #f) (set! store-rev (+ store-rev 1))
        (define revision (+ 1 (hash-ref stored "revision")))
        (set! stored (hash-insert (hash-insert stored "revision" revision) "entries"
            (append (hash-ref stored "entries") (list (hash "revision" revision "event_id" json-null
                "kind" "result" "text" "racing source" "created_at_ms" 20))))))
      (unless (= expected store-rev) (error "conflict"))
      (define payload (cadr (car (filter (lambda (pair) (equal? (car pair) "payload")) (hash-ref put "values")))))
      (define entries (hash-ref stored "entries"))
      (for-each (lambda (mutation)
          (define values (hash-ref (hash-ref mutation "Put") "values"))
          (define decoded (yinz-agent-model "agents" "EntriesFromRows"
              (list (rows (map car values) (list (map cadr values))))))
          (set! entries (append entries (hash-ref decoded "claude:handoff")))) (cdr mutations))
      (set! stored (hash-insert (yinz-json-parse (hash-ref payload "Text")) "entries" entries))
      (set! store-rev (+ store-rev 1)) #t)
"#;

#[test]
fn adopting_a_dirty_run_preserves_unsaved_history_across_failed_retries() {
    let (mut runtime, _) = plugin_with_source(
        "agent-service",
        r#"
        (require "./agent-service/state.scm" "./agent-service/messages.scm" "./agent-service/runs.scm")
        (define (setup run)
          (set-store run 0)
          (as-set! "host" "source-host") (as-set! "session" "source")
          (as-map-set! "runs" 7 run) (as-map-set! "run-bases" "claude:handoff" run))
        (define (append-local text)
          (define run (append-entry (as-map-get "runs" 7 #f) "result" text #f json-null 20))
          (as-map-set! "runs" 7 run) (as-map-set! "dirty-runs" "claude:handoff" run))
        (define (finish) (as-call "finish-run" 7 "Completed" 30))
        (define (resume)
          (as-set! "host" "destination-host") (as-set! "session" "destination")
          (as-set! "panes" (list (hash "pane_id" 8 "agent" (hash "run_id" "claude:handoff" "kind" "claude" "state" "Busy" "source" "Reported"))))
          (as-call "adopt-arrived-runs"))
        (define (persist-now) (as-call "persist"))
        (define (snapshot)
          (hash "stored" stored "store-rev" store-rev "attempts" attempts
            "dirty" (as-get "dirty-runs") "adoptions" (as-get "adoptions") "runs" (as-get "runs")
            "bases" (as-get "run-bases") "revisions" (as-get "run-revisions") "persisted" (as-get "persisted")))
        "#,
        json!({}),
        BTreeMap::new(),
        RUN_STORE_PRELUDE,
    );
    let entry = |revision, text: &str| json!({"revision":revision, "kind":"result", "text":text, "event_id":null, "created_at_ms":20});
    let id = "claude:handoff";
    let original = json!({
        "id":id, "pane_id":7, "kind":"claude", "session_id":"handoff",
        "host_id":"source-host", "yinz_session_name":"source", "started_at_ms":1,
        "ended_at_ms":null, "outcome":"active", "revision":0, "summarized_revision":0,
        "cwd":null, "host":null, "bubblewrap":null, "origin_run_id":null, "previous_run_id":null,
        "summary":null, "entries":[],
    });
    runtime.invoke("setup", vec![original]).unwrap();
    runtime
        .invoke("append-local", vec![json!("persisted")])
        .unwrap();
    runtime.invoke("persist-now", vec![]).unwrap();
    let saved = runtime.invoke("snapshot", vec![]).unwrap();
    assert_eq!(saved["stored"]["entries"], json!([entry(1, "persisted")]));
    assert_eq!(saved["store-rev"], 1);
    assert_eq!(saved["dirty"], json!({}));

    runtime
        .invoke("append-local", vec![json!("pending")])
        .unwrap();
    runtime.invoke("finish", vec![]).unwrap();
    runtime.invoke("write-failures", vec![json!(true)]).unwrap();
    runtime.invoke("persist-now", vec![]).unwrap();
    let ended = runtime.invoke("snapshot", vec![]).unwrap();
    assert_eq!(ended["stored"], saved["stored"]);
    assert_eq!(ended["attempts"], json!([0, 1, 1]));
    assert_eq!(ended["dirty"][id]["outcome"], "completed");
    assert_eq!(ended["dirty"][id]["ended_at_ms"], 30);
    assert_eq!(
        ended["dirty"][id]["entries"],
        json!([entry(1, "persisted"), entry(2, "pending")])
    );
    assert_eq!(ended["runs"], json!({}));
    runtime.invoke("resume", vec![]).unwrap();
    let adopted = runtime.invoke("snapshot", vec![]).unwrap();
    assert_eq!(adopted["bases"], ended["bases"]);
    assert_eq!(adopted["bases"][id], saved["stored"]);
    assert_eq!(adopted["revisions"][id], 1);
    assert_eq!(
        adopted["dirty"][id]["entries"],
        ended["dirty"][id]["entries"]
    );

    // Another server commits after adoption, then races the guarded reconciliation retry.
    let mut concurrent = saved["stored"].clone();
    concurrent["host_id"] = json!("other-host");
    concurrent["yinz_session_name"] = json!("other-session");
    concurrent["pane_id"] = json!(9);
    concurrent["outcome"] = json!("completed");
    concurrent["ended_at_ms"] = json!(40);
    concurrent["revision"] = json!(2);
    concurrent["entries"] = json!([entry(1, "persisted"), entry(2, "concurrent")]);
    runtime
        .invoke("set-store", vec![concurrent.clone(), json!(2)])
        .unwrap();
    runtime
        .invoke("write-failures", vec![json!(false)])
        .unwrap();
    runtime
        .invoke("faults", vec![json!(false), json!(true)])
        .unwrap();
    runtime.invoke("persist-now", vec![]).unwrap();
    let pending = runtime.invoke("snapshot", vec![]).unwrap();
    assert_eq!(
        pending["dirty"][id]["entries"],
        json!([
            entry(1, "persisted"),
            entry(2, "concurrent"),
            entry(3, "pending")
        ])
    );
    assert_eq!(pending["attempts"], json!([0, 1, 1, 1, 2]));
    assert_eq!(pending["store-rev"], 3);
    assert_eq!(pending["stored"]["host_id"], "other-host");
    assert_eq!(pending["adoptions"][id], true);
    assert_eq!(adopted["adoptions"][id], true);
    assert_eq!(pending["bases"][id], concurrent);
    assert_eq!(pending["revisions"][id], 2);
    assert_eq!(pending["persisted"][id], 2);
    assert_eq!(pending["runs"]["8"], pending["dirty"][id]);

    runtime.invoke("persist-now", vec![]).unwrap();
    let committed = runtime.invoke("snapshot", vec![]).unwrap();
    assert_eq!(committed["attempts"], json!([0, 1, 1, 1, 2, 2, 3]));
    assert_eq!(committed["store-rev"], 4);
    assert_eq!(
        committed["stored"]["entries"],
        json!([
            entry(1, "persisted"),
            entry(2, "concurrent"),
            entry(3, "racing source"),
            entry(4, "pending")
        ])
    );
    for run in [&pending["dirty"][id], &committed["stored"]] {
        assert_eq!(run["host_id"], "destination-host");
        assert_eq!(run["yinz_session_name"], "destination");
        assert_eq!(run["pane_id"], 8);
        assert_eq!(run["outcome"], "active");
        assert_eq!(run["ended_at_ms"], json!(null));
    }
    assert_eq!(committed["stored"]["revision"], 4);
    assert_eq!(committed["dirty"], json!({}));
    assert_eq!(committed["adoptions"], json!({}));
    assert_eq!(committed["bases"][id], committed["stored"]);
    assert_eq!(committed["runs"]["8"], committed["stored"]);
    assert_eq!(committed["revisions"][id], 4);
    assert_eq!(committed["persisted"][id], 4);
    runtime.invoke("persist-now", vec![]).unwrap();
    assert_eq!(runtime.invoke("snapshot", vec![]).unwrap(), committed);
}

#[test]
fn obsolete_releases_persist_pending_entries_without_overwriting_the_destination() {
    for fault in [
        None,
        Some("entries"),
        Some("validation"),
        Some("missing"),
        Some("retry"),
    ] {
        let (mut runtime, _) = plugin_with_source(
            "agent-service",
            r#"
            (require "./agent-service/state.scm" "./agent-service/messages.scm" "./agent-service/runs.scm")
            (define (setup run)
              (set-store run 10) (as-set! "host" "source-host") (as-set! "session" "source")
              (as-map-set! "runs" 7 (as-call "load-run" "claude:handoff")))
            (define (append-local)
              (define run (append-entry (as-map-get "runs" 7 #f) "result" "shared event" "shared" json-null 20))
              (set! run (append-entry run "result" "pending" #f json-null 20))
              (set! run (hash-insert (hash-insert run "cwd" "/source") "summarized_revision" 3))
              (set! run (hash-insert run "summary" (hash "text" "source" "awaiting" #f "title" "" "hash" 0 "created_at_ms" 20)))
              (as-map-set! "adoptions" "claude:handoff" #t)
              (as-map-set! "runs" 7 run) (as-map-set! "dirty-runs" "claude:handoff" run))
            (define (release) (as-call "finish-run" 7 "Released" 30))
            (define (persist-now) (as-call "persist"))
            (define (snapshot) (hash "stored" stored "rev" store-rev "attempts" attempts
              "dirty" (as-get "dirty-runs") "released" (as-get "released-runs") "adoptions" (as-get "adoptions")
              "bases" (as-get "run-bases") "revisions" (as-get "run-revisions") "persisted" (as-get "persisted")))
            "#,
            json!({}),
            BTreeMap::new(),
            RUN_STORE_PRELUDE,
        );
        let entry = |revision, text: &str, event_id: Option<&str>| json!({"revision":revision, "kind":"result", "text":text, "event_id":event_id, "created_at_ms":20});
        let id = "claude:handoff";
        let original = json!({
            "id":id, "pane_id":7, "kind":"claude", "session_id":"handoff",
            "host_id":"source-host", "yinz_session_name":"source", "started_at_ms":1,
            "ended_at_ms":null, "outcome":"active", "revision":1, "summarized_revision":0,
            "cwd":null, "host":null,
            "summary":null, "entries":[entry(1, "original", None)],
        });
        runtime.invoke("setup", vec![original.clone()]).unwrap();
        runtime.invoke("append-local", vec![]).unwrap();
        runtime.invoke("write-failures", vec![json!(true)]).unwrap();
        runtime.invoke("persist-now", vec![]).unwrap();
        runtime.invoke("release", vec![]).unwrap();
        let pending = runtime.invoke("snapshot", vec![]).unwrap();
        assert_eq!(pending["dirty"][id]["entries"].as_array().unwrap().len(), 3);

        // A different host with the same session name must also suppress the stale close.
        let mut destination = original;
        destination["host_id"] = json!("destination-host");
        destination["pane_id"] = json!(8);
        destination["cwd"] = json!("/destination");
        destination["revision"] = json!(3);
        destination["summarized_revision"] = json!(3);
        destination["summary"] = json!({"text":"destination", "awaiting":false, "title":"", "hash":0, "created_at_ms":20});
        destination["entries"] = json!([
            entry(1, "original", None),
            entry(2, "shared event", Some("shared")),
            entry(3, "destination", None),
        ]);
        runtime
            .invoke("set-store", vec![destination.clone(), json!(11)])
            .unwrap();
        runtime
            .invoke("write-failures", vec![json!(false)])
            .unwrap();
        runtime
            .invoke(
                "faults",
                vec![
                    json!(fault.filter(|value| *value != "retry")),
                    json!(fault == Some("retry")),
                ],
            )
            .unwrap();
        runtime.invoke("persist-now", vec![]).unwrap();
        if let Some(fault) = fault {
            let failed = runtime.invoke("snapshot", vec![]).unwrap();
            assert!(failed["dirty"][id]["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["text"] == "pending"));
            if fault == "retry" {
                assert_eq!(failed["released"], json!({}));
                assert_eq!(failed["adoptions"], json!({}));
                assert_eq!(failed["dirty"][id]["host_id"], "destination-host");
                assert_eq!(failed["revisions"][id], 11);
                // Another handoff before retry must not revive the source's adoption intent.
                let mut moved = failed["stored"].clone();
                moved["host_id"] = json!("third-host");
                moved["yinz_session_name"] = json!("third-session");
                moved["pane_id"] = json!(9);
                runtime.invoke("set-store", vec![moved, json!(13)]).unwrap();
                destination["host_id"] = json!("third-host");
                destination["yinz_session_name"] = json!("third-session");
                destination["pane_id"] = json!(9);
            } else {
                assert_eq!(failed["dirty"], pending["dirty"]);
                assert_eq!(failed["released"], pending["released"]);
                assert_eq!(failed["adoptions"], pending["adoptions"]);
                assert_eq!(failed["bases"], pending["bases"]);
                assert_eq!(failed["revisions"], pending["revisions"]);
                assert_eq!(failed["persisted"], pending["persisted"]);
            }
            runtime
                .invoke("faults", vec![json!(false), json!(false)])
                .unwrap();
            runtime.invoke("persist-now", vec![]).unwrap();
        }
        let committed = runtime.invoke("snapshot", vec![]).unwrap();
        let mut entries = destination["entries"].as_array().unwrap().clone();
        if fault == Some("retry") {
            entries.push(entry(4, "racing source", None));
        }
        entries.push(entry(entries.len() + 1, "pending", None));
        destination["revision"] = json!(entries.len());
        destination["entries"] = json!(entries);
        assert_eq!(committed["stored"], destination, "fault: {fault:?}");
        assert_eq!(committed["dirty"], json!({}));
        assert_eq!(committed["released"], json!({}));
        assert_eq!(committed["adoptions"], json!({}));
        assert_eq!(committed["bases"], json!({}));
        runtime.invoke("persist-now", vec![]).unwrap();
        assert_eq!(runtime.invoke("snapshot", vec![]).unwrap(), committed);
    }
}

#[test]
fn initial_run_hydration_validates_before_caching_or_claiming_entries() {
    for mode in ["load", "restore", "adopt"] {
        for fault in [
            "race",
            "race-moved",
            "race-closed",
            "metadata",
            "entries",
            "validation",
            "missing",
        ] {
            let (mut runtime, _) = plugin_with_source(
                "agent-service",
                r#"
                (require "./agent-service/state.scm" "./agent-service/messages.scm" "./agent-service/runs.scm")
                (define (setup run)
                  (set-store run 10) (as-set! "host" "host") (as-set! "session" "source")
                  (as-set! "panes" (list (hash "pane_id" 7 "agent" (hash "run_id" "claude:handoff" "kind" "claude" "state" "Busy" "source" "Reported")))))
                (define (load-now mode)
                  (cond ((equal? mode "restore") (as-call "restore-runs"))
                    ((equal? mode "adopt") (as-call "adopt-arrived-runs"))
                    (else (define run (as-call "load-run" "claude:handoff"))
                      (when run (as-map-set! "runs" 7 run)))))
                (define (append-local)
                  (define run (append-entry (as-map-get "runs" 7 #f) "result" "local" #f json-null 20))
                  (as-map-set! "runs" 7 run) (as-map-set! "dirty-runs" "claude:handoff" run))
                (define (persist-now) (as-call "persist"))
                (define (snapshot) (hash "stored" stored "rev" store-rev "attempts" attempts
                  "cache" (list (as-get "runs") (as-get "run-bases") (as-get "run-revisions")
                    (as-get "persisted") (as-get "dirty-runs") (as-get "adoptions") (as-get "run-lookups"))))
                "#,
                json!({}),
                BTreeMap::new(),
                RUN_STORE_PRELUDE,
            );
            let entry = |revision, text: &str| json!({"revision":revision, "kind":"result", "text":text, "event_id":null, "created_at_ms":20});
            let id = "claude:handoff";
            // Restoration must remember the stored owner before claiming legacy rows.
            let original = json!({
                "id":id, "pane_id":7, "kind":"claude", "session_id":"handoff",
                "host_id":"", "yinz_session_name":"source", "started_at_ms":1,
                "ended_at_ms":null, "outcome":"active", "revision":1, "summarized_revision":0,
                "cwd":null, "host":null,
                "summary":null, "entries":[entry(1, "original")],
            });
            runtime.invoke("setup", vec![original.clone()]).unwrap();
            let before = runtime.invoke("snapshot", vec![]).unwrap();
            let mut concurrent = original.clone();
            concurrent["revision"] = json!(2);
            concurrent["entries"] = json!([entry(1, "original"), entry(2, "imported")]);
            if fault == "race-moved" {
                concurrent["host_id"] = json!("other-host");
                concurrent["yinz_session_name"] = json!("other-session");
            } else if fault == "race-closed" {
                concurrent["ended_at_ms"] = json!(30);
                concurrent["outcome"] = json!("closed");
            }
            if fault.starts_with("race") {
                runtime
                    .invoke("commit-during-read", vec![concurrent.clone()])
                    .unwrap();
            } else {
                runtime
                    .invoke("faults", vec![json!(fault), json!(false)])
                    .unwrap();
            }
            runtime.invoke("load-now", vec![json!(mode)]).unwrap();
            let rejected = runtime.invoke("snapshot", vec![]).unwrap();
            assert_eq!(
                rejected["cache"], before["cache"],
                "mode: {mode}; fault: {fault}"
            );
            runtime
                .invoke("faults", vec![json!(false), json!(false)])
                .unwrap();
            runtime.invoke("load-now", vec![json!(mode)]).unwrap();
            let mut loaded = runtime.invoke("snapshot", vec![]).unwrap();
            let baseline = if mode == "restore" && matches!(fault, "race-moved" | "race-closed") {
                assert_eq!(
                    loaded["cache"], before["cache"],
                    "ineligible runs must not be restored"
                );
                runtime
                    .invoke("set-store", vec![original.clone(), json!(12)])
                    .unwrap();
                runtime.invoke("load-now", vec![json!(mode)]).unwrap();
                loaded = runtime.invoke("snapshot", vec![]).unwrap();
                original
            } else if fault.starts_with("race") {
                concurrent
            } else {
                original
            };
            assert_eq!(loaded["cache"][1][id], baseline);
            assert_eq!(loaded["cache"][2][id], loaded["rev"]);
            assert_eq!(loaded["cache"][3][id], baseline["revision"]);
            if mode == "restore" {
                assert_eq!(loaded["cache"][0]["7"]["host_id"], "host");
            }
            runtime.invoke("append-local", vec![]).unwrap();
            // Force reconciliation after the successful load; anonymous imported entries stay stored.
            runtime
                .invoke(
                    "set-store",
                    vec![baseline.clone(), json!(loaded["rev"].as_u64().unwrap() + 1)],
                )
                .unwrap();
            runtime.invoke("persist-now", vec![]).unwrap();
            let committed = runtime.invoke("snapshot", vec![]).unwrap();
            let mut entries = baseline["entries"].as_array().unwrap().clone();
            entries.push(entry(entries.len() + 1, "local"));
            assert_eq!(
                committed["stored"]["entries"],
                json!(entries),
                "mode: {mode}; fault: {fault}"
            );
            assert_eq!(committed["cache"][4], json!({}));
            runtime.invoke("persist-now", vec![]).unwrap();
            assert_eq!(runtime.invoke("snapshot", vec![]).unwrap(), committed);
        }
    }
}

#[test]
fn run_conflicts_merge_history_and_retry_without_losing_revision_guards() {
    // The mock store checks the actual emitted guards and commits metadata and entries together.
    // Each case schedules commits/failures explicitly; no threads or sleeps control the race.
    for (retry_conflict, read_fault, read_race, completed, local_summary) in [
        (false, None, false, false, false),
        (true, None, false, false, true),
        (false, Some("entries"), false, true, false),
        (false, Some("entries"), false, false, false),
        (false, Some("metadata"), false, false, false),
        (false, Some("validation"), false, false, false),
        (false, Some("missing"), false, false, false),
        (true, None, true, false, true),
    ] {
        let read_failure = read_fault.is_some();
        let (mut runtime, host) = plugin_with_source(
            "agent-service",
            r#"
            (require "./agent-service/state.scm" "./agent-service/messages.scm" "./agent-service/runs.scm")
            (define (setup run)
              (set-store run 10)
              (as-set! "host" "host") (as-set! "session" "destination")
              (as-set! "panes" (list (hash "pane_id" 8 "agent" (hash "run_id" "claude:handoff" "kind" "claude" "state" "Busy" "source" "Reported"))))
              (as-call "adopt-arrived-runs"))
            (define (append-local text event-id)
              (define run (append-entry (as-map-get "runs" 8 #f) "result" text event-id json-null 20))
              (as-map-set! "runs" 8 run) (as-map-set! "dirty-runs" "claude:handoff" run))
            (define (summarize-local summary)
              (append-local "local summary" #f)
              (define run (as-map-get "runs" 8 #f))
              (set! run (hash-insert (hash-insert run "summary" summary) "summarized_revision" (hash-ref run "revision")))
              (as-map-set! "runs" 8 run) (as-map-set! "dirty-runs" "claude:handoff" run))
            (define (finish) (as-call "finish-run" 8 "Completed" 30))
            (define (persist-now) (as-call "persist"))
            (define (snapshot) (list stored store-rev attempts (as-get "dirty-runs") (as-get "adoptions") (as-get "runs")
                (as-get "run-bases") (as-get "run-revisions") (as-get "persisted") (as-get "summaries")))
            (define (reload)
              (as-reset!) (as-set! "host" "host") (as-set! "session" "destination")
              (as-map-set! "runs" 8 (as-call "load-run" "claude:handoff")))
            (define (late-release original)
              (as-reset!) (as-set! "host" "host") (as-set! "session" "source")
              (as-map-set! "runs" 7 original) (as-map-set! "run-revisions" "claude:handoff" 10)
              (as-map-set! "run-bases" "claude:handoff" original) (as-map-set! "persisted" "claude:handoff" 1)
              (as-call "finish-run" 7 "Released" 50) (as-call "persist"))
            "#,
            json!({}),
            BTreeMap::new(),
            RUN_STORE_PRELUDE,
        );
        let entry = |revision, text: &str, event_id: Option<&str>| json!({"revision":revision, "kind":"result", "text":text, "event_id":event_id, "created_at_ms":20});
        let original = json!({
            "id":"claude:handoff", "pane_id":7, "kind":"claude", "session_id":"handoff",
            "host_id":"host", "yinz_session_name":"source", "started_at_ms":1,
            "ended_at_ms":if read_failure { json!(5) } else { json!(null) },
            "outcome":if read_failure { "completed" } else { "active" },
            "revision":1, "summarized_revision":0, "cwd":null, "host":null,
            "summary":null, "entries":[entry(1, "original", None)],
        });
        runtime.invoke("setup", vec![original.clone()]).unwrap();
        runtime
            .invoke("append-local", vec![json!("shared event"), json!("shared")])
            .unwrap();
        runtime
            .invoke("append-local", vec![json!("destination"), json!(null)])
            .unwrap();
        let summary = |text: &str| json!({"text":text, "awaiting":false, "title":"", "hash":0, "created_at_ms":20});
        if local_summary {
            runtime
                .invoke("summarize-local", vec![summary("local")])
                .unwrap();
        }
        if completed {
            runtime.invoke("finish", vec![]).unwrap();
        }
        let mut released = original.clone();
        released["ended_at_ms"] = json!(10);
        released["outcome"] = json!("closed");
        released["revision"] = json!(3);
        released["summarized_revision"] = json!(3);
        released["summary"] = summary("source");
        released["entries"] = json!([
            entry(1, "original", None),
            entry(2, "source", None),
            entry(3, "shared event", Some("shared"))
        ]);
        runtime
            .invoke("set-store", vec![released.clone(), json!(11)])
            .unwrap();
        let mut concurrent = released;
        if read_race {
            concurrent["revision"] = json!(4);
            concurrent["summarized_revision"] = json!(4);
            concurrent["summary"] = summary("concurrent");
            concurrent["entries"].as_array_mut().unwrap().push(entry(
                4,
                "concurrent summary",
                None,
            ));
            runtime
                .invoke("commit-during-read", vec![concurrent.clone()])
                .unwrap();
        }
        runtime
            .invoke("faults", vec![json!(read_fault), json!(retry_conflict)])
            .unwrap();
        let before = runtime.invoke("snapshot", vec![]).unwrap();
        runtime.invoke("persist-now", vec![]).unwrap();
        if read_race {
            let pending = runtime.invoke("snapshot", vec![]).unwrap();
            assert_eq!(pending[0], concurrent);
            assert_eq!(pending[1], 12);
            assert_eq!(pending[2], json!([10]));
            assert_eq!(
                pending.as_array().unwrap()[3..],
                before.as_array().unwrap()[3..],
                "an inconsistent read must leave every cached value unchanged"
            );
            // A consistent read now merges against rev 12, but a commit after validation
            // must still conflict with that guard and leave the merged edit pending.
            runtime.invoke("persist-now", vec![]).unwrap();
        }
        if retry_conflict || read_failure {
            let pending = runtime.invoke("snapshot", vec![]).unwrap();
            assert_eq!(pending[0]["yinz_session_name"], "source");
            assert!(pending[3].get("claude:handoff").is_some());
            assert!(pending[4].get("claude:handoff").is_some());
            assert_eq!(
                pending[2],
                if read_race {
                    json!([10, 10, 12])
                } else if read_failure {
                    json!([10])
                } else {
                    json!([10, 11])
                }
            );
            if read_failure {
                assert_eq!(
                    pending.as_array().unwrap()[3..],
                    before.as_array().unwrap()[3..]
                );
            }
            if read_race {
                assert_eq!(pending[4]["claude:handoff"], true);
                assert_eq!(pending[6]["claude:handoff"], concurrent);
                assert_eq!(pending[7]["claude:handoff"], 12);
                assert_eq!(pending[8]["claude:handoff"], 4);
            }
            runtime
                .invoke("faults", vec![json!(false), json!(false)])
                .unwrap();
            runtime.invoke("persist-now", vec![]).unwrap();
        }
        let state = runtime.invoke("snapshot", vec![]).unwrap();
        let stored = &state[0];
        assert_eq!(
            stored["yinz_session_name"],
            "destination",
            "state: {state}; commands: {:?}",
            host.lock().unwrap().commands
        );
        assert_eq!(stored["pane_id"], 8);
        assert_eq!(
            stored["outcome"],
            if completed { "completed" } else { "active" }
        );
        assert_eq!(
            stored["ended_at_ms"],
            if completed { json!(30) } else { json!(null) }
        );
        assert_eq!(
            stored["summary"],
            summary(if local_summary { "local" } else { "source" })
        );
        assert_eq!(
            stored["summarized_revision"],
            if local_summary {
                6 + usize::from(read_race)
            } else {
                3
            }
        );
        let entries = stored["entries"].as_array().unwrap();
        let mut texts = vec!["original", "source", "shared event"];
        if read_race {
            texts.push("concurrent summary");
        }
        if retry_conflict {
            texts.push("racing source");
        }
        texts.push("destination");
        if local_summary {
            texts.push("local summary");
        }
        assert_eq!(entries.len(), texts.len());
        for (index, text) in texts.iter().enumerate() {
            assert_eq!(entries[index]["text"], *text);
            assert_eq!(entries[index]["revision"], index + 1);
        }
        assert_eq!(state[3], json!({}));
        assert_eq!(state[4], json!({}));
        assert_eq!(
            state[2],
            if read_race {
                json!([10, 10, 12, 12, 13])
            } else if retry_conflict {
                json!([10, 11, 11, 12])
            } else if read_failure {
                json!([10, 10, 11])
            } else {
                json!([10, 11])
            }
        );
        if !completed {
            assert_eq!(state[5]["8"]["entries"], stored["entries"]);
            runtime
                .invoke("append-local", vec![json!("after adoption"), json!(null)])
                .unwrap();
            // A pending archive summary in the source session commits after adoption succeeded.
            let mut archived = stored.clone();
            let revision = archived["revision"].as_u64().unwrap() + 1;
            archived["revision"] = json!(revision);
            archived["summary"] = summary("late archive");
            archived["summarized_revision"] = json!(revision);
            archived["entries"].as_array_mut().unwrap().push(entry(
                revision,
                "late archive",
                Some("archive"),
            ));
            let expected = state[1].as_u64().unwrap();
            runtime
                .invoke("set-store", vec![archived.clone(), json!(expected + 1)])
                .unwrap();
            runtime
                .invoke("faults", vec![json!(read_fault), json!(retry_conflict)])
                .unwrap();
            runtime.invoke("persist-now", vec![]).unwrap();
            let mut attempts = state[2].as_array().unwrap().clone();
            attempts.push(json!(expected));
            if !read_failure {
                attempts.push(json!(expected + 1));
            }
            if retry_conflict || read_failure {
                let pending = runtime.invoke("snapshot", vec![]).unwrap();
                assert!(pending[3].get("claude:handoff").is_some());
                assert_eq!(pending[4], json!({}));
                assert_eq!(pending[2], json!(attempts));
                if read_failure {
                    assert_eq!(pending[6], state[6]);
                    assert_eq!(pending[7], state[7]);
                    assert_eq!(pending[8], state[8]);
                }
                runtime
                    .invoke("faults", vec![json!(false), json!(false)])
                    .unwrap();
                runtime.invoke("persist-now", vec![]).unwrap();
                attempts.extend(if read_failure {
                    [json!(expected), json!(expected + 1)]
                } else {
                    [json!(expected + 1), json!(expected + 2)]
                });
            }
            let after = runtime.invoke("snapshot", vec![]).unwrap();
            assert_eq!(after[2], json!(attempts));
            assert_eq!(after[0]["summary"], archived["summary"]);
            assert_eq!(after[0]["summarized_revision"], revision);
            let mut expected_entries = archived["entries"].as_array().unwrap().clone();
            if retry_conflict {
                expected_entries.push(entry(revision + 1, "racing source", None));
            }
            expected_entries.push(entry(
                expected_entries.len() as u64 + 1,
                "after adoption",
                None,
            ));
            assert_eq!(after[0]["entries"], json!(expected_entries));
            assert_eq!(after[5]["8"], after[0]);
            assert_eq!(after[3], json!({}));
            assert_eq!(after[4], json!({}));
            assert_eq!(after[6]["claude:handoff"], after[0]);

            // Reloading also supplies a baseline; an ordinary edit must preserve remote ownership.
            runtime.invoke("reload", vec![]).unwrap();
            let mut moved = after[0].clone();
            moved["host_id"] = json!("other-host");
            moved["yinz_session_name"] = json!("other-session");
            moved["pane_id"] = json!(9);
            runtime
                .invoke(
                    "set-store",
                    vec![moved, json!(after[1].as_u64().unwrap() + 1)],
                )
                .unwrap();
            runtime
                .invoke("append-local", vec![json!("after move"), json!(null)])
                .unwrap();
            runtime.invoke("persist-now", vec![]).unwrap();
            let moved = runtime.invoke("snapshot", vec![]).unwrap();
            assert_eq!(moved[5]["8"], moved[0]);
            assert_eq!(
                moved[0]["entries"].as_array().unwrap().len(),
                expected_entries.len() + 1
            );
            runtime
                .invoke(
                    "set-store",
                    vec![moved[0].clone(), json!(moved[1].as_u64().unwrap() + 1)],
                )
                .unwrap();
            runtime.invoke("finish", vec![]).unwrap();
            runtime.invoke("persist-now", vec![]).unwrap();
            let finished = runtime.invoke("snapshot", vec![]).unwrap();
            assert_eq!(finished[0]["host_id"], "other-host");
            assert_eq!(finished[0]["yinz_session_name"], "other-session");
            assert_eq!(finished[0]["pane_id"], 9);
            assert_eq!(finished[0]["outcome"], "completed");
            assert_eq!(finished[0]["ended_at_ms"], 30);
            assert_eq!(finished[0]["entries"], moved[0]["entries"]);
            assert_eq!(finished[3], json!({}));
            assert_eq!(finished[6], json!({}));
            assert_eq!(finished[7], json!({}));
            assert_eq!(finished[8], json!({}));
        }
        let before = runtime.invoke("snapshot", vec![]).unwrap();
        runtime.invoke("late-release", vec![original]).unwrap();
        let after = runtime.invoke("snapshot", vec![]).unwrap();
        assert_eq!(after[0], before[0]);
        let mut attempts = before[2].as_array().unwrap().clone();
        attempts.push(json!(10));
        assert_eq!(after[2], json!(attempts));
        assert_eq!(after[3], json!({}));
        assert_eq!(after[4], json!({}));
    }
}

#[test]
fn project_provider_chain_precedence_fallback_deadline_and_stale_replies() {
    let source = r#"
        (require "./agent-service/state.scm")
        (require "./agent-service/probes.scm")
        (define (seed providers)
          (as-set! "project-providers" providers)
          (as-record-cwd! 7 "/work")
          (as-call "resolve-repositories"))
        (define (answer result) (yinz-resolve! test-pending result) #f)
        (define (reject) (yinz-reject! test-pending "provider failed") #f)
        (define (expire) (set! test-now 6001) (reject))
        (define (move)
          (as-record-cwd! 7 "/other") (as-call "resolve-repositories"))
        (define (reload providers) (as-call "project-providers-changed" providers))
        (define (repositories) (as-get "repositories"))
        (define (calls) test-calls)
    "#;
    let prelude = r#"
        (define test-pending #f)
        (define test-calls '())
        (define test-now 1000)
        (define (yinz-wall-clock-ms) test-now)
        (define (yinz-rpc-call options)
          (set! test-calls (append test-calls (list options)))
          (set! test-pending (yinz-deferred)) test-pending)
    "#;
    let providers = json!([
        {"id":"first","priority":2,"plugin_id":20,"generation":1},
        {"id":"second","priority":1,"plugin_id":20,"generation":1}
    ]);
    let repository =
        json!({"root":"/work","provider_identity":"[\"first\",\"work\"]","display_name":"Work"});
    for scenario in [
        "match", "decline", "error", "invalid", "timeout", "move", "reload",
    ] {
        let (mut runtime, host) = plugin_with_source(
            "project-provider-test",
            source,
            json!({}),
            BTreeMap::from([("ValidateProjectMatch".into(), repository.clone())]),
            prelude,
        );
        runtime.invoke("seed", vec![providers.clone()]).unwrap();
        assert_eq!(
            runtime.invoke("calls", vec![]).unwrap()[0]["name"],
            "project-identify:first"
        );
        assert_eq!(
            runtime
                .invoke("calls", vec![])
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        match scenario {
            "match" => {
                runtime
                    .invoke(
                        "answer",
                        vec![json!({"identity":"work","root":"/work","name":"Work"})],
                    )
                    .unwrap();
            },
            "decline" => {
                runtime.invoke("answer", vec![json!(null)]).unwrap();
            },
            "error" => {
                runtime.invoke("reject", vec![]).unwrap();
            },
            "invalid" => {
                host.lock()
                    .unwrap()
                    .failures
                    .insert("ValidateProjectMatch".into(), "invalid root".into());
                runtime
                    .invoke("answer", vec![json!({"root":"relative"})])
                    .unwrap();
            },
            "timeout" => {
                runtime.invoke("expire", vec![]).unwrap();
            },
            "move" => {
                runtime.eval("(define old-pending test-pending)").unwrap();
                runtime.invoke("move", vec![]).unwrap();
                runtime
                    .eval("(yinz-resolve! old-pending (hash \"identity\" \"old\")) #f")
                    .unwrap();
            },
            "reload" => {
                runtime.eval("(define old-pending test-pending)").unwrap();
                runtime.invoke("reload", vec![json!([])]).unwrap();
                runtime
                    .eval("(yinz-resolve! old-pending (hash \"identity\" \"old\")) #f")
                    .unwrap();
            },
            _ => unreachable!(),
        }
        runtime.invoke("yinz-async-poll", vec![]).unwrap();
        let repositories = runtime.invoke("repositories", vec![]).unwrap();
        if scenario == "match" {
            assert_eq!(repositories["/work"], repository);
        } else {
            assert!(
                repositories.as_object().unwrap().is_empty(),
                "{scenario}: {repositories}"
            );
        }
        if matches!(scenario, "decline" | "error" | "invalid") {
            let calls = runtime.invoke("calls", vec![]).unwrap();
            assert_eq!(calls[1]["pluginId"], 20);
            assert_eq!(calls[1]["name"], "project-identify:second");
            runtime.invoke("answer", vec![json!(null)]).unwrap();
        }
        runtime.invoke("yinz-async-poll", vec![]).unwrap();
        let host = host.lock().unwrap();
        let git = host
            .commands
            .iter()
            .filter(|command| command.get("RunCommand").is_some())
            .count();
        assert_eq!(
            git,
            if matches!(
                scenario,
                "decline" | "error" | "invalid" | "timeout" | "reload"
            ) {
                2
            } else {
                0
            },
            "{scenario}"
        );
    }
}

#[test]
fn marker_project_example_runs_its_real_command() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join(".yinz-project");
    std::fs::create_dir(&marker).unwrap();
    std::fs::write(marker.join("identity"), "work-id").unwrap();
    std::fs::write(marker.join("name"), "Work").unwrap();
    let (mut runtime, host) = plugin_with_source(
        "marker-example",
        include_str!("../../../docs/examples/marker-project.scm"),
        json!({}),
        BTreeMap::new(),
        "",
    );
    runtime
        .eval(&format!(
            "(define result (identify {}))",
            json!(directory.path())
        ))
        .unwrap();
    let host = host.lock().unwrap();
    let command = host
        .commands
        .iter()
        .find_map(|command| command.get("RunCommand"))
        .unwrap()
        .clone();
    drop(host);
    let argv = command[0].as_array().unwrap();
    let output = std::process::Command::new(argv[0].as_str().unwrap())
        .args(argv[1..].iter().map(|arg| arg.as_str().unwrap()))
        .current_dir(command[2].as_str().unwrap())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["identity"], "work-id");
    assert_eq!(result["name"], "Work");
    runtime.eval("(define observed #f) (define (observed-result) observed) (yinz-catch (yinz-then result (lambda (value) (set! observed value))) (lambda (failure) (set! observed failure)))").unwrap();
    runtime
        .update(json!({"RunCommandResult":[0.0, output.stdout, output.stderr, command[3]]}))
        .unwrap();
    assert_eq!(runtime.invoke("observed-result", vec![]).unwrap(), result);
}

#[test]
fn counts_use_panel_classification_and_publish_only_changes() {
    let source = format!(
        "{}\n(define (load _) (as-reset!))\n(require \"./shared/agent-status.scm\")",
        include_str!("../../assets/plugins/agent-service.scm").replace(
            "(define (load configuration)",
            "(define (service-load configuration)"
        )
    );
    let (mut runtime, host) =
        plugin_with_source("agent-service", &source, json!({}), BTreeMap::new(), "");
    let panes: Vec<_> = (1..=11)
        .map(|id| {
            json!({
                "pane_id":id, "title":"task", "command":if id >= 10 {"shell"} else {"claude"},
                "status":match id {7 => "Held", 8 => "Exited", _ => "Running"},
                "cwd":null, "tab":[1,"agents"], "agent":null
            })
        })
        .collect();
    runtime
        .invoke("as-set!", vec![json!("panes"), json!(panes)])
        .unwrap();
    runtime.eval(r#"
        (as-set! "manual" (hash "10" "codex"))
        (as-set! "hooks" (hash "1" (hash "phase" "Busy") "2" (hash "phase" "Waiting") "3" (hash "phase" "Idle")))
        (as-set! "activity" (hash "4" '(12 0) "5" '(12 2) "6" '(13 2)))
        (as-set! "summaries" (hash "5" (hash "text" "Fresh?" "awaiting" #t "hash" 12)
            "6" (hash "text" "Stale?" "awaiting" #t "hash" 12)))
        (as-set! "suppressed" (as-get "panes"))
    "#).unwrap();
    let subscribe = |runtime: &mut yinz_steel::Runtime, id| {
        runtime.pipe(json!({"source":{"Plugin":id},"name":"agent_counts_subscribe","payload":null,"args":{},"is_private":true})).unwrap();
    };
    subscribe(&mut runtime, 42);
    let counts = runtime.invoke("as-get", vec![json!("counts")]).unwrap();
    assert_eq!(
        counts,
        json!({"total":10,"busy":2,"waiting":2,"idle":2,"held":1,"exited":1,"unknown":2})
    );
    // Panel details and the service aggregate use the same pure classification, without UI state.
    let rows = runtime.eval(r#"(define count-test-rows (as-model "StripRows" (as-panes) (as-get "manual") (as-get "activity") (hash) (hash) (as-get "summaries") (as-get "hooks")))"#);
    rows.unwrap();
    let count_rows = runtime.value("count-test-rows").unwrap();
    assert_eq!(
        runtime
            .invoke("agent-row-counts", vec![count_rows])
            .unwrap(),
        counts
    );
    let messages = || {
        host.lock()
            .unwrap()
            .commands
            .iter()
            .filter_map(|command| command.get("MessageToPlugin").cloned())
            .collect::<Vec<_>>()
    };
    assert_eq!(messages().len(), 1);
    runtime
        .invoke("as-publish-counts!", vec![json!(false)])
        .unwrap();
    assert_eq!(messages().len(), 1);
    subscribe(&mut runtime, 42);
    subscribe(&mut runtime, 43);
    assert_eq!(messages().len(), 3);
    assert_eq!(
        runtime
            .invoke("as-get", vec![json!("count-subscribers")])
            .unwrap(),
        json!([43, 42])
    );
    // Non-classification changes do not send counts; a fresh summary changing to stale does.
    runtime.eval(r#"(as-set! "focused" 5) (as-map-set! "summaries" 5 (hash "text" "Reworded?" "awaiting" #t "hash" 12))"#).unwrap();
    runtime
        .invoke("as-publish-counts!", vec![json!(false)])
        .unwrap();
    assert_eq!(messages().len(), 3);
    runtime
        .eval(r#"(as-map-set! "activity" 5 '(14 2))"#)
        .unwrap();
    runtime
        .invoke("as-publish-counts!", vec![json!(false)])
        .unwrap();
    let sent = messages();
    assert_eq!(sent.len(), 5);
    let updated: serde_json::Value =
        serde_json::from_str(sent[3]["message_payload"].as_str().unwrap()).unwrap();
    assert_eq!(updated["waiting"], 1);
    assert_eq!(updated["idle"], 3);
    assert_eq!(
        sent[3]["message_args"]["seq"],
        sent[4]["message_args"]["seq"]
    );
    assert!(
        sent[3]["message_args"]["seq"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > sent[2]["message_args"]["seq"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
    );
    runtime
        .invoke("as-publish-counts!", vec![json!(false)])
        .unwrap();
    assert_eq!(messages().len(), 5);
    // Provider snapshots classify custom commands without depending on the panel's provider load.
    let provider = yinz_types::agent::provider::AgentProvider {
        kind: "custom".into(),
        executable: "shell".into(),
        ..Default::default()
    };
    runtime
        .invoke("as-set!", vec![json!("providers"), json!([provider])])
        .unwrap();
    runtime
        .invoke("as-publish-counts!", vec![json!(false)])
        .unwrap();
    assert_eq!(
        runtime.invoke("as-get", vec![json!("counts")]).unwrap()["total"],
        11
    );
}
