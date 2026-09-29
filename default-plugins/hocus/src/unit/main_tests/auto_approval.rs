use super::*;
use crate::decisions::REVIEW_CONTEXT;
use serde_json::json;
use std::time::{Duration, Instant};
use yinz_tile::prelude::{PaneInfo, RowCount, SessionInfo};

fn review_state() -> State {
    review_state_for(
        "clear_pane",
        r#"{"pane_id":3,"is_plugin":false}"#,
        "Clear pane 3.",
    )
}

pub(super) fn review_state_for(name: &str, arguments: &str, request: &str) -> State {
    let mut state = idle_state();
    state
        .env
        .insert("OPENROUTER_API_KEY".into(), "test-key".into());
    state.messages.push(Message::user(request));
    let call = ToolCall {
        id: "call-1".into(),
        name: name.into(),
        arguments: arguments.into(),
    };
    state.messages.push(Message::tool_calls(vec![call.clone()]));
    state.pending.push_back(call);
    set_review_session(&mut state);
    state
}

fn plugin_review_state() -> State {
    review_state_for(
        "send_input",
        r#"{"pane_id":3,"is_plugin":true,"text":"j"}"#,
        "Type j into Strider in plugin pane 3.",
    )
}

pub(super) fn set_review_session(state: &mut State) {
    tools::test_seams::reset();
    let mut session = SessionInfo {
        name: state.yinz_session_name.clone(),
        is_current_session: true,
        tabs: vec![TabInfo {
            position: TabPosition::new(2),
            tab_id: TabId::new(8),
            name: "files".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    session.panes.panes.insert(
        TabPosition::new(2),
        vec![
            PaneInfo {
                id: PaneId::from_legacy(3, false),
                title: "unrelated terminal".into(),
                terminal_command: Some("codex".into()),
                cwd: Some("/work/other".into()),
                ..Default::default()
            },
            PaneInfo {
                id: PaneId::from_legacy(3, true),
                title: "Strider".into(),
                plugin_url: Some("yinz:strider".into()),
                ..Default::default()
            },
        ],
    );
    state.panes = terminal_panes(&session.panes, &session.tabs);
    tools::test_seams::CURRENT_SESSION.with_borrow_mut(|current| *current = Some(session));
}

pub(super) fn review_context(state: &State) -> BTreeMap<String, String> {
    let Status::ReviewingTool { request_id, .. } = &state.status else {
        panic!("expected a tool review");
    };
    BTreeMap::from([(REVIEW_CONTEXT.into(), request_id.clone())])
}

/// Jev's answers: `requested`, and one value for every hazard.
pub(super) fn answer(requested: f64, hazard: f64) -> Vec<u8> {
    let mut answers = serde_json::Map::new();
    answers.insert(
        crate::approval::REQUESTED.into(),
        json!({"type": "noul", "noul": requested}),
    );
    for name in crate::approval::HAZARDS {
        answers.insert(name.into(), json!({"type": "noul", "noul": hazard}));
    }
    serde_json::to_vec(&json!({
        "model": hocus_proto::decisions::DEFAULT_MODEL,
        "answers": answers,
        "usage": {"input_tokens": 100, "output_tokens": 2}
    }))
    .unwrap()
}

/// An answer that clears the gate: clearly requested, no hazard.
pub(super) fn pass() -> Vec<u8> {
    answer(1.0, 0.0)
}

fn notices(state: &State) -> Vec<String> {
    state
        .messages
        .iter()
        .filter(|message| message.role == Role::Notice)
        .map(|message| message.content.clone())
        .collect()
}

pub(super) fn respond(state: &mut State, context: BTreeMap<String, String>, body: Vec<u8>) -> bool {
    state.handle(Event::WebRequestResult(200, BTreeMap::new(), body, context))
}

fn reply_with_gated_call(state: &mut State) {
    set_review_session(state);
    let stream_id = state.stream.as_ref().unwrap().id.clone();
    let frame = tool_call_frame(
        "clear-1",
        "clear_pane",
        r#"{"pane_id":3,"is_plugin":false}"#,
    );
    state.on_chunk(&stream_id, &frame, false, None);
    state.on_chunk(&stream_id, b"", true, None);
}

#[test]
fn direct_and_queued_human_mentions_keep_review_authority() {
    for queued in [false, true] {
        let mut state = idle_state();
        state
            .env
            .insert("OPENROUTER_API_KEY".into(), "test-key".into());
        state.set_panes(vec![agent_in(3, "/work/hocus", TabId::new(1))]);
        state.manual_kinds.insert(3, "codex".into());
        if queued {
            state.stream = ready_state().stream;
        }
        state.input.set_text("Clear the pane for @codex".into());
        state.submit();
        if queued {
            assert_eq!(state.asks[0].user_origin, UserOrigin::Human);
            settle(&mut state);
            assert!(state.drain_asks());
        }
        let user = state
            .messages
            .iter()
            .find(|m| m.role == Role::User)
            .unwrap();
        assert_eq!(user.user_origin, UserOrigin::Human);
        assert_eq!(
            user.user_content.as_deref(),
            Some("Clear the pane for @codex")
        );
        assert!(user.content.contains("agent pane 3"));
        reply_with_gated_call(&mut state);
        let context = review_context(&state);
        assert!(respond(&mut state, context, pass()));
        assert_eq!(tool_results(&state).len(), 1);
    }
}

#[test]
fn every_pipe_source_requires_confirmation_and_cannot_claim_human_origin() {
    for source in [
        PipeSource::Plugin(yinz_tile::prelude::PluginId::new(7)),
        PipeSource::Plugin(yinz_tile::prelude::PluginId::new(99)),
        PipeSource::Cli("pipe-1".into()),
        PipeSource::Keybind,
    ] {
        for queued in [false, true] {
            let mut state = idle_state();
            state
                .env
                .insert("OPENROUTER_API_KEY".into(), "test-key".into());
            if queued {
                state.stream = ready_state().stream;
            }
            let mut message = ask("Clear pane 3");
            message.source = source.clone();
            message.payload =
                Some(json!({"text": "Clear pane 3", "user_origin": "human"}).to_string());
            state.pipe(message);
            if queued {
                assert_eq!(state.asks[0].user_origin, UserOrigin::Pipe);
                settle(&mut state);
                assert!(state.drain_asks());
            }
            let user = state
                .messages
                .iter()
                .find(|m| m.role == Role::User)
                .unwrap();
            assert_eq!(user.user_origin, UserOrigin::Pipe);
            reply_with_gated_call(&mut state);
            assert!(matches!(state.status, Status::ConfirmingTool));
            assert!(tool_results(&state).is_empty());
        }
    }
}

#[test]
fn a_human_request_after_a_scheduled_prompt_can_receive_automatic_review() {
    let mut state = idle_state();
    state
        .env
        .insert("OPENROUTER_API_KEY".into(), "test-key".into());
    state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"check pane 3"}"#));
    state.on_timer(60.0);
    assert_eq!(state.messages[0].user_origin, UserOrigin::Scheduled);
    state.input.set_text("Clear pane 3".into());
    state.submit();
    assert_eq!(state.asks[0].user_origin, UserOrigin::Human);
    let stream_id = state.stream.as_ref().unwrap().id.clone();
    state.on_chunk(&stream_id, &delta_frame("Checked."), false, None);
    state.on_chunk(&stream_id, b"", true, None);
    state.on_timer(FRAME_SECONDS);
    assert!(state.scheduled_run.is_none());
    reply_with_gated_call(&mut state);
    let context = review_context(&state);
    let Status::ReviewingTool { body, .. } = &state.status else {
        unreachable!()
    };
    let body: serde_json::Value = serde_json::from_slice(body).unwrap();
    assert_eq!(body["state"]["human_request"], "Clear pane 3");
    // The scheduled prompt is untrusted, so it is not sent at all.
    assert!(!body.to_string().contains("check pane 3"));
    assert!(respond(&mut state, context, pass()));
    assert_eq!(tool_results(&state).len(), 1);
}

#[test]
fn a_safe_review_runs_the_matching_call_only_once() {
    let mut state = review_state();
    state.pump_tools();
    let context = review_context(&state);
    assert!(tool_results(&state).is_empty());
    assert!(respond(&mut state, context.clone(), answer(0.75, 0.25)));
    assert!(state.pending.is_empty());
    assert_eq!(tool_results(&state).len(), 1);
    assert!(!respond(&mut state, context, pass()));
    assert_eq!(tool_results(&state).len(), 1);
}

#[test]
fn plugin_reviews_use_the_exact_target_and_ignore_terminal_and_geometry_changes() {
    for change_unrelated in [false, true] {
        let mut state = plugin_review_state();
        state.pump_tools();
        let context = review_context(&state);
        let Status::ReviewingTool { body, .. } = &state.status else {
            unreachable!();
        };
        let body: serde_json::Value = serde_json::from_slice(body).unwrap();
        assert_eq!(
            body["state"]["pending_call"]["target"]["panes"],
            json!([{
                "pane_id": 3,
                "is_plugin": true,
                "plugin_url": "yinz:strider",
                "title": "Strider",
                "session": "test-session",
                "tab": [8, "files"],
            }]),
        );
        if change_unrelated {
            state.panes[0].title = "changed terminal".into();
            state.panes[0].cwd = Some("/work/changed".into());
            tools::test_seams::CURRENT_SESSION.with_borrow_mut(|current| {
                let session = current.as_mut().unwrap();
                let panes = session.panes.panes.get_mut(&TabPosition::new(2)).unwrap();
                panes[0].title = "changed terminal".into();
                panes[0].cwd = Some("/work/changed".into());
                panes[1].pane_rows = RowCount::new(40);
                panes[1].is_focused = true;
                session.tabs[0].viewport_rows = RowCount::new(50);
            });
        }
        assert!(respond(&mut state, context, pass()));
        assert!(state.pending.is_empty());
        assert_eq!(tool_results(&state).len(), 1);
    }
}

#[test]
fn changed_plugin_metadata_keeps_manual_confirmation() {
    for change in ["url", "title", "session", "tab_id", "tab_name"] {
        let mut state = plugin_review_state();
        state.pump_tools();
        let context = review_context(&state);
        tools::test_seams::CURRENT_SESSION.with_borrow_mut(|current| {
            let session = current.as_mut().unwrap();
            let pane = &mut session.panes.panes.get_mut(&TabPosition::new(2)).unwrap()[1];
            match change {
                "url" => pane.plugin_url = Some("file:/plugins/other.wasm".into()),
                "title" => pane.title = "Other plugin".into(),
                "session" => session.name = "other-session".to_owned().into(),
                "tab_id" => session.tabs[0].tab_id = TabId::new(9),
                "tab_name" => session.tabs[0].name = "other tab".into(),
                _ => unreachable!(),
            }
        });
        assert!(respond(&mut state, context, pass()));
        assert!(matches!(state.status, Status::ConfirmingTool), "{change}");
        assert!(state.confirmation_prompt().is_some());
        assert_eq!(state.pending.len(), 1);
        assert!(tool_results(&state).is_empty());
    }
}

#[test]
fn missing_plugin_metadata_before_or_during_review_keeps_manual_confirmation() {
    for missing in ["session", "pane", "url", "blank_url", "tab"] {
        for during_review in [false, true] {
            let mut state = plugin_review_state();
            let context = during_review.then(|| {
                state.pump_tools();
                review_context(&state)
            });
            tools::test_seams::CURRENT_SESSION.with_borrow_mut(|current| {
                if missing == "session" {
                    *current = None;
                    return;
                }
                let session = current.as_mut().unwrap();
                let panes = session.panes.panes.get_mut(&TabPosition::new(2)).unwrap();
                match missing {
                    "pane" => panes.retain(|pane| !pane.is_plugin()),
                    "url" => panes[1].plugin_url = None,
                    "blank_url" => panes[1].plugin_url = Some("  ".into()),
                    "tab" => session.tabs.clear(),
                    _ => unreachable!(),
                }
            });
            if let Some(context) = context {
                assert!(respond(&mut state, context, pass()));
            } else {
                state.pump_tools();
            }
            assert!(
                matches!(state.status, Status::ConfirmingTool),
                "{missing}, during_review={during_review}",
            );
            assert!(state.confirmation_prompt().is_some());
            assert_eq!(state.pending.len(), 1);
            assert!(tool_results(&state).is_empty());
        }
    }
}

#[test]
fn terminal_reviews_include_suppressed_targets_and_ignore_plugins_and_geometry() {
    for suppressed in [false, true] {
        let mut state = review_state();
        tools::test_seams::CURRENT_SESSION.with_borrow_mut(|current| {
            let session = current.as_mut().unwrap();
            session.panes.panes.get_mut(&TabPosition::new(2)).unwrap()[0].is_suppressed =
                suppressed;
            state.panes = terminal_panes(&session.panes, &session.tabs);
        });
        assert_eq!(state.panes.is_empty(), suppressed);
        state.pump_tools();
        let context = review_context(&state);
        let Status::ReviewingTool { body, .. } = &state.status else {
            unreachable!();
        };
        let body: serde_json::Value = serde_json::from_slice(body).unwrap();
        let panes = body["state"]["pending_call"]["target"]["panes"]
            .as_array()
            .unwrap();
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0]["pane_id"], 3);
        assert_eq!(panes[0]["is_plugin"], false);
        assert_eq!(panes[0]["command"], "codex");
        assert_eq!(panes[0]["cwd"], "/work/other");
        assert_eq!(panes[0]["session"], "test-session");
        assert_eq!(panes[0]["tab"], json!([8, "files"]));
        tools::test_seams::CURRENT_SESSION.with_borrow_mut(|current| {
            let session = current.as_mut().unwrap();
            let panes = session.panes.panes.get_mut(&TabPosition::new(2)).unwrap();
            panes[0].pane_rows = RowCount::new(40);
            panes[0].is_focused = true;
            panes[1].plugin_url = Some("yinz:other".into());
            panes[1].title = "changed plugin".into();
            session.tabs[0].viewport_rows = RowCount::new(50);
            session.tabs[0].active = true;
        });
        state.panes.clear(); // The daemon's filtered snapshot is not the authority.
        assert!(respond(&mut state, context, pass()));
        assert!(state.pending.is_empty());
        assert_eq!(tool_results(&state).len(), 1);
    }
}

#[test]
fn changed_terminal_metadata_keeps_manual_confirmation() {
    for change in [
        "command",
        "cwd",
        "host",
        "session",
        "tab_id",
        "tab_name",
        "missing_command",
        "missing_cwd",
    ] {
        let mut state = review_state();
        state.pump_tools();
        let context = review_context(&state);
        tools::test_seams::CURRENT_SESSION.with_borrow_mut(|current| {
            let session = current.as_mut().unwrap();
            let pane = &mut session.panes.panes.get_mut(&TabPosition::new(2)).unwrap()[0];
            match change {
                "command" => pane.terminal_command = Some("sh".into()),
                "cwd" => pane.cwd = Some("/work/changed".into()),
                "host" => pane.host = Some("other-host".into()),
                "session" => session.name = "other-session".to_owned().into(),
                "tab_id" => session.tabs[0].tab_id = TabId::new(9),
                "tab_name" => session.tabs[0].name = "other tab".into(),
                "missing_command" => pane.terminal_command = None,
                "missing_cwd" => pane.cwd = None,
                _ => unreachable!(),
            }
        });
        assert!(respond(&mut state, context, pass()));
        assert!(matches!(state.status, Status::ConfirmingTool), "{change}");
        assert_eq!(state.pending.len(), 1);
        assert!(tool_results(&state).is_empty());
    }
}

#[test]
fn missing_terminal_records_before_or_during_review_keep_manual_confirmation() {
    for missing in ["session", "pane", "tab"] {
        for during_review in [false, true] {
            let mut state = review_state();
            let context = during_review.then(|| {
                state.pump_tools();
                review_context(&state)
            });
            tools::test_seams::CURRENT_SESSION.with_borrow_mut(|current| {
                if missing == "session" {
                    *current = None;
                    return;
                }
                let session = current.as_mut().unwrap();
                match missing {
                    // Keep the plugin with the same numeric id.
                    "pane" => session
                        .panes
                        .panes
                        .get_mut(&TabPosition::new(2))
                        .unwrap()
                        .retain(PaneInfo::is_plugin),
                    "tab" => session.tabs.clear(),
                    _ => unreachable!(),
                }
            });
            if let Some(context) = context {
                assert!(respond(&mut state, context, pass()));
            } else {
                state.pump_tools();
            }
            assert!(
                matches!(state.status, Status::ConfirmingTool),
                "{missing}, during_review={during_review}"
            );
            assert!(state.confirmation_prompt().is_some());
            assert_eq!(state.pending.len(), 1);
            assert!(tool_results(&state).is_empty());
        }
    }
}

#[test]
fn skill_reviews_follow_the_resolved_directory_in_foreground_and_parked_chats() {
    for park in [false, true] {
        for name in ["skill_write", "skill_enable", "skill_disable"] {
            for (configured, resolved) in [
                (None, "/plugins/hocus/skills"),
                (Some("~/skills"), "/home/me/skills"),
            ] {
                for changed in [false, true] {
                    let mut state = review_state_for(
                        name,
                        r#"{"name":"bench","scope":"user","source":"Run the benchmarks."}"#,
                        "Update my user bench skill.",
                    );
                    state.chat_id = Some(4);
                    state.env.insert("HOME".into(), "/home/me".into());
                    state.user_plugin_dir = "/plugins".into();
                    if let Some(dir) = configured {
                        state.kdl.insert("skills_dir".into(), dir.into());
                    }
                    state.pump_tools();
                    let context = review_context(&state);
                    let Status::ReviewingTool { body, .. } = &state.status else {
                        unreachable!();
                    };
                    let body: serde_json::Value = serde_json::from_slice(body).unwrap();
                    assert_eq!(
                        body["state"]["pending_call"]["target"]["skills_dir"],
                        resolved
                    );
                    if park {
                        state.new_chat();
                    }
                    let mut configuration = state.kdl.clone();
                    configuration.insert(
                        "skills_dir".into(),
                        if changed { "/other/skills" } else { resolved }.into(),
                    );
                    state.handle(Event::PluginConfigurationChanged(configuration));
                    assert!(respond(&mut state, context, pass()));
                    let check = |reviewed: &mut State| {
                        assert_eq!(reviewed.pending.len(), 1);
                        assert!(tool_results(reviewed).is_empty());
                        if changed {
                            assert!(matches!(reviewed.status, Status::ConfirmingTool));
                            assert!(reviewed.confirmation_prompt().is_some());
                        } else {
                            assert!(matches!(reviewed.status, Status::AwaitingCommand));
                        }
                    };
                    if park {
                        state.with_parked(0, check);
                    } else {
                        check(&mut state);
                    }
                }
            }
        }
    }
}

#[test]
fn a_response_from_before_reload_cannot_approve_a_new_call() {
    let mut before = review_state();
    before.pump_tools();
    let context = review_context(&before);
    let mut after = review_state();
    after.pending[0].arguments = r#"{"pane_id":9,"is_plugin":false}"#.into();
    tools::test_seams::CURRENT_SESSION.with_borrow_mut(|current| {
        let panes = current
            .as_mut()
            .unwrap()
            .panes
            .panes
            .get_mut(&TabPosition::new(2))
            .unwrap();
        let mut pane = panes[0].clone();
        pane.id = PaneId::from_legacy(9, false);
        panes.push(pane);
    });
    after.pump_tools();
    assert_ne!(context, review_context(&after));
    assert!(!respond(&mut after, context, pass()));
    assert!(tool_results(&after).is_empty());
    assert_eq!(after.pending.len(), 1);
}

#[test]
fn an_authorized_codex_launch_includes_the_prompt_and_cwd_then_dispatches() {
    tools::test_seams::reset();
    let mut state = review_state();
    let prompt = "Fix the parser regression and run its tests.";
    let request = format!("Open a Codex agent in /work/hocus with this prompt: {prompt}");
    let call = ToolCall {
        id: "spawn-1".into(),
        name: "agent_spawn".into(),
        arguments: json!({"agent": "codex", "cwd": "/work/hocus", "prompt": prompt}).to_string(),
    };
    state.messages = vec![
        Message::user(&request),
        Message::tool_calls(vec![call.clone()]),
    ];
    state.pending = vec![call].into();
    state.pump_tools();
    let context = review_context(&state);
    let Status::ReviewingTool { body, .. } = &state.status else {
        unreachable!();
    };
    let body: serde_json::Value = serde_json::from_slice(body).unwrap();
    assert_eq!(body["state"]["human_request"], request);
    let launch = &body["state"]["pending_call"]["launch"];
    assert_eq!(launch["program"], "yinz");
    assert_eq!(
        launch["args"],
        json!(["hocus", "codex", "--command", "codex", "--", prompt])
    );
    assert_eq!(launch["directory"], "/work/hocus");
    assert_eq!(launch["prompt"], prompt);
    let arguments = &body["state"]["pending_call"]["arguments"];
    assert_eq!(arguments["agent"], "codex");
    assert_eq!(arguments["prompt"], prompt);
    assert_eq!(arguments["cwd"], "/work/hocus");
    tools::test_seams::SPAWN_AGENT_CALLS.with_borrow(|calls| assert!(calls.is_empty()));
    assert!(respond(&mut state, context, pass()));
    assert!(state.pending.is_empty());
    assert_eq!(
        tool_results(&state)[0].tool_call_id.as_deref(),
        Some("spawn-1")
    );
    tools::test_seams::SPAWN_AGENT_CALLS.with_borrow(|calls| assert_eq!(calls.len(), 1));
}

#[test]
fn launches_need_a_resolved_directory_despite_focus_changes() {
    for name in ["new_pane", "new_remote_pane", "agent_spawn"] {
        for cwd in [None, Some(""), Some(" \t"), Some("/work/hocus")] {
            let mut arguments =
                json!({"command": "codex", "prompt": "Fix the parser", "remote_host": "builder"});
            if let Some(cwd) = cwd {
                arguments["cwd"] = json!(cwd);
            }
            let mut state =
                review_state_for(name, &arguments.to_string(), "Run Codex to fix the parser.");
            state.pump_tools();
            let resolved = cwd.is_some_and(|cwd| !cwd.trim().is_empty());
            let context = resolved.then(|| review_context(&state));
            tools::test_seams::CURRENT_SESSION.with_borrow_mut(|current| {
                let session = current.as_mut().unwrap();
                let pane = &mut session.panes.panes.get_mut(&TabPosition::new(2)).unwrap()[0];
                pane.is_focused = true;
                pane.cwd = Some("/work/elsewhere".into());
                session.tabs[0].active = true;
            });
            if let Some(context) = context {
                assert!(respond(&mut state, context, pass()));
                assert!(state.pending.is_empty());
                assert_eq!(tool_results(&state).len(), 1);
            } else {
                state.pump_tools();
                assert!(
                    matches!(state.status, Status::ConfirmingTool),
                    "{name}, {cwd:?}"
                );
                assert_eq!(state.pending.len(), 1);
                assert!(tool_results(&state).is_empty());
            }
        }
    }
}

#[test]
fn agent_launch_reviews_bind_the_selected_project_root() {
    for explicit_project in [false, true] {
        for changed in [false, true] {
            let mut arguments = json!({"agent": "codex", "prompt": "Fix the parser"});
            if explicit_project {
                arguments["project"] = json!("hocus");
            }
            let mut state = review_state_for(
                "agent_spawn",
                &arguments.to_string(),
                "Fix the parser in Hocus.",
            );
            state.projects.insert(
                "hocus".into(),
                projects::validate("hocus", "/work/hocus").unwrap(),
            );
            if !explicit_project {
                state.active_project = Some("hocus".into());
            }
            state.pump_tools();
            let context = review_context(&state);
            if changed {
                state.projects.get_mut("hocus").unwrap().root = "/work/elsewhere".into();
            }
            assert!(respond(&mut state, context, pass()));
            assert_eq!(matches!(state.status, Status::ConfirmingTool), changed);
            assert_eq!(tool_results(&state).len(), usize::from(!changed));
            tools::test_seams::SPAWN_AGENT_CALLS
                .with_borrow(|calls| assert_eq!(calls.len(), usize::from(!changed)));
        }
    }
}

#[test]
fn invalid_agent_project_directories_require_manual_confirmation() {
    for invalid in ["unknown_project", "outside_project", "blank_root"] {
        let mut arguments =
            json!({"agent": "codex", "prompt": "Fix the parser", "project": "hocus"});
        if invalid == "outside_project" {
            arguments["cwd"] = json!("/work/other");
        }
        let mut state = review_state_for(
            "agent_spawn",
            &arguments.to_string(),
            "Fix the parser in Hocus.",
        );
        if invalid != "unknown_project" {
            let mut project = projects::validate("hocus", "/work/hocus").unwrap();
            if invalid == "blank_root" {
                project.root = "  ".into();
            }
            state.projects.insert("hocus".into(), project);
        }
        state.pump_tools();
        assert!(matches!(state.status, Status::ConfirmingTool), "{invalid}");
        assert!(tool_results(&state).is_empty());
        tools::test_seams::SPAWN_AGENT_CALLS.with_borrow(|calls| assert!(calls.is_empty()));
    }
}

#[test]
fn slack_reviews_bind_effective_configuration_without_serializing_credentials() {
    for park in [false, true] {
        for change in [
            "unchanged",
            "dobbs_path",
            "dobbs_addr",
            "dobbs_token",
            "invalid_config",
        ] {
            let mut state = review_state_for(
                "slack_post",
                r#"{"channel":"C123","text":"Done."}"#,
                "Post Done to C123.",
            );
            state.chat_id = Some(4);
            state.granted = true;
            state.env.extend([
                ("OPENAI_API_KEY".into(), "chat-secret".into()),
                ("HOME".into(), "/home/me".into()),
                ("DOBBS_BIN".into(), "~/bin/dobbs".into()),
                ("DOBBS_ADDR".into(), "slack-daemon:1234".into()),
                ("DOBBS_TOKEN".into(), "slack-secret".into()),
            ]);
            state.reconfigure();
            state.pump_tools();
            let context = review_context(&state);
            let Status::ReviewingTool {
                body,
                execution_config,
                ..
            } = &state.status
            else {
                unreachable!();
            };
            assert_eq!(execution_config.as_ref().unwrap().0, "/home/me/bin/dobbs");
            assert_eq!(
                execution_config.as_ref().unwrap().1["DOBBS_TOKEN"],
                "slack-secret"
            );
            let serialized = serde_json::to_string(&state.status).unwrap();
            for secret in [
                "slack-secret",
                "chat-secret",
                "slack-daemon:1234",
                "/home/me/bin/dobbs",
            ] {
                assert!(!String::from_utf8_lossy(body).contains(secret));
                assert!(!serialized.contains(secret));
            }
            assert!(!serialized.contains("execution_config"));
            let restored: Status = serde_json::from_str(&serialized).unwrap();
            assert!(matches!(
                restored,
                Status::ReviewingTool {
                    execution_config: None,
                    ..
                }
            ));
            if park {
                state.new_chat();
            }
            let mut configuration = state.kdl.clone();
            // Different configuration syntax, identical resolved execution settings.
            configuration.extend([
                ("dobbs_path".into(), "/home/me/bin/dobbs".into()),
                ("dobbs_addr".into(), "slack-daemon:1234".into()),
                ("dobbs_token".into(), "slack-secret".into()),
            ]);
            match change {
                "unchanged" => {},
                "invalid_config" => {
                    configuration.insert("api_key_env".into(), "MISSING_KEY".into());
                },
                key => {
                    configuration.insert(key.into(), "changed".into());
                },
            }
            state.handle(Event::PluginConfigurationChanged(configuration));
            assert!(respond(&mut state, context, pass()));
            let check = |reviewed: &mut State| {
                assert_eq!(reviewed.pending.len(), 1);
                assert!(tool_results(reviewed).is_empty());
                if change == "unchanged" {
                    assert!(matches!(reviewed.status, Status::AwaitingCommand));
                } else {
                    assert!(
                        matches!(reviewed.status, Status::ConfirmingTool),
                        "{change}, park={park}"
                    );
                    assert!(reviewed.confirmation_prompt().is_some());
                }
            };
            if park {
                state.with_parked(0, check);
            } else {
                check(&mut state);
            }
        }
    }
}

pub(super) fn project_review_state(tool: &str, padded: bool) -> State {
    let name = if padded { " \tsource\n " } else { "source" };
    let target = if padded { " \ttarget\n " } else { "target" };
    let arguments = if tool == "project_forget" {
        json!({"name": name})
    } else {
        json!({"source": name, "target": target})
    };
    let mut state = review_state_for(tool, &arguments.to_string(), "Remove the source project.");
    for name in ["active", "source", "target"] {
        let mut project = projects::validate(name, &format!("/work/{name}")).unwrap();
        project.remote = Some("https://example.com/owner/repo".into());
        project.firecracker_image = Some("rust-dev".into());
        project.worktree_dir = Some(format!("/trees/{name}"));
        state.projects.insert(name.into(), project);
    }
    state.active_project = Some("active".into());
    state
        .store()
        .apply(state.projects.values().map(projects::to_mutation).collect())
        .unwrap();
    state
}

#[test]
fn project_reviews_bind_named_targets_even_before_the_cache_updates() {
    for (tool, name) in [
        ("project_forget", "source"),
        ("project_merge", "source"),
        ("project_merge", "target"),
    ] {
        for cached in [false, true] {
            for change in [
                "root",
                "remote",
                "firecracker_image",
                "worktree_dir",
                "revision",
                "replacement",
                "removed",
            ] {
                let mut state = project_review_state(tool, true);
                state.pump_tools();
                let context = review_context(&state);
                let project = state.projects[name].clone();
                let mutations = match change {
                    "removed" => vec![projects::delete_mutation(name)],
                    "replacement" => vec![
                        projects::delete_mutation(name),
                        projects::to_mutation(&project),
                    ],
                    "revision" => vec![projects::to_mutation(&project)],
                    field => vec![Mutation::update("project", Pred::eq("name", name))
                        .set(field, "/changed")
                        .build()],
                };
                state.store().apply(mutations).unwrap();
                let rows = state
                    .store()
                    .fetch(&FetchRequest::entity("project"))
                    .unwrap();
                if cached {
                    state.projects = projects::from_rows(&rows);
                }
                assert!(respond(&mut state, context.clone(), pass()));
                assert!(
                    matches!(state.status, Status::ConfirmingTool),
                    "{tool}, {name}, {change}, cached={cached}"
                );
                assert_eq!(state.pending.len(), 1);
                assert!(tool_results(&state).is_empty());
                assert_eq!(
                    state
                        .store()
                        .fetch(&FetchRequest::entity("project"))
                        .unwrap(),
                    rows
                );
                assert!(!respond(&mut state, context, pass()));
            }
        }
    }
}

#[test]
fn forgetting_also_binds_the_cached_project() {
    for removed in [false, true] {
        let mut state = project_review_state("project_forget", false);
        state.pump_tools();
        let context = review_context(&state);
        if removed {
            state.projects.remove("source");
        } else {
            state.projects.get_mut("source").unwrap().root = "/changed".into();
        }
        assert!(respond(&mut state, context, pass()));
        assert!(matches!(state.status, Status::ConfirmingTool));
        assert!(tool_results(&state).is_empty());
    }
}

#[test]
fn project_reviews_require_present_targets_and_successful_store_reads() {
    for (tool, name) in [
        ("project_forget", "source"),
        ("project_merge", "source"),
        ("project_merge", "target"),
    ] {
        for during_review in [false, true] {
            for failed_read in [false, true] {
                let mut state = project_review_state(tool, false);
                let context = during_review.then(|| {
                    state.pump_tools();
                    review_context(&state)
                });
                if failed_read {
                    // An undeclared entity makes the real store's fetch fail.
                    state.store = store::Local::shared("empty");
                } else {
                    state
                        .store()
                        .apply(vec![projects::delete_mutation(name)])
                        .unwrap();
                }
                if let Some(context) = context {
                    assert!(respond(&mut state, context, pass()));
                } else {
                    state.pump_tools();
                }
                assert!(matches!(state.status, Status::ConfirmingTool));
                assert_eq!(state.pending.len(), 1);
                assert!(tool_results(&state).is_empty());
            }
        }
    }
}

#[test]
fn unchanged_project_targets_are_reviewed_in_full_and_execute_once() {
    for tool in ["project_forget", "project_merge"] {
        for padded in [false, true] {
            let mut state = project_review_state(tool, padded);
            add_background_store(&mut state);
            state.pump_tools();
            let context = review_context(&state);
            let Status::ReviewingTool { body, .. } = &state.status else {
                unreachable!()
            };
            let body: serde_json::Value = serde_json::from_slice(body).unwrap();
            let targets = &body["state"]["pending_call"]["target"]["projects"];
            let fields = if tool == "project_forget" {
                vec![("name", "source")]
            } else {
                vec![("source", "source"), ("target", "target")]
            };
            for (field, name) in fields {
                assert_eq!(targets[field]["project"], json!(state.projects[name]));
                assert!(targets[field]["id"]["Int"].as_i64().unwrap() > 0);
                assert!(targets[field]["rev"]["Int"].as_i64().unwrap() > 0);
            }
            assert!(respond(&mut state, context.clone(), pass()));
            finish_background_project_change(&mut state);
            assert!(state.pending.is_empty());
            assert_eq!(tool_results(&state).len(), 1);
            assert_eq!(tool_results(&state)[0].content, tool);
            let rows = state
                .store()
                .fetch(&FetchRequest::entity("project"))
                .unwrap();
            assert!(!projects::from_rows(&rows).contains_key("source"));
            assert!(projects::from_rows(&rows).contains_key("target"));
            assert!(!respond(&mut state, context, pass()));
            assert_eq!(
                state
                    .store()
                    .fetch(&FetchRequest::entity("project"))
                    .unwrap(),
                rows
            );
            assert_eq!(tool_results(&state).len(), 1);
        }
    }
}

#[test]
fn vm_reviews_bind_effective_execution_settings_privately_in_every_chat() {
    for tool in ["vm_sync", "vm_close"] {
        for park in [false, true] {
            for change in [
                "unchanged",
                "equivalent",
                "yinz_path",
                "YINZ_CONFIG_FILE",
                "YINZ_CONFIG_DIR",
                "removed_file",
                "removed_dir",
                "invalid_config",
            ] {
                let mut state =
                    review_state_for(tool, r#"{"vm":"review"}"#, "Sync and close the review VM.");
                state.chat_id = Some(4);
                state.granted = true;
                state.env.extend([
                    ("OPENAI_API_KEY".into(), "chat-secret".into()),
                    ("HOME".into(), "/home/me".into()),
                    ("YINZ_CONFIG_FILE".into(), "/private/config.kdl".into()),
                    ("YINZ_CONFIG_DIR".into(), "/private/config-dir".into()),
                ]);
                state.kdl.insert("yinz_path".into(), "~/bin/yinz".into());
                state.reconfigure();
                state.pump_tools();
                let context = review_context(&state);
                let Status::ReviewingTool {
                    body,
                    execution_config,
                    ..
                } = &state.status
                else {
                    unreachable!()
                };
                let (executable, env) = execution_config.as_ref().unwrap();
                assert_eq!(executable, "/home/me/bin/yinz");
                assert_eq!(
                    env,
                    &BTreeMap::from([
                        ("YINZ_CONFIG_FILE".into(), "/private/config.kdl".into()),
                        ("YINZ_CONFIG_DIR".into(), "/private/config-dir".into()),
                    ])
                );
                let serialized = serde_json::to_string(&state.status).unwrap();
                for private in [
                    "chat-secret",
                    "/home/me/bin/yinz",
                    "/private/config.kdl",
                    "/private/config-dir",
                ] {
                    assert!(!String::from_utf8_lossy(body).contains(private));
                    assert!(!serialized.contains(private));
                }
                assert!(!serialized.contains("execution_config"));
                assert!(matches!(
                    serde_json::from_str::<Status>(&serialized).unwrap(),
                    Status::ReviewingTool {
                        execution_config: None,
                        ..
                    }
                ));
                if park {
                    state.new_chat();
                }
                let mut configuration = state.kdl.clone();
                match change {
                    "unchanged" => {},
                    "equivalent" => {
                        configuration.insert("yinz_path".into(), "/home/me/bin/yinz".into());
                    },
                    "yinz_path" => {
                        configuration.insert("yinz_path".into(), "/changed/yinz".into());
                    },
                    "invalid_config" => {
                        configuration.insert("api_key_env".into(), "MISSING_KEY".into());
                    },
                    "removed_file" => {
                        state.env.remove("YINZ_CONFIG_FILE");
                    },
                    "removed_dir" => {
                        state.env.remove("YINZ_CONFIG_DIR");
                    },
                    key => {
                        state.env.insert(key.into(), "/changed".into());
                    },
                }
                state.handle(Event::PluginConfigurationChanged(configuration));
                assert!(respond(&mut state, context.clone(), pass()));
                assert!(!respond(&mut state, context, pass()));
                let check = |reviewed: &mut State| {
                    assert_eq!(reviewed.pending.len(), 1);
                    assert!(tool_results(reviewed).is_empty());
                    if matches!(change, "unchanged" | "equivalent") {
                        assert!(matches!(reviewed.status, Status::AwaitingCommand));
                        let context = BTreeMap::from([("tool_call_id".into(), "call-1".into())]);
                        assert!(reviewed.on_command_result(Some(0), b"", b"", &context));
                        assert!(reviewed.pending.is_empty());
                        assert_eq!(tool_results(reviewed).len(), 1);
                    } else {
                        assert!(
                            matches!(reviewed.status, Status::ConfirmingTool),
                            "{tool}, {change}, park={park}"
                        );
                        assert!(reviewed.confirmation_prompt().is_some());
                    }
                };
                if park {
                    state.with_parked(0, check);
                } else {
                    check(&mut state);
                }
            }
        }
    }
}

#[test]
fn execution_reviews_require_valid_configuration_at_start() {
    for tool in ["slack_post", "vm_sync", "vm_close"] {
        let mut state = review_state_for(
            tool,
            r#"{"channel":"C123","text":"Done.","vm":"review"}"#,
            "Post Done to C123; sync and close the review VM.",
        );
        state.cfg = Config::resolve(&BTreeMap::new(), &BTreeMap::new());
        state.pump_tools();
        assert!(matches!(state.status, Status::ConfirmingTool));
        assert!(tool_results(&state).is_empty());
    }
}

#[test]
fn y_and_n_can_answer_while_jev_is_reviewing() {
    for key in ['y', 'n'] {
        let mut state = review_state();
        state.pump_tools();
        let context = review_context(&state);
        assert!(state.is_confirming());
        assert!(state.on_key(KeyWithModifier::new(BareKey::Char(key))));
        assert!(state.pending.is_empty());
        assert_eq!(
            tool_results(&state)[0].content.contains("declined"),
            key == 'n'
        );
        assert!(!respond(&mut state, context, pass()));
    }
}

#[test]
fn uncertain_dangerous_or_invalid_reviews_keep_manual_confirmation() {
    let mut wrong_type: serde_json::Value = serde_json::from_slice(&pass()).unwrap();
    wrong_type["answers"]["requested"] = json!({"type": "score", "score": 0.0});
    let mut missing: serde_json::Value = serde_json::from_slice(&pass()).unwrap();
    missing["answers"]
        .as_object_mut()
        .unwrap()
        .remove("destructive");
    for (status, body) in [
        (200, answer(0.749, 0.0)),
        (200, answer(1.0, 0.251)),
        (200, answer(-0.1, 0.0)),
        (200, answer(1.0, 1.1)),
        (200, answer(f64::NAN, 0.0)),
        (200, answer(1.0, f64::INFINITY)),
        (200, wrong_type.to_string().into_bytes()),
        (200, missing.to_string().into_bytes()),
        (200, b"not json".to_vec()),
        (200, b"{}".to_vec()),
        (401, pass()),
        (503, pass()),
    ] {
        let mut state = review_state();
        state.pump_tools();
        let context = review_context(&state);
        assert!(state.on_response(status, &body, &context));
        assert!(matches!(state.status, Status::ConfirmingTool));
        assert!(state.confirmation_prompt().is_some());
        assert_eq!(state.pending.len(), 1);
        assert!(tool_results(&state).is_empty());
        assert!(!respond(&mut state, context, pass()));
    }
}

#[test]
fn review_responses_cannot_override_a_manual_answer_or_interrupt() {
    for resolve in [
        State::confirm_tool as fn(&mut State) -> bool,
        State::decline_tool,
        State::abort_tools,
    ] {
        let mut state = review_state();
        state.pump_tools();
        let context = review_context(&state);
        assert!(!respond(
            &mut state,
            BTreeMap::from([(REVIEW_CONTEXT.into(), "wrong-review".into())]),
            pass(),
        ));
        assert!(tool_results(&state).is_empty());
        resolve(&mut state);
        let messages = state.messages.clone();
        assert!(!respond(&mut state, context, pass()));
        assert_eq!(state.messages, messages);
        assert!(state.pending.is_empty());
    }
}

#[test]
fn a_changed_call_conversation_or_project_cannot_use_an_older_review() {
    for change in 0..4 {
        let mut state = review_state();
        state.pump_tools();
        let context = review_context(&state);
        match change {
            0 => state.pending.front_mut().unwrap().arguments = r#"{"pane_id":9}"#.into(),
            1 => state.messages.push(Message::user("Leave that pane alone.")),
            2 => {
                state
                    .input
                    .set_text("Actually, leave that pane alone.".into());
                state.submit();
                assert_eq!(state.asks.len(), 1);
            },
            _ => {
                state.projects.insert(
                    "other".into(),
                    projects::validate("other", "/work/other").unwrap(),
                );
                state.active_project = Some("other".into());
            },
        }
        assert!(respond(&mut state, context, pass()));
        assert!(matches!(state.status, Status::ConfirmingTool));
        assert!(tool_results(&state).is_empty());
    }
}

#[test]
fn each_call_in_a_batch_needs_its_own_review() {
    let mut state = review_state();
    let mut second = state.pending[0].clone();
    second.id = "call-2".into();
    state
        .messages
        .last_mut()
        .unwrap()
        .tool_calls
        .push(second.clone());
    state.pending.push_back(second);
    state.pump_tools();
    let first = review_context(&state);
    assert!(respond(&mut state, first.clone(), pass()));
    let second = review_context(&state);
    assert_ne!(first, second);
    assert_eq!(tool_results(&state).len(), 1);
    assert!(!respond(&mut state, first, pass()));
    assert_eq!(state.pending.len(), 1);
    assert!(respond(&mut state, second, pass()));
    assert_eq!(tool_results(&state).len(), 2);
}

#[test]
fn a_review_follows_its_chat_even_when_tool_call_ids_collide() {
    let mut state = review_state();
    state.chat_id = Some(4);
    state.pump_tools();
    let parked_context = review_context(&state);
    state.new_chat();
    let fresh = review_state();
    state.messages = fresh.messages;
    state.pending = fresh.pending;
    state.pump_tools();
    let foreground_context = review_context(&state);
    assert_ne!(parked_context, foreground_context);
    assert!(respond(&mut state, parked_context, pass()));
    assert!(state.parked[0].pending.is_empty());
    assert_eq!(state.pending.len(), 1);
    assert!(tool_results(&state).is_empty());
    assert!(respond(&mut state, foreground_context, pass()));
    assert!(state.pending.is_empty());
}

#[test]
fn expired_or_restored_reviews_require_confirmation_with_or_without_a_timer() {
    for park in [false, true] {
        for poll in [false, true] {
            for restored in [false, true] {
                let mut state = review_state();
                state.chat_id = Some(4);
                state.pump_tools();
                let context = review_context(&state);
                if restored {
                    let encoded = serde_json::to_vec(&state.status).unwrap();
                    state.status = serde_json::from_slice(&encoded).unwrap();
                    assert!(matches!(
                        state.status,
                        Status::ReviewingTool { deadline: None, .. }
                    ));
                } else if let Status::ReviewingTool { deadline, .. } = &mut state.status {
                    *deadline = Some(Instant::now() - Duration::from_secs(1));
                }
                if park {
                    state.new_chat();
                }
                if poll {
                    // A delayed event still carries only its requested interval.
                    state.on_timer(FRAME_SECONDS);
                }
                assert_eq!(respond(&mut state, context.clone(), pass()), !poll);
                let check = |reviewed: &mut State| {
                    assert!(matches!(reviewed.status, Status::ConfirmingTool));
                    assert_eq!(reviewed.pending.len(), 1);
                    assert!(tool_results(reviewed).is_empty());
                };
                if park {
                    state.with_parked(0, check);
                } else {
                    check(&mut state);
                }
                assert!(!respond(&mut state, context, pass()));
            }
        }
    }
}

#[test]
fn timer_intervals_do_not_determine_review_expiry() {
    for park in [false, true] {
        let mut state = review_state();
        state.chat_id = Some(4);
        state.pump_tools();
        let context = review_context(&state);
        if let Status::ReviewingTool { deadline, .. } = &mut state.status {
            *deadline = Some(Instant::now() + Duration::from_secs(60));
        }
        if park {
            state.new_chat();
        }
        state.on_timer(300.0);
        assert!(respond(&mut state, context, pass()));
        if park {
            state.with_parked(0, |reviewed| assert_eq!(tool_results(reviewed).len(), 1));
        } else {
            assert_eq!(tool_results(&state).len(), 1);
        }
    }
}

#[test]
fn disabling_auto_approval_during_review_keeps_manual_confirmation() {
    let mut state = review_state();
    state.pump_tools();
    let context = review_context(&state);
    let mut configuration = state.kdl.clone();
    configuration.insert("decisions_auto_approve".into(), "false".into());
    state.handle(Event::PluginConfigurationChanged(configuration));

    respond(&mut state, context, pass());

    assert!(matches!(state.status, Status::ConfirmingTool));
    assert_eq!(state.pending.len(), 1);
    assert!(tool_results(&state).is_empty());
}

#[test]
fn missing_credentials_or_disabled_reviews_keep_the_existing_prompt() {
    for disabled in [false, true] {
        let mut state = review_state();
        if disabled {
            state
                .kdl
                .insert("decisions_auto_approve".into(), "false".into());
        } else {
            state.env.clear();
        }
        state.pump_tools();
        assert!(matches!(state.status, Status::ConfirmingTool));
        assert!(tool_results(&state).is_empty());
    }
}

#[test]
fn review_does_not_change_planning_or_scheduled_authority() {
    let mut state = review_state();
    state.plan = Plan::Planning;
    state.pump_tools();
    assert!(state.pending.is_empty());
    assert!(tool_results(&state)[0].content.contains("planning mode"));

    let mut state = review_state();
    state.pump_tools();
    let context = review_context(&state);
    state.start_plan("");
    assert!(state.pending.is_empty());
    assert!(state.is_planning());
    assert!(!respond(&mut state, context, pass()));

    for auto_confirm in [false, true] {
        let mut state = review_state();
        state.scheduled_run = Some(ScheduledRun {
            job_id: 1,
            auto_confirm,
        });
        state.pump_tools();
        assert!(state.pending.is_empty());
        assert_eq!(
            tool_results(&state)[0].content.contains("declined"),
            !auto_confirm
        );
    }
}

#[test]
fn a_review_says_what_jev_answered() {
    let mut state = review_state();
    state.pump_tools();
    let context = review_context(&state);
    assert!(respond(&mut state, context, answer(0.62, 0.03)));
    assert!(matches!(state.status, Status::ConfirmingTool));
    let notice = notices(&state).pop().unwrap();
    assert!(
        notice.contains("requested 0.62 (needs at least 0.75)"),
        "{notice}"
    );
    assert!(notice.contains("destructive 0.03"), "{notice}");
    assert!(notice.ends_with("asking you"), "{notice}");
    assert!(state.pending.len() == 1);
    assert!(tool_results(&state).is_empty());

    let mut state = review_state();
    state.pump_tools();
    let context = review_context(&state);
    assert!(respond(&mut state, context, pass()));
    let notice = notices(&state).pop().unwrap();
    assert!(
        notice.contains("requested 1.00 (needs at least 0.75)"),
        "{notice}"
    );
    assert!(notice.contains("destructive 0.00"), "{notice}");
    assert!(notice.ends_with("auto-approved clear_pane"), "{notice}");
    assert_eq!(tool_results(&state).len(), 1);

    let mut state = review_state();
    state.pump_tools();
    let context = review_context(&state);
    assert!(state.on_response(503, b"down", &context));
    assert!(notices(&state)
        .pop()
        .unwrap()
        .starts_with("Jev review failed"));

    let mut state = review_state();
    state.pump_tools();
    let context = review_context(&state);
    state.messages.push(Message::user("Leave that pane alone."));
    assert!(respond(&mut state, context, pass()));
    assert!(notices(&state).pop().unwrap().contains("no longer applies"));
}

#[test]
fn an_unavailable_review_says_so_except_when_never_configured() {
    let mut state = review_state_for(
        "agent_spawn",
        r#"{"agent":"codex","cwd":"/work/hocus","prompt":"Fix","permission_mode":"skip"}"#,
        "Run Codex on the parser with permission checks off.",
    );
    state.pump_tools();
    assert!(matches!(state.status, Status::ConfirmingTool));
    let notice = notices(&state).pop().unwrap();
    assert!(
        notice.starts_with("Jev cannot review this call"),
        "{notice}"
    );
    assert!(notice.contains("permission checks off"), "{notice}");
    assert!(tool_results(&state).is_empty());

    let mut state = review_state();
    state.pump_tools();
    if let Status::ReviewingTool { deadline, .. } = &mut state.status {
        *deadline = Some(Instant::now() - Duration::from_secs(1));
    }
    state.on_timer(FRAME_SECONDS);
    assert!(matches!(state.status, Status::ConfirmingTool));
    assert!(notices(&state)
        .pop()
        .unwrap()
        .contains("did not answer in time"));

    let mut state = review_state();
    state.env.clear();
    state.pump_tools();
    assert!(matches!(state.status, Status::ConfirmingTool));
    assert!(notices(&state).is_empty());
}

#[test]
fn kdl_thresholds_can_only_tighten_the_gate() {
    // (min_requested, max_hazard) as configured, and whether an answer of
    // requested 0.8 with every hazard at 0.2 - which clears the default - runs.
    for (configured, runs) in [
        (None, true),
        (Some(("0.9", "0.1")), false),
        (Some(("0.5", "0.5")), true),
    ] {
        let mut state = review_state();
        if let Some((min_requested, max_hazard)) = configured {
            state
                .kdl
                .insert("decisions_min_requested".into(), min_requested.into());
            state
                .kdl
                .insert("decisions_max_hazard".into(), max_hazard.into());
        }
        state.pump_tools();
        let context = review_context(&state);
        assert!(respond(&mut state, context, answer(0.8, 0.2)));
        assert_eq!(state.pending.is_empty(), runs, "{configured:?}");
        assert_eq!(tool_results(&state).len(), usize::from(runs));
        if !runs {
            assert!(notices(&state)
                .pop()
                .unwrap()
                .contains("needs at least 0.90"));
        }
    }
}

#[test]
fn a_gate_tightened_during_review_keeps_manual_confirmation() {
    let mut state = review_state();
    state.pump_tools();
    let context = review_context(&state);
    state
        .kdl
        .insert("decisions_min_requested".into(), "0.9".into());
    // Clears the new gate too: the review was started under another one.
    assert!(respond(&mut state, context, answer(0.95, 0.0)));
    assert!(matches!(state.status, Status::ConfirmingTool));
    assert!(tool_results(&state).is_empty());
    assert!(notices(&state).pop().unwrap().contains("no longer applies"));
}

#[test]
fn agent_launch_reviews_bind_the_project_launch_defaults() {
    for change in ["none", "worktree_dir", "firecracker_image"] {
        let mut state = review_state_for(
            "agent_spawn",
            &json!({"agent": "codex", "project": "hocus", "prompt": "Fix the parser"}).to_string(),
            "Fix the parser in Hocus.",
        );
        state.projects.insert(
            "hocus".into(),
            projects::validate("hocus", "/work/hocus").unwrap(),
        );
        state.pump_tools();
        let context = review_context(&state);
        let project = state.projects.get_mut("hocus").unwrap();
        match change {
            "worktree_dir" => project.worktree_dir = Some("/scratch".into()),
            "firecracker_image" => project.firecracker_image = Some("alpine".into()),
            _ => {},
        }
        let changed = change != "none";
        assert!(respond(&mut state, context, pass()));
        assert_eq!(
            matches!(state.status, Status::ConfirmingTool),
            changed,
            "{change}"
        );
        tools::test_seams::SPAWN_AGENT_CALLS
            .with_borrow(|calls| assert_eq!(calls.len(), usize::from(!changed), "{change}"));
    }
}

#[test]
fn extension_reviews_label_the_extension_and_bind_its_whole_registration() {
    const PATH: &str = "/plugins/hocus/weather.ts";
    let parameters = json!({"type": "object", "properties": {"id": {"type": "string"}}});
    for change in ["none", "path", "parameters"] {
        // Arguments no built-in tool could parse (`id` is a number there, `text`
        // a string, `seconds` a number): an extension's are its own.
        let mut state = review_state_for(
            "weather_forecast",
            r#"{"id":"abc","text":{"a":1},"seconds":"5"}"#,
            "Get the Chicago forecast.",
        );
        super::extensions::register_weather(
            &mut state,
            PATH,
            "confirm",
            Some("contacts the weather service"),
            parameters.clone(),
        );
        state.pump_tools();
        let context = review_context(&state);
        let Status::ReviewingTool { body, .. } = &state.status else {
            unreachable!();
        };
        let body: serde_json::Value = serde_json::from_slice(body).unwrap();
        let pending = &body["state"]["pending_call"];
        assert!(pending.get("purpose").is_none());
        assert!(pending.get("effect").is_none());
        assert_eq!(pending["extension"]["description"], "Read the forecast.");
        assert_eq!(
            pending["extension"]["claimed_effect"],
            "contacts the weather service"
        );
        assert!(!body.to_string().contains(PATH));
        match change {
            "path" => super::extensions::register_weather(
                &mut state,
                "/plugins/hocus/weather-2.ts",
                "confirm",
                Some("contacts the weather service"),
                parameters.clone(),
            ),
            "parameters" => super::extensions::register_weather(
                &mut state,
                PATH,
                "confirm",
                Some("contacts the weather service"),
                json!({"type": "object", "properties": {"id": {"type": "integer"}}}),
            ),
            _ => {},
        }
        assert!(respond(&mut state, context, pass()));
        assert_eq!(
            matches!(state.status, Status::ConfirmingTool),
            change != "none",
            "{change}"
        );
    }
}
