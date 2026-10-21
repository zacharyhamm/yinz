use crate::tests::plugin_with_source;
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn service_modules_compile_in_the_confined_graph() {
    let (_, _) = plugin_with_source("agent-service", "(require \"./agent-service/state.scm\") (require \"./agent-service/probes.scm\") (require \"./agent-service/messages.scm\") (require \"./agent-service/runs.scm\") (require \"./agent-service/observe.scm\") (require \"./agent-service/summaries.scm\") (require \"./agent-service/completions.scm\") (require \"./agent-service/operations.scm\")", json!({}), BTreeMap::new(), "");
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
        include_str!("../../assets/plugins/agent-service/summaries.scm"),
        r#"
        (require "./probes.scm")
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
    let (mut runtime, host) = plugin_with_source(
        "agent-service/summaries",
        &source,
        json!({}),
        BTreeMap::new(),
        "",
    );
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
