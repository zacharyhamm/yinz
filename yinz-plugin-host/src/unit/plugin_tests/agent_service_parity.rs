use super::{
    data_parity::DataHost,
    runtime_parity::{default_reply, Pair, Responder},
};
use prost::Message;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use yinz_plugin_api::event::ProtobufPaneScrollbackResponse;
use yinz_plugin_api::plugin_command::*;
use yinz_types::internal_ids::DataRevision;
use yinz_types::plugin_data::REV_ABSENT;
use yinz_types::{
    data::*,
    internal_ids::{PluginId, TabId, TabPosition, TerminalId},
    pane_id::PaneId,
    units::FractionalSeconds,
};

fn host(
    clock: Arc<AtomicU64>,
    session: SessionInfo,
    configured: bool,
) -> (Arc<DataHost>, Responder) {
    let data = DataHost::with_clock("agent-service-parity", clock);
    let responder = service_host(&data, session, configured);
    (data, responder)
}
fn service_host(data: &Arc<DataHost>, session: SessionInfo, configured: bool) -> Responder {
    data.responder(Arc::new(move |command| {
        Some(match command {
            PluginCommand::GetAgentProviders => ProtobufAgentProvidersResponse {
                json: "{\"Ok\":[]}".into(),
            }
            .encode_to_vec(),
            PluginCommand::GetSessionEnvironmentVariables => {
                ProtobufGetSessionEnvironmentVariablesResponse {
                    env_vars: if configured {
                        vec![EnvVariable {
                            name: "OPENAI_API_KEY".into(),
                            value: "parity-test-key".into(),
                        }]
                    } else {
                        vec![]
                    },
                }
                .encode_to_vec()
            },
            PluginCommand::GetPluginAliasConfiguration(_) => {
                ProtobufGetPluginAliasConfigurationResponse::default().encode_to_vec()
            },
            PluginCommand::GetSessionList => ProtobufGetSessionListResponse::from(
                GetSessionListResponse::Ok(SessionListSnapshot {
                    live_sessions: vec![session.clone()],
                    ..Default::default()
                }),
            )
            .encode_to_vec(),
            PluginCommand::GetPaneRunningCommand { .. } => {
                ProtobufGetPaneRunningCommandResponse::from(GetPaneRunningCommandResponse::Ok(
                    vec!["claude".into()],
                ))
                .encode_to_vec()
            },
            PluginCommand::GetPaneCwd { .. } => {
                ProtobufGetPaneCwdResponse::from(GetPaneCwdResponse::Ok("/work/topic".into()))
                    .encode_to_vec()
            },
            PluginCommand::GetPaneScrollback { .. } => {
                ProtobufPaneScrollbackResponse::try_from(PaneScrollbackResponse::Ok(PaneContents {
                    viewport: vec!["Working on the project".into()],
                    ..Default::default()
                }))
                .unwrap()
                .encode_to_vec()
            },
            PluginCommand::GetPaneInfo(id) => ProtobufGetPaneInfoResponse {
                pane_info: session
                    .panes
                    .panes
                    .values()
                    .flatten()
                    .find(|pane| pane.id == *id)
                    .cloned()
                    .map(|pane| pane.try_into().unwrap()),
            }
            .encode_to_vec(),
            PluginCommand::GetPluginIdsForUrl(url) => ProtobufGetPluginIdsForUrlResponse {
                plugin_ids: if url == "yinz:agents" {
                    vec![7]
                } else {
                    vec![]
                },
                error: None,
            }
            .encode_to_vec(),
            PluginCommand::NewTab { .. } => ProtobufNewTabResponse {
                result: Some(new_tab_response::Result::TabId(2)),
            }
            .encode_to_vec(),
            PluginCommand::BreakPanesToTabWithId(_, id, _) => {
                ProtobufBreakPanesToTabWithIdResponse {
                    result: Some(break_panes_to_tab_with_id_response::Result::TabId(
                        id.get() as u64
                    )),
                }
                .encode_to_vec()
            },
            PluginCommand::ProjectConfigRead { directory }
            | PluginCommand::ProjectConfigWrite { directory, .. } => ProtobufFsReadResponse {
                result: Some(fs_read_response::Result::Contents(
                    serde_json::to_vec(&json!({"root":directory,"source":null,"config":{"default_branch":"main"},"error":null}))
                    .unwrap(),
                )),
            }
            .encode_to_vec(),
            PluginCommand::GetPaneGitProbe { cwd, .. } => ProtobufGetPaneGitProbeResponse {
                result: Some(get_pane_git_probe_response::Result::Probe(
                    ProtobufPaneGitProbe {
                        host: "builder".into(),
                        cwd: cwd.clone(),
                        exit_code: Some(0),
                        metadata: format!("{cwd}\n{cwd}/.git\n{cwd}/.git\nmain\n").into_bytes(),
                        config: vec![],
                    },
                )),
            }
            .encode_to_vec(),
            _ => return default_reply(command),
        })
    }))
}
fn pair() -> (Pair, Arc<AtomicU64>, Arc<DataHost>, Arc<DataHost>) {
    pair_with_session(session(false))
}
fn session(populated: bool) -> SessionInfo {
    let mut session = SessionInfo::new("work".into());
    session.is_current_session = true;
    session.plugins.insert(
        PluginId::new(7),
        PluginInfo {
            location: "yinz:agents".into(),
            ..Default::default()
        },
    );
    if populated {
        session.tabs = vec![TabInfo {
            tab_id: TabId::new(1),
            position: TabPosition::new(0),
            name: "work".into(),
            active: true,
            ..Default::default()
        }];
        session.panes.panes.insert(
            TabPosition::new(0),
            vec![PaneInfo {
                id: PaneId::Terminal(TerminalId::new(10)),
                title: "claude".into(),
                terminal_command: Some("claude".into()),
                cwd: Some("/work/topic".into()),
                ..Default::default()
            }],
        );
    }
    session
}
fn pair_with_session(session: SessionInfo) -> (Pair, Arc<AtomicU64>, Arc<DataHost>, Arc<DataHost>) {
    pair_configured(session, false)
}
fn pair_configured(
    session: SessionInfo,
    configured: bool,
) -> (Pair, Arc<AtomicU64>, Arc<DataHost>, Arc<DataHost>) {
    let clock = Arc::new(AtomicU64::new(1_700_000_000_000));
    let (wasm, wh) = host(clock.clone(), session.clone(), configured);
    let (steel, sh) = host(clock.clone(), session.clone(), configured);
    (
        Pair::with_hosts(
            "agent-service",
            BTreeMap::new(),
            wh,
            sh,
            Some(clock.clone()),
        ),
        clock,
        wasm,
        steel,
    )
}
fn message(name: &str, source: PipeSource, payload: Option<String>) -> PipeMessage {
    PipeMessage {
        name: name.into(),
        source,
        payload,
        args: BTreeMap::new(),
        is_private: false,
        pane_id: None,
    }
}
#[test]
fn agent_service_actual_runtimes_match_empty_startup_subscriptions_and_heartbeat() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "agent_service_actual_runtimes_match_empty_startup_subscriptions_and_heartbeat",
    );
    let (mut pair, clock, _, _) = pair();
    pair.render(0, 0);
    for id in [7, 7, 8] {
        pair.pipe(message(
            "agent_subscribe",
            PipeSource::Plugin(PluginId::new(id)),
            None,
        ));
    }
    clock.fetch_add(5_000, Ordering::SeqCst);
    pair.event(Event::Timer(FractionalSeconds::new(5.0)));
    pair.pipe(message("agent_refresh", PipeSource::Keybind, None));
    pair.pipe(message(
        "agent_summarize",
        PipeSource::Keybind,
        Some("{}".into()),
    ));
}

#[test]
fn agent_service_echoes_open_request_only_to_the_requesting_subscriber() {
    use super::runtime_parity::SteelReference;

    let (_, responder) = host(Arc::new(AtomicU64::new(90_000)), session(false), false);
    let mut service = SteelReference::new(
        "agent-service",
        BTreeMap::new(),
        std::path::Path::new("/work"),
        responder,
    );
    for id in [7, 8] {
        service.pipe(&message(
            "agent_subscribe",
            PipeSource::Plugin(PluginId::new(id)),
            None,
        ));
    }
    service.take_commands();
    let mut request = message(
        "agent_subscribe",
        PipeSource::Plugin(PluginId::new(7)),
        None,
    );
    request.args.insert("open_request".into(), "visit-2".into());
    service.pipe(&request);
    let snapshots: Vec<_> = service
        .take_commands()
        .into_iter()
        .filter_map(|command| {
            let send = command.get("MessageToPlugin")?;
            (send["message_name"] == "agent_snapshot").then(|| send.clone())
        })
        .collect();
    assert_eq!(snapshots.len(), 2);
    for send in &snapshots {
        if send["destination_plugin_id"] == 7 {
            assert_eq!(send["message_args"]["open_request"], "visit-2");
        } else {
            assert_eq!(send["destination_plugin_id"], 8);
            assert!(send["message_args"].get("open_request").is_none());
        }
    }
    assert_eq!(
        snapshots[0]["message_args"]["seq"],
        snapshots[1]["message_args"]["seq"]
    );

    // Subsequent forced publications do not retain another request's token.
    service.pipe(&message(
        "agent_subscribe",
        PipeSource::Plugin(PluginId::new(8)),
        None,
    ));
    for command in service.take_commands() {
        if let Some(send) = command.get("MessageToPlugin") {
            assert!(send["message_args"].get("open_request").is_none());
        }
    }
}

fn hook(name: &str, extra: Value) -> Event {
    let mut raw = json!({"hook_event_name":name,"session_id":"conversation-1","cwd":"/work/topic"});
    raw.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    Event::AgentEvent(yinz_types::agent::AgentEvent {
        pane_id: TerminalId::new(10),
        kind: "claude".into(),
        hook: name.into(),
        session_id: Some("conversation-1".into()),
        state: None,
        detail: None,
        prompt: None,
        last_message: None,
        tool_name: None,
        cwd: Some("/work/topic".into()),
        raw: raw.to_string(),
    })
}
fn stored_equal(wasm: &DataHost, steel: &DataHost) {
    for entity in [
        "agent_run",
        "agent_run_entry",
        "agent_latch",
        "agent_summary",
        "agent_assignment",
    ] {
        let request = FetchRequest {
            namespace: Some("shared:yinz".into()),
            ..FetchRequest::entity(entity).sort_by("uid", SortDir::Asc)
        };
        // Guarded updates do not consume SQLite insert IDs; the persistent identity is uid.
        let without_ids = |host: &DataHost| {
            let mut rows = host.fetch(&request);
            assert!(!rows.truncated, "compare every stored {entity} row");
            let id = rows.column_index("id").unwrap();
            rows.columns.remove(id);
            for row in &mut rows.rows {
                row.remove(id);
            }
            rows
        };
        assert_eq!(without_ids(steel), without_ids(wasm), "stored {entity}");
    }
}
#[test]
fn agent_service_actual_runtimes_match_hooks_prose_deduplication_and_persistence() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "agent_service_actual_runtimes_match_hooks_prose_deduplication_and_persistence",
    );
    let (mut pair, clock, wasm, steel) = pair_with_session(session(true));
    pair.pipe(message(
        "agent_subscribe",
        PipeSource::Plugin(PluginId::new(7)),
        None,
    ));
    for (name, extra) in [
        ("SessionStart", json!({"source":"startup"})),
        (
            "UserPromptSubmit",
            json!({"prompt":"Investigate the parser failure","turn_id":"turn-1"}),
        ),
        (
            "PreToolUse",
            json!({"tool_name":"Bash","tool_use_id":"call-1","tool_input":{"command":"cargo test"},"turn_id":"turn-1"}),
        ),
        (
            "PostToolUse",
            json!({"tool_name":"Bash","tool_use_id":"call-1","tool_response":"All tests passed","turn_id":"turn-1"}),
        ),
        (
            "MessageDisplay",
            json!({"turn_id":"turn-1","message_id":"message-1","index":1,"final":true,"delta":"passed."}),
        ),
        (
            "MessageDisplay",
            json!({"turn_id":"turn-1","message_id":"message-1","index":0,"final":false,"delta":"All tests "}),
        ),
        (
            "Stop",
            json!({"turn_id":"turn-1","last_assistant_message":"All tests passed."}),
        ),
        (
            "Stop",
            json!({"turn_id":"turn-1","last_assistant_message":"All tests passed."}),
        ),
        ("SessionEnd", json!({})),
    ] {
        clock.fetch_add(100, Ordering::SeqCst);
        pair.event(hook(name, extra));
        stored_equal(&wasm, &steel);
    }
}

#[test]
fn agent_service_actual_runtimes_match_operation_authorization_and_registration() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "agent_service_actual_runtimes_match_operation_authorization_and_registration",
    );
    let (mut pair, _, wasm, steel) = pair_with_session(session(true));
    for (source, payload) in [
        (
            PipeSource::Plugin(PluginId::new(8)),
            json!({"request_id":"denied","op":"context","args":{}}),
        ),
        (
            PipeSource::Plugin(PluginId::new(7)),
            json!({"request_id":"context","op":"context","args":{"pane_id":10}}),
        ),
        (
            PipeSource::Cli("caller".into()),
            json!({"op":"agent_register","args":{"pane_id":10,"kind":"Invalid kind"}}),
        ),
        (
            PipeSource::Cli("caller".into()),
            json!({"op":"agent_register","args":{"pane_id":10,"kind":"custom"}}),
        ),
        (
            PipeSource::Cli("caller".into()),
            json!({"op":"pane_rerun","args":{"pane_id":10}}),
        ),
        (
            PipeSource::Plugin(PluginId::new(7)),
            json!({"request_id":"unknown","op":"unknown","args":{}}),
        ),
    ] {
        let message = message("agent_action", source.clone(), Some(payload.to_string()));
        if matches!(source, PipeSource::Plugin(_)) {
            // Consume the old recording, whose Agents exception was deliberately removed.
            pair.wasm.pipe(&message);
            pair.wasm.take_commands();
            pair.steel.pipe(&message);
            let commands = pair.steel.take_commands();
            let reply = commands
                .iter()
                .find_map(|command| command.get("MessageToPlugin"))
                .unwrap();
            let result: Value =
                serde_json::from_str(reply["message_payload"].as_str().unwrap()).unwrap();
            assert_eq!(result["result"]["ok"], false);
            assert_eq!(
                result["result"]["error"],
                "agent operations require a CLI caller"
            );
        } else {
            pair.pipe(message);
        }
        stored_equal(&wasm, &steel);
    }
    let expected = wasm.changed();
    let actual = steel.changed();
    assert_eq!(actual, expected);
    for event in expected {
        pair.event(event);
    }
    pair.pipe(message(
        "agent_action",
        PipeSource::Cli("caller".into()),
        Some("{".into()),
    ));
}

#[test]
fn agent_service_actual_runtimes_match_completion_validation_and_missing_endpoint() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "agent_service_actual_runtimes_match_completion_validation_and_missing_endpoint",
    );
    let (mut pair, _, _, _) = pair();
    for payload in [
        None,
        Some("{".into()),
        Some("{}".into()),
        Some(json!({"request_id":"empty","messages":[]}).to_string()),
        Some(
            json!({"request_id":"missing-key","messages":[{"role":"user","content":"hello"}]})
                .to_string(),
        ),
    ] {
        for source in [
            PipeSource::Cli("caller".into()),
            PipeSource::Plugin(PluginId::new(7)),
            PipeSource::Keybind,
        ] {
            pair.pipe(message("llm_complete", source, payload.clone()));
        }
    }
}

fn pipe_commands(pair: &mut Pair, message: PipeMessage) -> Vec<Value> {
    assert_eq!(pair.steel.pipe(&message), pair.wasm.pipe(&message));
    pair.commands()
}
fn web_context(commands: &[Value]) -> BTreeMap<String, String> {
    let request = commands
        .iter()
        .find_map(|command| command.get("WebRequest"))
        .expect("web request");
    serde_json::from_value(request[4].clone()).unwrap()
}
#[test]
fn agent_service_actual_runtimes_match_completion_limits_replies_and_timeout() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "agent_service_actual_runtimes_match_completion_limits_replies_and_timeout",
    );
    let (mut pair, clock, _, _) = pair_configured(session(false), true);
    let request = |id: &str| {
        json!({"request_id":id,"messages":[{"role":"user","content":"hello"}]}).to_string()
    };
    let first = pipe_commands(
        &mut pair,
        message(
            "llm_complete",
            PipeSource::Cli("caller".into()),
            Some(request("first")),
        ),
    );
    let context = web_context(&first);
    pair.pipe(message(
        "llm_complete",
        PipeSource::Cli("caller".into()),
        Some(request("second")),
    ));
    pair.pipe(message(
        "llm_complete",
        PipeSource::Cli("caller".into()),
        Some(request("over-limit")),
    ));
    for id in 10..17 {
        pair.pipe(message(
            "llm_complete",
            PipeSource::Plugin(PluginId::new(id)),
            Some(request("plugin")),
        ));
    }
    let response=json!({"choices":[{"message":{"content":"hello back","tool_calls":[{"id":"tool-1","type":"function","function":{"name":"inspect","arguments":"{\"id\":7}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":4,"completion_tokens":3,"total_tokens":7}}).to_string();
    pair.event(Event::WebRequestResult(
        200,
        BTreeMap::new(),
        response.into_bytes(),
        context.clone(),
    ));
    pair.event(Event::WebRequestResult(
        500,
        BTreeMap::new(),
        b"duplicate".to_vec(),
        context,
    ));
    for _ in 0..25 {
        clock.fetch_add(5_000, Ordering::SeqCst);
        pair.event(Event::Timer(FractionalSeconds::new(5.0)));
    }
}

#[test]
fn agent_service_actual_runtimes_match_live_and_archived_summary_replies() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "agent_service_actual_runtimes_match_live_and_archived_summary_replies",
    );
    for configured in [false, true] {
        let (mut pair, clock, wasm, steel) = pair_configured(session(true), configured);
        pair.pipe(message(
            "agent_subscribe",
            PipeSource::Plugin(PluginId::new(7)),
            None,
        ));
        pair.event(hook(
            "UserPromptSubmit",
            json!({"prompt":"Fix the parser","turn_id":"turn-1"}),
        ));
        pair.event(hook("Stop",json!({"last_assistant_message":"Fixed the parser and all tests pass.","turn_id":"turn-1"})));
        clock.fetch_add(100, Ordering::SeqCst);
        let commands = pipe_commands(
            &mut pair,
            message(
                "agent_summarize",
                PipeSource::Keybind,
                Some(json!({"pane_id":10}).to_string()),
            ),
        );
        if configured {
            let context = web_context(&commands);
            let mut stale = context.clone();
            stale.insert("summary_id".into(), "999".into());
            pair.event(Event::WebRequestResult(
                200,
                BTreeMap::new(),
                summary_body(),
                stale,
            ));
            pair.pipe(message(
                "agent_summarize",
                PipeSource::Keybind,
                Some(json!({"pane_id":10}).to_string()),
            ));
            pair.event(Event::WebRequestResult(
                200,
                BTreeMap::new(),
                summary_body(),
                context,
            ));
        }
        stored_equal(&wasm, &steel);
        clock.fetch_add(100, Ordering::SeqCst);
        let commands = pair.event(hook("SessionEnd", json!({})));
        if configured {
            let context = web_context(&commands);
            pair.event(Event::WebRequestResult(
                200,
                BTreeMap::new(),
                summary_body(),
                context,
            ));
        }
        stored_equal(&wasm, &steel);
        let commands = pipe_commands(
            &mut pair,
            message(
                "agent_summarize",
                PipeSource::Keybind,
                Some(json!({"run_id":"claude:conversation-1"}).to_string()),
            ),
        );
        if configured {
            let context = web_context(&commands);
            pair.event(Event::WebRequestResult(
                503,
                BTreeMap::new(),
                b"unavailable".to_vec(),
                context,
            ));
        }
        stored_equal(&wasm, &steel);
    }
}
fn summary_body() -> Vec<u8> {
    json!({"choices":[{"message":{"content":json!({"summary":"Fixed parser and tests","title":"Parser fixed","awaiting":false}).to_string()}}]}).to_string().into_bytes()
}

fn probe(kind: &str, id: u64, cwd: &str, code: i32, bytes: &[u8]) -> Event {
    Event::RunCommandResult(
        Some(code),
        bytes.to_vec(),
        vec![],
        BTreeMap::from([
            ("git_probe".into(), kind.into()),
            ("git_probe_id".into(), id.to_string()),
            ("cwd".into(), cwd.into()),
        ]),
    )
}
#[test]
fn agent_service_actual_runtimes_match_git_probes_cwd_and_heartbeat_activity() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "agent_service_actual_runtimes_match_git_probes_cwd_and_heartbeat_activity",
    );
    let (mut pair, clock, _, _) = pair_with_session(session(true));
    pair.pipe(message(
        "agent_subscribe",
        PipeSource::Plugin(PluginId::new(7)),
        None,
    ));
    pair.event(probe("metadata", 999, "/work/topic", 0, b"stale"));
    pair.event(probe(
        "config",
        1,
        "/work/topic",
        0,
        b"remote.origin.url\ngit@github.com:example/project.git\0",
    ));
    pair.event(probe(
        "metadata",
        1,
        "/work/topic",
        0,
        b"/work/topic\n/work/topic/.git\n/work/topic/.git\nmain\n",
    ));
    pair.event(Event::Timer(FractionalSeconds::new(5.0)));
    pair.event(Event::Timer(FractionalSeconds::new(5.0)));
    pair.pipe(message("agent_refresh", PipeSource::Keybind, None));
    pair.event(probe("metadata", 2, "/work/topic", 128, b""));
    pair.event(probe("config", 2, "/work/topic", 1, b""));
    pair.event(Event::CwdChanged(
        PaneId::Terminal(TerminalId::new(10)),
        "/work/other/../other".into(),
        vec![],
    ));
    pair.event(probe("metadata", 3, "/work/other", 127, b""));
    pair.event(probe("config", 3, "/work/other", 127, b""));
    for _ in 0..6 {
        clock.fetch_add(5_000, Ordering::SeqCst);
        pair.event(Event::Timer(FractionalSeconds::new(5.0)));
    }
    pair.pipe(message(
        "arrange_projects_refresh",
        PipeSource::Plugin(PluginId::new(7)),
        None,
    ));
    clock.fetch_add(15_000, Ordering::SeqCst);
    pair.event(Event::Timer(FractionalSeconds::new(15.0)));
}

fn seed(data: &Arc<DataHost>) {
    let host = data.responder(Arc::new(default_reply));
    for def in agent_proto::store::shared_defs() {
        host(&PluginCommand::DataDefineEntity {
            namespace: Some("shared:yinz".into()),
            def,
            allow_destructive: false,
        })
        .unwrap();
    }
}

#[test]
fn agent_service_merges_conflicting_run_and_other_runs_progress() {
    use super::runtime_parity::SteelReference;
    use agent_proto::agents::{rows, AgentRun};

    let data = DataHost::new("agent-service");
    seed(&data);
    let mut original = Vec::new();
    for (id, pane) in [("stale", 7), ("healthy", 8)] {
        let run: AgentRun = serde_json::from_value(json!({
            "id":id, "pane_id":pane, "host_id":"parity-host-id", "yinz_session_name":"work",
            "kind":"claude", "started_at_ms":1000, "revision":1,
            "entries":[{"revision":1,"kind":"prompt","text":"original"}]
        }))
        .unwrap();
        data.apply("shared:yinz", &rows::run_mutations(&run, REV_ABSENT));
        original.push(run);
    }
    let mut service = SteelReference::from_source(
        "agent-service",
        r#"
        (require "./agent-service/state.scm" "./agent-service/messages.scm" "./agent-service/runs.scm")
        (define (load _)
          (as-set! "host" "parity-host-id") (as-set! "session" "work")
          (as-call "restore-runs"))
        (define (edit pane)
          (define run (append-entry (as-map-get "runs" pane #f) "result" "local edit" #f json-null 2000))
          (as-map-set! "runs" pane run) (as-map-set! "dirty-runs" (hash-ref run "id") run))
        (define (restore) (as-call "restore-runs"))
        (define (persist) (as-call "persist"))
        (define (pending) (as-keys (as-get "dirty-runs")))
        (define (revisions) (as-get "run-revisions"))
        (define (release) (as-call "finish-run" 7 "Released" 3000) (as-call "persist"))
        "#,
        BTreeMap::new(),
        data.responder(Arc::new(default_reply)),
    );
    assert_eq!(
        service.runtime.invoke("revisions", vec![]).unwrap(),
        json!({"stale":1,"healthy":1})
    );
    service.runtime.invoke("edit", vec![json!(7)]).unwrap();
    let mut winner = original[0].clone();
    winner.cwd = Some("/winner".into());
    data.apply(
        "shared:yinz",
        &rows::run_mutations(&winner, DataRevision::new(1)),
    );
    service.runtime.invoke("restore", vec![]).unwrap();
    assert_eq!(
        service.runtime.invoke("revisions", vec![]).unwrap()["stale"],
        1
    );
    let fetch = |id: &str| data.fetch(&rows::run_by_id_request(id).in_namespace("shared:yinz"));
    let entries = |id: &str| {
        data.fetch(&rows::entries_request([id.into()]).in_namespace("shared:yinz"))
            .rows
            .len()
    };
    // The conflicting write merges its local entry onto the winner and retries with the winner's rev.
    for (expected, conflict) in [(2, true), (3, false)] {
        service.runtime.invoke("edit", vec![json!(8)]).unwrap();
        service.runtime.invoke("persist", vec![]).unwrap();
        assert_eq!(
            service.runtime.invoke("pending", vec![]).unwrap(),
            json!([])
        );
        assert_eq!(
            service.runtime.invoke("revisions", vec![]).unwrap(),
            json!({"stale":3,"healthy":expected})
        );
        let saved = rows::runs_from_rows(&fetch("stale")).remove(0);
        assert_eq!(saved.cwd, winner.cwd);
        assert_eq!(saved.revision, 2);
        assert_eq!(entries("stale"), 2, "the merge keeps both timelines");
        assert_eq!(
            rows::runs_from_rows(&fetch("healthy"))[0].revision,
            expected
        );
        assert_eq!(
            service.runtime.take_logs().unwrap().contains("conflict"),
            conflict
        );
    }
    let mut winner = rows::runs_from_rows(&fetch("stale")).remove(0);
    winner.host_id = "other-host".into();
    winner.yinz_session_name = "other-session".into();
    data.apply(
        "shared:yinz",
        &rows::run_mutations(&winner, DataRevision::new(3)),
    );
    service.runtime.invoke("release", vec![]).unwrap();
    assert_eq!(
        service.runtime.invoke("pending", vec![]).unwrap(),
        json!([])
    );
    assert_eq!(
        rows::runs_from_rows(&fetch("stale"))[0].host_id,
        winner.host_id
    );
}

#[test]
fn agent_service_actual_runtimes_match_reload_claim_and_released_run_ownership() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "agent_service_actual_runtimes_match_reload_claim_and_released_run_ownership",
    );
    let clock = Arc::new(AtomicU64::new(1_700_000_000_000));
    let wasm = DataHost::with_clock("agent-service-parity", clock.clone());
    let steel = DataHost::with_clock("agent-service-parity", clock.clone());
    let mut run:agent_proto::agents::AgentRun=serde_json::from_value(json!({"id":"claude:conversation-1","pane_id":10,"yinz_session_name":"work","kind":"claude","session_id":"conversation-1","cwd":"/work/topic","started_at_ms":1000,"revision":1,"entries":[{"revision":1,"kind":"prompt","text":"Earlier work","created_at_ms":1000}]})).unwrap();
    for data in [&wasm, &steel] {
        seed(data);
        data.apply(
            "shared:yinz",
            &agent_proto::agents::rows::run_mutations(&run, REV_ABSENT),
        );
    }
    let mut pair = Pair::with_hosts(
        "agent-service",
        BTreeMap::new(),
        service_host(&wasm, session(true), false),
        service_host(&steel, session(true), false),
        Some(clock.clone()),
    );
    pair.pipe(message(
        "agent_subscribe",
        PipeSource::Plugin(PluginId::new(7)),
        None,
    ));
    pair.event(hook(
        "UserPromptSubmit",
        json!({"prompt":"Continue after reload"}),
    ));
    stored_equal(&wasm, &steel);
    let rows = wasm.fetch(&FetchRequest {
        namespace: Some("shared:yinz".into()),
        ..agent_proto::agents::rows::run_by_id_request(&run.id)
    });
    let DataValue::Int(revision) = rows.get(&rows.rows[0], "rev").unwrap() else {
        panic!("run has a revision")
    };
    run = agent_proto::agents::rows::runs_from_rows(&rows).remove(0);
    assert_eq!(run.host_id.as_str(), "parity-host-id");
    let mut released = run.clone();
    released.entries = agent_proto::agents::rows::entries_from_rows(&wasm.fetch(
        &agent_proto::agents::rows::entries_request([run.id.clone()]).in_namespace("shared:yinz"),
    ))
    .remove(&run.id)
    .unwrap();
    released.outcome = agent_proto::agents::RunOutcome::Closed;
    released.ended_at_ms = Some(clock.load(Ordering::SeqCst) as i64);
    let owner =
        agent_proto::agents::rows::Owner::new(run.host_id.clone(), run.yinz_session_name.clone());
    run.host_id = "other-host".into();
    run.yinz_session_name = "other-session".into();
    run.pane_id = 88;
    for data in [&wasm, &steel] {
        data.apply(
            "shared:yinz",
            &[agent_proto::agents::rows::run_mutation(
                &run,
                DataRevision::new(*revision),
            )],
        );
    }
    let event = Event::PaneUpdate(PaneManifest::default());
    assert_eq!(pair.steel.update(&event), pair.wasm.update(&event));
    pair.compare_commands_for_event(false, |actual, expected| {
        // Both runtimes send the owner-filtered release, which the foreign owner turns into a no-op.
        // The current runtime then reconciles: it rereads the run and its timeline and drops the stale release.
        let timeline = PluginCommand::DataFetch {
            request: agent_proto::agents::rows::entries_request([run.id.clone()])
                .in_namespace("shared:yinz"),
        };
        assert_eq!(actual.pop(), Some(serde_json::to_value(timeline).unwrap()));
        let lookup = PluginCommand::DataFetch {
            request: FetchRequest {
                select: vec!["payload".into()],
                ..agent_proto::agents::rows::run_by_id_request(&run.id).in_namespace("shared:yinz")
            },
        };
        assert_eq!(actual.pop(), Some(serde_json::to_value(lookup).unwrap()));
        let mut no_op = serde_json::to_value(PluginCommand::DataApply {
            namespace: Some("shared:yinz".into()),
            mutations: vec![agent_proto::agents::rows::run_owned_mutation(
                &released, &owner, REV_ABSENT,
            )],
        })
        .unwrap();
        no_op["DataApply"]["mutations"][0]["Update"]["expect_rev"] = Value::Null;
        assert_eq!(expected.last(), Some(&no_op));
    });
    stored_equal(&wasm, &steel);
    let rows = wasm.fetch(&FetchRequest {
        namespace: Some("shared:yinz".into()),
        ..agent_proto::agents::rows::run_by_id_request(&run.id)
    });
    let kept = agent_proto::agents::rows::runs_from_rows(&rows).remove(0);
    assert_eq!(kept.host_id.as_str(), "other-host");
    assert_eq!(kept.pane_id, 88);
}

#[test]
fn agent_service_remembers_last_focused_agent_until_its_pane_closes() {
    use super::runtime_parity::SteelReference;

    let clock = Arc::new(AtomicU64::new(90_000));
    let mut session = session(true);
    session.panes.panes.get_mut(&TabPosition::new(0)).unwrap()[0].is_focused = true;
    let agent = session.panes.panes[&TabPosition::new(0)][0].clone();
    let (_, responder) = host(clock.clone(), session, false);
    let mut service = SteelReference::with_clock(
        "agent-service",
        BTreeMap::new(),
        std::path::Path::new("/work"),
        responder,
        Some(clock),
    );
    let snapshot = |service: &mut SteelReference| -> Value {
        service.pipe(&message(
            "agent_subscribe",
            PipeSource::Plugin(PluginId::new(7)),
            None,
        ));
        service
            .take_commands()
            .iter()
            .find_map(|command| {
                let send = command.get("MessageToPlugin")?;
                (send["message_name"] == "agent_snapshot").then(|| {
                    serde_json::from_str(send["message_payload"].as_str().unwrap()).unwrap()
                })
            })
            .unwrap()
    };
    assert_eq!(snapshot(&mut service)["last_focused_agent"], 10);
    let mut panes = vec![agent];
    panes[0].is_focused = false;
    panes.push(PaneInfo {
        id: PaneId::Terminal(TerminalId::new(11)),
        is_focused: true,
        terminal_command: Some("/bin/sh".into()),
        ..Default::default()
    });
    service.update(&Event::PaneUpdate(PaneManifest {
        panes: [(TabPosition::new(0), panes.clone())].into(),
    }));
    let shell = snapshot(&mut service);
    assert_eq!(shell["focused"], 11);
    assert_eq!(shell["last_focused_agent"], 10);

    panes[1].id = PaneId::Plugin(PluginId::new(11));
    service.update(&Event::PaneUpdate(PaneManifest {
        panes: [(TabPosition::new(0), panes.clone())].into(),
    }));
    let panel = snapshot(&mut service);
    assert!(panel["focused"].is_null());
    assert_eq!(panel["last_focused_agent"], 10);

    panes.remove(0);
    service.update(&Event::PaneUpdate(PaneManifest {
        panes: [(TabPosition::new(0), panes)].into(),
    }));
    assert!(snapshot(&mut service)["last_focused_agent"].is_null());
}

#[test]
fn agent_service_rebuild_preserves_hidden_runs_and_detects_reruns_and_closures() {
    use super::runtime_parity::SteelReference;
    use agent_proto::agents::{rows, RunOutcome};

    let clock = Arc::new(AtomicU64::new(1_700_000_000_000));
    let session = session(true);
    let mut pane = session.panes.panes[&TabPosition::new(0)][0].clone();
    let (data, responder) = host(clock.clone(), session, false);
    let root = tempfile::tempdir().unwrap();
    let mut service = SteelReference::with_clock(
        "agent-service",
        BTreeMap::new(),
        root.path(),
        responder,
        Some(clock),
    );
    let stored = || {
        rows::runs_from_rows(&data.fetch(&FetchRequest {
            namespace: Some("shared:yinz".into()),
            ..FetchRequest::entity("agent_run")
        }))
    };
    let manifest = |pane| {
        Event::PaneUpdate(PaneManifest {
            panes: [(TabPosition::new(0), vec![pane])].into(),
        })
    };
    service.update(&hook("SessionStart", json!({})));
    assert_eq!(stored().len(), 1);
    for stack_member in [false, true] {
        pane.is_suppressed = true;
        pane.is_stack_list_member = stack_member;
        service.update(&manifest(pane.clone()));
        assert!(stored()[0].ended_at_ms.is_none());
    }
    pane.is_suppressed = false;
    pane.is_stack_list_member = false;
    pane.is_held = true;
    service.update(&manifest(pane.clone()));
    pane.is_held = false;
    service.update(&manifest(pane.clone()));
    assert_eq!(stored()[0].outcome, RunOutcome::Replaced);

    service.update(&hook(
        "SessionStart",
        json!({"session_id":"conversation-2"}),
    ));
    assert_eq!(stored().len(), 2);
    // A plugin with the same numeric ID must not keep a terminal's run alive.
    pane.id = PaneId::Plugin(PluginId::new(10));
    service.update(&manifest(pane));
    let closed = stored()
        .into_iter()
        .find(|run| run.session_id == "conversation-2")
        .unwrap();
    assert!(closed.ended_at_ms.is_some());
    assert_eq!(closed.outcome, RunOutcome::Closed);
}

#[test]
fn agent_service_actual_runtimes_match_tool_capture_partial_messages_and_context_clear() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "agent_service_actual_runtimes_match_tool_capture_partial_messages_and_context_clear",
    );
    let (mut pair, clock, wasm, steel) = pair_with_session(session(true));
    let mut tool:yinz_types::agent::HookEvent=serde_json::from_value(json!({"hook_event_name":"PreToolUse","session_id":"conversation-1","tool_name":"Bash","tool_use_id":"secret-call","tool_input":{"command":"echo token","password":"must-be-redacted"}})).unwrap();
    tool.tool_capture = yinz_types::agent::tool_capture::prepare("claude", &tool);
    pair.event(hook("PreToolUse", serde_json::to_value(&tool).unwrap()));
    for (name, extra) in [
        (
            "MessageDisplay",
            json!({"turn_id":"one","message_id":"partial","index":2,"final":true,"delta":"last"}),
        ),
        (
            "UserPromptSubmit",
            json!({"turn_id":"two","prompt":"Next question"}),
        ),
        (
            "SubagentStop",
            json!({"agent_id":"worker","agent_type":"","last_assistant_message":"Hidden helper"}),
        ),
        (
            "SubagentStop",
            json!({"agent_id":"worker","agent_type":"general-purpose","last_assistant_message":"Useful findings"}),
        ),
        (
            "Stop",
            json!({"assistant_messages":[{"source_id":"transcript","turn_id":"two","text":"Completed work","incomplete":false}],"messages_omitted":true}),
        ),
        ("SessionEnd", json!({})),
        (
            "SessionStart",
            json!({"session_id":"conversation-2","source":"clear"}),
        ),
        (
            "SessionEnd",
            json!({"session_id":"conversation-1","assistant_messages":[{"source_id":"late","turn_id":"two","text":"Late transcript","incomplete":false}]}),
        ),
    ] {
        clock.fetch_add(100, Ordering::SeqCst);
        pair.event(hook(name, extra));
        stored_equal(&wasm, &steel);
    }
}

fn action(pair: &mut Pair, op: &str, args: Value) {
    let commands = pipe_commands(
        pair,
        message(
            "agent_action",
            PipeSource::Cli("caller".into()),
            Some(json!({"op":op,"args":args}).to_string()),
        ),
    );
    let reply = commands
        .iter()
        .find_map(|c| c.get("CliPipeOutput"))
        .expect("operation reply");
    let value: Value = serde_json::from_str(reply[1].as_str().unwrap()).unwrap();
    assert_eq!(value["ok"], true, "{op}: {value}");
}

#[test]
fn agent_service_actual_runtimes_match_project_and_pane_operations() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "agent_service_actual_runtimes_match_project_and_pane_operations",
    );
    let clock = Arc::new(AtomicU64::new(1_700_000_000_000));
    let wasm = DataHost::with_clock("agent-service-parity", clock.clone());
    let steel = DataHost::with_clock("agent-service-parity", clock.clone());
    let project: agent_proto::projects::Project =
        serde_json::from_value(json!({"name":"project","root":"/work/topic"})).unwrap();
    for data in [&wasm, &steel] {
        seed(data);
        data.apply(
            "shared:yinz",
            &[agent_proto::projects::to_mutation(&project)],
        );
    }
    let mut session = session(true);
    let first = session
        .panes
        .panes
        .get_mut(&TabPosition::new(0))
        .unwrap()
        .first_mut()
        .unwrap();
    first.is_selectable = true;
    first.pane_columns = yinz_types::position::ColumnCount::new(20);
    first.pane_rows = yinz_types::position::RowCount::new(10);
    first.is_held = true;
    let mut second = first.clone();
    second.id = PaneId::Terminal(TerminalId::new(11));
    second.pane_x = yinz_types::position::Column::new(20);
    let mut floating = first.clone();
    floating.id = PaneId::Terminal(TerminalId::new(12));
    floating.is_floating = true;
    session
        .panes
        .panes
        .get_mut(&TabPosition::new(0))
        .unwrap()
        .extend([second, floating]);
    let mut pair = Pair::with_hosts(
        "agent-service",
        BTreeMap::new(),
        service_host(&wasm, session.clone(), false),
        service_host(&steel, session, false),
        Some(clock),
    );
    for (op, args) in [
        ("project_assign", json!({"project":"project","tab_id":1})),
        ("context", json!({"pane_id":10})),
        ("project_config_read", json!({"project":"project"})),
        (
            "project_config_write",
            json!({"project":"project","expected":null,"source":"agent_exit { default_branch \"main\"; }"}),
        ),
        (
            "project_open",
            json!({"project":"project","tab_name":"More work"}),
        ),
        ("plugin_reload", json!({"url":"yinz:agents"})),
        ("plugin_reload", json!({"url":"file:/missing.scm"})),
        ("pane_rerun", json!({"pane_id":10})),
        (
            "pane_move_to_tab",
            json!({"pane_id":"terminal_10","tab_id":1}),
        ),
        (
            "pane_swap",
            json!({"first":"terminal_10","second":"terminal_11"}),
        ),
        ("pane_pin", json!({"pane_id":"terminal_12"})),
        ("pane_unpin", json!({"pane_id":"terminal_12"})),
    ] {
        action(&mut pair, op, args);
        stored_equal(&wasm, &steel);
    }
    pair.event(Event::TabUpdate(vec![]));
    stored_equal(&wasm, &steel);
}
#[test]
fn agent_service_actual_runtimes_match_remote_repository_refresh() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "agent_service_actual_runtimes_match_remote_repository_refresh",
    );
    let mut session = session(true);
    session.panes.panes.get_mut(&TabPosition::new(0)).unwrap()[0].host = Some("builder".into());
    let (mut pair, clock, _, _) = pair_with_session(session);
    pair.pipe(message(
        "agent_subscribe",
        PipeSource::Plugin(PluginId::new(7)),
        None,
    ));
    pair.event(Event::Timer(FractionalSeconds::new(5.0)));
    clock.fetch_add(1_234, Ordering::SeqCst);
    pair.pipe(message(
        "arrange_projects_refresh",
        PipeSource::Plugin(PluginId::new(7)),
        None,
    ));
}

#[test]
fn agent_service_actual_runtimes_match_delayed_bootstrap_after_newer_pane_event() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "agent_service_actual_runtimes_match_delayed_bootstrap_after_newer_pane_event",
    );
    let clock = Arc::new(AtomicU64::new(1_700_000_000_000));
    let make = || {
        let (data, base) = host(clock.clone(), session(true), false);
        let scans = std::sync::atomic::AtomicUsize::new(0);
        let responder: Responder = Arc::new(move |command| {
            if matches!(command, PluginCommand::GetSessionList)
                && scans.fetch_add(1, Ordering::SeqCst) == 0
            {
                return Some(
                    ProtobufGetSessionListResponse::from(GetSessionListResponse::Err(
                        "starting".into(),
                    ))
                    .encode_to_vec(),
                );
            }
            base(command)
        });
        (data, responder)
    };
    let (wasm, wh) = make();
    let (steel, sh) = make();
    let mut pair = Pair::with_hosts(
        "agent-service",
        BTreeMap::new(),
        wh,
        sh,
        Some(clock.clone()),
    );
    let mut fresh = session(true);
    fresh.panes.panes.get_mut(&TabPosition::new(0)).unwrap()[0].title = "Newer live title".into();
    pair.event(Event::PaneUpdate(fresh.panes));
    pair.pipe(message(
        "agent_subscribe",
        PipeSource::Plugin(PluginId::new(7)),
        None,
    ));
    clock.fetch_add(5_000, Ordering::SeqCst);
    pair.event(Event::Timer(FractionalSeconds::new(5.0)));
    pair.event(hook(
        "UserPromptSubmit",
        json!({"prompt":"startup finished"}),
    ));
    stored_equal(&wasm, &steel);
}
