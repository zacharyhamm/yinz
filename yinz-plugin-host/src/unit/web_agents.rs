use super::*;
use std::thread;
use yinz_runtime::channels::{self, SenderWithContext};
use yinz_types::{
    agent::{AgentInfo, AgentSource, AgentState},
    data::PaneInfo,
    internal_ids::TerminalId,
    pane_id::PaneId,
};

fn data() -> (tempfile::TempDir, DataAccess) {
    let directory = tempfile::tempdir().unwrap();
    let data = DataAccess::sqlite(directory.path());
    for def in project_data::store::shared_defs() {
        data.plugin_data()
            .define_entity(project_data::store::SHARED, "test", &def, false)
            .unwrap();
    }
    (directory, data)
}
fn live(session: &str, state: AgentState, current_session: bool) -> WebLiveAgent {
    let id = format!("live:{session}:7");
    let pane = PaneInfo {
        id: PaneId::Terminal(TerminalId::new(7)),
        title: "task".into(),
        agent: Some(AgentInfo {
            kind: "codex".into(),
            state,
            source: AgentSource::Reported,
            detail: None,
            waiting_on: Some("approve".into()),
            wait_needs_a_person: true,
            cwd: None,
            session_id: Some("conversation".into()),
            run_id: Some("run".into()),
        }),
        ..Default::default()
    };
    WebLiveAgent {
        local_tab: None,
        tab_group_session: session.into(),
        tab_group: Some(yinz_types::tab_bar::TabGroupInfo {
            id: 0,
            name: format!("Group for {session}"),
        }),
        id: id.clone(),
        session: session.into(),
        current_session,
        peer_instance: (!current_session).then(|| uuid::Uuid::new_v4().to_string()),
        pane,
        metadata: BTreeMap::from([("state".into(), state.to_string().to_lowercase())]),
        run: Some("run".into()),
        label_source: palette::AgentLabelSource {
            id,
            host: None,
            cwd: None,
            run: None,
        },
        provider: "codex".into(),
        activity: "task".into(),
        host: None,
        pane_label: "7".into(),
        glyph: "!",
        status: "needs input",
        focused: false,
        selectable: true,
        suppressed: false,
        usage_unavailable: (!current_session)
            .then_some("Quota unavailable for peer-session agents"),
        details: "live details".into(),
        header: yinz_bus::web::WebLiveHeader {
            name: "live task".into(),
            ..Default::default()
        },
    }
}
fn with_snapshot(input: Value, agents: Vec<WebLiveAgent>, data: &DataAccess) -> Value {
    let (tx, rx) = channels::unbounded();
    let senders = ThreadSenders {
        to_screen: Some(SenderWithContext::new(tx)),
        ..Default::default()
    };
    thread::scope(|scope| {
        scope.spawn(move || {
            let (ScreenInstruction::WebLiveAgents { reply }, _) =
                rx.recv_timeout(Duration::from_secs(5)).unwrap()
            else {
                panic!("expected snapshot")
            };
            reply.send(agents).unwrap();
        });
        handle(&input.to_string(), ClientId::new(1), &senders, data)
    })
}
#[test]
fn web_agents_validation_rejects_bad_operations_and_bounds_without_dispatch() {
    let (_dir, data) = data();
    let identity = json!({"id":"live","kind":"codex","session_id":"conversation"});
    let mut bad = vec![
        json!(null),
        json!([]),
        json!({}),
        json!({"op":"restart"}),
        json!({"op":"list","session":"arbitrary"}),
        json!({"op":"launch","submission_id":format!("{}.unknown",yinz_bus::agent_launch::server_instance()),"project":"project","kind":"unknown","checkout":{"type":"new","branch":"main"}}),
        json!({"op":"launch","submission_id":"test","project":"project","kind":"codex","checkout":{"type":"existing","path":"relative"}}),
        json!({"op":"launch","submission_id":"test","project":"project","kind":"codex","checkout":{"type":"new","branch":"topic","path":"/other"}}),
        json!({"op":"move","agent":identity,"destination":{"type":"tab","group":0,"tab":1}}),
        json!({"op":"move","agent":identity,"destination":{"type":"new_group","name":"bad\nname"}}),
        json!({"op":"close","agent":identity,"session":"peer"}),
        json!({"op":"event","run":"run","revision":0}),
        json!({"op":"event","run":"run","revision":i64::MAX as u64}),
        json!({"op":"event","run":"run","revision":u64::MAX}),
        json!({"op":"history","run":" ","from":0}),
        json!({"op":"history","run":"r","from":-1}),
        json!({"op":"history","run":"r","before":{"revision":1,"created_at_ms":i64::MAX}}),
        json!({"op":"history","run":"r","query":"x","before":{"revision":1,"created_at_ms":null}}),
        json!({"op":"history","run":"r","before":{"revision":1},"after_revision":2}),
        json!({"op":"tools","run":"r","revisions":[]}),
        json!({"op":"tools","run":"r","revisions":vec![1;31]}),
        json!({"op":"tools","run":"r","revisions":[1],"range":{}}),
        json!({"op":"tools","run":"r","range":{"start":{"revision":3},"end":{"revision":2}}}),
        json!({"op":"prompt","agent":identity,"text":"ok","submission_id":"x".repeat(65)}),
        json!({"op":"prompt","agent":identity,"text":"雪".repeat(11000),"submission_id":"retry"}),
        json!({"op":"prompt","agent":identity,"text":"bad\u{1b}text","submission_id":"retry"}),
        json!({"op":"prompt","agent":identity,"text":"  ","submission_id":"retry"}),
        json!({"op":"prompt","agent":identity,"text":"ok","submission_id":"has spaces"}),
        json!({"op":"keys","agent":identity,"keys":["Ctrl c"]}),
        json!({"op":"keys","agent":identity,"keys":[]}),
        json!({"op":"keys","agent":identity,"keys":vec!["enter";9]}),
        json!({"op":"terminal_input","agent":identity,"data":{"mouse":""}}),
        json!({"op":"terminal_input","agent":identity,"data":{"mouse":"x".repeat(32769)}}),
        json!({"op":"terminal_input","agent":identity,"data":{"mouse":123}}),
        json!({"op":"terminal_input","agent":identity,"data":{"mouse":"G1tNICEh","extra":true}}),
        json!({"op":"screen","agent":identity,"lines":0}),
        json!({"op":"screen","agent":identity,"lines":201}),
    ];
    for field in ["id", "kind", "session_id"] {
        let mut empty = identity.clone();
        empty[field] = json!("");
        bad.push(json!({"op":"focus","agent":empty}));
    }
    for request in bad {
        assert_eq!(
            handle(
                &request.to_string(),
                ClientId::new(1),
                &ThreadSenders::default(),
                &data
            )["error"]["code"],
            "invalid",
            "{request}"
        );
    }
    assert_eq!(
        Operation::parse(&" ".repeat(65537)).unwrap_err().code,
        "invalid"
    );
    for key in yinz_types::agent::control::WEB_KEYS {
        let op = Operation::parse(&json!({"op":"keys","agent":identity,"keys":[key]}).to_string())
            .unwrap();
        assert!(matches!(op, Operation::Keys { .. }));
        assert!(
            key.parse::<yinz_types::keys::KeyWithModifier>().is_ok(),
            "{key}"
        );
    }
}
#[test]
fn web_agents_list_uses_shared_order_and_peer_usage_without_local_collection() {
    let (_dir, data) = data();
    let mut peer = live("peer", AgentState::Waiting, false);
    peer.tab_group_session = "renamed-peer".into();
    let peer_id = peer.peer_detail_id().unwrap();
    let mut ungrouped = live("busy", AgentState::Busy, true);
    ungrouped.tab_group = None;
    let agents = vec![live("local", AgentState::Idle, true), peer, ungrouped];
    let response = with_snapshot(json!({"op":"list"}), agents.clone(), &data);
    assert_eq!(
        response["ok"]["counts"],
        json!({"waiting":1,"busy":1,"idle":1})
    );
    for (index, session, owner, group) in [
        (
            0,
            "peer",
            "renamed-peer",
            json!({"id":0,"name":"Group for peer"}),
        ),
        (1, "busy", "busy", Value::Null),
        (
            2,
            "local",
            "local",
            json!({"id":0,"name":"Group for local"}),
        ),
    ] {
        let row = &response["ok"]["agents"][index];
        assert_eq!(row["session"], session);
        assert_eq!(row["tab_group_session"], owner);
        assert_eq!(row.get("tab_group"), Some(&group));
        let id = if session == "peer" {
            peer_id.clone()
        } else {
            format!("live:{session}:7")
        };
        assert_eq!(
            row["ref"],
            json!({"id":id,"kind":"codex","session_id":"conversation"})
        );
    }
    assert_eq!(response["ok"]["agents"][0]["can_prompt"], true);
    let response = with_snapshot(json!({"op":"usage","agent":peer_id}), agents, &data);
    assert_eq!(response["ok"]["usage"]["status"], "unavailable");
    assert_eq!(
        response["ok"]["usage"]["message"],
        "Quota unavailable for peer-session agents"
    );
}
#[test]
fn web_agents_list_isolates_peer_labels_for_aliased_rows() {
    use project_data::{
        location::Location,
        projects::{self, Repository},
    };
    let (directory, data) = history_navigation_data();
    let mut original = live("stored", AgentState::Waiting, false);
    let mut replacement = live("stored", AgentState::Waiting, false);
    original.tab_group_session = "renamed".into();
    replacement.tab_group_session = "stored".into();
    assert_eq!(original.label_source.id, replacement.label_source.id);
    let mut locations = serde_json::Map::new();
    for (agent, project, branch, run, owner) in [
        (&mut original, "alpha", "release", "run", "stored"),
        (&mut replacement, "beta", "develop", "replacement", "stored"),
    ] {
        let path = directory.path().join(project);
        let path = path.to_str().unwrap();
        let location = Location::local(path);
        locations.insert(
            location.key(),
            json!(Repository {
                root: path.into(),
                branch: Some(branch.into()),
                worktree_status: Some(projects::WorktreeStatus::Primary),
                ..Default::default()
            }),
        );
        data.plugin_data()
            .apply(
                project_data::store::SHARED,
                &[projects::to_mutation(
                    &projects::validate(project, path).unwrap(),
                )],
            )
            .unwrap();
        agent.label_source.cwd = Some(path.into());
        agent.label_source.run = Some(palette::AgentRunSource {
            id: run.into(),
            host_id: yinz_runtime::host_identity::host_id(),
            session: owner.into(),
        });
        agent.run = Some(run.into());
    }
    crate::project_provider::publish(
        1,
        json!({
            "providers": yinz_types::project_provider::providers(),
            "locations": locations,
        }),
    )
    .unwrap();
    for has_reply in [true, false] {
        if !has_reply {
            replacement.label_source.run.as_mut().unwrap().session = "different-owner".into();
        }
        for agents in [
            vec![original.clone(), replacement.clone()],
            vec![replacement.clone(), original.clone()],
        ] {
            let result = with_snapshot(json!({"op":"list"}), agents, &data);
            let rows = result["ok"]["agents"].as_array().unwrap();
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0]["tab_group_session"], "renamed");
            assert_eq!(rows[0]["project"], "alpha");
            assert_eq!(rows[0]["branch"], "release");
            assert_eq!(rows[0]["last_reply"]["text"], "needle run 220");
            assert_eq!(rows[1]["project"], "beta");
            assert_eq!(rows[1]["branch"], "develop");
            if has_reply {
                assert_eq!(
                    rows[1]["last_reply"]["text"],
                    "unrelated replacement message"
                );
            } else {
                assert!(rows[1]["last_reply"].is_null());
            }
        }
    }
    assert!(crate::project_provider::clear(1));
}

#[test]
fn web_agents_actions_reject_missing_and_stale_refs_without_routing() {
    let (_dir, data) = data();
    for (id, kind, conversation) in [
        ("unknown:peer:7", "codex", "conversation"),
        ("live:local:7", "claude", "conversation"),
        ("live:local:7", "codex", "old"),
    ] {
        for op in ["focus", "screen", "keys", "prompt"] {
            let mut request =
                json!({"op":op,"agent":{"id":id,"kind":kind,"session_id":conversation}});
            if op == "keys" {
                request["keys"] = json!(["enter"]);
            }
            if op == "prompt" {
                request["text"] = json!("continue");
                request["submission_id"] = json!("retry");
            }
            let response =
                with_snapshot(request, vec![live("local", AgentState::Idle, true)], &data);
            assert_eq!(response["error"]["code"], "target_changed");
        }
    }
}
#[test]
fn web_agents_controls_keep_complete_guard_and_uncertain_submission_and_sanitize_reads() {
    let (_dir, data) = data();
    for op in ["prompt", "screen", "keys", "focus"] {
        let agent = live("local", AgentState::Idle, true);
        let mut request =
            json!({"op":op,"agent":{"id":agent.id,"kind":"codex","session_id":"conversation"}});
        if op == "prompt" {
            request["text"] = json!("continue");
            request["submission_id"] = json!("retry");
        }
        if op == "keys" {
            request["keys"] = json!(["enter"]);
        }
        let (tx, rx) = channels::unbounded();
        let senders = ThreadSenders {
            to_screen: Some(SenderWithContext::new(tx)),
            ..Default::default()
        };
        thread::scope(|scope| {
            scope.spawn(move || {
                let (ScreenInstruction::WebLiveAgents { reply }, _) = rx.recv().unwrap() else {
                    panic!("snapshot")
                };
                reply.send(vec![agent]).unwrap();
                let (instruction, _) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
                if let ScreenInstruction::WebFocusAgent { reply, .. } = instruction {
                    reply.send(Err(WebFocusError::NoTerminal)).unwrap();
                    return;
                }
                let ScreenInstruction::AgentControlReply(
                    AgentRequest::WebGuarded {
                        expected_kind,
                        expected_session_id,
                        action,
                        ..
                    },
                    reply,
                ) = instruction
                else {
                    panic!("full guard")
                };
                assert_eq!(expected_kind, "codex");
                assert_eq!(expected_session_id, "conversation");
                if action.submission_id().is_some() {
                    drop(reply);
                } else {
                    let mut response = AgentResponse::new(AgentOutcome::Ok);
                    response.output = Some("hello\u{1b}\u{7f}\u{80}\n\tworld".into());
                    reply.send(response).unwrap();
                }
            });
            let response = handle(&request.to_string(), ClientId::new(1), &senders, &data);
            match op {
                "prompt" => {
                    assert_eq!(response["ok"]["response"]["outcome"], "timed_out");
                    assert_eq!(response["ok"]["response"]["submission_id"], "retry");
                },
                "focus" => {
                    assert_eq!(response["error"]["code"], "unavailable");
                    assert_eq!(response["error"]["message"], "No terminal attached");
                },
                _ => assert_eq!(response["ok"]["response"]["output"], "hello\n\tworld"),
            }
        });
    }
}
#[test]
fn web_agents_history_preserves_navigation_and_empty_activity_replacements() {
    let c = |revision| Cursor {
        created_at_ms: None,
        revision,
    };
    let scope = Range {
        start: Some(c(1)),
        end: Some(c(9)),
        worker: Some("worker".into()),
    };
    let reply = HistoryReply::Run {
        header: Default::default(),
        events: vec![palette::HistoryEvent {
            range: Some(scope.clone()),
            tool_calls: 2,
            revision: 3,
            kind: "tools".into(),
            time: "time".into(),
            created_at_ms: None,
            markdown: "".into(),
            truncated: false,
        }],
        suppressed_result_revisions: vec![],
        activity_ranges: vec![
            scope.clone(),
            Range {
                start: Some(c(9)),
                end: None,
                worker: Some("worker".into()),
            },
        ],
        rows: vec![
            palette::HistoryRow {
                id: "earlier".into(),
                label: "".into(),
                description: "".into(),
                action: HistoryAction::Conversation {
                    id: "run".into(),
                    worker: Some("worker".into()),
                    before: Some(c(2)),
                    after_revision: None,
                },
            },
            palette::HistoryRow {
                id: "received".into(),
                label: "".into(),
                description: "".into(),
                action: HistoryAction::Conversation {
                    id: "run".into(),
                    worker: Some("worker".into()),
                    before: None,
                    after_revision: Some(10),
                },
            },
            palette::HistoryRow {
                id: "resume".into(),
                label: "".into(),
                description: "".into(),
                action: HistoryAction::Resume { id: "run".into() },
            },
        ],
        status: "partial".into(),
    };
    let page = adapt(reply, "run", None, None, false, Some("missing"), false).unwrap();
    assert_eq!(page["activity_ranges"].as_array().unwrap().len(), 2);
    assert_eq!(
        page["events"][0]["range"],
        serde_json::to_value(scope).unwrap()
    );
    assert_eq!(page["events"][0]["revisions"], json!([]));
    assert_eq!(page["navigation"]["received"]["after_revision"], 10);
    assert_eq!(
        page["navigation"]["earlier"]["before"]["created_at_ms"],
        Value::Null
    );
    assert!(page["navigation"].get("resume").is_none());
    assert_eq!(page["resumable"], false);
    for request in page["navigation"].as_object().unwrap().values() {
        Operation::parse(&request.to_string()).unwrap();
    }
    Operation::parse(&page["events"][0]["request"].to_string()).unwrap();
}
#[test]
fn web_agents_stored_history_without_live_snapshot_and_missing_event_error() {
    let (_dir, data) = data();
    let reply = handle(
        r#"{"op":"history","run":"old"}"#,
        ClientId::new(1),
        &ThreadSenders::default(),
        &data,
    );
    assert_eq!(reply["ok"]["run"], "old");
    let reply = handle(
        r#"{"op":"event","run":"old","revision":1}"#,
        ClientId::new(1),
        &ThreadSenders::default(),
        &data,
    );
    assert_eq!(reply["error"]["code"], "not_found");
    let reply = with_snapshot(
        json!({"op":"history","agent":"missing","run":"old"}),
        vec![],
        &data,
    );
    assert_eq!(reply["ok"]["agent_gone"], true);
    let reply = with_snapshot(
        json!({"op":"history","agent":"live:local:7","run":"old","before":{"revision":99},"worker":"old-worker"}),
        vec![live("local", AgentState::Idle, true)],
        &data,
    );
    assert_eq!(reply["ok"]["run"], "run");
    assert_eq!(reply["ok"]["retargeted"], true);
    assert_eq!(reply["ok"]["header"]["name"], "live task");
}

fn history_navigation_data() -> (tempfile::TempDir, DataAccess) {
    use agent_proto::agents::{rows, AgentRun, RunEntry, RunEntryKind, RunOutcome};
    use yinz_types::plugin_data::REV_ABSENT;
    let (directory, data) = data();
    for (id, revisions) in [
        ("run", 101..=220),
        ("replacement", 1..=4),
        ("previous", 1..=1),
        ("original", 1..=1),
    ] {
        let run = AgentRun {
            id: id.into(),
            pane_id: 7,
            yinz_session_name: "stored".into(),
            host_id: yinz_runtime::host_identity::host_id(),
            origin_run_id: (id == "run").then(|| "original".into()),
            previous_run_id: (id == "run").then(|| "previous".into()),
            session_id: "conversation".into(),
            kind: "codex".into(),
            cwd: None,
            host: None,
            bubblewrap: None,
            started_at_ms: 100,
            ended_at_ms: None,
            outcome: RunOutcome::Active,
            revision: *revisions.end(),
            summarized_revision: 0,
            summary: None,
            omitted_entries: 0,
            entries: revisions
                .map(|revision| RunEntry {
                    worker_id: (id == "run" && revision > 180).then(|| "old-worker".into()),
                    conversation_turn_id: None,
                    revision,
                    event_id: None,
                    kind: RunEntryKind::Message,
                    text: if id == "replacement" && revision == 4 {
                        "unrelated replacement message".into()
                    } else {
                        format!("needle {id} {revision}")
                    },
                    // An old chronological cursor excludes the replacement's later messages;
                    // an old delivery/search watermark excludes its lower revisions.
                    created_at_ms: Some(if id == "replacement" {
                        2_000_000 + revision as i64
                    } else {
                        revision as i64 * 1000
                    }),
                    tool: None,
                })
                .collect(),
        };
        data.plugin_data()
            .apply(
                project_data::store::SHARED,
                &rows::run_mutations(&run, REV_ABSENT),
            )
            .unwrap();
    }
    (directory, data)
}

fn history_navigation_page(
    mut request: Value,
    agents: Option<Vec<WebLiveAgent>>,
    data: &DataAccess,
) -> Value {
    // Follow the emitted request as documented, without restoring its agent binding.
    request["tz"] = json!("UTC");
    Operation::parse(&request.to_string()).unwrap();
    let reply = match agents {
        Some(agents) => with_snapshot(request.clone(), agents, data),
        None => handle(
            &request.to_string(),
            ClientId::new(1),
            &ThreadSenders::default(),
            data,
        ),
    };
    assert!(reply.get("ok").is_some(), "{request}: {reply}");
    let page = reply["ok"].clone();
    for request in page["navigation"].as_object().unwrap().values() {
        Operation::parse(&request.to_string()).unwrap();
    }
    for event in page["events"].as_array().unwrap() {
        Operation::parse(&event["request"].to_string()).unwrap();
        assert!(event["request"].get("agent").is_none());
    }
    page
}

#[test]
fn web_agents_refresh_reports_legacy_completion_repaired_since_last_visit() {
    use agent_proto::agents::{rows, RunEntry, RunEntryKind};
    let (_dir, data) = history_navigation_data();
    let request = json!({"op":"history","run":"replacement"});
    let mut entry = RunEntry {
        revision: 5,
        created_at_ms: Some(2_000_005),
        kind: RunEntryKind::Result,
        text: "repaired answer".into(),
        worker_id: None,
        conversation_turn_id: None,
        event_id: None,
        tool: None,
    };
    let save = |e: &RunEntry| {
        data.plugin_data()
            .apply(
                project_data::store::SHARED,
                &[rows::entry_mutation(
                    "replacement",
                    &yinz_runtime::host_identity::host_id(),
                    e,
                )],
            )
            .unwrap();
    };
    save(&entry);
    let first = history_navigation_page(request.clone(), None, &data);
    assert_eq!(
        first["events"].as_array().unwrap().last().unwrap()["kind"],
        "result"
    );
    entry.revision = 6;
    entry.kind = RunEntryKind::Message;
    save(&entry);
    let catch_up = history_navigation_page(first["navigation"]["received"].clone(), None, &data);
    let fresh = history_navigation_page(request, None, &data);
    assert_eq!(fresh["suppressed_result_revisions"], json!([5]));
    assert_eq!(catch_up["suppressed_result_revisions"], json!([5]));
    assert!(fresh["events"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["revision"] != 5));
}

#[test]
fn web_agents_peer_detail_selector_keeps_aliased_rows_and_history_separate() {
    let (_directory, data) = history_navigation_data();
    let mut renamed = live("old-name", AgentState::Waiting, false);
    renamed.tab_group_session = "new-name".into();
    let mut replacement = live("old-name", AgentState::Waiting, false);
    replacement.pane.agent.as_mut().unwrap().session_id = Some("other-conversation".into());
    replacement.run = Some("replacement".into());
    assert_eq!(renamed.id, replacement.id);
    let original_id = renamed.peer_detail_id().unwrap();
    let other_id = replacement.peer_detail_id().unwrap();
    assert_ne!(original_id, other_id);
    assert!(!original_id.contains("conversation"));
    let forward = vec![renamed.clone(), replacement.clone()];
    let reverse = vec![replacement.clone(), renamed.clone()];
    for agents in [&forward, &reverse] {
        let list = with_snapshot(json!({"op":"list"}), agents.clone(), &data);
        for (id, name) in [(&original_id, "new-name"), (&other_id, "old-name")] {
            let rows = list["ok"]["agents"].as_array().unwrap();
            assert_eq!(rows.iter().filter(|row| row["detail_id"] == *id).count(), 1);
            assert!(rows
                .iter()
                .any(|row| row["detail_id"] == *id && row["tab_group_session"] == name));
        }
        for (id, run, conversation) in [
            (&original_id, "run", "conversation"),
            (&other_id, "replacement", "other-conversation"),
        ] {
            let page = history_navigation_page(
                json!({"op":"history","agent":id,"run":run}),
                Some(agents.clone()),
                &data,
            );
            assert_eq!(page["run"], run);
            assert_eq!(page["retargeted"], false);
            assert_eq!(page["live"]["ref"]["session_id"], conversation);
            assert_eq!(page["live"]["detail_id"], *id);
            assert_eq!(page["live"]["ref"]["id"], *id);
            assert_eq!(page["navigation"]["received"]["agent"], *id);
            assert!(page["navigation"]
                .get("previous")
                .is_none_or(|r| r.get("agent").is_none()));
            assert!(page["navigation"]
                .get("origin")
                .is_none_or(|r| r.get("agent").is_none()));
        }
    }
    for (id, agents) in [
        (renamed.id.clone(), reverse.clone()),
        (
            original_id.clone(),
            vec![renamed.clone(), renamed.clone(), replacement.clone()],
        ),
        (original_id.clone(), vec![replacement.clone()]),
    ] {
        let page = history_navigation_page(
            json!({"op":"history","agent":id,"run":"run"}),
            Some(agents.clone()),
            &data,
        );
        assert_eq!(page["run"], "run");
        assert_eq!(page["agent_gone"], true);
        assert!(page["live"].is_null());
        assert!(page["events"].to_string().contains("needle run"));
        assert_eq!(
            with_snapshot(json!({"op":"usage","agent":id}), agents, &data)["error"]["code"],
            "not_found"
        );
    }
    let usage = with_snapshot(json!({"op":"usage","agent":original_id}), reverse, &data);
    assert_eq!(
        usage["ok"]["usage"]["message"],
        "Quota unavailable for peer-session agents"
    );
    let mut ambiguous_ref = renamed.clone();
    ambiguous_ref.tab_group_session = "duplicate-cache-owner".into();
    let ambiguous_agents = vec![renamed.clone(), ambiguous_ref];
    let list = with_snapshot(json!({"op":"list"}), ambiguous_agents.clone(), &data);
    let rows = list["ok"]["agents"].as_array().unwrap();
    assert!(rows.iter().all(|row| row.get("detail_id").is_none()));
    assert!(rows
        .iter()
        .all(|row| row["ref"]["session_id"] == "" && row["can_prompt"] == false));
    let ambiguous = history_navigation_page(
        json!({"op":"history","agent":original_id,"run":"run"}),
        Some(ambiguous_agents),
        &data,
    );
    assert_eq!(ambiguous["agent_gone"], true);
    assert!(ambiguous["live"].is_null());

    renamed.run = None;
    assert_eq!(
        renamed.peer_detail_id().as_deref(),
        Some(original_id.as_str())
    );
    let before = history_navigation_page(
        json!({"op":"history","agent":original_id,"run":"run"}),
        Some(vec![renamed.clone()]),
        &data,
    );
    renamed.session = renamed.tab_group_session.clone();
    renamed.id = "live:new-name:7".into();
    renamed.run = Some("replacement".into());
    assert_eq!(
        renamed.peer_detail_id().as_deref(),
        Some(original_id.as_str())
    );
    let after = history_navigation_page(
        before["navigation"]["received"].clone(),
        Some(vec![replacement.clone(), renamed.clone()]),
        &data,
    );
    assert_eq!(after["run"], "replacement");
    assert_eq!(after["retargeted"], true);
    assert_eq!(after["live"]["ref"]["id"], original_id);
    assert_eq!(after["live"]["ref"]["session_id"], "conversation");
    assert!(after["events"].to_string().contains("needle replacement 1"));
    renamed.pane.agent.as_mut().unwrap().session_id = None;
    let runless_id = renamed.peer_detail_id().unwrap();
    assert_ne!(runless_id, original_id);
    let page = history_navigation_page(
        after["navigation"]["received"].clone(),
        Some(vec![renamed]),
        &data,
    );
    assert_eq!(page["agent_gone"], true);
    assert!(page["live"].is_null());
}

#[test]
fn web_agents_history_navigation_refresh_tracks_live_gone_and_returned_agents() {
    let (_directory, data) = history_navigation_data();
    let agent = live("local", AgentState::Idle, true);
    let page = history_navigation_page(
        json!({"op":"history","agent":agent.id,"run":"run"}),
        Some(vec![agent.clone()]),
        &data,
    );
    let refresh = page["navigation"]["received"].clone();
    assert_eq!(refresh["agent"], agent.id);
    assert!(refresh["after_revision"].as_u64().unwrap() > 100);
    let mut updated = agent.clone();
    updated.header.name = "updated live header".into();
    let unchanged = history_navigation_page(refresh, Some(vec![updated]), &data);
    assert_eq!(unchanged["retargeted"], false);
    assert_eq!(unchanged["agent_gone"], false);
    assert_eq!(unchanged["header"]["name"], "updated live header");
    assert_eq!(unchanged["live"]["id"], agent.id);
    assert!(unchanged["live"].get("tab_group").is_none());
    assert!(unchanged["live"].get("tab_group_session").is_none());
    assert_eq!(
        unchanged["navigation"]["received"],
        page["navigation"]["received"]
    );

    // Latest retains actual stored content when the agent disappears; its returned
    // refresh must keep checking for the same agent on every subsequent call.
    let gone = history_navigation_page(page["navigation"]["latest"].clone(), Some(vec![]), &data);
    assert_eq!(gone["run"], "run");
    assert_eq!(gone["retargeted"], false);
    assert_eq!(gone["agent_gone"], true);
    assert!(gone["live"].is_null());
    assert!(gone["events"].to_string().contains("needle run"));
    let gone_again =
        history_navigation_page(gone["navigation"]["received"].clone(), Some(vec![]), &data);
    assert_eq!(gone_again["agent_gone"], true);
    assert!(gone_again["live"].is_null());
    assert_eq!(gone_again["navigation"]["received"]["agent"], agent.id);

    let mut returned = agent.clone();
    returned.run = Some("replacement".into());
    returned.header.name = "returned agent".into();
    let replacement = history_navigation_page(
        gone_again["navigation"]["received"].clone(),
        Some(vec![returned]),
        &data,
    );
    assert_eq!(replacement["run"], "replacement");
    assert_eq!(replacement["retargeted"], true);
    assert_eq!(replacement["agent_gone"], false);
    assert_eq!(replacement["header"]["name"], "returned agent");
    assert_eq!(replacement["live"]["id"], agent.id);
    assert_eq!(replacement["events"].as_array().unwrap().len(), 4);
    assert!(replacement["events"]
        .to_string()
        .contains("needle replacement 1"));
    assert_eq!(replacement["navigation"]["received"]["run"], "replacement");
    assert_eq!(replacement["navigation"]["received"]["agent"], agent.id);
    assert_eq!(replacement["navigation"]["received"]["after_revision"], 5);
}

#[test]
fn web_agents_history_navigation_continuations_retarget_and_reset_cursors() {
    let (_directory, data) = history_navigation_data();
    let agent = live("local", AgentState::Idle, true);
    let page = history_navigation_page(
        json!({"op":"history","agent":agent.id,"run":"run","worker":"old-worker"}),
        Some(vec![agent.clone()]),
        &data,
    );
    let earlier = page["navigation"]["earlier"].clone();
    assert_eq!(earlier["agent"], agent.id);
    assert_eq!(earlier["worker"], "old-worker");
    assert!(earlier["before"]["revision"].as_u64().unwrap() > 180);
    let older = history_navigation_page(earlier.clone(), Some(vec![agent.clone()]), &data);
    assert_eq!(older["retargeted"], false);
    assert_eq!(older["live"]["id"], agent.id);
    let revisions: Vec<_> = older["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["revision"].as_u64().unwrap())
        .collect();
    assert!(!revisions.is_empty());
    assert!(revisions
        .iter()
        .all(|rev| *rev > 180 && *rev < earlier["before"]["revision"].as_u64().unwrap()));
    assert_eq!(older["navigation"]["latest"]["worker"], "old-worker");

    let search = history_navigation_page(
        json!({"op":"history","agent":agent.id,"run":"run","query":"needle"}),
        Some(vec![agent.clone()]),
        &data,
    );
    let continuation = search["navigation"]["continue"].clone();
    assert_eq!(continuation["agent"], agent.id);
    assert_eq!(continuation["query"], "needle");
    assert!(continuation["from"].as_u64().unwrap() > 100);
    let next = history_navigation_page(continuation.clone(), Some(vec![agent.clone()]), &data);
    assert_eq!(next["retargeted"], false);
    assert!(
        next["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|event| event["revision"].as_u64().unwrap()
                >= continuation["from"].as_u64().unwrap())
    );

    let mut replaced = agent.clone();
    replaced.run = Some("replacement".into());
    for request in [
        earlier,
        page["navigation"]["received"].clone(),
        page["navigation"]["latest"].clone(),
        continuation,
    ] {
        let searching = request["query"] == "needle";
        let replacement = history_navigation_page(request, Some(vec![replaced.clone()]), &data);
        assert_eq!(replacement["retargeted"], true);
        assert_eq!(replacement["run"], "replacement");
        assert_eq!(
            replacement["events"].as_array().unwrap().len(),
            if searching { 3 } else { 4 }
        );
        assert!(replacement["events"]
            .to_string()
            .contains("needle replacement 1"));
        assert!(!replacement["events"].to_string().contains("needle run"));
        for request in replacement["navigation"].as_object().unwrap().values() {
            assert_eq!(request["run"], "replacement");
            if request["op"] == "history" {
                assert_eq!(request["agent"], agent.id);
            } else {
                // Retained event/hit/tools readers stay independent of the live pane.
                assert!(request.get("agent").is_none());
            }
            assert!(request["worker"].is_null());
            assert!(request["before"].is_null());
        }
    }

    // Intentional lineage browsing stays snapshot-free even if the live agent
    // has moved elsewhere. Run-only history also emits no live binding.
    for (link, run) in [("previous", "previous"), ("origin", "original")] {
        let request = page["navigation"][link].clone();
        assert!(request.get("agent").is_none());
        let stored = history_navigation_page(request, None, &data);
        assert_eq!(stored["run"], run);
        assert_eq!(stored["retargeted"], false);
        assert_eq!(stored["agent_gone"], false);
        assert!(stored["live"].is_null());
        assert!(stored["events"]
            .to_string()
            .contains(&format!("needle {run} 1")));
        for request in stored["navigation"].as_object().unwrap().values() {
            assert!(request.get("agent").is_none());
        }
    }
}

#[test]
fn web_agents_history_navigation_excludes_self_lineage_and_other_run_links() {
    let reply = HistoryReply::Run {
        header: Default::default(),
        events: vec![],
        suppressed_result_revisions: vec![],
        activity_ranges: vec![],
        rows: ["previous", "origin", "latest", "other"]
            .into_iter()
            .map(|id| palette::HistoryRow {
                id: id.into(),
                label: String::new(),
                description: String::new(),
                action: HistoryAction::Run {
                    id: if id == "other" { "other-run" } else { "run" }.into(),
                    from: None,
                    query: String::new(),
                },
            })
            .collect(),
        status: String::new(),
    };
    let page = adapt(reply, "run", None, None, false, Some("missing"), false).unwrap();
    assert_eq!(page["navigation"]["latest"]["agent"], "missing");
    for id in ["previous", "origin", "other"] {
        assert!(page["navigation"][id].get("agent").is_none());
    }
    for request in page["navigation"].as_object().unwrap().values() {
        Operation::parse(&request.to_string()).unwrap();
    }
}

#[test]
fn web_agents_peer_controls_keep_wire_guard_and_retry_id_on_disconnect() {
    use nix::poll::{poll, PollFd, PollFlags};
    use std::os::{fd::AsFd, unix::net::UnixListener};
    use yinz_ipc::ipc::{ClientToServerMsg, IpcRecv, IpcSenderWithContext, ServerToClientMsg};
    let (_dir, data) = data();
    let dir = tempfile::tempdir().unwrap();
    let session = format!("web-{}", dir.path().file_name().unwrap().to_str().unwrap());
    let renamed_session = format!("{session}-new");
    std::fs::create_dir_all(&*yinz_runtime::consts::YINZ_SOCK_DIR).unwrap();
    let socket = yinz_runtime::consts::YINZ_SOCK_DIR.join(&session);
    let renamed_socket = yinz_runtime::consts::YINZ_SOCK_DIR.join(&renamed_session);
    let listener = UnixListener::bind(&socket).unwrap();
    let renamed_listener = UnixListener::bind(&renamed_socket).unwrap();
    let mut original = live(&session, AgentState::Idle, false);
    original.tab_group_session = renamed_session.clone().into();
    let replacement = live(&session, AgentState::Idle, false);
    let legacy_id = original.id.clone();
    let mut published = original.clone();
    published.session = renamed_session.clone().into();
    published.id = format!("live:{renamed_session}:7");
    let reference = |id: &str| json!({"id":id,"kind":"codex","session_id":"conversation"});
    let original_id = original.peer_detail_id().unwrap();
    let replacement_id = replacement.peer_detail_id().unwrap();
    assert_eq!(original.id, replacement.id);
    assert_eq!(
        published.peer_detail_id().as_deref(),
        Some(original_id.as_str())
    );
    let agents = vec![replacement.clone(), published.clone()];
    for (op, target, snapshot, destination, other) in [
        (
            "screen",
            &original,
            vec![original.clone()],
            &renamed_listener,
            &listener,
        ),
        (
            "keys",
            &published,
            agents.clone(),
            &renamed_listener,
            &listener,
        ),
        (
            "prompt",
            &original,
            vec![original.clone()],
            &renamed_listener,
            &listener,
        ),
        (
            "prompt",
            &published,
            agents.clone(),
            &renamed_listener,
            &listener,
        ),
        (
            "keys",
            &replacement,
            agents.clone(),
            &listener,
            &renamed_listener,
        ),
    ] {
        let reference = reference(&target.peer_detail_id().unwrap());
        let mut request = json!({"op":op,"agent":reference});
        if op == "keys" {
            request["keys"] = json!(["y"]);
        }
        if op == "prompt" {
            request["text"] = json!("continue");
            request["submission_id"] = json!("peer-retry");
        }
        let (tx, rx) = channels::unbounded();
        let senders = ThreadSenders {
            to_screen: Some(SenderWithContext::new(tx)),
            ..Default::default()
        };
        thread::scope(|scope| {
            scope.spawn(move || {
                let (ScreenInstruction::WebLiveAgents { reply }, _) =
                    rx.recv_timeout(Duration::from_secs(5)).unwrap()
                else {
                    panic!("snapshot")
                };
                reply.send(snapshot).unwrap();
            });
            scope.spawn(|| {
                let mut sockets = [
                    PollFd::new(destination.as_fd(), PollFlags::POLLIN),
                    PollFd::new(other.as_fd(), PollFlags::POLLIN),
                ];
                assert_eq!(poll(&mut sockets, 5000u16).unwrap(), 1);
                if sockets[1].revents().unwrap().contains(PollFlags::POLLIN) {
                    drop(other.accept().unwrap());
                    panic!("guarded request went to the wrong owner");
                }
                let (connection, _) = destination.accept().unwrap();
                let connection =
                    interprocess::os::unix::uds_local_socket::Stream::from(connection).into();
                let mut sender = IpcSenderWithContext::<ServerToClientMsg>::new(connection);
                let mut receiver = sender.get_receiver::<ClientToServerMsg>().unwrap();
                receiver
                    .set_recv_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let IpcRecv::Msg(
                    ClientToServerMsg::AgentControl(AgentRequest::WebPeerGuarded {
                        expected_instance,
                        pane_id,
                        expected_kind,
                        expected_session_id,
                        action,
                    }),
                    _,
                ) = receiver.recv_client_msg()
                else {
                    panic!("guarded wire request")
                };
                assert_eq!(Some(&expected_instance), target.peer_instance.as_ref());
                assert_eq!(pane_id, original.pane.id);
                assert_eq!(expected_kind, "codex");
                assert_eq!(expected_session_id, "conversation");
                if op == "prompt" {
                    assert_eq!(action.submission_id(), Some("peer-retry"));
                    return;
                }
                let mut response = AgentResponse::new(AgentOutcome::TargetChanged);
                response.message = Some("replacement detected by owning screen".into());
                sender
                    .send_server_msg(ServerToClientMsg::AgentControl(response))
                    .unwrap();
                assert!(matches!(
                    receiver.recv_client_msg(),
                    IpcRecv::Msg(ClientToServerMsg::ClientExited, _)
                ));
            });
            let response = handle(&request.to_string(), ClientId::new(1), &senders, &data);
            if op == "prompt" {
                assert_eq!(response["ok"]["response"]["submission_id"], "peer-retry");
                assert!(response["ok"]["response"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("unknown"));
            } else {
                assert_eq!(response["ok"]["response"]["outcome"], "target_changed");
            }
        });
    }
    let keys = |id| json!({"op":"keys","agent":reference(id),"keys":["y"]});
    for (id, snapshot) in [
        (legacy_id.as_str(), vec![replacement.clone()]),
        (original_id.as_str(), vec![replacement.clone()]),
        (
            replacement_id.as_str(),
            vec![replacement.clone(), replacement.clone()],
        ),
    ] {
        assert_eq!(
            with_snapshot(keys(id), snapshot, &data)["error"]["code"],
            "target_changed"
        );
    }
    let mut legacy = replacement.clone();
    legacy.peer_instance = None;
    let list = with_snapshot(json!({"op":"list"}), vec![legacy], &data);
    let row = &list["ok"]["agents"][0];
    assert!(row.get("detail_id").is_none());
    assert_eq!(row["can_prompt"], false);
    assert_eq!(row["run"], "run");
    drop(renamed_listener);
    std::fs::remove_file(renamed_socket).unwrap();
    let disconnected = with_snapshot(keys(&original_id), agents, &data);
    assert!(disconnected["ok"]["response"]["message"]
        .as_str()
        .unwrap()
        .contains("Cannot connect to agent session"));
    listener.set_nonblocking(true).unwrap();
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    drop(listener);
    std::fs::remove_file(socket).unwrap();
}

#[test]
fn web_agents_archive_adapter_pages_ties_nulls_workers_search_tools_and_live_delivery() {
    use agent_proto::agents::{rows, AgentRun, RunEntry, RunEntryKind, RunOutcome};
    use yinz_types::plugin_data::REV_ABSENT;
    let (_dir, data) = data();
    let entry = |rev: u64, kind, worker: Option<&str>| RunEntry {
        worker_id: worker.map(str::to_owned),
        conversation_turn_id: None,
        revision: rev,
        event_id: None,
        kind,
        text: format!("needle {rev}"),
        created_at_ms: if rev <= 2 {
            None
        } else {
            Some((rev / 2) as i64 * 1000)
        },
        tool: None,
    };
    let entries = (1..=80)
        .map(|rev| {
            entry(
                rev,
                if rev % 5 == 0 {
                    RunEntryKind::ToolCall
                } else {
                    RunEntryKind::Message
                },
                None,
            )
        })
        .chain((81..=84).map(|rev| entry(rev, RunEntryKind::Message, Some("worker"))))
        .collect();
    let run = AgentRun {
        id: "archive".into(),
        pane_id: 7,
        yinz_session_name: "stored".into(),
        host_id: yinz_runtime::host_identity::host_id(),
        origin_run_id: Some("original".into()),
        previous_run_id: Some("previous".into()),
        session_id: "stored".into(),
        kind: "codex".into(),
        cwd: None,
        host: None,
        bubblewrap: None,
        started_at_ms: 100,
        ended_at_ms: Some(200),
        outcome: RunOutcome::Completed,
        revision: 84,
        summarized_revision: 0,
        summary: None,
        omitted_entries: 0,
        entries,
    };
    data.plugin_data()
        .apply(
            project_data::store::SHARED,
            &rows::run_mutations(&run, REV_ABSENT),
        )
        .unwrap();
    let call = |mut request: Value| {
        request["tz"] = json!("UTC");
        let response = handle(
            &request.to_string(),
            ClientId::new(1),
            &ThreadSenders::default(),
            &data,
        );
        assert!(response.get("ok").is_some(), "{request}: {response}");
        response["ok"].clone()
    };
    let mut page = call(json!({"op":"history","run":"archive"}));
    assert_eq!(page["previous_run"], "previous");
    assert_eq!(page["origin_run"], "original");
    assert_eq!(page["resumable"], false);
    let refresh = page["navigation"]["received"].clone();
    let mut seen = std::collections::BTreeSet::new();
    loop {
        for e in page["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["range"].is_null())
        {
            let rev = e["revision"].as_u64().unwrap();
            assert!(seen.insert(rev), "duplicate {rev}");
        }
        if page["earlier"] != true {
            break;
        }
        page = call(page["navigation"]["earlier"].clone());
    }
    assert_eq!(seen, (1..=80).filter(|rev| rev % 5 != 0).collect());
    let worker = call(json!({"op":"history","run":"archive","worker":"worker"}));
    let worker_revs: Vec<_> = worker["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["range"].is_null())
        .map(|e| e["revision"].as_u64().unwrap())
        .collect();
    assert_eq!(worker_revs, vec![81, 82, 83, 84]);
    let mut search = call(json!({"op":"history","run":"archive","query":"needle"}));
    let hit = search["events"][0]["request"].clone();
    assert_eq!(hit["op"], "hit");
    let context = call(hit);
    assert!(context["navigation"].get("context").is_some());
    call(context["navigation"]["context"].clone());
    call(context["navigation"]["latest"].clone());
    let mut hits = search["events"].as_array().unwrap().len();
    while !search["next_from"].is_null() {
        search = call(search["navigation"]["continue"].clone());
        hits += search["events"].as_array().unwrap().len();
    }
    assert_eq!(hits, 84);
    let reader = call(json!({"op":"tools","run":"archive","revisions":[5,10]}));
    assert!(reader["markdown"].as_str().unwrap().contains("needle 5"));
    assert!(reader["markdown"].as_str().unwrap().contains("needle 10"));
    let tools =
        call(json!({"op":"tools","run":"archive","range":{"start":null,"end":null,"worker":null}}));
    assert!(tools["header"].is_string());
    assert!(tools["page"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["revision"] == 5));
    // A later delivery revision can belong chronologically inside an older page.
    let mut late = entry(85, RunEntryKind::Message, None);
    late.created_at_ms = Some(6000);
    data.plugin_data()
        .apply(
            project_data::store::SHARED,
            &[rows::entry_mutation(&run.id, &run.host_id, &late)],
        )
        .unwrap();
    let delivery = call(refresh);
    assert!(delivery["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["revision"] == 85));
    assert!(!delivery["activity_ranges"].as_array().unwrap().is_empty());
    let utc = call(json!({"op":"history","run":"archive"}));
    let local = handle(
        r#"{"op":"history","run":"archive","tz":"America/Los_Angeles"}"#,
        ClientId::new(1),
        &ThreadSenders::default(),
        &data,
    );
    assert_ne!(utc["header"]["started"], local["ok"]["header"]["started"]);
}

#[test]
fn web_agents_peer_focus_requires_one_positive_reply_from_the_scanned_socket() {
    use nix::poll::{poll, PollFd, PollFlags};
    use std::os::{fd::AsFd, unix::net::UnixListener};
    use yinz_ipc::ipc::{ClientToServerMsg, IpcRecv, IpcSenderWithContext, ServerToClientMsg};

    let (_dir, data) = data();
    let directory = tempfile::tempdir().unwrap();
    let session = format!(
        "focus-{}",
        directory.path().file_name().unwrap().to_str().unwrap()
    );
    let socket = yinz_runtime::consts::YINZ_SOCK_DIR.join(&session);
    std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
    let agent = live(&session, AgentState::Waiting, false);
    let reference = WebAgentRef {
        id: agent.peer_detail_id().unwrap(),
        kind: "codex".into(),
        session_id: "conversation".into(),
    };
    let input = json!({"op":"focus","agent":reference});
    // Keep the same snapshot as an older peer, a replacement and a verified
    // owner take turns listening at this path.
    for mode in [
        "refused",
        "older",
        "replacement",
        "unselectable",
        "silent",
        "verified",
    ] {
        let listener = (mode != "refused").then(|| UnixListener::bind(&socket).unwrap());
        if mode == "refused" {
            drop(UnixListener::bind(&socket).unwrap());
        }
        let (tx, rx) = channels::unbounded();
        let senders = ThreadSenders {
            to_screen: Some(SenderWithContext::new(tx)),
            ..Default::default()
        };
        thread::scope(|scope| {
            let screen = scope.spawn(|| {
                let (ScreenInstruction::WebLiveAgents { reply }, _) =
                    rx.recv_timeout(Duration::from_secs(5)).unwrap()
                else {
                    panic!("expected snapshot")
                };
                reply.send(vec![agent.clone()]).unwrap();
            });
            let probe = listener.as_ref().map(|listener| {
                scope.spawn(|| {
                    let mut sockets = [PollFd::new(listener.as_fd(), PollFlags::POLLIN)];
                    assert_eq!(poll(&mut sockets, 5000u16).unwrap(), 1);
                    let (connection, _) = listener.accept().unwrap();
                    let connection =
                        interprocess::os::unix::uds_local_socket::Stream::from(connection).into();
                    let mut sender = IpcSenderWithContext::<ServerToClientMsg>::new(connection);
                    let mut receiver = sender.get_receiver::<ClientToServerMsg>().unwrap();
                    receiver
                        .set_recv_timeout(Some(Duration::from_secs(8)))
                        .unwrap();
                    let IpcRecv::Msg(ClientToServerMsg::AgentControl(request), _) =
                        receiver.recv_client_msg()
                    else {
                        panic!("expected verification probe")
                    };
                    assert_eq!(
                        request,
                        AgentRequest::WebPeerGuarded {
                            expected_instance: agent.peer_instance.clone().unwrap(),
                            pane_id: agent.pane.id,
                            expected_kind: reference.kind.clone(),
                            expected_session_id: reference.session_id.clone(),
                            action: WebAgentAction::VerifyFocus,
                        }
                    );
                    assert!(rx.try_recv().is_err(), "dispatch before verification");
                    let reply = match mode {
                        "older" => ServerToClientMsg::LogError {
                            lines: vec!["unsupported action".into()],
                        },
                        "replacement" => ServerToClientMsg::AgentControl(AgentResponse::new(
                            AgentOutcome::TargetChanged,
                        )),
                        "unselectable" => ServerToClientMsg::AgentControl(AgentResponse::new(
                            AgentOutcome::Rejected,
                        )),
                        "silent" => {
                            assert!(matches!(receiver.recv_client_msg(), IpcRecv::Eof));
                            return;
                        },
                        "verified" => {
                            ServerToClientMsg::AgentControl(AgentResponse::new(AgentOutcome::Ok))
                        },
                        _ => unreachable!(),
                    };
                    sender.send_server_msg(reply).unwrap();
                    if mode == "verified" {
                        let (
                            ScreenInstruction::WebFocusAgent {
                                agent: target,
                                owner,
                                peer_instance,
                                deadline,
                                reply,
                            },
                            _,
                        ) = rx.recv_timeout(Duration::from_secs(5)).unwrap()
                        else {
                            panic!("expected serving Screen focus")
                        };
                        assert_eq!(target, reference);
                        assert_eq!(owner, agent.tab_group_session);
                        assert_eq!(peer_instance, agent.peer_instance);
                        assert!(deadline > Instant::now());
                        reply.send(Ok(())).unwrap();
                    }
                })
            });
            let response = handle(&input.to_string(), ClientId::new(1), &senders, &data);
            screen.join().unwrap();
            if let Some(probe) = probe {
                probe.join().unwrap();
            }
            if mode == "verified" {
                assert_eq!(response["ok"], true);
            } else {
                assert_eq!(
                    response["error"]["code"],
                    if mode == "replacement" {
                        "target_changed"
                    } else {
                        "unavailable"
                    },
                    "{mode}"
                );
                assert!(rx.try_recv().is_err(), "{mode} routed to serving Screen");
            }
        });
        drop(listener);
        std::fs::remove_file(&socket).unwrap();
    }
    let mut renaming = agent;
    renaming.session = "previous-name".into();
    let response = with_snapshot(input, vec![renaming], &data);
    assert_eq!(response["error"]["code"], "unavailable");
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("renamed session"));
}

#[test]
fn web_agents_peer_receive_deadline_bounds_cleanup_without_retrying() {
    use interprocess::local_socket::prelude::*;
    use yinz_ipc::ipc::{ClientToServerMsg, IpcRecv, IpcSenderWithContext, ServerToClientMsg};
    std::fs::create_dir_all(&*yinz_runtime::consts::YINZ_SOCK_DIR).unwrap();
    let directory = tempfile::tempdir_in(&*yinz_runtime::consts::YINZ_SOCK_DIR).unwrap();
    let session = format!(
        "{}-read",
        directory.path().file_name().unwrap().to_str().unwrap()
    );
    let socket = yinz_runtime::consts::YINZ_SOCK_DIR.join(&session);
    let listener = yinz_ipc::consts::ipc_bind(&socket).unwrap();
    thread::scope(|scope| {
        let (done, result) = crossbeam::channel::bounded(1);
        let session = &session;
        scope.spawn(move || {
            let start = Instant::now();
            let request = AgentRequest::WebPeerGuarded {
                expected_instance: uuid::Uuid::new_v4().to_string(),
                pane_id: PaneId::Terminal(TerminalId::new(7)),
                expected_kind: "codex".into(),
                expected_session_id: "conversation".into(),
                action: WebAgentAction::Read { lines: 80 },
            };
            let response = palette_agents::control_peer(
                &session.clone().into(),
                request,
                start + Duration::from_millis(250),
            );
            done.send((response, start.elapsed())).unwrap();
        });
        let connection = listener.accept().unwrap();
        let sender = IpcSenderWithContext::<ServerToClientMsg>::new(connection);
        let mut receiver = sender.get_receiver::<ClientToServerMsg>().unwrap();
        receiver
            .set_recv_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        assert!(matches!(
            receiver.recv_client_msg(),
            IpcRecv::Msg(
                ClientToServerMsg::AgentControl(AgentRequest::WebPeerGuarded { .. }),
                _
            )
        ));
        // Keep the peer silent until the original deadline has ended the complete call.
        let (response, elapsed) = result.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(response.unwrap_err().contains("outcome unknown"));
        assert!(elapsed < Duration::from_secs(1));
    });
    drop(listener);
    if socket.exists() {
        std::fs::remove_file(socket).unwrap();
    }
}

#[test]
fn web_agents_slash_command_routing_preserves_text_and_complete_target() {
    for text in ["/plan", "/review argument", "/custom:command first\nsecond"] {
        let identity = json!({"id":"live:local:7","kind":"codex","session_id":"conversation"});
        let operation = Operation::parse(
            &json!({"op":"prompt", "agent":identity,
            "text":text, "submission_id":"command-route"})
            .to_string(),
        )
        .unwrap();
        let Operation::Prompt {
            agent,
            text: routed,
            submission_id,
        } = operation
        else {
            panic!("prompt routing")
        };
        assert_eq!(agent.kind, "codex");
        assert_eq!(agent.session_id, "conversation");
        assert_eq!(routed, text);
        assert_eq!(submission_id, "command-route");
    }
}
