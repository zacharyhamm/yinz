use crate::tests::plugin_with_source;
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn service_modules_compile_in_the_confined_graph() {
    let (_, _) = plugin_with_source("agent-service", "(require \"./agent-service/state.scm\") (require \"./agent-service/probes.scm\") (require \"./agent-service/messages.scm\") (require \"./agent-service/runs.scm\") (require \"./agent-service/observe.scm\") (require \"./agent-service/summaries.scm\") (require \"./agent-service/completions.scm\") (require \"./agent-service/operations.scm\")", json!({}), BTreeMap::new(), "");
}

#[test]
fn live_hook_receipt_times_are_published_and_survive_pane_updates() {
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
    let manifest = json!({"panes":{"0":[pane(7), pane(8)]}});
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
        runtime
            .invoke(
                "hook",
                vec![json!(id), json!(name), json!(now), json!(false)],
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
    runtime.invoke("rebuild", vec![manifest.clone()]).unwrap();
    assert_eq!(
        runtime.invoke("stamp", vec![json!(7)]).unwrap(),
        json!(null)
    );
    let mut held = manifest.clone();
    held["panes"]["0"][1]["is_held"] = json!(true);
    runtime.invoke("rebuild", vec![held]).unwrap();
    assert_eq!(runtime.invoke("stamp", vec![json!(8)]).unwrap(), 400);
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
