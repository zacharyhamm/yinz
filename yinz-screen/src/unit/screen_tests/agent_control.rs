use super::*;
use crate::screen::thread_main::apply_queued_agent_report;
use std::time::{Duration, Instant};
use yinz_types::agent::control::{AgentOutcome, AgentRequest, AgentResponse, NotReadyReason};
use yinz_types::agent::report::{AgentReport, AgentReportState};
use yinz_types::agent::{hook_outcome, AgentEvent, AgentState, HookEvent, HookOutcome};
use yinz_types::command::PaneTarget;
use yinz_types::internal_ids::PluginId;
use yinz_types::keys::{BareKey, KeyWithModifier};

const PANE: PaneId = PaneId::Terminal(TerminalId::new(7));
const CLIENT: ClientId = ClientId::new(1);

fn setup(
    command: &str,
) -> (
    Screen,
    CapturedClientMessages,
    Receiver<(PtyWriteInstruction, ErrorContext)>,
) {
    setup_on_target(command, PaneTarget::Local)
}

fn setup_on_target(
    command: &str,
    target: PaneTarget,
) -> (
    Screen,
    CapturedClientMessages,
    Receiver<(PtyWriteInstruction, ErrorContext)>,
) {
    setup_with_size(command, target, Size { cols: 80, rows: 24 })
}

fn setup_with_size(
    command: &str,
    target: PaneTarget,
    size: Size,
) -> (
    Screen,
    CapturedClientMessages,
    Receiver<(PtyWriteInstruction, ErrorContext)>,
) {
    setup_with_placement(command, target, size, NewPanePlacement::default())
}

fn setup_with_placement(
    command: &str,
    target: PaneTarget,
    size: Size,
    placement: NewPanePlacement,
) -> (
    Screen,
    CapturedClientMessages,
    Receiver<(PtyWriteInstruction, ErrorContext)>,
) {
    setup_with_pane_limit(command, target, size, placement, None)
}

fn setup_with_pane_limit(
    command: &str,
    target: PaneTarget,
    size: Size,
    placement: NewPanePlacement,
    max_panes: Option<usize>,
) -> (
    Screen,
    CapturedClientMessages,
    Receiver<(PtyWriteInstruction, ErrorContext)>,
) {
    let mut screen = create_non_mirrored_screen(size);
    screen.max_panes = max_panes;
    let os = FakeInputOutput::default();
    let messages = os.server_to_client_messages.clone();
    screen.bus.os_input = Some(Box::new(os));
    let (to_writer, writes) = channels::unbounded();
    screen.bus.senders.to_pty_writer = Some(SenderWithContext::new(to_writer));
    new_tab(&mut screen, 1, TabId::new(0));
    let tab = screen.tabs.get_mut(&TabId::new(0)).unwrap();
    tab.new_pane(
        PANE,
        Some("codex".into()),
        (!command.is_empty()).then(|| {
            Run::Command(RunCommand {
                command: PathBuf::from("env"),
                args: vec![
                    format!("{}=1", yinz_types::agent::AGENT_PANE_ENV),
                    command.into(),
                ],
                cwd: Some(PathBuf::from("/work")),
                target,
                ..Default::default()
            })
        }),
        false,
        true,
        placement,
        Some(CLIENT),
        None,
    )
    .unwrap();
    tab.handle_pty_bytes(TerminalId::new(7), b"\x1b[?2004h".to_vec())
        .unwrap();
    if max_panes.is_some() {
        screen.close_pane_explicitly(PaneId::Terminal(TerminalId::new(1)), None);
    }
    (screen, messages, writes)
}

fn pane_targets() -> [PaneTarget; 2] {
    [PaneTarget::Local, PaneTarget::ssh("test-host").unwrap()]
}

fn replies(messages: &CapturedClientMessages, client: ClientId) -> Vec<AgentResponse> {
    messages
        .lock()
        .unwrap()
        .remove(&client)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|message| match message {
            ServerToClientMsg::AgentControl(response) => Some(response),
            _ => None,
        })
        .collect()
}

#[test]
fn bubblewrap_support_probe_confirms_the_exact_token() {
    let (mut screen, messages, _writes) = setup("bash");
    screen.agent_control(AgentRequest::SpawnBubblewrapSupport, CLIENT);
    let response = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(response.outcome, AgentOutcome::Ok);
    assert_eq!(response.output.as_deref(), Some("agent-bubblewrap-v1"));
}

fn hook(name: &str) -> HookEvent {
    HookEvent {
        hook_event_name: name.into(),
        session_id: "conversation".into(),
        turn_id: Some("turn".into()),
        prompt_id: Some("prompt".into()),
        claude_turn_complete: (name == "Stop").then_some(true),
        agy_turn_complete: (name == "Stop").then_some(true),
        source: (name == "SessionStart").then(|| "startup".into()),
        prompt: (name == "UserPromptSubmit").then(|| "review".into()),
        ..Default::default()
    }
}

fn permission_wait(kind: &str) -> HookEvent {
    if kind == "claude" {
        HookEvent {
            notification_type: Some("elicitation_dialog".into()),
            ..hook("Notification")
        }
    } else {
        hook("PermissionRequest")
    }
}

fn report(screen: &mut Screen, hook: HookEvent) {
    report_kind(screen, "codex", hook);
}

fn report_kind(screen: &mut Screen, kind: &str, hook: HookEvent) {
    let state = match hook_outcome(kind, &hook) {
        HookOutcome::Ended => AgentReportState::Clear,
        HookOutcome::Reported(agent) => AgentReportState::Set(agent),
        HookOutcome::Nothing => AgentReportState::Unchanged,
    };
    report_state(screen, kind, hook, state);
}

fn report_state(screen: &mut Screen, kind: &str, hook: HookEvent, state: AgentReportState) {
    apply_queued_agent_report(
        screen,
        &AgentReport {
            report_id: "00000000-0000-4000-8000-000000000001".into(),
            pane_id: PANE,
            state,
            event: Some(AgentEvent::from_hook(
                kind,
                TerminalId::new(7),
                &hook,
                serde_json::to_string(&hook).unwrap(),
            )),
            trace: false,
            process_id: ProcessId::new(42),
            kind: kind.into(),
        },
        Instant::now(),
    )
    .unwrap();
    screen.refresh_agent_controls();
}

fn prompt(wait: bool) -> AgentRequest {
    AgentRequest::Prompt {
        preserve_draft: false,
        submission_id: None,
        pane_id: PANE,
        expected_session_id: None,
        prompt: "review".into(),
        wait,
        timeout_ms: 30_000,
    }
}

fn wait(until: AgentState) -> AgentRequest {
    AgentRequest::Wait {
        pane_id: PANE,
        expected_session_id: None,
        until,
        timeout_ms: 30_000,
    }
}

fn send_keys(keys: Vec<BareKey>) -> AgentRequest {
    AgentRequest::SendKeys {
        pane_id: PANE,
        expected_session_id: None,
        keys: keys.into_iter().map(KeyWithModifier::new).collect(),
    }
}

#[test]
fn readiness_reports_each_failed_gate_without_writing_input() {
    for (case, expected) in [
        ("unsupported", vec!["unsupported_kind"]),
        ("unreported", vec!["not_reported"]),
        ("missing", vec!["lifecycle_missing"]),
        ("mismatch", vec!["lifecycle_mismatch"]),
        ("suspended", vec!["suspended"]),
        ("sync", vec!["sync_excluded"]),
        ("shell", vec!["command_mismatch"]),
        ("unmanaged", vec!["unmanaged"]),
        ("held", vec!["held", "pane_exited", "agent_exited"]),
        ("paste", vec!["bracketed_paste_disabled"]),
        (
            "multiple",
            vec!["lifecycle_missing", "suspended", "bracketed_paste_disabled"],
        ),
    ] {
        let kind = if case == "unsupported" {
            "gemini"
        } else {
            "codex"
        };
        let (mut screen, messages, writes) = setup(match case {
            "shell" => "bash",
            "unmanaged" => "",
            _ => kind,
        });
        report_kind(&mut screen, kind, hook("SessionStart"));
        match case {
            "missing" | "multiple" => {
                screen.agent_lifecycle.clear();
            },
            "mismatch" | "unreported" => {
                // Deliberately bypass invalidation to exercise the diagnostic distinction.
                let tab = screen.tab_containing_pane_mut(PANE).unwrap();
                let mut agent = tab.pane_agent_info(PANE).unwrap();
                if case == "mismatch" {
                    agent.session_id = Some("different-conversation".into());
                } else {
                    agent.source = yinz_types::agent::AgentSource::Spawned;
                }
                tab.set_pane_agent(PANE, Some(agent));
            },
            _ => {},
        }
        let tab = screen.tab_containing_pane_mut(PANE).unwrap();
        if case == "held" {
            tab.hold_pane(PANE, Some(0), false, tab.pane_run_command(PANE).unwrap());
        }
        if matches!(case, "suspended" | "multiple") {
            tab.get_pane_with_id_mut(PANE).unwrap().set_suspended(true);
        }
        if case == "sync" {
            tab.toggle_sync_panes_is_active();
            tab.get_pane_with_id_mut(PANE)
                .unwrap()
                .set_exclude_from_sync(true);
        }
        if matches!(case, "paste" | "multiple") {
            tab.handle_pty_bytes(TerminalId::new(7), b"\x1b[?2004l".to_vec())
                .unwrap();
        }
        writes.try_iter().for_each(drop);
        screen.agent_control_with_clear(prompt(false), CLIENT, &writes);
        let response = replies(&messages, CLIENT).pop().unwrap();
        let status = &response.agents[0];
        let readiness = status.readiness.as_ref().unwrap();
        assert_eq!(readiness.failures(), expected, "{case}");
        assert_eq!(
            readiness.status_available(),
            status.status_available,
            "{case}"
        );
        assert_eq!(
            readiness.prompt_supported(),
            status.prompt_supported,
            "{case}"
        );
        assert_eq!(response.outcome, AgentOutcome::StatusUnavailable, "{case}");
        for reason in expected {
            assert!(
                response.message.as_ref().unwrap().contains(reason),
                "{case}: {response:?}"
            );
        }
        assert!(!status.ready);
        let encoded = serde_json::to_value(status).unwrap();
        assert_eq!(
            encoded["readiness"]["bracketed_paste"],
            readiness.bracketed_paste
        );
        assert_eq!(
            serde_json::from_value::<yinz_types::agent::control::AgentStatus>(encoded).unwrap(),
            *status
        );
        assert!(
            writes
                .try_iter()
                .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))),
            "{case}"
        );
    }
}

#[test]
fn missing_lifecycle_requires_a_parent_hook_and_preserves_unknown_input() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    assert!(screen.agent_status_for_test().ready);
    // A server restart loses this map; the restored pane record is not authority.
    screen.agent_lifecycle.clear();
    let status = screen.agent_status_for_test();
    assert_eq!(
        status.readiness.unwrap().failures(),
        vec!["lifecycle_missing"]
    );
    for event in [
        HookEvent {
            session_id: String::new(),
            ..hook("Stop")
        },
        HookEvent {
            agent_id: Some("worker".into()),
            ..hook("Stop")
        },
    ] {
        report(&mut screen, event);
        assert!(!screen.agent_status_for_test().status_available);
    }
    report(&mut screen, hook("Stop"));
    let status = screen.agent_status_for_test();
    assert!(status.status_available && status.prompt_supported);
    assert_eq!(
        status.not_ready_reason,
        Some(NotReadyReason::UnconsumedInput)
    );
    assert!(status.readiness.unwrap().failures().is_empty());
    writes.try_iter().for_each(drop);
    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    assert_eq!(
        replies(&messages, CLIENT).pop().unwrap().outcome,
        AgentOutcome::Ok
    );
    assert!(writes
        .try_iter()
        .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))));
}

#[test]
fn paste_mode_only_gates_prompts_and_sync_exclusion_requires_active_sync() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    let tab = screen.tab_containing_pane_mut(PANE).unwrap();
    tab.get_pane_with_id_mut(PANE)
        .unwrap()
        .set_exclude_from_sync(true);
    assert!(screen.agent_status_for_test().ready);
    screen
        .tab_containing_pane_mut(PANE)
        .unwrap()
        .handle_pty_bytes(TerminalId::new(7), b"\x1b[?2004l".to_vec())
        .unwrap();
    assert_eq!(
        screen.agent_status_for_test().readiness.unwrap().failures(),
        vec!["bracketed_paste_disabled"]
    );
    screen.agent_control(send_keys(vec![BareKey::Esc]), CLIENT);
    assert_eq!(
        replies(&messages, CLIENT).pop().unwrap().outcome,
        AgentOutcome::Ok
    );
}

#[test]
fn hand_started_agent_width_tracks_foreground_command_without_lifecycle_changes() {
    use crate::screen::thread_main::apply_pane_foreground_command;
    let (mut screen, _, _) = setup_with_size(
        "",
        PaneTarget::Local,
        Size {
            cols: 201,
            rows: 24,
        },
    );
    screen.tabs.get_mut(&TabId::new(0)).unwrap().max_agent_width = 80;
    screen.render_to_clients().unwrap();
    let original = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PANE)
        .unwrap()
        .render_geom();
    assert!(apply_pane_foreground_command(
        &mut screen,
        TerminalId::new(7),
        vec!["/usr/local/bin/codex".into()],
        Some("codex".into())
    ));
    screen.render_to_clients().unwrap();
    let pane = screen.tabs[&TabId::new(0)].get_pane_with_id(PANE).unwrap();
    assert_eq!(pane.get_content_columns(), 80);
    assert!(pane.agent_info().is_none());
    assert!(!pane.was_an_agent());
    assert!(!screen.agent_lifecycle.contains_key(&PANE));
    assert!(apply_pane_foreground_command(
        &mut screen,
        TerminalId::new(7),
        vec!["/bin/bash".into()],
        Some("bash".into())
    ));
    screen.render_to_clients().unwrap();
    assert_eq!(
        screen.tabs[&TabId::new(0)]
            .get_pane_with_id(PANE)
            .unwrap()
            .render_geom(),
        original
    );
}

#[test]
fn list_names_a_hand_started_agent_in_a_shell_as_detected_and_unmanaged() {
    let (mut screen, messages, _writes) = setup("bash");
    let list = |screen: &mut Screen| {
        screen.agent_control(AgentRequest::List { project: None }, CLIENT);
        replies(&messages, CLIENT).pop().unwrap()
    };
    assert!(list(&mut screen).agents.is_empty());
    screen.terminal_foreground_cmds.insert(
        TerminalId::new(7),
        vec![
            "/usr/local/bin/codex".into(),
            "--model".into(),
            "luna".into(),
        ],
    );
    let listed = list(&mut screen);
    assert_eq!(listed.agents.len(), 1);
    let status = &listed.agents[0];
    assert_eq!(status.pane_id, PANE);
    assert_eq!(status.agent.kind, "codex");
    assert_eq!(
        status.agent.source,
        yinz_types::agent::AgentSource::Detected
    );
    assert_eq!(status.not_ready_reason, Some(NotReadyReason::Unmanaged));
    assert!(!status.ready && !status.status_available && !status.prompt_supported);
    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let refused = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(refused.outcome, AgentOutcome::NotAgent);
    assert!(refused.message.unwrap().contains("did not launch"));
    // A reported record wins, and is listed once.
    report(&mut screen, hook("SessionStart"));
    let listed = list(&mut screen);
    assert_eq!(listed.agents.len(), 1);
    assert_eq!(
        listed.agents[0].agent.source,
        yinz_types::agent::AgentSource::Reported
    );
    // Something else in the foreground is not an agent.
    report(&mut screen, hook("SessionEnd"));
    screen
        .terminal_foreground_cmds
        .insert(TerminalId::new(7), vec!["vim".into(), "codex".into()]);
    assert!(list(&mut screen).agents.is_empty());
}

#[test]
fn initial_status_distinguishes_missing_non_agent_and_exited_targets() {
    for target in pane_targets() {
        for case in ["missing", "shell", "cleared-exit", "retained-exit"] {
            let (mut screen, messages, writes) = setup_on_target(
                if case == "shell" { "bash" } else { "codex" },
                target.clone(),
            );
            let (expected, message) = match case {
                "missing" => {
                    screen.close_pane_explicitly(PANE, None);
                    (AgentOutcome::TargetNotFound, "target pane does not exist")
                },
                "shell" => (AgentOutcome::NotAgent, "target does not contain an agent"),
                _ => {
                    report(&mut screen, hook("SessionStart"));
                    if case == "cleared-exit" {
                        report(&mut screen, hook("SessionEnd"));
                    }
                    let tab = screen.tab_containing_pane_mut(PANE).unwrap();
                    tab.hold_pane(PANE, Some(0), false, tab.pane_run_command(PANE).unwrap());
                    (AgentOutcome::TargetExited, "agent process has exited")
                },
            };
            for conversation in [None, Some("conversation"), Some("other")] {
                screen.agent_control(
                    AgentRequest::Status {
                        pane_id: PANE,
                        expected_session_id: conversation.map(str::to_owned),
                    },
                    CLIENT,
                );
                let response = replies(&messages, CLIENT).pop().unwrap();
                let matching_pin = case == "retained-exit" && conversation == Some("conversation");
                assert_eq!(
                    response.outcome,
                    if conversation.is_none() || matching_pin {
                        expected
                    } else if case == "missing" {
                        AgentOutcome::TargetClosed
                    } else {
                        AgentOutcome::TargetChanged
                    },
                    "{target:?}, {case}, {conversation:?}"
                );
                if conversation.is_none() {
                    assert_eq!(response.message.as_deref(), Some(message));
                }
                if case == "retained-exit" {
                    assert_eq!(response.agents[0].agent.state, AgentState::Exited);
                    assert!(!response.agents[0].ready);
                    assert!(!response.agents[0].prompt_supported);
                } else {
                    assert!(response.agents.is_empty());
                }
            }
            assert!(screen.agent_controls.is_empty());
            assert!(writes
                .try_iter()
                .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))));
        }
    }
}

#[test]
fn an_agent_held_before_its_first_run_has_not_exited() {
    let (mut screen, messages, _writes) = setup("codex");
    let tab = screen.tab_containing_pane_mut(PANE).unwrap();
    tab.hold_pane(PANE, None, true, tab.pane_run_command(PANE).unwrap());
    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let response = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(response.outcome, AgentOutcome::StatusUnavailable);
    assert!(response.message.is_none());
    assert!(!response.agents[0].ready);
}

#[test]
fn startup_dialog_keys_are_consumed_only_by_verified_startup() {
    for kind in ["claude", "codex", "pi"] {
        for split in [false, true] {
            let (mut screen, messages, writes) = setup(kind);
            screen
                .tab_containing_pane_mut(PANE)
                .unwrap()
                .handle_pty_bytes(TerminalId::new(7), b"\x1b[?2004l".to_vec())
                .unwrap();
            screen.agent_control(wait(AgentState::Idle), CLIENT);
            let key = if kind == "codex" {
                BareKey::Char('y')
            } else {
                BareKey::Down
            };
            let batches = if split {
                vec![vec![key], vec![BareKey::Enter]]
            } else {
                vec![vec![key, BareKey::Enter]]
            };
            for keys in batches {
                screen.agent_control(send_keys(keys), ClientId::new(2));
                let result = replies(&messages, ClientId::new(2)).pop().unwrap();
                assert_eq!(result.outcome, AgentOutcome::Ok, "{kind}");
                assert!(!result.agents[0].status_available);
                assert!(!result.agents[0].ready);
            }
            assert_eq!(
                finish_paste(&writes),
                if kind == "codex" {
                    &b"y\r"[..]
                } else {
                    &b"\x1b[B\r"[..]
                }
            );
            assert!(replies(&messages, CLIENT).is_empty());
            report_kind(&mut screen, kind, hook("SessionStart"));
            assert!(screen.agent_dialog_inputs.is_empty());
            assert!(replies(&messages, CLIENT).is_empty());
            screen
                .tab_containing_pane_mut(PANE)
                .unwrap()
                .handle_pty_bytes(TerminalId::new(7), b"\x1b[?2004h".to_vec())
                .unwrap();
            screen.refresh_agent_controls();
            let result = replies(&messages, CLIENT).pop().unwrap();
            assert_eq!(result.outcome, AgentOutcome::Ok, "{kind}, {split}");
            assert!(result.agents[0].ready);
            screen.agent_control_with_clear(prompt(false), CLIENT, &writes);
            assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
        }
    }
}

#[test]
fn permission_keys_preserve_the_accepted_prompt_until_progress_and_completion() {
    for kind in ["claude", "codex", "pi"] {
        for split in [false, true] {
            for progress in [false, true] {
                let (mut screen, messages, writes) = setup(kind);
                report_kind(&mut screen, kind, hook("SessionStart"));
                screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
                finish_paste(&writes);
                report_kind(&mut screen, kind, hook("UserPromptSubmit"));
                report_kind(&mut screen, kind, permission_wait(kind));
                assert_eq!(
                    replies(&messages, CLIENT)[0].outcome,
                    AgentOutcome::NeedsInput
                );
                let batches = if split {
                    vec![vec![BareKey::Down], vec![BareKey::Enter]]
                } else {
                    vec![vec![BareKey::Down, BareKey::Enter]]
                };
                for keys in batches {
                    screen.agent_control(send_keys(keys), ClientId::new(2));
                    assert_eq!(
                        replies(&messages, ClientId::new(2))[0].outcome,
                        AgentOutcome::Ok
                    );
                    screen.refresh_agent_controls();
                    assert_eq!(screen.agent_controls.len(), 1);
                }
                assert_eq!(finish_paste(&writes), b"\x1b[B\r");
                screen.agent_control(wait(AgentState::Idle), CLIENT);
                assert!(replies(&messages, CLIENT).is_empty());
                assert_eq!(
                    screen.get_pane_info(PANE).unwrap().agent.unwrap().state,
                    AgentState::Waiting
                );
                if progress {
                    report_kind(&mut screen, kind, hook("PostToolUse"));
                    assert!(replies(&messages, CLIENT).is_empty());
                    assert!(screen.agent_dialog_inputs.is_empty());
                }
                report_kind(
                    &mut screen,
                    kind,
                    HookEvent {
                        last_assistant_message: Some("WROTE_OK".into()),
                        ..hook("Stop")
                    },
                );
                let result = replies(&messages, CLIENT).pop().unwrap();
                assert_eq!(
                    result.outcome,
                    AgentOutcome::Ok,
                    "{kind}, {split}, {progress}"
                );
                assert!(result.agents[0].ready);
                // The answered turn's final text reaches the wait, not only the run archive.
                assert_eq!(result.message.as_deref(), Some("WROTE_OK"));
                assert!(screen.agent_controls.is_empty());
                screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
                assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
                for name in ["UserPromptSubmit", "Stop"] {
                    report_kind(
                        &mut screen,
                        kind,
                        HookEvent {
                            prompt_id: Some("next-prompt".into()),
                            ..hook(name)
                        },
                    );
                }
                let completed = replies(&messages, CLIENT).pop().unwrap();
                assert_eq!(completed.outcome, AgentOutcome::Completed);
                assert!(completed.acknowledged);
                assert!(completed.agents[0].ready);
            }
        }
    }
}

#[test]
fn a_fresh_waiting_hook_requires_another_dialog_answer() {
    for kind in ["claude", "codex", "pi"] {
        let (mut screen, messages, writes) = setup(kind);
        report_kind(&mut screen, kind, hook("SessionStart"));
        screen.agent_control_with_clear(prompt(false), CLIENT, &writes);
        finish_paste(&writes);
        report_kind(&mut screen, kind, hook("UserPromptSubmit"));
        replies(&messages, CLIENT);
        report_kind(&mut screen, kind, permission_wait(kind));
        screen.agent_control(send_keys(vec![BareKey::Down]), CLIENT);
        assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
        screen.agent_control(wait(AgentState::Idle), CLIENT);
        assert!(replies(&messages, CLIENT).is_empty());
        report_kind(&mut screen, kind, permission_wait(kind));
        assert_eq!(
            replies(&messages, CLIENT)[0].outcome,
            AgentOutcome::NeedsInput
        );
        screen.agent_control(send_keys(vec![BareKey::Enter]), CLIENT);
        assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
        screen.agent_control(wait(AgentState::Idle), CLIENT);
        assert!(replies(&messages, CLIENT).is_empty());
        report_kind(&mut screen, kind, hook("Stop"));
        assert!(replies(&messages, CLIENT)[0].agents[0].ready);
        assert_eq!(finish_paste(&writes), b"\x1b[B\r");
    }
}

#[test]
fn completion_reply_accounts_for_permission_keys_before_reporting_readiness() {
    for kind in ["claude", "codex", "pi"] {
        let (mut screen, messages, writes) = setup(kind);
        report_kind(&mut screen, kind, hook("SessionStart"));
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        finish_paste(&writes);
        report_kind(&mut screen, kind, hook("UserPromptSubmit"));
        // A state-only update can expose the dialog before its hook finishes the prompt wait.
        let HookOutcome::Reported(waiting) = hook_outcome(kind, &permission_wait(kind)) else {
            panic!("human input prompt reports a waiting agent");
        };
        crate::screen::apply_pane_agent_report(&mut screen, PANE, Some(waiting)).unwrap();
        screen.agent_control(send_keys(vec![BareKey::Enter]), ClientId::new(2));
        assert_eq!(
            replies(&messages, ClientId::new(2))[0].outcome,
            AgentOutcome::Ok
        );
        report_kind(&mut screen, kind, hook("Stop"));
        let result = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(result.outcome, AgentOutcome::Completed);
        assert!(result.acknowledged);
        assert!(result.agents[0].ready);
    }
}

#[test]
fn worker_progress_cannot_consume_parent_dialog_keys() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    report(&mut screen, hook("PermissionRequest"));
    screen.agent_control(send_keys(vec![BareKey::Enter]), CLIENT);
    assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
    screen.agent_control(wait(AgentState::Idle), CLIENT);
    for name in ["PostToolUse", "Stop"] {
        let mut worker = hook(name);
        worker.agent_id = Some("worker".into());
        report(&mut screen, worker);
        assert!(screen.agent_dialog_inputs.contains_key(&PANE));
        assert!(replies(&messages, CLIENT).is_empty());
    }
    report(&mut screen, hook("Stop"));
    assert!(replies(&messages, CLIENT)[0].agents[0].ready);
}

#[test]
fn dialog_tracking_is_cleared_on_recovery_invalidation_and_closure() {
    for cleanup in ["recover", "invalidate", "close", "unverified", "restart"] {
        let (mut screen, messages, _writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        report(&mut screen, hook("PermissionRequest"));
        screen.agent_control(send_keys(vec![BareKey::Enter]), CLIENT);
        assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
        assert!(screen.agent_dialog_inputs.contains_key(&PANE));
        match cleanup {
            "recover" => {
                recover_after_quiet(
                    &mut screen,
                    AgentRequest::Recover {
                        pane_id: PANE,
                        expected_session_id: "conversation".into(),
                        confirm_empty: true,
                        interrupted: true,
                    },
                );
                assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
                screen.agent_write_for_test();
            },
            "invalidate" => screen.invalidate_agent_lifecycle(PANE, "test"),
            "close" => screen.close_pane_explicitly(PANE, None),
            "restart" => {
                report(&mut screen, hook("SessionStart"));
                report(&mut screen, hook("Stop"));
                screen.agent_control(
                    AgentRequest::Status {
                        pane_id: PANE,
                        expected_session_id: None,
                    },
                    CLIENT,
                );
                let result = replies(&messages, CLIENT).pop().unwrap();
                assert_eq!(result.outcome, AgentOutcome::Ok);
                assert!(!result.agents[0].ready);
            },
            "unverified" => {
                let mut start = hook("SessionStart");
                start.codex_hooks_ready = Some(false);
                report(&mut screen, start);
            },
            _ => unreachable!(),
        }
        assert!(screen.agent_dialog_inputs.is_empty(), "{cleanup}");
    }
}

#[test]
fn dialog_keys_cannot_consume_manual_input_or_a_failed_batch() {
    for kind in ["claude", "codex", "pi"] {
        for startup in [false, true] {
            for interference in [
                "before",
                "after",
                "between",
                "invalid",
                "partial",
                "replacement",
            ] {
                let (mut screen, messages, writes) = setup(kind);
                if !startup {
                    report_kind(&mut screen, kind, hook("SessionStart"));
                    report_kind(&mut screen, kind, permission_wait(kind));
                }
                if interference == "before" {
                    screen
                        .tab_containing_pane_mut(PANE)
                        .unwrap()
                        .write_key_to_pane_id(PANE, KeyWithModifier::new(BareKey::Char('x')), None)
                        .unwrap();
                }
                screen.agent_control(send_keys(vec![BareKey::Down]), CLIENT);
                assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
                if matches!(interference, "after" | "between") {
                    screen
                        .tab_containing_pane_mut(PANE)
                        .unwrap()
                        .write_key_to_pane_id(PANE, KeyWithModifier::new(BareKey::Char('x')), None)
                        .unwrap();
                }
                if interference == "between" {
                    screen.agent_control(send_keys(vec![BareKey::Enter]), CLIENT);
                    assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
                } else if matches!(interference, "invalid" | "partial") {
                    let keys = if interference == "partial" {
                        vec![BareKey::Enter, BareKey::F(0)]
                    } else {
                        vec![BareKey::F(0)]
                    };
                    screen.agent_control(send_keys(keys), CLIENT);
                    assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Failed);
                    assert!(screen.agent_dialog_inputs.is_empty());
                }
                if interference == "replacement" {
                    if startup {
                        screen.invalidate_agent_lifecycle(PANE, "test");
                    } else {
                        let mut replacement = hook("SessionStart");
                        replacement.session_id = "replacement".into();
                        report_kind(&mut screen, kind, replacement);
                    }
                    assert!(screen.agent_dialog_inputs.is_empty());
                }
                report_kind(
                    &mut screen,
                    kind,
                    hook(if startup { "SessionStart" } else { "Stop" }),
                );
                screen.agent_control(
                    AgentRequest::Status {
                        pane_id: PANE,
                        expected_session_id: None,
                    },
                    CLIENT,
                );
                let result = replies(&messages, CLIENT).pop().unwrap();
                assert_eq!(
                    result.outcome,
                    AgentOutcome::Ok,
                    "{kind}, {startup}, {interference}"
                );
                assert!(!result.agents[0].ready);
                finish_paste(&writes);
            }
        }
    }
}

#[test]
fn permission_keys_do_not_advance_an_unacknowledged_or_delivering_prompt() {
    for acknowledged in [false, true] {
        let (mut screen, messages, writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        if acknowledged {
            report(&mut screen, hook("UserPromptSubmit"));
        } else {
            finish_paste(&writes);
        }
        report(&mut screen, hook("PermissionRequest"));
        replies(&messages, CLIENT);
        screen.agent_control(send_keys(vec![BareKey::Enter]), ClientId::new(2));
        assert_eq!(
            replies(&messages, ClientId::new(2))[0].outcome,
            AgentOutcome::Ok
        );
        assert!(screen.agent_dialog_inputs.is_empty());
        finish_paste(&writes);
        report(&mut screen, hook("UserPromptSubmit"));
        report(&mut screen, hook("Stop"));
        screen.agent_control(
            AgentRequest::Status {
                pane_id: PANE,
                expected_session_id: None,
            },
            ClientId::new(2),
        );
        assert!(!replies(&messages, ClientId::new(2))[0].agents[0].ready);
    }
}

#[test]
fn respawn_restores_readiness_without_consuming_a_new_draft() {
    for kind in ["claude", "codex", "pi"] {
        for keep_scrollback in [false, true] {
            for replace in [false, true] {
                for draft in [false, true] {
                    let (mut screen, messages, writes) = setup(kind);
                    report_kind(&mut screen, kind, hook("SessionStart"));
                    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
                    report_kind(&mut screen, kind, hook("UserPromptSubmit"));
                    report_kind(&mut screen, kind, hook("Stop"));
                    assert_eq!(
                        replies(&messages, CLIENT)[0].outcome,
                        AgentOutcome::Completed
                    );
                    assert!(
                        screen
                            .tab_containing_pane(PANE)
                            .unwrap()
                            .agent_input_revision(PANE)
                            > 0
                    );
                    report_kind(&mut screen, kind, hook("SessionEnd"));

                    let tab = screen.tab_containing_pane_mut(PANE).unwrap();
                    let command = tab.pane_run_command(PANE).unwrap();
                    tab.hold_pane(PANE, Some(0), false, command.clone());
                    let pane = tab.get_pane_with_id_mut(PANE).unwrap();
                    pane.handle_pty_bytes(b"saved scrollback\x1b[?2004l".to_vec());
                    assert!(pane
                        .respawn(replace.then_some(command), keep_scrollback)
                        .is_some());
                    assert_eq!(pane.agent_input_revision(), 0);
                    assert_eq!(
                        pane.dump_screen(true, None).contains("saved scrollback"),
                        keep_scrollback
                    );
                    assert!(!pane.supports_bracketed_paste());
                    tab.handle_pty_bytes(TerminalId::new(7), b"\x1b[?2004h".to_vec())
                        .unwrap();
                    if draft {
                        tab.write_to_active_terminal(
                            &None,
                            b"new draft".to_vec(),
                            false,
                            CLIENT,
                            None,
                        )
                        .unwrap();
                    }
                    writes.try_iter().for_each(drop);
                    let mut startup = hook("SessionStart");
                    startup.session_id = "new-conversation".into();
                    report_kind(&mut screen, kind, startup);
                    screen.agent_control_with_clear(prompt(false), CLIENT, &writes);
                    assert!(replies(&messages, CLIENT).is_empty());
                    assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
                }
            }
        }
    }
}

#[test]
fn agent_read_limits_physical_lines_and_preserves_internal_spacing() {
    // v2-05-read-claude captured 26 blank rows before these four UI rows.
    let captured = "────\n❯\u{a0}Try \"refactor <filepath>\"\n────\n  ⏸ manual mode on · ? for shortcuts · ← for agents";
    let leading_blanks = format!("{}{captured}", "\n".repeat(26));
    let paragraphs = "\nold\n\n  first\n\u{2003}\n\n  second\n\nthird\u{a0}\n \t \n\u{2003}\n";
    for (input, lines, expected) in [
        (leading_blanks.as_str(), 30, captured),
        ("first\n\nsecond\n \t \n\u{2003}\n", 80, "first\n\nsecond"),
        (paragraphs, 1, "third\u{a0}"),
        (paragraphs, 2, "third\u{a0}"),
        (paragraphs, 3, "  second\n\nthird\u{a0}"),
        (paragraphs, 4, "  second\n\nthird\u{a0}"),
        (
            paragraphs,
            6,
            "  first\n\u{2003}\n\n  second\n\nthird\u{a0}",
        ),
        (
            paragraphs,
            80,
            "old\n\n  first\n\u{2003}\n\n  second\n\nthird\u{a0}",
        ),
        ("", 80, ""),
        ("\n \t \n\u{2003}\n\u{a0}\n", 1, ""),
    ] {
        let (mut screen, messages, _writes) = setup("codex");
        screen
            .tab_containing_pane_mut(PANE)
            .unwrap()
            .handle_pty_bytes(TerminalId::new(7), input.replace('\n', "\r\n").into_bytes())
            .unwrap();
        screen.agent_control(
            AgentRequest::Read {
                pane_id: PANE,
                expected_session_id: None,
                lines,
            },
            CLIENT,
        );
        let result = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(result.outcome, AgentOutcome::Ok);
        assert!(result.output.as_ref().unwrap().lines().count() <= lines);
        assert_eq!(
            result.output.as_deref(),
            Some(expected),
            "{lines}: {input:?}"
        );
    }
}

#[test]
fn agent_read_returns_the_latest_lines_even_when_scrolled_up() {
    let (mut screen, messages, _writes) = setup("codex");
    let lines: Vec<_> = (1..=60).map(|n| n.to_string()).collect();
    screen
        .tab_containing_pane_mut(PANE)
        .unwrap()
        .handle_pty_bytes(TerminalId::new(7), lines.join("\r\n").into_bytes())
        .unwrap();
    for scroll in [0, 20] {
        let pane = screen
            .tab_containing_pane_mut(PANE)
            .unwrap()
            .get_pane_with_id_mut(PANE)
            .unwrap();
        pane.scroll_up(scroll, Some(CLIENT));
        pane.start_selection(&Position::new(1, 0), CLIENT);
        pane.end_selection(&Position::new(2, 2), CLIENT);
        let selection = pane.get_selected_text(CLIENT);
        let viewport = pane.dump_screen(false, Some(CLIENT));
        screen.agent_control(
            AgentRequest::Read {
                pane_id: PANE,
                expected_session_id: None,
                lines: 10,
            },
            CLIENT,
        );
        let result = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(result.outcome, AgentOutcome::Ok);
        assert_eq!(result.output, Some(lines[50..].join("\n")));
        let pane = screen
            .tab_containing_pane(PANE)
            .unwrap()
            .get_pane_with_id(PANE)
            .unwrap();
        assert_eq!(pane.dump_screen(false, Some(CLIENT)), viewport);
        assert_eq!(pane.get_selected_text(CLIENT), selection);
    }
}

#[test]
fn status_only_provider_idle_waits_observe_lifecycle_without_prompt_support() {
    use yinz_types::agent::provider::{register, unregister_owner, AgentProvider};
    let kind = "screen-status-only-test";
    let owner = 70104;
    register(
        owner,
        AgentProvider {
            kind: kind.into(),
            executable: kind.into(),
            ..Default::default()
        },
    )
    .unwrap();
    for initially_busy in [false, true] {
        let (mut screen, messages, writes) = setup(kind);
        report_kind(
            &mut screen,
            kind,
            hook(if initially_busy {
                "UserPromptSubmit"
            } else {
                "SessionStart"
            }),
        );
        screen.agent_control(wait(AgentState::Idle), CLIENT);
        if initially_busy {
            assert!(replies(&messages, CLIENT).is_empty());
            report_kind(&mut screen, kind, hook("Stop"));
        }
        let result = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(result.outcome, AgentOutcome::Ok);
        assert!(result.agents[0].status_available);
        assert!(!result.agents[0].ready);
        assert!(!result.agents[0].prompt_supported);
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        assert_eq!(
            replies(&messages, CLIENT)[0].outcome,
            AgentOutcome::StatusUnavailable
        );
        assert!(writes
            .try_iter()
            .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))));
    }
    unregister_owner(owner);
}

#[test]
fn status_only_provider_idle_wait_requires_matching_lifecycle_evidence() {
    use yinz_types::agent::provider::{register, unregister_owner, AgentProvider};
    let kind = "screen-lifecycle-evidence-test";
    let owner = 70105;
    register(
        owner,
        AgentProvider {
            kind: kind.into(),
            executable: kind.into(),
            ..Default::default()
        },
    )
    .unwrap();
    for evidence in ["missing", "state-only", "wrong-kind", "wrong-conversation"] {
        let (mut screen, messages, _writes) = setup(kind);
        if evidence.starts_with("wrong-") {
            let kind = if evidence == "wrong-kind" {
                "codex"
            } else {
                kind
            };
            report_kind(&mut screen, kind, hook("SessionStart"));
        }
        if evidence != "missing" {
            let mut startup = hook("SessionStart");
            if evidence == "wrong-conversation" {
                startup.session_id = "other".into();
            }
            let HookOutcome::Reported(agent) = hook_outcome(kind, &startup) else {
                panic!("startup reports an idle agent");
            };
            crate::screen::apply_pane_agent_report(&mut screen, PANE, Some(agent)).unwrap();
        }
        screen.agent_control(wait(AgentState::Idle), CLIENT);
        assert!(replies(&messages, CLIENT).is_empty(), "{evidence}");
        screen.agent_controls[0].deadline = Some(Instant::now());
        screen.refresh_agent_controls();
        assert_eq!(
            replies(&messages, CLIENT)[0].outcome,
            AgentOutcome::StatusUnavailable,
            "{evidence}"
        );
    }
    unregister_owner(owner);
}

#[test]
fn supported_idle_waits_require_readiness_even_without_prompt_support() {
    for kind in ["claude", "codex", "muse", "pi"] {
        let (mut screen, messages, _writes) = setup(kind);
        report_kind(&mut screen, kind, hook("SessionStart"));
        screen
            .tab_containing_pane_mut(PANE)
            .unwrap()
            .handle_pty_bytes(TerminalId::new(7), b"\x1b[?2004l".to_vec())
            .unwrap();
        screen.agent_control(wait(AgentState::Idle), CLIENT);
        assert!(replies(&messages, CLIENT).is_empty());
        screen.agent_controls[0].deadline = Some(Instant::now());
        screen.refresh_agent_controls();
        let result = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(result.outcome, AgentOutcome::TimedOut);
        assert!(!result.agents[0].prompt_supported);
        assert!(!result.agents[0].ready);
    }

    // Antigravity does not emit SessionStart; it bootstraps readiness from native
    // PreInvocation and Stop lifecycle events.
    let (mut screen, messages, _writes) = setup("agy");
    for name in ["UserPromptSubmit", "Stop"] {
        let mut event = hook(name);
        event.prompt_id = Some("startup".into());
        report_kind(&mut screen, "agy", event);
    }
    screen
        .tab_containing_pane_mut(PANE)
        .unwrap()
        .handle_pty_bytes(TerminalId::new(7), b"\x1b[?2004l".to_vec())
        .unwrap();
    screen.agent_control(wait(AgentState::Idle), CLIENT);
    assert!(replies(&messages, CLIENT).is_empty());
    screen.agent_controls[0].deadline = Some(Instant::now());
    screen.refresh_agent_controls();
    let result = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(result.outcome, AgentOutcome::TimedOut);
    assert!(!result.agents[0].prompt_supported);
    assert!(!result.agents[0].ready);
}

#[test]
fn agy_native_turn_bootstraps_readiness_and_allows_prompt() {
    let (mut screen, messages, writes) = setup("agy");
    let mut startup = hook("UserPromptSubmit");
    startup.prompt_id = Some("startup".into());
    report_kind(&mut screen, "agy", startup.clone());
    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let status_busy = replies(&messages, CLIENT)
        .pop()
        .unwrap()
        .agents
        .pop()
        .unwrap();
    assert_eq!(status_busy.agent.state, AgentState::Busy);
    assert!(status_busy.status_available);
    assert!(status_busy.prompt_supported);
    assert!(!status_busy.ready);

    startup.hook_event_name = "Stop".into();
    startup.agy_turn_complete = Some(true);
    report_kind(&mut screen, "agy", startup);
    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let status_idle = replies(&messages, CLIENT)
        .pop()
        .unwrap()
        .agents
        .pop()
        .unwrap();
    assert_eq!(status_idle.agent.state, AgentState::Idle);
    assert!(status_idle.status_available);
    assert!(status_idle.prompt_supported);
    assert!(status_idle.ready);

    screen.agent_control(wait(AgentState::Idle), CLIENT);
    let wait_res = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(wait_res.outcome, AgentOutcome::Ok);
    assert!(wait_res.agents[0].ready);

    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
    screen.refresh_agent_controls();
    report_kind(&mut screen, "agy", hook("UserPromptSubmit"));
    report_kind(&mut screen, "agy", hook("PreToolUse"));
    report_kind(&mut screen, "agy", hook("PostToolUse"));
    report_kind(&mut screen, "agy", hook("PreInvocation"));
    assert!(replies(&messages, CLIENT).is_empty());
    report_kind(&mut screen, "agy", hook("Stop"));
    let done = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(done.outcome, AgentOutcome::Completed);
    assert!(done.agents[0].ready);
}

#[test]
fn agy_native_turn_with_draft_leaves_agent_not_ready() {
    let (mut screen, messages, _writes) = setup("agy");
    let mut turn_hook = hook("UserPromptSubmit");
    turn_hook.session_id = "conversation".into();
    report_kind(&mut screen, "agy", turn_hook.clone());
    screen
        .tab_containing_pane_mut(PANE)
        .unwrap()
        .write_to_active_terminal(&None, b"stray draft".to_vec(), false, CLIENT, None)
        .unwrap();
    turn_hook.hook_event_name = "Stop".into();
    turn_hook.agy_turn_complete = Some(true);
    report_kind(&mut screen, "agy", turn_hook);
    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let status = replies(&messages, CLIENT)
        .pop()
        .unwrap()
        .agents
        .pop()
        .unwrap();
    assert_eq!(status.agent.state, AgentState::Idle);
    assert!(status.status_available);
    assert!(status.prompt_supported);
    assert!(!status.ready);

    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let prompt_res = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(prompt_res.outcome, AgentOutcome::Ok);

    screen.agent_control(wait(AgentState::Idle), CLIENT);
    assert!(replies(&messages, CLIENT).is_empty());
    screen.agent_controls[0].deadline = Some(Instant::now());
    screen.refresh_agent_controls();
    let wait_res = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(wait_res.outcome, AgentOutcome::TimedOut);
    assert!(!wait_res.agents[0].ready);

    screen.agent_control(recover(), CLIENT);
    let recovered = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(recovered.outcome, AgentOutcome::Ok);
    assert!(recovered.agents[0].ready);
}

#[test]
fn agy_orphan_stop_without_preinvocation_cannot_establish_readiness() {
    let (mut screen, messages, writes) = setup("agy");
    report_kind(&mut screen, "agy", hook("Stop"));
    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let status = replies(&messages, CLIENT)
        .pop()
        .unwrap()
        .agents
        .pop()
        .unwrap();
    assert_eq!(status.agent.state, AgentState::Busy);
    assert!(!status.status_available);
    assert!(!status.prompt_supported);
    assert!(!status.ready);

    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    let result = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(result.outcome, AgentOutcome::StatusUnavailable);
    assert!(!result.agents[0].ready);
    assert!(writes
        .try_iter()
        .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))));
}

#[test]
fn agy_late_user_prompt_submit_preserves_unconsumed_draft() {
    let (mut screen, messages, _writes) = setup("agy");
    for name in ["UserPromptSubmit", "Stop"] {
        let mut event = hook(name);
        event.prompt_id = Some("startup".into());
        report_kind(&mut screen, "agy", event);
    }
    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let status = replies(&messages, CLIENT)
        .pop()
        .unwrap()
        .agents
        .pop()
        .unwrap();
    assert!(status.ready);

    // Native prompt submission
    screen
        .tab_containing_pane_mut(PANE)
        .unwrap()
        .write_to_active_terminal(&None, b"review\n".to_vec(), false, CLIENT, None)
        .unwrap();

    // User types a new draft before acceptance hook reaches the screen
    screen
        .tab_containing_pane_mut(PANE)
        .unwrap()
        .write_to_active_terminal(&None, b"unsubmitted draft".to_vec(), false, CLIENT, None)
        .unwrap();

    // Acceptance arrives late
    report_kind(&mut screen, "agy", hook("UserPromptSubmit"));
    report_kind(&mut screen, "agy", hook("Stop"));

    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let status = replies(&messages, CLIENT)
        .pop()
        .unwrap()
        .agents
        .pop()
        .unwrap();
    assert_eq!(status.agent.state, AgentState::Idle);
    assert!(!status.ready, "Unconsumed draft must prevent ready status");

    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let result = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(result.outcome, AgentOutcome::Ok);
    assert!(!result.agents[0].ready);
}

#[test]
fn agy_late_preinvocation_preserves_unconsumed_draft() {
    let (mut screen, messages, _writes) = setup("agy");
    for name in ["UserPromptSubmit", "Stop"] {
        let mut event = hook(name);
        event.prompt_id = Some("startup".into());
        report_kind(&mut screen, "agy", event);
    }

    // Native prompt submission
    screen
        .tab_containing_pane_mut(PANE)
        .unwrap()
        .write_to_active_terminal(&None, b"review\n".to_vec(), false, CLIENT, None)
        .unwrap();

    // Unsubmitted draft before hook
    screen
        .tab_containing_pane_mut(PANE)
        .unwrap()
        .write_to_active_terminal(&None, b"another draft".to_vec(), false, CLIENT, None)
        .unwrap();

    // PreInvocation arrives late without correlation
    report_kind(&mut screen, "agy", hook("PreInvocation"));
    let mut completed = hook("Stop");
    completed.prompt_id = Some("startup".into());
    report_kind(&mut screen, "agy", completed);

    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let status = replies(&messages, CLIENT)
        .pop()
        .unwrap()
        .agents
        .pop()
        .unwrap();
    assert_eq!(status.agent.state, AgentState::Idle);
    assert!(!status.ready);

    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let result = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(result.outcome, AgentOutcome::Ok);
    assert!(!result.agents[0].ready);
}

#[test]
fn agy_late_acceptance_after_interrupted_prompt_preserves_unconsumed_draft() {
    let (mut screen, messages, writes) = setup("agy");
    for name in ["UserPromptSubmit", "Stop"] {
        let mut event = hook(name);
        event.prompt_id = Some("startup".into());
        report_kind(&mut screen, "agy", event);
    }

    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");

    // User types new draft before acceptance
    screen
        .tab_containing_pane_mut(PANE)
        .unwrap()
        .write_to_active_terminal(&None, b"interrupted draft".to_vec(), false, CLIENT, None)
        .unwrap();

    screen.refresh_agent_controls();
    let interrupted = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(interrupted.outcome, AgentOutcome::Interrupted);

    // Late acceptance arrives
    report_kind(&mut screen, "agy", hook("UserPromptSubmit"));
    report_kind(&mut screen, "agy", hook("PreInvocation"));
    report_kind(&mut screen, "agy", hook("Stop"));

    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let status = replies(&messages, CLIENT)
        .pop()
        .unwrap()
        .agents
        .pop()
        .unwrap();
    assert_eq!(status.agent.state, AgentState::Idle);
    assert!(
        !status.ready,
        "Interrupted unsubmitted draft must not be marked consumed"
    );

    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let result = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(result.outcome, AgentOutcome::Ok);
    assert!(!result.agents[0].ready);
}

#[test]
fn agy_draft_before_startup_lifecycle_leaves_agent_not_ready() {
    let (mut screen, messages, _writes) = setup("agy");
    screen
        .tab_containing_pane_mut(PANE)
        .unwrap()
        .write_to_active_terminal(&None, b"draft before start".to_vec(), false, CLIENT, None)
        .unwrap();
    for name in ["UserPromptSubmit", "Stop"] {
        let mut event = hook(name);
        event.prompt_id = Some("startup".into());
        report_kind(&mut screen, "agy", event);
    }

    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let status = replies(&messages, CLIENT)
        .pop()
        .unwrap()
        .agents
        .pop()
        .unwrap();
    assert_eq!(status.agent.state, AgentState::Idle);
    assert!(
        !status.ready,
        "Pane with prior input must not bootstrap readiness"
    );

    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let result = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(result.outcome, AgentOutcome::Ok);
    assert!(!result.agents[0].ready);
}

/// A Muse session reports the full lifecycle - startup, turns, session ids - so once
/// it has reported, prompts are supported, idle waits complete on readiness, and a
/// submitted prompt reaches the pane.
#[test]
fn muse_prompts_are_supported_once_the_session_reports() {
    let (mut screen, messages, writes) = setup("muse");
    report_kind(&mut screen, "muse", hook("SessionStart"));
    screen.agent_control(wait(AgentState::Idle), CLIENT);
    let result = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(result.outcome, AgentOutcome::Ok);
    assert!(result.agents[0].status_available);
    assert!(result.agents[0].prompt_supported);
    assert!(result.agents[0].ready);
    screen.agent_control_with_clear(prompt(false), CLIENT, &writes);
    assert!(replies(&messages, CLIENT).is_empty());
    assert!(writes
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, PtyWriteInstruction::Write(..))));
}

/// A Muse approval need ends a waited prompt without turn identity: the provider's
/// `Notification` carries no turn id, and the pane is blocked on a person whatever
/// turn the provider would have named. Completions still require the acknowledged turn.
#[test]
fn a_session_scoped_muse_approval_ends_a_waited_prompt() {
    let (mut screen, messages, writes) = setup("muse");
    report_kind(&mut screen, "muse", hook("SessionStart"));
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    finish_paste(&writes);
    report_kind(&mut screen, "muse", hook("UserPromptSubmit"));
    assert!(replies(&messages, CLIENT).is_empty());
    // The translated shape of the provider's approval notification: `Elicitation`
    // with the session but no turn.
    let mut approval = hook("Elicitation");
    approval.turn_id = None;
    approval.prompt_id = None;
    approval.message = Some("Allow `cargo build`?".into());
    report_kind(&mut screen, "muse", approval);
    let needs_input = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(needs_input.outcome, AgentOutcome::NeedsInput);
    assert!(needs_input.acknowledged);
    assert_eq!(needs_input.turn_id.as_deref(), Some("turn"));
    let agent = screen.get_pane_info(PANE).unwrap().agent.unwrap();
    assert_eq!(agent.state, AgentState::Waiting);
    assert!(agent.wait_needs_a_person);
    assert_eq!(agent.waiting_on.as_deref(), Some("Allow `cargo build`?"));
    // A completion for another turn is not this turn's end.
    let mut foreign = hook("Stop");
    foreign.turn_id = Some("other".into());
    report_kind(&mut screen, "muse", foreign);
    assert!(replies(&messages, CLIENT).is_empty());
    // The acknowledged turn's end still idles the pane, with no second reply:
    // the wait was already answered.
    report_kind(&mut screen, "muse", hook("Stop"));
    assert!(replies(&messages, CLIENT).is_empty());
    assert_eq!(
        screen.get_pane_info(PANE).unwrap().agent.unwrap().state,
        AgentState::Idle
    );
}

#[test]
fn startup_wait_requires_lifecycle_evidence_and_binds_the_first_conversation() {
    for target in pane_targets() {
        let (mut screen, messages, writes) = setup_on_target("codex", target);
        screen.agent_control(
            AgentRequest::Status {
                pane_id: PANE,
                expected_session_id: None,
            },
            CLIENT,
        );
        let status = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(status.outcome, AgentOutcome::StatusUnavailable);
        assert!(!status.agents[0].ready);
        screen.agent_control(wait(AgentState::Idle), CLIENT);
        assert!(replies(&messages, CLIENT).is_empty());
        screen.agent_control_with_clear(prompt(true), ClientId::new(2), &writes);
        assert_eq!(
            replies(&messages, ClientId::new(2))[0].outcome,
            AgentOutcome::StatusUnavailable
        );
        report(&mut screen, hook("SessionStart"));
        let result = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(result.outcome, AgentOutcome::Ok);
        assert!(result.agents[0].status_available);
        assert!(result.agents[0].ready);
        assert!(result.agents[0].prompt_supported);
        assert_eq!(
            result.agents[0].agent.session_id.as_deref(),
            Some("conversation")
        );

        screen.agent_control_with_clear(prompt(false), CLIENT, &writes);
        assert!(replies(&messages, CLIENT).is_empty());
        let input: Vec<_> = writes
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                PtyWriteInstruction::Write(bytes, terminal, _)
                    if terminal == TerminalId::new(7) =>
                {
                    Some(bytes)
                },
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(input, b"\x1b[200~review\x1b[201~");
        report(&mut screen, hook("UserPromptSubmit"));
        assert_eq!(
            replies(&messages, CLIENT)[0].outcome,
            AgentOutcome::Submitted
        );
        screen.agent_control(wait(AgentState::Idle), CLIENT);
        assert!(replies(&messages, CLIENT).is_empty());
        report(&mut screen, hook("Stop"));
        let result = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(result.outcome, AgentOutcome::Ok);
        assert!(result.agents[0].ready);
    }
}

#[test]
fn repeated_startup_keeps_a_prompt_on_the_announced_conversation() {
    for kind in ["claude", "codex", "pi"] {
        for replacement in [false, true] {
            let (mut screen, messages, writes) = setup(kind);
            // Codex's bridge announces the conversation when the UI selects it; Claude reports
            // its startup once per hook registration.
            report_kind(&mut screen, kind, hook("SessionStart"));
            screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
            screen.agent_control(wait(AgentState::Exited), ClientId::new(2));
            let mut repeated = hook("SessionStart");
            if replacement {
                repeated.session_id = "other".into();
            }
            report_kind(&mut screen, kind, repeated);
            if replacement {
                assert_eq!(
                    replies(&messages, CLIENT)[0].outcome,
                    AgentOutcome::TargetChanged,
                    "{kind}"
                );
                assert_eq!(
                    replies(&messages, ClientId::new(2))[0].outcome,
                    AgentOutcome::TargetChanged,
                    "{kind}"
                );
                continue;
            }
            assert!(replies(&messages, CLIENT).is_empty(), "{kind}");
            assert!(replies(&messages, ClientId::new(2)).is_empty(), "{kind}");
            report_kind(&mut screen, kind, hook("UserPromptSubmit"));
            report_kind(&mut screen, kind, hook("Stop"));
            assert_eq!(
                replies(&messages, CLIENT).pop().unwrap().outcome,
                AgentOutcome::Completed,
                "{kind}"
            );
            assert!(replies(&messages, ClientId::new(2)).is_empty(), "{kind}");
        }
    }
}

#[test]
fn startup_wait_survives_a_repeated_start_before_the_composer_is_ready() {
    for kind in ["claude", "codex", "pi"] {
        for replacement in [false, true] {
            let (mut screen, messages, _writes) = setup(kind);
            // The agent reports its startup before its composer enables bracketed paste.
            screen
                .tab_containing_pane_mut(PANE)
                .unwrap()
                .handle_pty_bytes(TerminalId::new(7), b"\x1b[?2004l".to_vec())
                .unwrap();
            screen.agent_control(wait(AgentState::Idle), CLIENT);
            report_kind(&mut screen, kind, hook("SessionStart"));
            assert!(replies(&messages, CLIENT).is_empty(), "{kind}");
            let mut repeated = hook("SessionStart");
            if replacement {
                repeated.session_id = "other".into();
            }
            report_kind(&mut screen, kind, repeated);
            if replacement {
                assert_eq!(
                    replies(&messages, CLIENT).pop().unwrap().outcome,
                    AgentOutcome::TargetChanged,
                    "{kind}"
                );
                continue;
            }
            assert!(replies(&messages, CLIENT).is_empty(), "{kind}");
            screen
                .tab_containing_pane_mut(PANE)
                .unwrap()
                .handle_pty_bytes(TerminalId::new(7), b"\x1b[?2004h".to_vec())
                .unwrap();
            screen.refresh_agent_controls();
            let result = replies(&messages, CLIENT).pop().unwrap();
            assert_eq!(result.outcome, AgentOutcome::Ok, "{kind}");
            assert!(result.agents[0].ready, "{kind}");
        }
    }
}

#[test]
fn invalid_function_keys_fail_without_writing_or_disrupting_screen_requests() {
    use yinz_types::keys::{BareKey, KeyWithModifier};

    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    for key in [
        KeyWithModifier::new(BareKey::F(0)),
        KeyWithModifier::new(BareKey::F(0)).with_ctrl_modifier(),
    ] {
        screen.agent_control(
            AgentRequest::SendKeys {
                pane_id: PANE,
                expected_session_id: None,
                keys: vec![key],
            },
            CLIENT,
        );
        let result = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(result.outcome, AgentOutcome::Failed);
        assert!(result
            .message
            .unwrap()
            .contains("key has no terminal encoding"));
        assert!(writes
            .try_iter()
            .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))));
        screen.agent_control(
            AgentRequest::Status {
                pane_id: PANE,
                expected_session_id: None,
            },
            CLIENT,
        );
        let status = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(status.outcome, AgentOutcome::Ok);
        assert!(status.agents[0].ready);
    }
}

#[test]
fn startup_reports_preserve_consumption_without_consuming_a_newer_draft() {
    for previous in ["none", "startup", "completed", "unknown", "replacement"] {
        let (mut screen, messages, writes) = setup("codex");
        if previous != "none" {
            report(&mut screen, hook("SessionStart"));
        }
        if matches!(previous, "completed" | "replacement") {
            screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
            report(&mut screen, hook("UserPromptSubmit"));
            report(&mut screen, hook("Stop"));
            assert_eq!(
                replies(&messages, CLIENT)[0].outcome,
                AgentOutcome::Completed
            );
            report(&mut screen, hook("SessionStart"));
            screen.agent_control(wait(AgentState::Idle), CLIENT);
            assert!(replies(&messages, CLIENT)[0].agents[0].ready);
        } else if previous == "unknown" {
            report(&mut screen, hook("UserPromptSubmit"));
            report(&mut screen, hook("Stop"));
        }
        screen
            .tab_containing_pane_mut(PANE)
            .unwrap()
            .write_to_active_terminal(&None, b"manual draft".to_vec(), false, CLIENT, None)
            .unwrap();
        writes.try_iter().for_each(drop);
        for source in ["startup", "resume", "fork", "startup"] {
            let mut start = hook("SessionStart");
            start.source = Some(source.into());
            if previous == "replacement" {
                start.session_id = "replacement".into();
            }
            report(&mut screen, start);
            screen.agent_control(
                AgentRequest::Status {
                    pane_id: PANE,
                    expected_session_id: None,
                },
                CLIENT,
            );
            let status = replies(&messages, CLIENT).pop().unwrap();
            assert_eq!(status.outcome, AgentOutcome::Ok);
            assert!(!status.agents[0].ready, "{previous}, {source}");
            screen.agent_control(
                AgentRequest::Status {
                    pane_id: PANE,
                    expected_session_id: None,
                },
                CLIENT,
            );
            assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
            assert!(screen.agent_controls.is_empty());
            assert!(writes
                .try_iter()
                .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))));
        }
    }
}

#[test]
fn worker_reports_preserve_parent_identity_claims_and_plugin_broadcasts() {
    for acknowledged in [false, true] {
        for name in ["Stop", "SessionEnd", "SubagentStart", "SubagentStop"] {
            let (mut screen, messages, writes) = setup("codex");
            report(&mut screen, hook("SessionStart"));
            screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
            if acknowledged {
                report(&mut screen, hook("UserPromptSubmit"));
            }
            writes.try_iter().for_each(drop);
            let parent = screen.get_pane_info(PANE).unwrap().agent.unwrap();
            let (to_plugin, plugins) = channels::unbounded();
            screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
            let mut worker = hook(name);
            worker.session_id = "worker-conversation".into();
            if matches!(name, "Stop" | "SessionEnd") {
                worker.agent_id = Some("worker".into());
            }
            report(&mut screen, worker.clone());
            // Already queued reports can carry a state computed without worker filtering.
            let mut stale_agent = parent.clone();
            stale_agent.session_id = Some(worker.session_id.clone());
            let stale_state = if matches!(name, "SessionEnd" | "SubagentStop") {
                AgentReportState::Clear
            } else {
                AgentReportState::Set(stale_agent)
            };
            report_state(&mut screen, "codex", worker.clone(), stale_state);
            assert!(
                replies(&messages, CLIENT).is_empty(),
                "{name}, {acknowledged}"
            );
            assert_eq!(
                screen.get_pane_info(PANE).unwrap().agent.as_ref(),
                Some(&parent)
            );
            assert_eq!(screen.agent_controls.len(), 1);
            screen.agent_control_with_clear(prompt(true), ClientId::new(2), &writes);
            let rejected = replies(&messages, ClientId::new(2)).pop().unwrap();
            assert_eq!(rejected.outcome, AgentOutcome::Rejected);
            assert!(rejected.agents[0].status_available);
            assert!(writes
                .try_iter()
                .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))));
            let events: Vec<_> = plugins
                .try_iter()
                .flat_map(|(instruction, _)| match instruction {
                    PluginInstruction::Update(updates) => updates,
                    _ => vec![],
                })
                .filter_map(|(_, _, event)| match event {
                    Event::AgentEvent(event) => Some(event),
                    _ => None,
                })
                .collect();
            let expected = AgentEvent::from_hook(
                "codex",
                TerminalId::new(7),
                &worker,
                serde_json::to_string(&worker).unwrap(),
            );
            assert_eq!(events, vec![expected.clone(), expected]);
            report(&mut screen, hook("UserPromptSubmit"));
            assert!(replies(&messages, CLIENT).is_empty());
            report(&mut screen, hook("Stop"));
            let completed = replies(&messages, CLIENT).pop().unwrap();
            assert_eq!(completed.outcome, AgentOutcome::Completed);
            assert!(completed.acknowledged);
            assert!(completed.agents[0].ready);
            assert!(screen.agent_controls.is_empty());
        }
    }
}

#[test]
fn state_only_reports_and_worker_events_cannot_certify_readiness() {
    for target in pane_targets() {
        let (mut screen, messages, writes) = setup_on_target("codex", target);
        let HookOutcome::Reported(agent) = hook_outcome("codex", &hook("SessionStart")) else {
            panic!("startup reports an idle agent");
        };
        crate::screen::apply_pane_agent_report(&mut screen, PANE, Some(agent)).unwrap();
        for name in [
            "SessionStart",
            "Stop",
            "SessionEnd",
            "SubagentStart",
            "SubagentStop",
        ] {
            let mut worker = hook(name);
            if !matches!(name, "SubagentStart" | "SubagentStop") {
                worker.agent_id = Some("worker".into());
            }
            report(&mut screen, worker);
            screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
            let result = replies(&messages, CLIENT).pop().unwrap();
            assert_eq!(result.outcome, AgentOutcome::StatusUnavailable);
            assert!(!result.agents[0].status_available);
            assert!(!result.agents[0].ready);
        }
        assert!(writes
            .try_iter()
            .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))));
    }
}

#[test]
fn prompt_claim_precedes_input_and_acknowledgement_does_not_release_it() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    screen.agent_control_with_clear(prompt(false), CLIENT, &writes);
    assert!(
        replies(&messages, CLIENT).is_empty(),
        "a write is not acceptance"
    );
    let input: Vec<_> = writes
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PtyWriteInstruction::Write(bytes, terminal, _) if terminal == TerminalId::new(7) => {
                Some(bytes)
            },
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(input, b"\x1b[200~review\x1b[201~");
    screen.agent_control_with_clear(prompt(true), ClientId::new(2), &writes);
    assert_eq!(
        replies(&messages, ClientId::new(2))[0].outcome,
        AgentOutcome::Rejected
    );
    report(&mut screen, hook("UserPromptSubmit"));
    let submitted = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(submitted.outcome, AgentOutcome::Submitted);
    assert!(submitted.acknowledged);
    assert_eq!(submitted.turn_id.as_deref(), Some("turn"));
    report(&mut screen, hook("UserPromptSubmit"));
    assert!(replies(&messages, CLIENT).is_empty());
    let mut old_stop = hook("Stop");
    old_stop.turn_id = Some("old-turn".into());
    report(&mut screen, old_stop);
    screen.agent_control_with_clear(prompt(true), ClientId::new(2), &writes);
    assert_eq!(
        replies(&messages, ClientId::new(2))[0].outcome,
        AgentOutcome::Rejected
    );
    report(&mut screen, hook("Stop"));
    assert!(
        replies(&messages, CLIENT).is_empty(),
        "one acknowledgement reply"
    );
    assert!(screen.agent_controls.is_empty());
    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    assert!(replies(&messages, CLIENT)[0].agents[0].ready);
}

#[test]
fn fast_turn_hooks_preserve_completion_failure_and_interruption() {
    for (name, outcome) in [
        ("Stop", AgentOutcome::Completed),
        ("StopFailure", AgentOutcome::Failed),
        ("Interrupt", AgentOutcome::Interrupted),
    ] {
        let (mut screen, messages, writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        finish_paste(&writes);
        report(&mut screen, hook("Stop"));
        assert!(
            replies(&messages, CLIENT).is_empty(),
            "old idle does not finish a prompt"
        );
        report(&mut screen, hook("UserPromptSubmit"));
        assert!(replies(&messages, CLIENT).is_empty());
        let mut end = hook(name);
        end.last_assistant_message = Some("result".into());
        report(&mut screen, end);
        let result = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(result.outcome, outcome);
        assert!(result.message.as_deref().unwrap().starts_with("result"));
        if outcome == AgentOutcome::Interrupted {
            assert!(result
                .message
                .as_deref()
                .unwrap()
                .contains("The turn was interrupted."));
            assert_eq!(
                result.agents[0].not_ready_reason,
                Some(NotReadyReason::UnresolvedTurn)
            );
        }
        assert!(result.acknowledged);
        assert_eq!(result.agents[0].agent.state, AgentState::Idle);
        if outcome == AgentOutcome::Completed {
            assert!(result.agents[0].ready);
        }
        assert_eq!(
            screen.agent_controls.is_empty(),
            outcome != AgentOutcome::Interrupted
        );
        screen.agent_control(
            wait_submission(result.submission_id.as_deref().unwrap()),
            CLIENT,
        );
        let retained = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(retained.outcome, outcome);
        assert_eq!(retained.submission_id, result.submission_id);
    }
}

#[test]
fn manual_terminal_input_interrupts_correlation_before_a_fast_completion() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    finish_paste(&writes);
    report(&mut screen, hook("UserPromptSubmit"));
    screen
        .tab_containing_pane_mut(PANE)
        .unwrap()
        .write_to_active_terminal(&None, b"manual".to_vec(), false, CLIENT, None)
        .unwrap();
    report(&mut screen, hook("Stop"));
    let result = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(result.outcome, AgentOutcome::Interrupted);
    assert!(result.acknowledged);
    assert!(screen.agent_controls.is_empty());
}

#[test]
fn replacement_and_closure_do_not_finish_the_original_conversation() {
    for closed in [false, true] {
        let (mut screen, messages, writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        report(&mut screen, hook("UserPromptSubmit"));
        let expected = if closed {
            screen.close_pane_explicitly(PANE, None);
            screen.refresh_agent_controls();
            AgentOutcome::TargetClosed
        } else {
            let mut restarted = hook("SessionStart");
            restarted.session_id = "replacement".into();
            report(&mut screen, restarted);
            AgentOutcome::TargetChanged
        };
        assert_eq!(replies(&messages, CLIENT)[0].outcome, expected);
        assert!(screen.agent_controls.is_empty());
    }
}

#[test]
fn claude_compaction_preserves_reported_state_without_a_lifecycle() {
    for (initial, expected) in [
        ("Stop", AgentState::Idle),
        ("UserPromptSubmit", AgentState::Busy),
        ("PreToolUse", AgentState::Waiting),
    ] {
        let (mut screen, _messages, _writes) = setup("claude");
        report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
        let mut initial_hook = hook(initial);
        initial_hook.tool_name = Some("AskUserQuestion".into());
        report_kind(&mut screen, "claude", initial_hook);
        // Reproduce an ordinary reported pane without managed lifecycle evidence.
        screen.agent_lifecycle.remove(&PANE);
        let before = screen.get_pane_info(PANE).unwrap().agent.unwrap();
        assert_eq!(before.state, expected);
        for name in ["PreCompact", "PostCompact", "SessionStart"] {
            let compact: HookEvent = serde_json::from_value(serde_json::json!({
                "hook_event_name": name,
                "session_id": "conversation",
                "trigger": "auto",
                "source": (name == "SessionStart").then_some("compact"),
            }))
            .unwrap();
            report_kind(&mut screen, "claude", compact);
            assert_eq!(
                screen.get_pane_info(PANE).unwrap().agent,
                Some(before.clone())
            );
            assert!(!screen.has_agent_lifecycle(PANE, "claude", "conversation"));
        }
    }
}

#[test]
fn claude_active_turn_compaction_does_not_finish_a_pending_prompt() {
    let (mut screen, messages, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    finish_paste(&writes);
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    for name in ["PreCompact", "PostCompact", "SessionStart"] {
        let mut compact = hook(name);
        compact.source = (name == "SessionStart").then(|| "compact".into());
        report_kind(&mut screen, "claude", compact);
        assert!(replies(&messages, CLIENT).is_empty());
        assert_eq!(
            screen.get_pane_info(PANE).unwrap().agent.unwrap().state,
            AgentState::Busy
        );
        assert!(!screen.agent_controls.is_empty());
    }
    // Compaction is also not a substitute for confirmed Stop completion.
    let mut stop = hook("Stop");
    stop.claude_turn_complete = None;
    report_kind(&mut screen, "claude", stop);
    assert!(replies(&messages, CLIENT).is_empty());
    report_kind(&mut screen, "claude", hook("Stop"));
    assert_eq!(
        replies(&messages, CLIENT).pop().unwrap().outcome,
        AgentOutcome::Completed
    );
    assert!(screen.agent_controls.is_empty());
}

#[test]
fn automatic_approval_and_compaction_continue_until_a_person_is_needed() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    report(&mut screen, hook("UserPromptSubmit"));
    let mut permission = hook("PermissionRequest");
    permission.tool_name = Some("Bash".into());
    permission.approvals_reviewer = Some("auto_review".into());
    report(&mut screen, permission.clone());
    let mut compact = hook("SessionStart");
    compact.source = Some("compact".into());
    report(&mut screen, compact);
    assert!(replies(&messages, CLIENT).is_empty());
    permission.approvals_reviewer = None;
    report(&mut screen, permission);
    let result = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(result.outcome, AgentOutcome::NeedsInput);
    assert!(result.agents[0].agent.is_waiting_on_a_person());
}

#[test]
fn waiting_state_succeeds_while_other_state_waits_need_human_input() {
    for name in ["PermissionRequest", "PreToolUse"] {
        for already_waiting in [false, true] {
            for (until, expected) in [
                (AgentState::Waiting, AgentOutcome::Ok),
                (AgentState::Idle, AgentOutcome::NeedsInput),
                (AgentState::Exited, AgentOutcome::NeedsInput),
            ] {
                let (mut screen, messages, _writes) = setup("codex");
                report(&mut screen, hook("SessionStart"));
                report(&mut screen, hook("UserPromptSubmit"));
                let mut waiting = hook(name);
                waiting.tool_name = Some(if name == "PreToolUse" {
                    "AskUserQuestion".into()
                } else {
                    "Bash".into()
                });
                if already_waiting {
                    report(&mut screen, waiting.clone());
                }
                screen.agent_control(wait(until), CLIENT);
                if !already_waiting {
                    assert!(replies(&messages, CLIENT).is_empty());
                    report(&mut screen, waiting);
                }
                let result = replies(&messages, CLIENT);
                assert_eq!(result.len(), 1);
                assert_eq!(
                    result[0].outcome, expected,
                    "{name}, {already_waiting}, {until}"
                );
                assert!(result[0].agents[0].agent.is_waiting_on_a_person());
                assert!(screen.agent_controls.is_empty());
            }
        }
    }
}

#[test]
fn idle_wait_does_not_succeed_with_a_manual_draft() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    screen
        .tab_containing_pane_mut(PANE)
        .unwrap()
        .write_to_active_terminal(&None, b"manual draft".to_vec(), false, CLIENT, None)
        .unwrap();
    screen.agent_control(wait(AgentState::Idle), CLIENT);
    assert!(replies(&messages, CLIENT).is_empty());
    screen.agent_controls[0].deadline = Some(Instant::now());
    screen.refresh_agent_controls();
    assert_eq!(
        replies(&messages, CLIENT)[0].outcome,
        AgentOutcome::TimedOut
    );
}

#[test]
fn exit_wait_survives_session_end_until_exit_timeout_replacement_or_closure() {
    for expected in [
        AgentOutcome::Ok,
        AgentOutcome::TimedOut,
        AgentOutcome::TargetChanged,
        AgentOutcome::TargetClosed,
    ] {
        let (mut screen, messages, _writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        screen.agent_control(wait(AgentState::Exited), CLIENT);
        assert!(replies(&messages, CLIENT).is_empty());
        report(&mut screen, hook("SessionEnd"));
        assert!(replies(&messages, CLIENT).is_empty());
        let info = screen.get_pane_info(PANE).unwrap();
        assert!(info.agent.is_none());
        assert!(!info.exited);
        assert_eq!(screen.agent_controls.len(), 1);
        assert!(screen.agent_control_deadline().is_some());

        match expected {
            AgentOutcome::Ok => {
                screen.tab_containing_pane_mut(PANE).unwrap().hold_pane(
                    PANE,
                    Some(0),
                    false,
                    RunCommand {
                        command: PathBuf::from("codex"),
                        ..Default::default()
                    },
                );
                let info = screen.get_pane_info(PANE).unwrap();
                assert!(info.exited);
                assert!(info.agent.is_none());
            },
            AgentOutcome::TimedOut => {
                screen.agent_controls[0].deadline = Some(Instant::now());
            },
            AgentOutcome::TargetChanged => {
                let mut replacement = hook("SessionStart");
                replacement.session_id = "replacement".into();
                report(&mut screen, replacement);
            },
            AgentOutcome::TargetClosed => screen.close_pane_explicitly(PANE, None),
            _ => unreachable!(),
        }
        screen.refresh_agent_controls();
        let result = replies(&messages, CLIENT);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].outcome, expected);
        assert!(screen.agent_controls.is_empty());
    }
}

#[test]
fn exit_wait_ignores_worker_clear_reports_but_not_unrelated_parent_clears() {
    for unrelated in ["no-event", "conversation", "kind", "worker"] {
        let (mut screen, messages, _writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        screen.agent_control(wait(AgentState::Exited), CLIENT);
        let mut end = hook("SessionEnd");
        if unrelated == "conversation" {
            end.session_id = "other-conversation".into();
        } else if unrelated == "worker" {
            end.agent_id = Some("worker".into());
        }
        let kind = if unrelated == "kind" {
            "claude"
        } else {
            "codex"
        };
        apply_queued_agent_report(
            &mut screen,
            &AgentReport {
                report_id: "00000000-0000-4000-8000-000000000002".into(),
                pane_id: PANE,
                state: AgentReportState::Clear,
                event: (unrelated != "no-event").then(|| {
                    AgentEvent::from_hook(
                        kind,
                        TerminalId::new(7),
                        &end,
                        serde_json::to_string(&end).unwrap(),
                    )
                }),
                trace: false,
                process_id: ProcessId::new(42),
                kind: kind.into(),
            },
            Instant::now(),
        )
        .unwrap();
        screen.refresh_agent_controls();
        if unrelated == "worker" {
            assert!(replies(&messages, CLIENT).is_empty());
            assert_eq!(screen.agent_controls.len(), 1);
            assert_eq!(
                screen
                    .get_pane_info(PANE)
                    .unwrap()
                    .agent
                    .unwrap()
                    .session_id
                    .as_deref(),
                Some("conversation")
            );
            continue;
        }
        assert_eq!(
            replies(&messages, CLIENT)[0].outcome,
            AgentOutcome::TargetChanged,
            "{unrelated}"
        );
        assert!(screen.agent_controls.is_empty());
        assert!(screen.get_pane_info(PANE).unwrap().agent.is_none());
    }
}

#[test]
fn conversation_pin_and_session_end_prevent_keys_reaching_a_replaced_agent() {
    use yinz_types::keys::{BareKey, KeyWithModifier};

    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    let keys = |expected_session_id| AgentRequest::SendKeys {
        pane_id: PANE,
        expected_session_id,
        keys: vec![KeyWithModifier::new(BareKey::Esc)],
    };
    screen.agent_control(keys(Some("old-conversation".into())), CLIENT);
    assert_eq!(
        replies(&messages, CLIENT)[0].outcome,
        AgentOutcome::TargetChanged
    );
    report(&mut screen, hook("SessionEnd"));
    screen.agent_control(keys(None), CLIENT);
    assert_eq!(
        replies(&messages, CLIENT)[0].outcome,
        AgentOutcome::NotAgent
    );
    assert!(writes
        .try_iter()
        .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))));
}

#[test]
fn moving_an_agent_to_another_tab_preserves_pending_prompt_correlation() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    report(&mut screen, hook("UserPromptSubmit"));
    let revision = screen
        .tab_containing_pane(PANE)
        .unwrap()
        .agent_input_revision(PANE);
    assert!(revision > 0);
    new_tab(&mut screen, 2, TabId::new(1));
    let moved = screen
        .break_multiple_panes_to_tab_with_id(vec![PANE], TabId::new(1), false, CLIENT)
        .unwrap();
    assert!(moved.succeeded(), "{moved:?}");
    let destination = screen.tabs.get_mut(&TabId::new(1)).unwrap();
    assert_eq!(destination.agent_input_revision(PANE), revision);
    destination
        .write_to_pane_id(&None, Vec::new(), false, PANE, None, None)
        .unwrap();
    assert_eq!(destination.agent_input_revision(PANE), revision);
    screen.refresh_agent_controls();
    assert!(replies(&messages, CLIENT).is_empty());
    report(&mut screen, hook("Stop"));
    assert_eq!(
        replies(&messages, CLIENT)[0].outcome,
        AgentOutcome::Completed
    );
}

#[test]
fn timeout_keeps_the_claim_until_the_original_submission_is_resolved() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    finish_paste(&writes);
    screen.agent_controls[0].deadline = Some(Instant::now());
    screen.refresh_agent_controls();
    let timeout = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(timeout.outcome, AgentOutcome::TimedOut);
    assert!(!timeout.acknowledged);
    assert!(!timeout.agents[0].ready);
    assert_eq!(screen.agent_controls.len(), 1);
    assert!(screen.agent_control_deadline().is_none());

    screen.agent_control_with_clear(prompt(true), ClientId::new(2), &writes);
    assert_eq!(
        replies(&messages, ClientId::new(2))[0].outcome,
        AgentOutcome::Rejected
    );
    report(&mut screen, hook("UserPromptSubmit"));
    report(&mut screen, hook("Stop"));
    assert!(
        replies(&messages, CLIENT).is_empty(),
        "the timed-out caller is detached"
    );
    assert!(screen.agent_controls.is_empty());

    screen.agent_control_with_clear(prompt(true), ClientId::new(2), &writes);
    assert!(replies(&messages, ClientId::new(2)).is_empty());
    assert_eq!(screen.agent_controls.len(), 1);
}

#[test]
fn prompt_rejects_busy_waiting_and_unprepared_terminals_without_writing() {
    for (phase, outcome) in [
        ("UserPromptSubmit", AgentOutcome::Rejected),
        ("PermissionRequest", AgentOutcome::Rejected),
        ("no-paste", AgentOutcome::StatusUnavailable),
        ("sync-excluded", AgentOutcome::StatusUnavailable),
    ] {
        let (mut screen, messages, writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        if phase == "no-paste" {
            screen
                .tab_containing_pane_mut(PANE)
                .unwrap()
                .handle_pty_bytes(TerminalId::new(7), b"\x1b[?2004l".to_vec())
                .unwrap();
        } else if phase == "sync-excluded" {
            let tab = screen.tab_containing_pane_mut(PANE).unwrap();
            tab.toggle_sync_panes_is_active();
            tab.get_pane_with_id_mut(PANE)
                .unwrap()
                .set_exclude_from_sync(true);
        } else {
            report(&mut screen, hook(phase));
        }
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        let result = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(result.outcome, outcome, "{phase}");
        assert!(!result.agents[0].ready);
        assert!(writes
            .try_iter()
            .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))));
    }
}

#[test]
fn keys_reject_shells_with_stale_reports_and_held_agents_without_writing() {
    use yinz_types::keys::{BareKey, KeyWithModifier};

    for command in ["bash", "codex"] {
        let (mut screen, messages, writes) = setup(command);
        report(&mut screen, hook("SessionStart"));
        if command == "codex" {
            screen.tab_containing_pane_mut(PANE).unwrap().hold_pane(
                PANE,
                Some(0),
                false,
                RunCommand {
                    command: PathBuf::from(command),
                    ..Default::default()
                },
            );
        }
        screen.agent_control(
            AgentRequest::SendKeys {
                pane_id: PANE,
                expected_session_id: Some("conversation".into()),
                keys: vec![KeyWithModifier::new(BareKey::Enter)],
            },
            CLIENT,
        );
        assert!(matches!(
            replies(&messages, CLIENT)[0].outcome,
            AgentOutcome::StatusUnavailable | AgentOutcome::TargetChanged
        ));
        assert!(writes
            .try_iter()
            .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))));
    }
}

#[test]
fn delayed_and_duplicate_acknowledgements_do_not_clear_a_manual_draft() {
    for acknowledged_before_draft in [false, true] {
        let (mut screen, messages, writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        if acknowledged_before_draft {
            report(&mut screen, hook("UserPromptSubmit"));
        }
        screen
            .tab_containing_pane_mut(PANE)
            .unwrap()
            .write_to_active_terminal(&None, b"manual draft".to_vec(), false, CLIENT, None)
            .unwrap();
        writes.try_iter().for_each(drop);
        screen.refresh_agent_controls();
        let interrupted = replies(&messages, CLIENT);
        assert_eq!(interrupted.len(), 1);
        assert_eq!(interrupted[0].outcome, AgentOutcome::Interrupted);
        assert_eq!(interrupted[0].acknowledged, acknowledged_before_draft);
        assert!(!interrupted[0].agents[0].ready);
        assert_eq!(screen.agent_controls.len(), 1);
        assert!(screen.agent_control_deadline().is_none());
        screen.agent_control_with_clear(prompt(true), ClientId::new(2), &writes);
        assert_eq!(
            replies(&messages, ClientId::new(2))[0].outcome,
            AgentOutcome::Rejected
        );
        report(&mut screen, hook("UserPromptSubmit"));
        report(&mut screen, hook("UserPromptSubmit"));
        report(&mut screen, hook("Stop"));
        assert!(replies(&messages, CLIENT).is_empty());
        assert!(screen.agent_controls.is_empty());

        screen.agent_control(
            AgentRequest::Status {
                pane_id: PANE,
                expected_session_id: None,
            },
            ClientId::new(2),
        );
        let result = replies(&messages, ClientId::new(2)).pop().unwrap();
        assert_eq!(result.outcome, AgentOutcome::Ok);
        assert_eq!(result.agents[0].agent.state, AgentState::Idle);
        assert!(!result.agents[0].ready);
        assert!(writes
            .try_iter()
            .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))));
    }
}

#[test]
fn delayed_manual_acceptance_does_not_consume_a_newer_draft() {
    for startup_reported in [false, true] {
        let (mut screen, messages, writes) = setup("codex");
        if startup_reported {
            report(&mut screen, hook("SessionStart"));
        }
        let tab = screen.tab_containing_pane_mut(PANE).unwrap();
        tab.write_to_active_terminal(&None, b"review\r".to_vec(), false, CLIENT, None)
            .unwrap();
        tab.write_to_active_terminal(&None, b"newer draft".to_vec(), false, CLIENT, None)
            .unwrap();
        writes.try_iter().for_each(drop);

        report(&mut screen, hook("UserPromptSubmit"));
        report(&mut screen, hook("UserPromptSubmit"));
        report(&mut screen, hook("PreToolUse"));
        report(&mut screen, hook("Stop"));
        screen.agent_control(
            AgentRequest::Status {
                pane_id: PANE,
                expected_session_id: None,
            },
            CLIENT,
        );
        let result = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(result.outcome, AgentOutcome::Ok);
        assert_eq!(result.agents[0].agent.state, AgentState::Idle);
        assert!(result.agents[0].status_available);
        assert!(!result.agents[0].ready);
        assert!(screen.agent_controls.is_empty());
        assert!(writes
            .try_iter()
            .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))));
    }
}

#[test]
fn tool_and_completion_reports_without_startup_cannot_establish_readiness() {
    for first_hook in ["PreToolUse", "Stop"] {
        let (mut screen, messages, writes) = setup("codex");
        report(&mut screen, hook(first_hook));
        report(&mut screen, hook("Stop"));
        screen.agent_control(
            AgentRequest::Status {
                pane_id: PANE,
                expected_session_id: None,
            },
            CLIENT,
        );
        let result = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(result.outcome, AgentOutcome::Ok, "{first_hook}");
        assert!(result.agents[0].status_available);
        assert!(!result.agents[0].ready);
        assert!(writes
            .try_iter()
            .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))));
    }
}

#[test]
fn a_cli_prompt_replaces_manual_input_even_when_idle_readiness_is_false() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    screen
        .tab_containing_pane_mut(PANE)
        .unwrap()
        .write_to_active_terminal(&None, b"manual draft".to_vec(), false, CLIENT, None)
        .unwrap();
    writes.try_iter().for_each(drop);
    assert!(!screen.agent_status_for_test().ready);
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
    assert!(replies(&messages, CLIENT).is_empty());
}

#[test]
fn tool_and_completion_reports_do_not_clear_a_draft_typed_while_busy() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    report(&mut screen, hook("UserPromptSubmit"));
    screen
        .tab_containing_pane_mut(PANE)
        .unwrap()
        .write_to_active_terminal(&None, b"manual draft".to_vec(), false, CLIENT, None)
        .unwrap();
    writes.try_iter().for_each(drop);
    report(&mut screen, hook("PreToolUse"));
    report(&mut screen, hook("Stop"));
    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let result = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(result.outcome, AgentOutcome::Ok);
    assert_eq!(result.agents[0].agent.state, AgentState::Idle);
    assert!(!result.agents[0].ready);
    assert!(writes
        .try_iter()
        .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))));

    // An uncorrelated acceptance cannot prove which input revision was consumed.
    report(&mut screen, hook("UserPromptSubmit"));
    report(&mut screen, hook("Stop"));
    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let result = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(result.outcome, AgentOutcome::Ok);
    assert!(!result.agents[0].ready);
    assert!(screen.agent_controls.is_empty());
    assert!(writes
        .try_iter()
        .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))));
}

fn finish_paste(writes: &Receiver<(PtyWriteInstruction, ErrorContext)>) -> Vec<u8> {
    writes
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PtyWriteInstruction::Write(bytes, _, completion) => {
                drop(completion);
                Some(bytes)
            },
            _ => None,
        })
        .flatten()
        .collect()
}

impl Screen {
    // Tests of post-delivery behavior advance only the clearing phase; the prompt write stays pending.
    fn agent_control_with_clear(
        &mut self,
        request: AgentRequest,
        client: ClientId,
        writes: &Receiver<(PtyWriteInstruction, ErrorContext)>,
    ) {
        self.agent_control_with_clear_reply(
            request,
            crate::screen::agent_control::Reply::Client(client),
            writes,
        );
    }

    fn agent_control_with_clear_reply(
        &mut self,
        request: AgentRequest,
        client: crate::screen::agent_control::Reply,
        writes: &Receiver<(PtyWriteInstruction, ErrorContext)>,
    ) {
        let clearing = self.agent_draft_clears.contains_key(&PANE);
        let prompt = matches!(request, AgentRequest::Prompt { .. });
        let now = Instant::now();
        let delay = if self
            .agent_status(PANE, None)
            .is_some_and(|status| status.agent.kind == "pi")
        {
            600
        } else {
            0
        };
        self.agent_control_at(request, client, now - Duration::from_millis(251 + delay));
        if prompt && !clearing && self.agent_draft_clears.contains_key(&PANE) {
            let marker = finish_paste(writes);
            assert!(marker.starts_with(b"\x1b[200~yz") && marker.ends_with(b"\x1b[201~"));
            self.refresh_agent_controls_at(now - Duration::from_millis(250 + delay));
            self.refresh_agent_controls_at(now - Duration::from_millis(250));
            let clear = finish_paste(writes);
            let kind = self.agent_status_for_test().agent.kind;
            let provider = yinz_types::agent::provider::provider(&kind);
            let key = provider
                .as_ref()
                .and_then(|p| p.input.as_ref())
                .map_or("Ctrl c", |input| input.clear_key.as_str());
            assert_eq!(
                clear,
                match key {
                    "Ctrl c" => vec![3],
                    "Ctrl u" => vec![21],
                    _ => panic!("unexpected test clear key: {key}"),
                }
            );
            self.refresh_agent_controls_at(now - Duration::from_millis(250));
            self.refresh_agent_controls_at(now);
        }
    }
}

fn recover() -> AgentRequest {
    AgentRequest::Recover {
        pane_id: PANE,
        expected_session_id: "conversation".into(),
        confirm_empty: true,
        interrupted: false,
    }
}

fn clear_prompt() -> AgentRequest {
    AgentRequest::Prompt {
        preserve_draft: false,
        submission_id: None,
        pane_id: PANE,
        expected_session_id: None,
        prompt: "/clear".into(),
        wait: true,
        timeout_ms: 30_000,
    }
}

fn clear_start() -> HookEvent {
    HookEvent {
        source: Some("clear".into()),
        session_id: "cleared-conversation".into(),
        ..hook("SessionStart")
    }
}

fn deliver_clear(screen: &mut Screen, writes: &Receiver<(PtyWriteInstruction, ErrorContext)>) {
    screen.agent_control_with_clear(clear_prompt(), CLIENT, writes);
    assert!(!screen.agent_clear_inputs.contains_key(&PANE));
    assert_eq!(finish_paste(writes), b"/clear\x1b[200~\x1b[201~");
    let now = Instant::now();
    screen.refresh_agent_controls_at(now);
    screen.refresh_agent_controls_at(now + Duration::from_millis(501));
    assert_eq!(finish_paste(writes), b"\r");
    assert!(screen.agent_clear_inputs.contains_key(&PANE));
}

#[test]
fn cli_clear_restores_readiness_and_allows_the_next_prompt() {
    for target in pane_targets() {
        let (mut screen, messages, writes) = setup_on_target("claude", target);
        report_kind(&mut screen, "claude", hook("SessionStart"));
        deliver_clear(&mut screen, &writes);
        report_kind(&mut screen, "claude", hook("SessionEnd"));
        assert_eq!(
            replies(&messages, CLIENT)[0].outcome,
            AgentOutcome::TargetChanged
        );
        assert!(screen.get_pane_info(PANE).unwrap().agent.is_none());
        assert!(screen.agent_control_deadline().is_some());
        report_kind(&mut screen, "claude", hook("SessionEnd"));
        // An older installed hook can report no state before the updated inline hook.
        report_state(
            &mut screen,
            "claude",
            clear_start(),
            AgentReportState::Unchanged,
        );
        assert!(screen.agent_clear_inputs.contains_key(&PANE));
        report_kind(&mut screen, "claude", clear_start());
        assert!(screen.agent_clear_inputs.is_empty());
        assert!(screen.agent_control_deadline().is_none());
        // Duplicate registrations repeat the clear-start hook without losing consumption.
        report_kind(&mut screen, "claude", clear_start());
        for request in [
            AgentRequest::List { project: None },
            AgentRequest::Status {
                pane_id: PANE,
                expected_session_id: None,
            },
            wait(AgentState::Idle),
        ] {
            screen.agent_control_with_clear(request, CLIENT, &writes);
            let result = replies(&messages, CLIENT).pop().unwrap();
            assert_eq!(result.outcome, AgentOutcome::Ok);
            assert_eq!(
                result.agents[0].agent.session_id.as_deref(),
                Some("cleared-conversation")
            );
            assert_eq!(result.agents[0].agent.state, AgentState::Idle);
            assert!(result.agents[0].ready);
        }
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        assert!(replies(&messages, CLIENT).is_empty());
        assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
        for name in ["UserPromptSubmit", "Stop"] {
            report_kind(
                &mut screen,
                "claude",
                HookEvent {
                    session_id: "cleared-conversation".into(),
                    ..hook(name)
                },
            );
        }
        let completed = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(completed.outcome, AgentOutcome::Completed);
        assert!(completed.agents[0].ready);
    }
}

#[test]
fn cli_codex_new_restores_readiness_like_claude_clear() {
    for target in pane_targets() {
        let (mut screen, messages, writes) = setup_on_target("codex", target);
        report_kind(&mut screen, "codex", hook("SessionStart"));
        screen.agent_control_with_clear(
            AgentRequest::Prompt {
                preserve_draft: false,
                submission_id: None,
                pane_id: PANE,
                expected_session_id: None,
                prompt: "/new".into(),
                wait: true,
                timeout_ms: 30_000,
            },
            CLIENT,
            &writes,
        );
        // Typed alone: an empty paste after it would discard it in Codex's paste-burst handling.
        assert_eq!(finish_paste(&writes), b"/new");
        let now = Instant::now();
        screen.refresh_agent_controls_at(now);
        screen.refresh_agent_controls_at(now + Duration::from_millis(501));
        assert_eq!(finish_paste(&writes), b"\r");
        assert!(screen.agent_clear_inputs.contains_key(&PANE));
        // The bridge reports a replaced thread as an end and a clear start.
        report_kind(&mut screen, "codex", hook("SessionEnd"));
        assert_eq!(
            replies(&messages, CLIENT)[0].outcome,
            AgentOutcome::TargetChanged
        );
        // A wait issued before the new conversation starts waits for it.
        screen.agent_control(wait(AgentState::Idle), CLIENT);
        assert!(replies(&messages, CLIENT).is_empty());
        report_kind(
            &mut screen,
            "codex",
            HookEvent {
                codex_hooks_ready: Some(true),
                ..clear_start()
            },
        );
        assert!(screen.agent_clear_inputs.is_empty());
        let waited = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(waited.outcome, AgentOutcome::Ok);
        assert_eq!(
            waited.agents[0].agent.session_id.as_deref(),
            Some("cleared-conversation")
        );
        assert!(waited.agents[0].ready);
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        assert!(replies(&messages, CLIENT).is_empty());
        finish_paste(&writes);
        for name in ["UserPromptSubmit", "Stop"] {
            report_kind(
                &mut screen,
                "codex",
                HookEvent {
                    session_id: "cleared-conversation".into(),
                    ..hook(name)
                },
            );
        }
        let completed = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(completed.outcome, AgentOutcome::Completed);
        assert!(completed.agents[0].ready);
    }
}

#[test]
fn conversation_reset_commands_are_exact_and_per_kind() {
    use yinz_types::agent::control::is_command_prompt;
    assert!(!is_command_prompt("claude", "/clear"));
    assert!(!is_command_prompt("codex", "/new"));
    for (kind, prompt) in [
        ("claude", "/new"),
        ("codex", "/clear"),
        ("codex", " /new"),
        ("codex", "/new please"),
    ] {
        assert!(is_command_prompt(kind, prompt), "{kind} {prompt:?}");
    }
}

#[test]
fn clear_start_consumes_only_the_submitted_revision() {
    for draft_at in ["before_end", "after_end", "after_start"] {
        let (mut screen, messages, writes) = setup("claude");
        report_kind(&mut screen, "claude", hook("SessionStart"));
        deliver_clear(&mut screen, &writes);
        let submitted = screen
            .tab_containing_pane(PANE)
            .unwrap()
            .agent_input_revision(PANE);
        if draft_at == "before_end" {
            screen.agent_write_for_test();
        }
        report_kind(&mut screen, "claude", hook("SessionEnd"));
        replies(&messages, CLIENT);
        if draft_at == "after_end" {
            screen.agent_write_for_test();
        }
        report_kind(&mut screen, "claude", clear_start());
        if draft_at == "after_start" {
            screen.agent_write_for_test();
        }
        report_kind(&mut screen, "claude", clear_start());
        assert!(
            screen
                .tab_containing_pane(PANE)
                .unwrap()
                .agent_input_revision(PANE)
                > submitted
        );
        screen.agent_control(
            AgentRequest::Status {
                pane_id: PANE,
                expected_session_id: None,
            },
            CLIENT,
        );
        let result = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(result.outcome, AgentOutcome::Ok, "{draft_at}");
        assert!(!result.agents[0].ready);
    }
}

#[test]
fn failed_or_unsubmitted_clear_cannot_restore_readiness() {
    for reason in [
        "delivery",
        "enter",
        "timeout",
        "manual",
        "not_exact",
        "codex",
    ] {
        let kind = if reason == "codex" { "codex" } else { "claude" };
        let (mut screen, messages, writes) = setup(kind);
        report_kind(&mut screen, kind, hook("SessionStart"));
        if reason == "manual" {
            screen.agent_write_for_test();
        } else {
            let request = if reason == "not_exact" {
                AgentRequest::Prompt {
                    preserve_draft: false,
                    submission_id: None,
                    pane_id: PANE,
                    expected_session_id: None,
                    prompt: "/clear extra".into(),
                    wait: true,
                    timeout_ms: 30_000,
                }
            } else {
                clear_prompt()
            };
            screen.agent_control_with_clear(request, CLIENT, &writes);
            if reason == "delivery" {
                for (instruction, _) in writes.try_iter() {
                    if let PtyWriteInstruction::Write(_, _, Some(mut completion)) = instruction {
                        completion.set_error_message("paste failed".into());
                    }
                }
            } else {
                finish_paste(&writes);
            }
            let now = Instant::now();
            screen.refresh_agent_controls_at(now);
            if reason == "enter" {
                let (sender, receiver) = channels::unbounded();
                drop(receiver);
                let senders = &mut screen.tab_containing_pane_mut(PANE).unwrap().senders;
                senders.should_silently_fail = false;
                senders.to_pty_writer = Some(SenderWithContext::new(sender));
            }
            if reason == "timeout" {
                screen.agent_controls[0].deadline = Some(now);
            }
            screen.refresh_agent_controls_at(now + Duration::from_millis(501));
        }
        assert!(screen.agent_clear_inputs.is_empty(), "{reason}");
        report_kind(&mut screen, kind, hook("SessionEnd"));
        report_kind(&mut screen, kind, clear_start());
        replies(&messages, CLIENT);
        screen.agent_control(
            AgentRequest::Status {
                pane_id: PANE,
                expected_session_id: None,
            },
            CLIENT,
        );
        let status = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(status.outcome, AgentOutcome::Ok);
        assert_eq!(
            status.agents[0].ready,
            matches!(reason, "not_exact" | "codex"),
            "{reason}"
        );
        // Manual/raw clear still restores the record, so explicit recovery is available.
        screen.agent_control(
            AgentRequest::Recover {
                pane_id: PANE,
                expected_session_id: "cleared-conversation".into(),
                confirm_empty: true,
                interrupted: false,
            },
            CLIENT,
        );
        let recovered = replies(&messages, CLIENT).pop().unwrap();
        if reason != "enter" {
            assert_eq!(recovered.outcome, AgentOutcome::Ok, "{reason}");
            assert!(recovered.agents[0].ready);
        }
    }
}

#[test]
fn clear_tracking_is_discarded_on_unrelated_lifecycle_or_cleanup() {
    for reason in [
        "timeout",
        "recover",
        "invalidate",
        "closed",
        "process",
        "held",
        "kind",
        "wrong_end",
        "startup",
        "compact",
        "unknown",
        "empty",
        "same",
        "missing_end",
    ] {
        let (mut screen, messages, writes) = setup("claude");
        report_kind(&mut screen, "claude", hook("SessionStart"));
        deliver_clear(&mut screen, &writes);
        if reason == "wrong_end" {
            report_kind(
                &mut screen,
                "claude",
                HookEvent {
                    session_id: "other".into(),
                    ..hook("SessionEnd")
                },
            );
        } else if reason != "missing_end" {
            report_kind(&mut screen, "claude", hook("SessionEnd"));
        }
        replies(&messages, CLIENT);
        match reason {
            "timeout" => screen.refresh_agent_controls_at(Instant::now() + Duration::from_secs(31)),
            "recover" => {
                report_kind(&mut screen, "claude", hook("SessionStart"));
                screen.agent_control(recover(), CLIENT);
                assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
                screen.agent_write_for_test();
            },
            "invalidate" => screen.invalidate_agent_lifecycle(PANE, "test"),
            "closed" => screen.close_pane_explicitly(PANE, None),
            "process" => {
                let tab = screen.tab_containing_pane_mut(PANE).unwrap();
                tab.hold_pane(PANE, Some(0), false, tab.pane_run_command(PANE).unwrap());
                screen.run_command_in_pane_with_id(
                    TerminalId::new(7),
                    RunCommand {
                        command: "claude".into(),
                        ..Default::default()
                    },
                    false,
                );
            },
            "held" => {
                let tab = screen.tab_containing_pane_mut(PANE).unwrap();
                tab.hold_pane(PANE, Some(0), false, tab.pane_run_command(PANE).unwrap());
            },
            "startup" | "compact" | "unknown" => report_kind(
                &mut screen,
                "claude",
                HookEvent {
                    source: Some(reason.into()),
                    ..clear_start()
                },
            ),
            "kind" => report_kind(&mut screen, "codex", clear_start()),
            "empty" => report_kind(
                &mut screen,
                "claude",
                HookEvent {
                    session_id: String::new(),
                    ..clear_start()
                },
            ),
            "same" => report_kind(
                &mut screen,
                "claude",
                HookEvent {
                    session_id: "conversation".into(),
                    ..clear_start()
                },
            ),
            _ => {},
        }
        if reason == "process" {
            assert!(screen.agent_clear_inputs.is_empty());
        }
        report_kind(&mut screen, "claude", clear_start());
        assert!(screen.agent_clear_inputs.is_empty(), "{reason}");
        if reason != "closed" {
            screen.agent_control(
                AgentRequest::Status {
                    pane_id: PANE,
                    expected_session_id: None,
                },
                CLIENT,
            );
            let result = replies(&messages, CLIENT).pop().unwrap();
            // An accepted process restart also clears the old input draft.
            assert_eq!(result.agents[0].ready, reason == "process", "{reason}");
        }
    }
}

#[test]
fn screen_paste_sends_one_preprocessed_write_with_its_completion() {
    for client in [None, Some(CLIENT)] {
        for (modes, expected) in [
            (b"\x1b[?2004l".as_slice(), b"\r".as_slice()),
            (b"\x1b[?2004l\x1b[20h".as_slice(), b"\r\n".as_slice()),
            (b"".as_slice(), b"\x1b[200~\r\x1b[201~".as_slice()),
            (b"\x1b[20h".as_slice(), b"\x1b[200~\r\n\x1b[201~".as_slice()),
        ] {
            let (mut screen, _, writes) = setup("codex");
            screen
                .tab_containing_pane_mut(PANE)
                .unwrap()
                .handle_pty_bytes(TerminalId::new(7), modes.to_vec())
                .unwrap();
            writes.try_iter().for_each(drop);
            let (tx, mut result) = tokio::sync::oneshot::channel();
            screen
                .paste_to_pane_id(
                    b"\r".to_vec(),
                    PANE,
                    client,
                    Some(yinz_bus::route::NotificationEnd::new(tx)),
                )
                .unwrap();
            let mut instructions = writes.try_iter().collect::<Vec<_>>();
            assert_eq!(instructions.len(), 1);
            let (PtyWriteInstruction::Write(bytes, id, Some(mut completion)), _) =
                instructions.pop().unwrap()
            else {
                panic!("paste must own one write completion");
            };
            assert_eq!(id, TerminalId::new(7));
            assert_eq!(bytes, expected);
            assert!(matches!(
                result.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ));
            completion.set_error_message("paste failed".into());
            drop(completion);
            assert_eq!(
                result.try_recv().unwrap().error_message.as_deref(),
                Some("paste failed")
            );
        }
    }
}

#[test]
fn paste_completion_and_delay_gate_enter_without_sleeping() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    let now = Instant::now();
    screen.refresh_agent_controls_at(now + Duration::from_millis(500));
    assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
    screen.refresh_agent_controls_at(now + Duration::from_millis(500));
    screen.refresh_agent_controls_at(now + Duration::from_millis(599));
    assert!(finish_paste(&writes).is_empty());
    screen.refresh_agent_controls_at(now + Duration::from_millis(600));
    assert_eq!(finish_paste(&writes), b"\r");
    assert!(replies(&messages, CLIENT).is_empty());
    report(&mut screen, hook("UserPromptSubmit"));
    report(&mut screen, hook("Stop"));
    assert_eq!(
        replies(&messages, CLIENT)[0].outcome,
        AgentOutcome::Completed
    );
}

#[test]
fn enter_is_cancelled_on_input_failure_early_acknowledgement_or_replacement() {
    for reason in [
        "input",
        "failure",
        "overflow",
        "ack",
        "replacement",
        "closed",
        "timeout",
    ] {
        let (mut screen, messages, writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        let mut instructions = writes
            .try_iter()
            .map(|(instruction, _)| instruction)
            .filter(|instruction| matches!(instruction, PtyWriteInstruction::Write(..)))
            .collect::<Vec<_>>();
        assert_eq!(instructions.len(), 1);
        let PtyWriteInstruction::Write(bytes, id, Some(mut completion)) =
            instructions.pop().unwrap()
        else {
            panic!("agent paste must own one write completion");
        };
        assert_eq!(id, TerminalId::new(7));
        assert_eq!(bytes, b"\x1b[200~review\x1b[201~");
        match reason {
            "input" => screen.agent_write_for_test(),
            "failure" => completion.set_error_message("partial write failed".into()),
            "overflow" => completion.set_error_message("PTY write queue overflow".into()),
            "ack" => report(&mut screen, hook("UserPromptSubmit")),
            "replacement" => {
                let tab = screen.tab_containing_pane_mut(PANE).unwrap();
                tab.hold_pane(PANE, Some(0), false, tab.pane_run_command(PANE).unwrap());
                screen.run_command_in_pane_with_id(
                    TerminalId::new(7),
                    RunCommand {
                        command: "codex".into(),
                        ..Default::default()
                    },
                    false,
                );
            },
            "closed" => screen.close_pane_explicitly(PANE, None),
            "timeout" => screen.agent_controls[0].deadline = Some(Instant::now()),
            _ => unreachable!(),
        }
        finish_paste(&writes);
        drop(completion);
        let now = Instant::now();
        screen.refresh_agent_controls_at(now);
        screen.refresh_agent_controls_at(now + Duration::from_millis(501));
        assert!(finish_paste(&writes).is_empty(), "{reason}");
        if reason == "ack" {
            assert!(replies(&messages, CLIENT).is_empty());
        } else {
            let responses = replies(&messages, CLIENT);
            assert_eq!(responses.len(), 1, "{reason}");
            if matches!(reason, "failure" | "overflow") {
                assert_eq!(responses[0].outcome, AgentOutcome::Failed, "{reason}");
            }
        }
    }
}

impl Screen {
    fn agent_write_for_test(&mut self) {
        self.tab_containing_pane_mut(PANE)
            .unwrap()
            .write_key_to_pane_id(PANE, KeyWithModifier::new(BareKey::Char('x')), None)
            .unwrap();
    }
}

#[test]
fn uncertain_submission_requires_explicit_recovery_and_replies_once() {
    for target in pane_targets() {
        let (mut screen, messages, writes) = setup_on_target("codex", target);
        report(&mut screen, hook("SessionStart"));
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        screen.agent_control(recover(), ClientId::new(2));
        assert_eq!(
            replies(&messages, ClientId::new(2))[0].outcome,
            AgentOutcome::Rejected
        );
        screen.agent_controls[0].deadline = Some(Instant::now());
        screen.refresh_agent_controls();
        let timed_out = replies(&messages, CLIENT);
        assert_eq!(timed_out[0].outcome, AgentOutcome::TimedOut);
        assert!(timed_out[0]
            .message
            .as_deref()
            .unwrap()
            .contains("agent recover"));
        screen.agent_control(recover(), ClientId::new(2));
        assert_eq!(
            replies(&messages, ClientId::new(2))[0].outcome,
            AgentOutcome::Rejected,
            "paste still in flight"
        );
        finish_paste(&writes);
        screen.refresh_agent_controls();
        assert!(screen.agent_control_deadline().is_none());
        report(&mut screen, hook("Stop"));
        for _ in 0..3 {
            screen.refresh_agent_controls();
        }
        assert!(replies(&messages, CLIENT).is_empty());
        screen.agent_control_with_clear(prompt(true), ClientId::new(2), &writes);
        assert_eq!(
            replies(&messages, ClientId::new(2))[0].outcome,
            AgentOutcome::Rejected
        );
        for request in [
            AgentRequest::Recover {
                pane_id: PANE,
                expected_session_id: "wrong".into(),
                confirm_empty: true,
                interrupted: false,
            },
            AgentRequest::Recover {
                pane_id: PANE,
                expected_session_id: "conversation".into(),
                confirm_empty: false,
                interrupted: false,
            },
        ] {
            screen.agent_control_with_clear(request, ClientId::new(2), &writes);
            assert_ne!(
                replies(&messages, ClientId::new(2))[0].outcome,
                AgentOutcome::Ok
            );
        }
        screen.agent_control(recover(), ClientId::new(2));
        let recovered = replies(&messages, ClientId::new(2));
        assert_eq!(recovered[0].outcome, AgentOutcome::Ok);
        assert!(recovered[0].agents[0].ready);
        assert!(!recovered[0].acknowledged);
        assert!(finish_paste(&writes).is_empty());
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        assert!(replies(&messages, CLIENT).is_empty());
        assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
    }
}

#[test]
fn claude_permission_notification_waits_even_in_auto_mode() {
    for target in pane_targets() {
        for evaluation in [false, true] {
            let (mut screen, _messages, _writes) = setup_on_target("claude", target.clone());
            report_kind(&mut screen, "claude", hook("SessionStart"));
            if evaluation {
                report_kind(
                    &mut screen,
                    "claude",
                    HookEvent {
                        permission_mode: Some("auto".into()),
                        tool_name: Some("Bash".into()),
                        ..hook("PermissionRequest")
                    },
                );
                assert_eq!(
                    screen.get_pane_info(PANE).unwrap().agent.unwrap().state,
                    AgentState::Busy
                );
            }
            for _ in 0..2 {
                report_kind(
                    &mut screen,
                    "claude",
                    HookEvent {
                        permission_mode: Some("auto".into()),
                        notification_type: Some("permission_prompt".into()),
                        message: Some("Allow Bash?".into()),
                        ..hook("Notification")
                    },
                );
                let agent = screen.get_pane_info(PANE).unwrap().agent.unwrap();
                assert_eq!(agent.state, AgentState::Waiting);
                assert_eq!(agent.waiting_on.as_deref(), Some("Allow Bash?"));
                assert!(agent.wait_needs_a_person);
            }
            report_kind(&mut screen, "claude", hook("PostToolUse"));
            let agent = screen.get_pane_info(PANE).unwrap().agent.unwrap();
            assert_eq!(agent.state, AgentState::Busy);
            assert_eq!(agent.waiting_on, None);
        }
    }
}

fn claude_permission_prompt() -> HookEvent {
    HookEvent {
        notification_type: Some("permission_prompt".into()),
        message: Some("Allow Bash?".into()),
        ..hook("Notification")
    }
}

fn permission_key(screen: &mut Screen, key: KeyWithModifier, cli: bool) {
    if cli {
        screen.agent_control(
            AgentRequest::SendKeys {
                pane_id: PANE,
                expected_session_id: None,
                keys: vec![key],
            },
            CLIENT,
        );
    } else {
        let bytes = match key.bare_key {
            BareKey::Enter => b"\r".to_vec(),
            BareKey::Up => b"\x1b[A".to_vec(),
            BareKey::Down => b"\x1b[B".to_vec(),
            BareKey::Char(c) => c.to_string().into_bytes(),
            _ => panic!("unsupported fixture key"),
        };
        screen
            .tab_containing_pane_mut(PANE)
            .unwrap()
            .write_to_active_terminal(&Some(key), bytes, false, CLIENT, None)
            .unwrap();
    }
    screen.refresh_agent_controls();
}

#[test]
fn claude_permission_enter_clears_wait_before_tool_completion_in_every_placement() {
    use yinz_types::data::{OverlayKind, OverlayPlacement};
    for placement in [
        NewPanePlacement::default(),
        NewPanePlacement::Floating(None),
        NewPanePlacement::Overlay(OverlayPlacement::new(OverlayKind::Modal, None)),
    ] {
        for cli in [false, true] {
            let (mut screen, messages, writes) = setup_with_placement(
                "claude",
                PaneTarget::Local,
                Size { cols: 80, rows: 24 },
                placement.clone(),
            );
            report_kind(&mut screen, "claude", hook("SessionStart"));
            permission_key(&mut screen, KeyWithModifier::new(BareKey::Enter), cli);
            report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
            let (sender, plugins) = channels::unbounded();
            screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
            screen.background_plugin_subscriptions.insert(
                (PluginId::new(99), CLIENT),
                HashSet::from([EventType::PaneUpdate]),
            );
            for _ in 0..2 {
                report_kind(&mut screen, "claude", claude_permission_prompt());
                let waiting = screen.get_pane_info(PANE).unwrap().agent.unwrap();
                assert_eq!(
                    waiting.state,
                    AgentState::Waiting,
                    "old Enter is not an answer"
                );
                for key in [
                    KeyWithModifier::new(BareKey::Down),
                    KeyWithModifier::new(BareKey::Up),
                    KeyWithModifier::new(BareKey::Char('y')),
                    KeyWithModifier::new(BareKey::Enter).with_shift_modifier(),
                    KeyWithModifier::new(BareKey::Enter).with_ctrl_modifier(),
                ] {
                    permission_key(&mut screen, key, cli);
                    assert_eq!(
                        screen.get_pane_info(PANE).unwrap().agent,
                        Some(waiting.clone())
                    );
                }
                screen
                    .tab_containing_pane_mut(PANE)
                    .unwrap()
                    .paste_to_pane_id(b"yes\r\n".to_vec(), PANE, None, None)
                    .unwrap();
                screen.refresh_agent_controls();
                assert_eq!(screen.get_pane_info(PANE).unwrap().agent, Some(waiting));
                plugins.try_iter().for_each(drop);
                finish_paste(&writes);
                permission_key(&mut screen, KeyWithModifier::new(BareKey::Enter), cli);
                assert_eq!(finish_paste(&writes), b"\r");
                let busy = screen.get_pane_info(PANE).unwrap().agent.unwrap();
                assert_eq!(busy.state, AgentState::Busy);
                assert_eq!(busy.waiting_on, None);
                assert_eq!(busy.detail, None);
                assert!(!busy.wait_needs_a_person);
                assert!(screen.claude_permission_waits.is_empty());
                assert!(plugins.try_iter().any(|(instruction, _)| {
                    let PluginInstruction::Update(updates) = instruction else { return false };
                    updates.into_iter().any(|(_, _, event)| {
                        matches!(event, Event::PaneUpdate(manifest) if manifest.panes.values().flatten()
                            .any(|pane| pane.id == PANE && pane.agent.as_ref() == Some(&busy)))
                    })
                }));
                report_kind(&mut screen, "claude", hook("PostToolUse"));
                assert_eq!(
                    screen.get_pane_info(PANE).unwrap().agent.unwrap().state,
                    AgentState::Busy
                );
            }
            report_kind(&mut screen, "claude", hook("Stop"));
            assert_eq!(
                screen.get_pane_info(PANE).unwrap().agent.unwrap().state,
                AgentState::Idle
            );
            assert!(replies(&messages, CLIENT)
                .iter()
                .all(|reply| reply.outcome == AgentOutcome::Ok));
        }
    }
}

#[test]
fn claude_permission_enter_ignores_other_waits_and_replaced_conversations() {
    for case in [
        "question",
        "elicitation",
        "conversation",
        "direct",
        "codex",
        "pi",
    ] {
        let kind = if matches!(case, "codex" | "pi") {
            case
        } else {
            "claude"
        };
        let (mut screen, _, _writes) = setup(kind);
        report_kind(&mut screen, kind, hook("SessionStart"));
        if kind == "claude" {
            report_kind(&mut screen, kind, claude_permission_prompt());
        }
        let mut replacement = permission_wait(kind);
        if case == "question" {
            replacement = HookEvent {
                tool_name: Some("AskUserQuestion".into()),
                ..hook("PreToolUse")
            };
        } else if case == "conversation" {
            replacement.session_id = "new-conversation".into();
        }
        if case == "direct" {
            let agent = screen.get_pane_info(PANE).unwrap().agent;
            crate::screen::apply_pane_agent_report(&mut screen, PANE, agent).unwrap();
        } else {
            report_kind(&mut screen, kind, replacement);
        }
        let waiting = screen.get_pane_info(PANE).unwrap().agent.unwrap();
        assert_eq!(waiting.state, AgentState::Waiting, "{case}");
        assert!(screen.claude_permission_waits.is_empty(), "{case}");
        permission_key(&mut screen, KeyWithModifier::new(BareKey::Enter), true);
        assert_eq!(
            screen.get_pane_info(PANE).unwrap().agent,
            Some(waiting),
            "{case}"
        );
    }
}

#[test]
fn claude_permission_enter_preserves_unrelated_worker_waits() {
    for worker_first in [false, true] {
        let (mut screen, _, _writes) = setup("claude");
        report_kind(&mut screen, "claude", hook("SessionStart"));
        let worker = HookEvent {
            agent_id: Some("worker".into()),
            tool_use_id: Some("worker-question".into()),
            tool_name: Some("AskUserQuestion".into()),
            ..hook("PreToolUse")
        };
        if worker_first {
            report_kind(&mut screen, "claude", worker.clone());
        }
        report_kind(&mut screen, "claude", claude_permission_prompt());
        if !worker_first {
            report_kind(&mut screen, "claude", worker.clone());
        }
        let waiting = screen.get_pane_info(PANE).unwrap().agent;
        permission_key(&mut screen, KeyWithModifier::new(BareKey::Enter), true);
        assert_eq!(screen.get_pane_info(PANE).unwrap().agent, waiting);
        assert!(screen.claude_permission_waits.is_empty());
        assert!(screen.agent_worker_waits.contains_key(&PANE));
        report_kind(
            &mut screen,
            "claude",
            HookEvent {
                hook_event_name: "PostToolUse".into(),
                ..worker
            },
        );
        let agent = screen.get_pane_info(PANE).unwrap().agent.unwrap();
        assert_eq!(agent.state, AgentState::Busy);
        assert_eq!(agent.waiting_on, None);
        assert!(!agent.wait_needs_a_person);
    }
}

#[test]
fn claude_permission_wait_requires_forwarded_enter_and_stays_with_its_pane() {
    let (mut screen, _, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    report_kind(&mut screen, "claude", claude_permission_prompt());
    let waiting = screen.get_pane_info(PANE).unwrap().agent;
    screen
        .tab_containing_pane_mut(PANE)
        .unwrap()
        .get_pane_with_id_mut(PANE)
        .unwrap()
        .set_suspended(true);
    finish_paste(&writes);
    permission_key(&mut screen, KeyWithModifier::new(BareKey::Enter), false);
    assert!(finish_paste(&writes).is_empty());
    assert_eq!(screen.get_pane_info(PANE).unwrap().agent, waiting);
    screen
        .tab_containing_pane_mut(PANE)
        .unwrap()
        .get_pane_with_id_mut(PANE)
        .unwrap()
        .set_suspended(false);
    new_tab(&mut screen, 2, TabId::new(1));
    screen
        .tabs
        .get_mut(&TabId::new(1))
        .unwrap()
        .write_to_active_terminal(
            &Some(KeyWithModifier::new(BareKey::Enter)),
            b"\r".to_vec(),
            false,
            CLIENT,
            None,
        )
        .unwrap();
    screen.refresh_agent_controls();
    assert_eq!(screen.get_pane_info(PANE).unwrap().agent, waiting);
    let moved = screen
        .break_multiple_panes_to_tab_with_id(vec![PANE], TabId::new(1), false, CLIENT)
        .unwrap();
    assert!(moved.succeeded());
    screen.refresh_agent_controls();
    assert_eq!(screen.get_pane_info(PANE).unwrap().agent, waiting);
    permission_key(&mut screen, KeyWithModifier::new(BareKey::Enter), true);
    assert_eq!(
        screen.get_pane_info(PANE).unwrap().agent.unwrap().state,
        AgentState::Busy
    );
    report_kind(&mut screen, "claude", claude_permission_prompt());
    let waiting = screen.get_pane_info(PANE).unwrap().agent;
    drop(writes);
    let tab = screen.tab_containing_pane_mut(PANE).unwrap();
    tab.senders.should_silently_fail = false;
    assert!(tab
        .write_key_to_pane_id(PANE, KeyWithModifier::new(BareKey::Enter), None)
        .is_err());
    tab.senders.should_silently_fail = true;
    screen.refresh_agent_controls();
    assert_eq!(screen.get_pane_info(PANE).unwrap().agent, waiting);
    screen.close_pane_explicitly(PANE, None);
    assert!(screen.claude_permission_waits.is_empty());
}

#[test]
fn claude_permission_tracking_requires_an_accepted_state_report() {
    for event in [
        HookEvent {
            archive_only: true,
            ..claude_permission_prompt()
        },
        HookEvent {
            agent_id: Some("worker".into()),
            ..claude_permission_prompt()
        },
        HookEvent {
            notification_type: Some("worker_permission_prompt".into()),
            ..claude_permission_prompt()
        },
    ] {
        let (mut screen, _, _writes) = setup("claude");
        report_kind(&mut screen, "claude", hook("SessionStart"));
        report_kind(&mut screen, "claude", event);
        assert!(screen.claude_permission_waits.is_empty());
    }
    let (mut screen, _, _writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    report_kind(&mut screen, "claude", hook("Interrupt"));
    report_kind(&mut screen, "claude", claude_permission_prompt());
    assert!(screen.claude_permission_waits.is_empty());
}

#[test]
fn claude_worker_permission_reminders_preserve_parent_state_and_worker_waits() {
    for target in pane_targets() {
        for parent_state in [AgentState::Idle, AgentState::Busy] {
            let (mut screen, messages, _writes) = setup_on_target("claude", target.clone());
            report_kind(&mut screen, "claude", hook("SessionStart"));
            if parent_state == AgentState::Busy {
                report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
            }
            let parent = screen.get_pane_info(PANE).unwrap().agent.unwrap();
            let reminder = HookEvent {
                notification_type: Some("worker_permission_prompt".into()),
                message: Some("A teammate needs permission".into()),
                ..hook("Notification")
            };
            // Real teammate reminders have no agent_id; they cannot replace the parent.
            for _ in 0..2 {
                report_kind(&mut screen, "claude", reminder.clone());
                assert_eq!(
                    screen.get_pane_info(PANE).unwrap().agent,
                    Some(parent.clone())
                );
                assert!(screen.agent_worker_waits.is_empty());
            }
            let mut worker = HookEvent {
                agent_id: Some("worker".into()),
                tool_name: Some("Bash".into()),
                ..hook("SubagentStop")
            };
            report_kind(&mut screen, "claude", worker.clone());
            assert_eq!(
                screen.get_pane_info(PANE).unwrap().agent,
                Some(parent.clone())
            );

            // The previous worker stopped; subsequent waits belong to another worker.
            worker.agent_id = Some("live-worker".into());
            worker.hook_event_name = "PermissionRequest".into();
            for mode in ["plan", "default", "acceptEdits", "auto"] {
                worker.permission_mode = Some(mode.into());
                report_kind(&mut screen, "claude", worker.clone());
                assert_eq!(
                    screen.get_pane_info(PANE).unwrap().agent,
                    Some(parent.clone())
                );
                assert!(screen.agent_worker_waits.is_empty());
            }
            for tool in ["AskUserQuestion", "ExitPlanMode"] {
                worker.tool_name = Some(tool.into());
                worker.tool_use_id = Some(format!("input-{tool}"));
                worker.hook_event_name = "PreToolUse".into();
                report_kind(&mut screen, "claude", worker.clone());
                worker.hook_event_name = "PermissionRequest".into();
                report_kind(&mut screen, "claude", worker.clone());
                let waiting = screen.get_pane_info(PANE).unwrap().agent.unwrap();
                assert_eq!(waiting.state, AgentState::Waiting);
                report_kind(&mut screen, "claude", reminder.clone());
                assert_eq!(screen.get_pane_info(PANE).unwrap().agent, Some(waiting));
                assert!(screen.agent_worker_waits.contains_key(&PANE));
                worker.hook_event_name = "PostToolUse".into();
                report_kind(&mut screen, "claude", worker.clone());
                assert_eq!(
                    screen.get_pane_info(PANE).unwrap().agent,
                    Some(parent.clone())
                );
                assert!(screen.agent_worker_waits.is_empty());
            }
            worker.tool_use_id = Some("eliciting-tool".into());
            worker.hook_event_name = "Elicitation".into();
            report_kind(&mut screen, "claude", worker.clone());
            report_kind(&mut screen, "claude", reminder);
            assert_eq!(
                screen.get_pane_info(PANE).unwrap().agent.unwrap().state,
                AgentState::Waiting
            );
            worker.hook_event_name = "ElicitationResult".into();
            report_kind(&mut screen, "claude", worker);
            assert_eq!(screen.get_pane_info(PANE).unwrap().agent, Some(parent));
            assert!(screen.agent_worker_waits.is_empty());
            assert!(replies(&messages, CLIENT).is_empty());
        }
    }
}

#[test]
fn concurrent_worker_waits_restore_latest_parent_and_keep_identity() {
    for target in pane_targets() {
        for name in ["PermissionRequest", "PreToolUse", "Elicitation"] {
            let (mut screen, messages, writes) = setup_on_target("codex", target.clone());
            let mut start = hook("SessionStart");
            start.cwd = Some("/parent".into());
            report(&mut screen, start.clone());
            screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
            finish_paste(&writes);
            report(&mut screen, hook("UserPromptSubmit"));
            for id in ["one", "two"] {
                let mut worker = hook(name);
                worker.agent_id = Some(id.into());
                worker.tool_name = Some("AskUserQuestion".into());
                worker.cwd = Some("/worker".into());
                report(&mut screen, worker);
                let agent = screen.get_pane_info(PANE).unwrap().agent.unwrap();
                assert_eq!(agent.state, AgentState::Waiting);
                assert_eq!(agent.session_id.as_deref(), Some("conversation"));
                assert_ne!(agent.cwd.as_deref(), Some("/worker"));
                if id == "one" {
                    let mut updated = agent;
                    updated.run_id = Some("new-run".into());
                    updated.cwd = Some("/new-checkout".into());
                    screen
                        .tab_containing_pane_mut(PANE)
                        .unwrap()
                        .set_pane_agent(PANE, Some(updated));
                } else {
                    assert_eq!(agent.run_id.as_deref(), Some("new-run"));
                    assert_eq!(agent.cwd.as_deref(), Some("/new-checkout"));
                }
            }
            assert_eq!(
                replies(&messages, CLIENT)[0].outcome,
                AgentOutcome::NeedsInput
            );
            report(&mut screen, hook("Stop"));
            for (id, end, expected) in [
                ("one", "PostToolUse", AgentState::Waiting),
                ("two", "SubagentStop", AgentState::Idle),
            ] {
                let mut worker = hook(end);
                worker.agent_id = Some(id.into());
                report(&mut screen, worker);
                assert_eq!(
                    screen.get_pane_info(PANE).unwrap().agent.unwrap().state,
                    expected
                );
            }
            screen.agent_control(
                AgentRequest::Status {
                    pane_id: PANE,
                    expected_session_id: None,
                },
                CLIENT,
            );
            assert!(replies(&messages, CLIENT)[0].agents[0].ready);
            assert!(screen.agent_worker_waits.is_empty());
            let mut automatic = hook("PermissionRequest");
            automatic.agent_id = Some("auto".into());
            automatic.approvals_reviewer = Some("auto_review".into());
            report(&mut screen, automatic);
            assert_eq!(
                screen.get_pane_info(PANE).unwrap().agent.unwrap().state,
                AgentState::Idle
            );
        }
    }
}

#[test]
fn agent_spawn_placement_works_before_keyboard_input_and_without_clients() {
    for detached in [false, true] {
        let (mut screen, _, _) = setup("codex");
        if detached {
            screen.active_tab_ids.clear();
        }
        let pane = PaneId::Terminal(TerminalId::new(88));
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        screen
            .place_new_pane(
                pane,
                None,
                None,
                Some(Run::Command(RunCommand {
                    command: "env".into(),
                    args: vec![
                        format!("{}=1", yinz_types::agent::AGENT_PANE_ENV),
                        "codex".into(),
                    ],
                    ..Default::default()
                })),
                NewPanePlacement::default(),
                false,
                crate::pty::ClientTabIndexOrPaneId::ClientId(ClientId::new(99)),
                Some(crate::route::NotificationEnd::new(tx)),
                false,
                &mut Default::default(),
            )
            .unwrap();
        assert!(screen.tab_containing_pane(pane).is_some());
        assert_eq!(rx.try_recv().unwrap().affected_pane_id, Some(pane));
    }
}

#[test]
fn programmatic_mouse_reports_do_not_interrupt_agent_but_typing_does() {
    for mode in [
        b"\x1b[?1000h\x1b[?1006h".as_slice(),
        b"\x1b[?1049h\x1b[?1000h\x1b[?1006h",
    ] {
        let (mut screen, messages, writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        finish_paste(&writes);
        report(&mut screen, hook("UserPromptSubmit"));
        let tab = screen.tab_containing_pane_mut(PANE).unwrap();
        tab.handle_pty_bytes(TerminalId::new(7), mode.to_vec())
            .unwrap();
        let revision = tab.agent_input_revision(PANE);
        tab.scroll_terminal_up(TerminalId::new(7)).unwrap();
        tab.scroll_terminal_down(TerminalId::new(7)).unwrap();
        assert_eq!(tab.agent_input_revision(PANE), revision);
        assert!(!finish_paste(&writes).is_empty());
        screen.refresh_agent_controls();
        assert!(replies(&messages, CLIENT).is_empty());
        screen.agent_write_for_test();
        screen.refresh_agent_controls();
        assert_eq!(
            replies(&messages, CLIENT)[0].outcome,
            AgentOutcome::Interrupted
        );
    }
}

#[test]
fn mouse_reports_preserve_agent_input_ownership() {
    use yinz_types::mouse::MouseEvent;
    use yinz_types::position::Position;
    for mode in [
        b"\x1b[?1000h\x1b[?1006h".as_slice(),
        b"\x1b[?1049h\x1b[?1000h\x1b[?1006h",
    ] {
        let (mut screen, messages, writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        finish_paste(&writes);
        report(&mut screen, hook("UserPromptSubmit"));
        let tab = screen.tab_containing_pane_mut(PANE).unwrap();
        tab.handle_pty_bytes(TerminalId::new(7), mode.to_vec())
            .unwrap();
        let revision = tab.agent_input_revision(PANE);
        let pane = tab.get_pane_with_id(PANE).unwrap();
        let position = Position::new((pane.y() + 2) as i32, (pane.x() + 2) as u16);
        screen.handle_mouse_event(MouseEvent::new_left_press_event(position), CLIENT);
        screen.handle_mouse_event(MouseEvent::new_left_motion_event(position), CLIENT);
        screen.handle_mouse_event(MouseEvent::new_left_release_event(position), CLIENT);
        screen.handle_mouse_event(MouseEvent::new_scroll_up_event(position), CLIENT);
        screen.handle_mouse_event(MouseEvent::new_scroll_down_event(position), CLIENT);
        assert_eq!(
            screen
                .tab_containing_pane(PANE)
                .unwrap()
                .agent_input_revision(PANE),
            revision
        );
        assert!(!finish_paste(&writes).is_empty());
        screen.refresh_agent_controls();
        assert!(replies(&messages, CLIENT).is_empty());
    }
}

#[test]
fn synthesized_scroll_arrows_interrupt_agent_before_delayed_enter() {
    use yinz_types::mouse::MouseEvent;
    use yinz_types::position::Position;

    for mouse_wheel in [false, true] {
        for up in [false, true] {
            let (mut screen, messages, writes) = setup("codex");
            report(&mut screen, hook("SessionStart"));
            screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
            assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
            let now = Instant::now();
            screen.refresh_agent_controls_at(now);
            let tab = screen.tab_containing_pane_mut(PANE).unwrap();
            tab.handle_pty_bytes(TerminalId::new(7), b"\x1b[?1049h".to_vec())
                .unwrap();
            let revision = tab.agent_input_revision(PANE);
            if mouse_wheel {
                let pane = tab.get_pane_with_id(PANE).unwrap();
                let position = Position::new((pane.y() + 2) as i32, (pane.x() + 2) as u16);
                let event = if up {
                    MouseEvent::new_scroll_up_event(position)
                } else {
                    MouseEvent::new_scroll_down_event(position)
                };
                screen.handle_mouse_event(event, CLIENT);
            } else if up {
                tab.scroll_terminal_up(TerminalId::new(7)).unwrap();
            } else {
                tab.scroll_terminal_down(TerminalId::new(7)).unwrap();
            }
            let lines = if mouse_wheel { 3 } else { 1 };
            let arrow = if up { b"\x1b[A" } else { b"\x1b[B" };
            assert_eq!(finish_paste(&writes), arrow.repeat(lines));
            assert_eq!(
                screen
                    .tab_containing_pane(PANE)
                    .unwrap()
                    .agent_input_revision(PANE),
                revision + lines as u64
            );
            screen.refresh_agent_controls_at(now + Duration::from_millis(501));
            assert_eq!(
                replies(&messages, CLIENT)[0].outcome,
                AgentOutcome::Interrupted
            );
            assert!(finish_paste(&writes).is_empty(), "Enter must be cancelled");
        }
    }
}

#[test]
fn lifecycle_survives_moving_a_pane_and_is_cleared_on_closure() {
    let (mut screen, messages, _) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    screen.agent_launches.insert(
        PANE,
        yinz_types::agent::SpawnedAgent {
            pane_id: PANE,
            cwd: Some("/checkout".into()),
            branch: Some("branch".into()),
            ..Default::default()
        },
    );
    new_tab(&mut screen, 9, TabId::new(1));
    assert!(screen
        .break_multiple_panes_to_tab_with_id(vec![PANE], TabId::new(1), false, CLIENT)
        .unwrap()
        .succeeded());
    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    assert!(replies(&messages, CLIENT)[0].agents[0].ready);
    screen.close_pane_explicitly(PANE, None);
    assert!(!screen.agent_lifecycle.contains_key(&PANE));
    assert!(!screen.agent_launches.contains_key(&PANE));
    assert!(!screen.agent_worker_waits.contains_key(&PANE));
}

#[test]
fn recovery_rejects_busy_waiting_unreported_and_dead_targets() {
    for condition in [
        "busy",
        "waiting",
        "unreported",
        "closed",
        "unsupported",
        "exited",
    ] {
        let kind = if condition == "unsupported" {
            "opencode"
        } else {
            "codex"
        };
        let (mut screen, messages, writes) = setup(kind);
        if condition != "unreported" {
            report_kind(&mut screen, kind, hook("SessionStart"));
        }
        match condition {
            "busy" => report(&mut screen, hook("UserPromptSubmit")),
            "waiting" => report(&mut screen, hook("PermissionRequest")),
            "closed" => screen.close_pane_explicitly(PANE, None),
            "exited" => {
                screen.tab_containing_pane_mut(PANE).unwrap().hold_pane(
                    PANE,
                    Some(0),
                    false,
                    RunCommand {
                        command: "codex".into(),
                        ..Default::default()
                    },
                );
            },
            _ => {},
        }
        screen.agent_control(recover(), CLIENT);
        assert_ne!(
            replies(&messages, CLIENT)[0].outcome,
            AgentOutcome::Ok,
            "{condition}"
        );
        assert!(finish_paste(&writes).is_empty());
    }
}

#[test]
fn interrupted_recovery_resets_a_silent_busy_claude_after_a_quiet_period() {
    let (mut screen, messages, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    finish_paste(&writes);
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    assert!(replies(&messages, CLIENT).is_empty());
    // Claude reports nothing when Esc interrupts the turn.
    screen.agent_control(
        AgentRequest::SendKeys {
            pane_id: PANE,
            expected_session_id: None,
            keys: vec![KeyWithModifier::new(BareKey::Esc)],
        },
        ClientId::new(2),
    );
    replies(&messages, ClientId::new(2));
    screen.refresh_agent_controls();
    assert_eq!(
        replies(&messages, CLIENT)[0].outcome,
        AgentOutcome::Interrupted
    );
    finish_paste(&writes);
    let interrupted = AgentRequest::Recover {
        pane_id: PANE,
        expected_session_id: "conversation".into(),
        confirm_empty: true,
        interrupted: true,
    };
    screen.agent_control(recover(), CLIENT);
    let plain = replies(&messages, CLIENT);
    assert_eq!(plain[0].outcome, AgentOutcome::Rejected);
    assert!(plain[0]
        .message
        .as_deref()
        .unwrap()
        .contains("--interrupted"));
    screen.agent_control(interrupted.clone(), CLIENT);
    assert_eq!(
        replies(&messages, CLIENT)[0].outcome,
        AgentOutcome::Rejected,
        "a hook may still be on its way"
    );
    let (to_plugin, plugins) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
    recover_after_quiet(&mut screen, interrupted);
    let recovered = replies(&messages, CLIENT);
    assert_eq!(recovered[0].outcome, AgentOutcome::Ok);
    assert_eq!(recovered[0].agents[0].agent.state, AgentState::Idle);
    assert!(recovered[0].agents[0].ready);
    let events: Vec<_> = plugins
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::Update(updates) => Some(updates),
            _ => None,
        })
        .flatten()
        .filter_map(|(_, _, event)| match event {
            yinz_types::data::Event::AgentEvent(event) => Some(event),
            _ => None,
        })
        .collect();
    assert_eq!(events.len(), 1, "plugins see the interrupted turn end");
    assert_eq!(events[0].hook, "Interrupt");
    assert_eq!(events[0].kind, "claude");
    assert_eq!(events[0].session_id.as_deref(), Some("conversation"));
    assert_eq!(events[0].state, Some(AgentState::Idle));
    let raw: HookEvent = serde_json::from_str(&events[0].raw).unwrap();
    assert_eq!(raw.hook_event_name, "Interrupt");
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    assert!(replies(&messages, CLIENT).is_empty());
    assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
}

fn interrupted_recover() -> AgentRequest {
    AgentRequest::Recover {
        pane_id: PANE,
        expected_session_id: "conversation".into(),
        confirm_empty: true,
        interrupted: true,
    }
}

fn recover_after_quiet(screen: &mut Screen, request: AgentRequest) {
    let quiet = crate::screen::agent_control::RECOVER_QUIET;
    screen.agent_control_at(
        request,
        crate::screen::agent_control::Reply::Client(CLIENT),
        Instant::now() + quiet + Duration::from_millis(1),
    );
}

fn send_esc(screen: &mut Screen, messages: &CapturedClientMessages) {
    screen.agent_control(
        AgentRequest::SendKeys {
            pane_id: PANE,
            expected_session_id: None,
            keys: vec![KeyWithModifier::new(BareKey::Esc)],
        },
        ClientId::new(2),
    );
    assert_eq!(
        replies(messages, ClientId::new(2))[0].outcome,
        AgentOutcome::Ok
    );
}

#[test]
fn interrupted_recovery_of_a_waiting_agent_requires_input_after_its_last_report() {
    let (mut screen, messages, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    screen.agent_control_with_clear(prompt(false), CLIENT, &writes);
    finish_paste(&writes);
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    replies(&messages, CLIENT);
    let mut permission = permission_wait("claude");
    permission.tool_name = Some("Bash".into());
    report_kind(&mut screen, "claude", permission);
    let state = screen.get_pane_info(PANE).unwrap().agent.unwrap().state;
    assert_eq!(state, AgentState::Waiting);
    // A dialog nobody acted on is quiet, however long it waits.
    recover_after_quiet(&mut screen, interrupted_recover());
    let refused = replies(&messages, CLIENT);
    assert_eq!(refused[0].outcome, AgentOutcome::Rejected);
    assert!(refused[0].message.as_deref().unwrap().contains("Esc"));
    // A worker hook after the input shows the agent is still reporting.
    send_esc(&mut screen, &messages);
    let mut worker = hook("SubagentStop");
    worker.agent_id = Some("worker".into());
    report_kind(&mut screen, "claude", worker);
    recover_after_quiet(&mut screen, interrupted_recover());
    assert_eq!(
        replies(&messages, CLIENT)[0].outcome,
        AgentOutcome::Rejected,
        "a worker hook follows the input"
    );
    send_esc(&mut screen, &messages);
    recover_after_quiet(&mut screen, interrupted_recover());
    let recovered = replies(&messages, CLIENT);
    assert_eq!(recovered[0].outcome, AgentOutcome::Ok);
    assert_eq!(recovered[0].agents[0].agent.state, AgentState::Idle);
    assert!(recovered[0].agents[0].ready);
}

#[test]
fn recovery_rejections_name_the_check_that_failed() {
    let (mut screen, messages, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    finish_paste(&writes);
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    let message = |screen: &mut Screen, request| {
        screen.agent_control_with_clear(request, ClientId::new(3), &writes);
        let response = replies(&messages, ClientId::new(3)).remove(0);
        assert_eq!(response.outcome, AgentOutcome::Rejected);
        response.message.unwrap()
    };
    let unconfirmed = message(
        &mut screen,
        AgentRequest::Recover {
            pane_id: PANE,
            expected_session_id: "conversation".into(),
            confirm_empty: false,
            interrupted: true,
        },
    );
    assert!(unconfirmed.contains("--confirm-empty"), "{unconfirmed}");
    assert!(!unconfirmed.contains("--interrupted"), "{unconfirmed}");
    let in_flight = message(&mut screen, interrupted_recover());
    assert!(in_flight.contains("in flight"), "{in_flight}");
    let busy = message(&mut screen, recover());
    assert!(busy.contains("in flight"), "{busy}");
}

#[test]
fn collapsed_claude_pastes_acknowledge_the_prompt_they_wrap() {
    let (mut screen, messages, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    let text = (1..=10)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    screen.agent_control_with_clear(
        AgentRequest::Prompt {
            preserve_draft: false,
            submission_id: None,
            pane_id: PANE,
            expected_session_id: None,
            prompt: text.clone(),
            wait: true,
            timeout_ms: 30_000,
        },
        CLIENT,
        &writes,
    );
    let lead_in = yinz_types::agent::control::PASTE_LEAD_IN;
    assert_eq!(
        finish_paste(&writes),
        format!("{lead_in} \x1b[200~{text}\x1b[201~").into_bytes(),
        "the lead-in is typed ahead of the paste"
    );
    let mut submit = hook("UserPromptSubmit");
    submit.prompt = Some(format!(
        "{lead_in} \n\n<pasted_content id=\"8c9e\">\n{text}\n</pasted_content id=\"8c9e\">\n"
    ));
    report_kind(&mut screen, "claude", submit);
    report_kind(&mut screen, "claude", hook("Stop"));
    let replies = replies(&messages, CLIENT);
    assert_eq!(
        replies
            .iter()
            .map(|reply| reply.outcome)
            .collect::<Vec<_>>(),
        [AgentOutcome::Completed]
    );
    assert!(replies[0].acknowledged);
    assert!(replies[0].agents[0].ready);
    screen.agent_control_with_clear(prompt(false), CLIENT, &writes);
    assert_eq!(
        finish_paste(&writes),
        b"\x1b[200~review\x1b[201~",
        "a short prompt is pasted alone"
    );
}

#[test]
fn prompt_delivery_normalizes_only_claude_format_controls() {
    for kind in ["claude", "codex", "pi"] {
        for text in [
            "review \u{200b}text".into(),
            "review \u{202e}text".into(),
            "review \u{200b}\u{202e}👩\u{200d}💻 العربية".into(),
            "\u{200b}line one\nline two\u{202e}".into(),
            format!("\u{200b}{}\u{202e}", "long ".repeat(120)),
            format!("{}\u{200b}\u{202e}", "a".repeat(499)),
            "\u{200b}\u{202e} \t\n".into(),
        ] {
            let (mut screen, messages, writes) = setup(kind);
            report_kind(&mut screen, kind, hook("SessionStart"));
            screen.agent_control_with_clear(
                AgentRequest::Prompt {
                    preserve_draft: false,
                    submission_id: None,
                    pane_id: PANE,
                    expected_session_id: None,
                    prompt: text.clone(),
                    wait: true,
                    timeout_ms: 30_000,
                },
                CLIENT,
                &writes,
            );
            let delivered = if kind == "claude" {
                text.replace(['\u{200b}', '\u{202e}'], "")
            } else {
                text.clone()
            };
            if delivered.trim().is_empty() {
                let response = replies(&messages, CLIENT).pop().unwrap();
                assert_eq!(response.outcome, AgentOutcome::Rejected);
                assert!(response.agents[0].ready);
                assert!(screen.agent_controls.is_empty());
                assert!(finish_paste(&writes).is_empty());
                continue;
            }
            let lead =
                kind == "claude" && (delivered.trim().contains('\n') || delivered.len() >= 500);
            let prefix = if lead {
                format!("{} ", yinz_types::agent::control::PASTE_LEAD_IN)
            } else {
                String::new()
            };
            assert_eq!(
                finish_paste(&writes),
                format!("{prefix}\x1b[200~{delivered}\x1b[201~").as_bytes()
            );
            let mut submit = hook("UserPromptSubmit");
            submit.prompt = Some(if lead {
                format!("{prefix}<pasted_content id=\"1\">{delivered}</pasted_content id=\"1\">")
            } else {
                delivered
            });
            report_kind(&mut screen, kind, submit);
            report_kind(&mut screen, kind, hook("Stop"));
            let response = replies(&messages, CLIENT).pop().unwrap();
            assert_eq!(response.outcome, AgentOutcome::Completed, "{kind}: {text}");
            assert!(response.acknowledged);
            assert!(response.agents[0].ready);
        }
    }
}

#[test]
fn commands_are_rejected_before_claiming_or_writing_input() {
    for kind in ["claude", "codex", "pi"] {
        for command in [
            "/probe",
            "/cost",
            " /probe\narguments",
            "/clear ",
            "\t/clear",
            " /préfixe ",
        ]
        .into_iter()
        .chain((kind == "claude").then_some("!printf BANG_OK"))
        .chain((kind == "claude").then_some("\u{200b} \u{202e}/probe"))
        .chain((kind == "claude").then_some("\u{200b}/clear"))
        .chain((kind == "claude").then_some("\u{202e}!printf hidden"))
        {
            let (mut screen, messages, writes) = setup(kind);
            report_kind(&mut screen, kind, hook("SessionStart"));
            screen.agent_control_with_clear(
                AgentRequest::Prompt {
                    preserve_draft: false,
                    submission_id: None,
                    pane_id: PANE,
                    expected_session_id: None,
                    prompt: command.into(),
                    wait: true,
                    timeout_ms: 30_000,
                },
                CLIENT,
                &writes,
            );
            let response = replies(&messages, CLIENT).pop().unwrap();
            assert_eq!(
                response.outcome,
                AgentOutcome::Rejected,
                "{kind}: {command}"
            );
            assert!(response.message.unwrap().contains("send-keys"));
            assert!(response.agents[0].ready);
            assert!(screen.agent_controls.is_empty());
            assert!(finish_paste(&writes).is_empty());
        }
    }
}

#[test]
fn host_bridge_and_cli_share_readiness_identity_outcomes_and_recovery() {
    for bridge in [false, true] {
        for end in ["Stop", "StopFailure", "Interrupt", "timeout", "replacement"] {
            let (mut screen, messages, writes) = setup("codex");
            let (sender, receiver) = std::sync::mpsc::channel();
            let submit = |screen: &mut Screen, request| {
                if bridge {
                    screen.agent_control_with_clear_reply(
                        request,
                        crate::screen::agent_control::Reply::Host(sender.clone()),
                        &writes,
                    );
                } else {
                    screen.agent_control_with_clear(request, CLIENT, &writes);
                }
            };
            let answers = || {
                if bridge {
                    receiver.try_iter().collect::<Vec<_>>()
                } else {
                    replies(&messages, CLIENT)
                }
            };
            submit(&mut screen, prompt(true));
            assert_eq!(answers()[0].outcome, AgentOutcome::StatusUnavailable);
            report(&mut screen, hook("SessionStart"));
            submit(
                &mut screen,
                AgentRequest::Status {
                    pane_id: PANE,
                    expected_session_id: Some("wrong".into()),
                },
            );
            assert_eq!(answers()[0].outcome, AgentOutcome::TargetChanged);
            submit(&mut screen, prompt(true));
            assert!(answers().is_empty(), "PTY writes are not acknowledgements");
            submit(&mut screen, prompt(true));
            assert_eq!(answers()[0].outcome, AgentOutcome::Rejected);
            finish_paste(&writes);
            if end == "timeout" {
                screen.agent_controls[0].deadline = Some(Instant::now());
                screen.refresh_agent_controls();
                let result = answers().pop().unwrap();
                assert_eq!(result.outcome, AgentOutcome::TimedOut);
                assert!(!result.acknowledged);
                submit(&mut screen, prompt(true));
                assert_eq!(answers()[0].outcome, AgentOutcome::Rejected);
                report(&mut screen, hook("Stop"));
                submit(&mut screen, recover());
                let result = answers().pop().unwrap();
                assert_eq!(result.outcome, AgentOutcome::Ok);
                assert!(result.agents[0].ready);
            } else if end == "replacement" {
                let mut replacement = hook("SessionStart");
                replacement.session_id = "replacement".into();
                report(&mut screen, replacement);
                assert_eq!(answers()[0].outcome, AgentOutcome::TargetChanged);
            } else {
                report(&mut screen, hook("UserPromptSubmit"));
                assert!(
                    answers().is_empty(),
                    "wait observes this turn after acceptance"
                );
                report(&mut screen, hook(end));
                let result = answers().pop().unwrap();
                assert_eq!(
                    result.outcome,
                    match end {
                        "Stop" => AgentOutcome::Completed,
                        "StopFailure" => AgentOutcome::Failed,
                        _ => AgentOutcome::Interrupted,
                    }
                );
                assert!(result.acknowledged);
                assert_eq!(result.turn_id.as_deref(), Some("turn"));
            }
        }
    }
}

#[test]
fn verified_codex_bootstrap_readies_the_first_automatic_prompt() {
    let (mut screen, messages, writes) = setup("codex");
    screen.agent_control(wait(AgentState::Idle), CLIENT);
    let mut bootstrap = hook("SessionStart");
    bootstrap.codex_hooks_ready = Some(true);
    report(&mut screen, bootstrap);
    let ready = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(ready.outcome, AgentOutcome::Ok);
    assert!(ready.agents[0].ready);
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    // Keep the provider's deferred first-turn SessionStart handling.
    report(&mut screen, hook("SessionStart"));
    report(&mut screen, hook("UserPromptSubmit"));
    assert!(replies(&messages, CLIENT).is_empty());
    report(&mut screen, hook("Stop"));
    let completed = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(completed.outcome, AgentOutcome::Completed);
    assert!(completed.acknowledged);
}

#[test]
fn duplicate_bootstrap_cannot_overwrite_provider_state_or_an_acknowledged_prompt() {
    for waiting in [false, true] {
        let (mut screen, messages, writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        report(&mut screen, hook("UserPromptSubmit"));
        if waiting {
            let mut permission = hook("PermissionRequest");
            permission.permission_mode = Some("default".into());
            report(&mut screen, permission);
            replies(&messages, CLIENT);
        }
        let parent = screen.get_pane_info(PANE).unwrap().agent.unwrap();
        assert_eq!(
            parent.state,
            if waiting {
                AgentState::Waiting
            } else {
                AgentState::Busy
            }
        );
        let (to_plugin, plugins) = channels::unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
        let mut bootstrap = hook("SessionStart");
        bootstrap.codex_hooks_ready = Some(true);
        report(&mut screen, bootstrap);
        assert_eq!(screen.get_pane_info(PANE).unwrap().agent.unwrap(), parent);
        assert!(replies(&messages, CLIENT).is_empty());
        assert_eq!(screen.agent_controls.len(), 1);
        assert!(plugins
            .try_iter()
            .all(|(instruction, _)| !matches!(instruction, PluginInstruction::Update(_))));
        if !waiting {
            report(&mut screen, hook("Stop"));
            assert_eq!(
                replies(&messages, CLIENT).pop().unwrap().outcome,
                AgentOutcome::Completed
            );
        }
    }
}

#[test]
fn unverified_selection_cannot_inherit_lifecycle_readiness() {
    for replacement in [false, true] {
        let (mut screen, messages, writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        let mut bootstrap = hook("SessionStart");
        bootstrap.codex_hooks_ready = Some(false);
        if replacement {
            bootstrap.session_id = "other".into();
        }
        report(&mut screen, bootstrap.clone());
        screen.agent_control(
            AgentRequest::Status {
                pane_id: PANE,
                expected_session_id: None,
            },
            CLIENT,
        );
        let status = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(status.outcome, AgentOutcome::StatusUnavailable);
        assert!(!status.agents[0].ready);
        assert!(!status.agents[0].prompt_supported);
        assert_eq!(
            status.agents[0].agent.session_id.as_deref(),
            Some(bootstrap.session_id.as_str())
        );
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        assert_eq!(
            replies(&messages, CLIENT).pop().unwrap().outcome,
            AgentOutcome::StatusUnavailable
        );
        // Ordinary provider hooks can establish evidence after manual input.
        report(
            &mut screen,
            HookEvent {
                codex_hooks_ready: None,
                ..bootstrap
            },
        );
        screen.agent_control(wait(AgentState::Idle), CLIENT);
        assert_eq!(
            replies(&messages, CLIENT).pop().unwrap().outcome,
            AgentOutcome::Ok
        );
    }
}

fn turn_hook(name: &str, turn: Option<&str>) -> HookEvent {
    HookEvent {
        turn_id: turn.map(str::to_owned),
        tool_name: name.contains("Tool").then(|| "Bash".into()),
        ..hook(name)
    }
}

fn pane_state(screen: &Screen) -> AgentState {
    screen
        .tab_containing_pane(PANE)
        .and_then(|tab| tab.pane_agent_info(PANE))
        .unwrap()
        .state
}

/// Codex finishes a cancelled tool call after its `Interrupt`, and that call's hooks carry the
/// ended turn's id. They must not make the idle agent busy with no way back but a second Esc.
#[test]
fn hooks_from_an_interrupted_codex_turn_leave_the_agent_idle() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    finish_paste(&writes);
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    finish_paste(&writes);
    report(&mut screen, turn_hook("UserPromptSubmit", Some("t1")));
    report(&mut screen, turn_hook("PreToolUse", Some("t1")));
    assert_eq!(pane_state(&screen), AgentState::Busy);
    send_esc(&mut screen, &messages);
    report(&mut screen, turn_hook("Interrupt", Some("t1")));
    assert_eq!(
        replies(&messages, CLIENT)[0].outcome,
        AgentOutcome::Interrupted
    );
    assert_eq!(pane_state(&screen), AgentState::Idle);
    for late in ["PostToolUse", "PostToolUseFailure", "PermissionRequest"] {
        report(&mut screen, turn_hook(late, Some("t1")));
        assert_eq!(pane_state(&screen), AgentState::Idle, "{late}");
    }
    screen.agent_control(recover(), CLIENT);
    let recovered = replies(&messages, CLIENT);
    assert_eq!(recovered[0].outcome, AgentOutcome::Ok);
    assert!(recovered[0].agents[0].ready);

    finish_paste(&writes);
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    finish_paste(&writes);
    report(&mut screen, turn_hook("UserPromptSubmit", Some("t2")));
    report(&mut screen, turn_hook("PostToolUse", Some("t2")));
    assert_eq!(pane_state(&screen), AgentState::Busy);
    report(&mut screen, turn_hook("Stop", Some("t2")));
    let done = replies(&messages, CLIENT);
    assert_eq!(done[0].outcome, AgentOutcome::Completed);
    assert!(done[0].agents[0].ready);
}

#[test]
fn interrupted_accepted_turn_reports_unresolved_until_recovery() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    finish_paste(&writes);
    report(&mut screen, hook("UserPromptSubmit"));
    report(&mut screen, hook("Interrupt"));
    let interrupted = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(interrupted.outcome, AgentOutcome::Interrupted);
    assert!(interrupted.acknowledged);
    assert_eq!(
        interrupted.agents[0].not_ready_reason,
        Some(NotReadyReason::UnresolvedTurn)
    );
    screen.refresh_agent_controls_at(Instant::now() + Duration::from_secs(41));
    assert_eq!(screen.agent_controls.len(), 1);
    for request in [
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        AgentRequest::List { project: None },
        prompt(true),
    ] {
        screen.agent_control_with_clear(request, CLIENT, &writes);
        let response = replies(&messages, CLIENT).pop().unwrap();
        assert!(!response.agents[0].ready);
        assert_eq!(
            response.agents[0].not_ready_reason,
            Some(NotReadyReason::UnresolvedTurn)
        );
        if response.outcome == AgentOutcome::Rejected {
            assert!(response
                .message
                .unwrap()
                .contains("unresolved accepted turn"));
        }
    }
    screen.agent_control(wait(AgentState::Idle), CLIENT);
    assert!(replies(&messages, CLIENT).is_empty());
    screen.refresh_agent_controls_at(Instant::now() + Duration::from_secs(41));
    let timed_out = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(timed_out.outcome, AgentOutcome::TimedOut);
    assert_eq!(
        timed_out.agents[0].not_ready_reason,
        Some(NotReadyReason::UnresolvedTurn)
    );
    assert_eq!(screen.agent_controls.len(), 1);
    screen.agent_control(recover(), CLIENT);
    let recovered = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(recovered.outcome, AgentOutcome::Ok);
    assert!(recovered.agents[0].ready);
    assert_eq!(recovered.agents[0].not_ready_reason, None);
    assert!(screen.agent_controls.is_empty());
}

/// A blocking Stop hook continues the same turn, and a hook without a turn id cannot be
/// placed in one, so neither is dropped.
#[test]
fn work_after_a_stop_or_without_a_turn_id_still_reports_busy() {
    for (kind, turn) in [("codex", Some("t1")), ("claude", None)] {
        let (mut screen, _messages, _writes) = setup(kind);
        report_kind(&mut screen, kind, turn_hook("SessionStart", turn));
        report_kind(&mut screen, kind, turn_hook("UserPromptSubmit", turn));
        report_kind(&mut screen, kind, turn_hook("Stop", turn));
        assert_eq!(pane_state(&screen), AgentState::Idle);
        report_kind(&mut screen, kind, turn_hook("PreToolUse", turn));
        assert_eq!(pane_state(&screen), AgentState::Busy, "{kind} after Stop");
        report_kind(&mut screen, kind, turn_hook("Interrupt", turn));
        report_kind(&mut screen, kind, turn_hook("PostToolUse", turn));
        assert_eq!(
            pane_state(&screen),
            if turn.is_some() {
                AgentState::Idle
            } else {
                AgentState::Busy
            },
            "{kind} after Interrupt"
        );
    }
}

#[test]
fn claude_pastes_wait_for_settling_and_quiet_output_and_enter_only_once() {
    for text in [
        "review \u{200b}\u{202e}👩\u{200d}💻".into(),
        "review ".repeat(10000),
    ] {
        for manual in [false, true] {
            let (mut screen, messages, writes) = setup("claude");
            report_kind(&mut screen, "claude", hook("SessionStart"));
            screen.agent_control_with_clear(
                AgentRequest::Prompt {
                    preserve_draft: false,
                    submission_id: None,
                    pane_id: PANE,
                    expected_session_id: None,
                    prompt: text.clone(),
                    wait: true,
                    timeout_ms: 30_000,
                },
                CLIENT,
                &writes,
            );
            let written = finish_paste(&writes);
            let normalized = yinz_types::agent::control::claude_prompt_text(&text);
            assert!(written
                .windows(normalized.len())
                .any(|part| part == normalized.as_bytes()));
            let now = Instant::now();
            screen.refresh_agent_controls_at(now);
            screen.refresh_agent_controls_at(now + Duration::from_millis(499));
            assert!(finish_paste(&writes).is_empty());
            screen
                .pane_output_activity
                .insert(PANE, now + Duration::from_millis(480));
            screen.refresh_agent_controls_at(now + Duration::from_millis(500));
            assert!(finish_paste(&writes).is_empty());
            if manual {
                screen.agent_control(send_keys(vec![BareKey::Char('x')]), CLIENT);
                assert_eq!(finish_paste(&writes), b"x");
                replies(&messages, CLIENT);
            }
            screen.refresh_agent_controls_at(now + Duration::from_millis(579));
            assert!(finish_paste(&writes).is_empty());
            screen.refresh_agent_controls_at(now + Duration::from_millis(580));
            if manual {
                assert!(finish_paste(&writes).is_empty());
                assert_eq!(screen.agent_controls.len(), 1);
            } else {
                assert_eq!(finish_paste(&writes), b"\r");
                screen.refresh_agent_controls_at(now + Duration::from_secs(1));
                assert!(finish_paste(&writes).is_empty());
            }
        }
    }
}

#[test]
fn claude_blocking_stop_and_stale_confirmations_cannot_finish_a_prompt() {
    for target in pane_targets() {
        let (mut screen, messages, writes) = setup_on_target("claude", target);
        report_kind(&mut screen, "claude", hook("SessionStart"));
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        finish_paste(&writes);
        report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
        let mut stop = hook("Stop");
        stop.claude_turn_complete = None;
        stop.prompt_id = None;
        stop.last_assistant_message = Some("FIRST. BLOCKME".into());
        // Replay an old installed reporter's Idle state.
        let mut old_idle = screen.get_pane_info(PANE).unwrap().agent.unwrap();
        old_idle.state = AgentState::Idle;
        report_state(
            &mut screen,
            "claude",
            stop.clone(),
            AgentReportState::Set(old_idle),
        );
        assert!(replies(&messages, CLIENT).is_empty());
        let mut preserving = prompt(true);
        if let AgentRequest::Prompt { preserve_draft, .. } = &mut preserving {
            *preserve_draft = true;
        }
        screen.agent_control_with_clear(preserving, ClientId::new(2), &writes);
        let rejected = replies(&messages, ClientId::new(2)).pop().unwrap();
        assert_eq!(rejected.outcome, AgentOutcome::Rejected);
        assert!(!rejected.agents[0].ready);
        assert_eq!(rejected.agents[0].agent.state, AgentState::Busy);
        assert!(finish_paste(&writes).is_empty());
        report_kind(&mut screen, "claude", hook("PostToolUse"));
        let mut confirmation = hook("Stop");
        confirmation.prompt_id = Some("old".into());
        confirmation.last_assistant_message = Some("OLD".into());
        report_kind(&mut screen, "claude", confirmation.clone());
        assert!(replies(&messages, CLIENT).is_empty());
        confirmation.prompt_id = Some("prompt".into());
        confirmation.last_assistant_message = Some("SECOND".into());
        report_kind(&mut screen, "claude", confirmation.clone());
        let completed = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(completed.outcome, AgentOutcome::Completed);
        assert_eq!(completed.message.as_deref(), Some("SECOND"));
        assert!(completed.agents[0].ready);
        report_kind(&mut screen, "claude", confirmation);
        report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
        report_kind(&mut screen, "claude", stop);
        assert!(replies(&messages, CLIENT).is_empty());
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        finish_paste(&writes);
        let mut next = hook("UserPromptSubmit");
        next.prompt_id = Some("next".into());
        report_kind(&mut screen, "claude", next);
        report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
        report_kind(&mut screen, "claude", hook("Stop"));
        assert!(
            replies(&messages, CLIENT).is_empty(),
            "old turn cannot finish next prompt"
        );
    }
}

#[test]
fn claude_interruption_confirmation_clears_busy_but_cannot_clear_a_newer_turn() {
    for target in pane_targets() {
        let (mut screen, _messages, _writes) = setup_on_target("claude", target);
        report_kind(&mut screen, "claude", hook("SessionStart"));
        report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
        assert_eq!(pane_state(&screen), AgentState::Busy);

        // The transcript observer reports cancellation through the same confirmed Stop
        // path as a turn_duration boundary, preserving any assistant text collected.
        let interrupted = claude_stop("prompt", "Work so far.", false);
        report_kind(&mut screen, "claude", interrupted.clone());
        assert_eq!(pane_state(&screen), AgentState::Idle);

        report_kind(
            &mut screen,
            "claude",
            HookEvent {
                prompt_id: Some("next".into()),
                ..hook("UserPromptSubmit")
            },
        );
        assert_eq!(pane_state(&screen), AgentState::Busy);
        report_kind(&mut screen, "claude", interrupted);
        assert_eq!(pane_state(&screen), AgentState::Busy);
        report_kind(
            &mut screen,
            "claude",
            claude_stop("next", "Next turn.", false),
        );
        assert_eq!(pane_state(&screen), AgentState::Idle);
    }
}

#[test]
fn claude_confirmation_failure_retains_the_uncertain_submission_claim() {
    let (mut screen, messages, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    finish_paste(&writes);
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    let mut unavailable = hook("Stop");
    unavailable.claude_turn_complete = Some(false);
    report_kind(&mut screen, "claude", unavailable);
    let response = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(response.outcome, AgentOutcome::StatusUnavailable);
    assert!(response.acknowledged);
    assert!(!response.agents[0].ready);
    assert_eq!(screen.agent_controls.len(), 1);
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    assert_eq!(
        replies(&messages, CLIENT)[0].outcome,
        AgentOutcome::Rejected
    );
    assert!(finish_paste(&writes).is_empty());
    // A second observer can supply valid late evidence after the first was unavailable.
    report_kind(&mut screen, "claude", hook("Stop"));
    assert!(screen.agent_controls.is_empty());
    assert!(
        replies(&messages, CLIENT).is_empty(),
        "the caller already received unavailable"
    );
    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    assert!(replies(&messages, CLIENT)[0].agents[0].ready);
}

#[test]
fn readiness_reasons_follow_priority_in_status_list_and_prompt_rejections() {
    for (reason, rejection) in [
        (Some(NotReadyReason::PromptUnsupported), "lifecycle hooks"),
        (Some(NotReadyReason::NotIdle), "not idle"),
        (
            Some(NotReadyReason::PendingSubmission),
            "pending submission",
        ),
        (Some(NotReadyReason::UnconsumedInput), ""),
        (None, ""),
    ] {
        let (mut screen, messages, writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        if matches!(
            reason,
            Some(
                NotReadyReason::PromptUnsupported
                    | NotReadyReason::NotIdle
                    | NotReadyReason::PendingSubmission
            )
        ) {
            screen.agent_control_with_clear(prompt(true), ClientId::new(2), &writes);
        }
        if matches!(
            reason,
            Some(NotReadyReason::PromptUnsupported | NotReadyReason::NotIdle)
        ) {
            report(&mut screen, hook("UserPromptSubmit"));
        }
        if reason.is_some() {
            screen.agent_write_for_test();
        }
        if reason == Some(NotReadyReason::PromptUnsupported) {
            screen
                .tab_containing_pane_mut(PANE)
                .unwrap()
                .handle_pty_bytes(TerminalId::new(7), b"\x1b[?2004l".to_vec())
                .unwrap();
        }
        for request in [
            AgentRequest::Status {
                pane_id: PANE,
                expected_session_id: None,
            },
            AgentRequest::List { project: None },
        ] {
            screen.agent_control_with_clear(request, CLIENT, &writes);
            let status = replies(&messages, CLIENT).pop().unwrap().agents.remove(0);
            assert_eq!(status.not_ready_reason, reason);
            assert_eq!(status.ready, reason.is_none());
        }
        if reason == Some(NotReadyReason::UnconsumedInput) {
            finish_paste(&writes);
            screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
            assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
            assert!(replies(&messages, CLIENT).is_empty());
        } else if reason.is_some() {
            screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
            let response = replies(&messages, CLIENT).pop().unwrap();
            assert_eq!(response.agents[0].not_ready_reason, reason);
            assert!(response.message.unwrap().contains(rejection));
        }
    }
}

#[test]
fn timeout_and_input_interruption_explain_acceptance_and_conditional_recovery() {
    for acknowledged in [false, true] {
        for interrupted in [false, true] {
            let (mut screen, messages, writes) = setup("codex");
            report(&mut screen, hook("SessionStart"));
            screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
            finish_paste(&writes);
            screen.refresh_agent_controls();
            if acknowledged {
                report(&mut screen, hook("UserPromptSubmit"));
            }
            if interrupted {
                screen.agent_write_for_test();
            } else {
                screen.agent_controls[0].deadline = Some(Instant::now());
            }
            screen.refresh_agent_controls();
            let response = replies(&messages, CLIENT).pop().unwrap();
            assert_eq!(
                response.outcome,
                if interrupted {
                    AgentOutcome::Interrupted
                } else {
                    AgentOutcome::TimedOut
                }
            );
            assert_eq!(response.acknowledged, acknowledged);
            assert_eq!(
                response.agents[0].not_ready_reason,
                Some(if acknowledged {
                    NotReadyReason::NotIdle
                } else {
                    NotReadyReason::PendingSubmission
                })
            );
            let message = response.message.unwrap();
            assert_eq!(
                message.contains("acceptance is uncertain"),
                !acknowledged,
                "{message}"
            );
            if acknowledged && !interrupted {
                assert!(message.contains("completion was not observed"));
                assert!(message.contains("may still be running"));
            }
            assert!(message.contains(
                "Only if the same live conversation has stopped and unresolved input remains"
            ));
            assert!(message
                .contains("agent recover terminal_7 --conversation conversation --confirm-empty"));
        }
    }
}

#[test]
fn failed_turn_replies_once_releases_its_claim_and_preserves_newer_input() {
    for target in pane_targets() {
        for manual in [false, true] {
            let (mut screen, messages, writes) = setup_on_target("codex", target.clone());
            report(&mut screen, hook("SessionStart"));
            screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
            finish_paste(&writes);
            screen.refresh_agent_controls();
            report(&mut screen, hook("UserPromptSubmit"));
            if manual {
                screen.agent_write_for_test();
            }
            let mut failure = hook("StopFailure");
            failure.last_assistant_message = Some("HTTP 400: unsupported model".into());
            report(&mut screen, failure.clone());
            let response = replies(&messages, CLIENT).pop().unwrap();
            assert!(response.acknowledged);
            assert_eq!(response.turn_id.as_deref(), Some("turn"));
            assert_eq!(
                response.outcome,
                if manual {
                    AgentOutcome::Interrupted
                } else {
                    AgentOutcome::Failed
                }
            );
            if !manual {
                let message = response.message.unwrap();
                assert!(message.contains("HTTP 400: unsupported model"));
                assert!(message.contains("accepted turn failed"));
                assert!(!message.contains("recover"));
                assert!(response.agents[0].ready);
                assert_eq!(response.agents[0].not_ready_reason, None);
            }
            assert!(screen.agent_controls.is_empty());
            report(&mut screen, failure);
            assert!(replies(&messages, CLIENT).is_empty());
            screen.agent_control(
                AgentRequest::Status {
                    pane_id: PANE,
                    expected_session_id: None,
                },
                CLIENT,
            );
            let status = replies(&messages, CLIENT).pop().unwrap().agents.remove(0);
            assert_eq!(status.ready, !manual);
            assert_eq!(
                status.not_ready_reason,
                manual.then_some(NotReadyReason::UnconsumedInput)
            );
            finish_paste(&writes);
            screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
            assert!(replies(&messages, CLIENT).is_empty());
            assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
        }
    }
}

#[test]
fn lost_or_exited_targets_never_recommend_recovery() {
    for change in ["closed", "replaced", "exited", "session-ended"] {
        for acknowledged in [false, true] {
            let (mut screen, messages, writes) = setup("codex");
            report(&mut screen, hook("SessionStart"));
            screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
            finish_paste(&writes);
            screen.refresh_agent_controls();
            if acknowledged {
                report(&mut screen, hook("UserPromptSubmit"));
            }
            match change {
                "closed" => {
                    screen.close_pane_explicitly(PANE, None);
                },
                "replaced" => {
                    let mut start = hook("SessionStart");
                    start.session_id = "other".into();
                    report(&mut screen, start);
                },
                "exited" => {
                    let tab = screen.tab_containing_pane_mut(PANE).unwrap();
                    tab.hold_pane(PANE, Some(0), false, tab.pane_run_command(PANE).unwrap());
                },
                _ => report(&mut screen, hook("SessionEnd")),
            }
            screen.refresh_agent_controls();
            let response = replies(&messages, CLIENT).pop().unwrap();
            assert!(matches!(
                response.outcome,
                AgentOutcome::TargetClosed
                    | AgentOutcome::TargetChanged
                    | AgentOutcome::StatusUnavailable
            ));
            let message = response.message.unwrap_or_default();
            assert!(!message.contains("recover"), "{change}: {message}");
            assert!(!message.contains("clear"), "{change}: {message}");
        }
    }
}

fn task_notification(prompt_id: &str, tasks: &[&str]) -> HookEvent {
    let prompt = tasks
        .iter()
        .map(|task| {
            format!(
                "<task-notification>\n<task-id>{task}</task-id>\n<status>completed</status>\n</task-notification>"
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    HookEvent {
        prompt_id: Some(prompt_id.into()),
        prompt: Some(prompt),
        ..hook("UserPromptSubmit")
    }
}

/// A turn confirmation, with whether Claude's registry still showed background work.
fn claude_stop(prompt_id: &str, message: &str, background: bool) -> HookEvent {
    HookEvent {
        prompt_id: Some(prompt_id.into()),
        last_assistant_message: Some(message.into()),
        claude_background: Some(background),
        ..hook("Stop")
    }
}

/// The background watch of `prompt_id`'s turn reporting Claude's registry idle.
fn background_idle(prompt_id: &str) -> HookEvent {
    HookEvent {
        prompt_id: Some(prompt_id.into()),
        claude_background: Some(false),
        ..hook(yinz_types::agent::CLAUDE_BACKGROUND_IDLE)
    }
}

/// Prompt Claude and confirm a turn, possibly leaving background work running.
fn complete_with_background(
    screen: &mut Screen,
    messages: &CapturedClientMessages,
    writes: &Receiver<(PtyWriteInstruction, ErrorContext)>,
    background: bool,
) -> AgentResponse {
    report_kind(screen, "claude", hook("SessionStart"));
    screen.agent_control_with_clear(prompt(true), CLIENT, writes);
    finish_paste(writes);
    report_kind(screen, "claude", hook("UserPromptSubmit"));
    report_kind(
        screen,
        "claude",
        claude_stop("prompt", "Started.", background),
    );
    replies(messages, CLIENT).pop().unwrap()
}

#[test]
fn a_notification_after_adoption_can_confirm_its_completed_turn() {
    for target in pane_targets() {
        let (mut screen, _messages, _writes) = setup_on_target("claude", target);
        let HookOutcome::Reported(agent) = hook_outcome("claude", &hook("UserPromptSubmit")) else {
            panic!("the adopted pane has a reported agent");
        };
        // Adoption carries the visible record, but not the old server's lifecycle.
        crate::screen::apply_pane_agent_report(&mut screen, PANE, Some(agent)).unwrap();
        assert!(!screen.has_agent_lifecycle(PANE, "claude", "conversation"));

        report_kind(
            &mut screen,
            "claude",
            task_notification("notification", &["bash"]),
        );
        assert!(screen.has_agent_lifecycle(PANE, "claude", "conversation"));
        report_kind(
            &mut screen,
            "claude",
            HookEvent {
                claude_turn_complete: None,
                ..claude_stop("notification", "Finished after restart.", false)
            },
        );
        assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Busy);

        report_kind(
            &mut screen,
            "claude",
            claude_stop("notification", "Finished after restart.", false),
        );
        assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
        assert!(!screen.claude_background_pending_for_test());
    }
}

#[test]
fn background_work_is_reported_and_an_idle_wait_returns_its_final_answer() {
    for target in pane_targets() {
        let (mut screen, messages, writes) = setup_on_target("claude", target);
        let completed = complete_with_background(&mut screen, &messages, &writes, true);
        assert_eq!(completed.outcome, AgentOutcome::Completed);
        assert_eq!(completed.message.as_deref(), Some("Started."));
        assert!(completed.background);
        assert_eq!(completed.agents[0].agent.state, AgentState::Busy);
        assert!(
            !completed.agents[0].ready,
            "steering requires a visible empty composer"
        );
        draft_frame(&mut screen, "claude", "", 0, Instant::now());
        assert!(screen.agent_status_for_test().ready);
        assert!(completed.agents[0].background);

        screen.agent_control(wait(AgentState::Idle), CLIENT);
        assert!(replies(&messages, CLIENT).is_empty());
        // The registry still shows the agent working after the Bash notification turn.
        for (prompt_id, task, text, working) in [
            ("first", "bash", "BGDONE", true),
            ("second", "agent", "SUB=sub", false),
        ] {
            report_kind(&mut screen, "claude", task_notification(prompt_id, &[task]));
            let busy = screen.get_pane_info(PANE).unwrap().agent.unwrap();
            assert_eq!(busy.state, AgentState::Busy);
            // Claude's own Stop is not the transcript confirmation.
            report_kind(
                &mut screen,
                "claude",
                HookEvent {
                    claude_turn_complete: None,
                    ..claude_stop(prompt_id, text, working)
                },
            );
            assert!(replies(&messages, CLIENT).is_empty());
            report_kind(&mut screen, "claude", claude_stop(prompt_id, text, working));
            assert_eq!(
                screen.agent_status_for_test().agent.state,
                if working {
                    AgentState::Busy
                } else {
                    AgentState::Idle
                }
            );
        }
        let done = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(done.outcome, AgentOutcome::Ok);
        assert_eq!(done.message.as_deref(), Some("SUB=sub"));
        assert!(!done.background);
        assert!(done.agents[0].ready, "notification turns consume no input");
        assert!(!done.agents[0].background);

        // A duplicate hook for a confirmed notification turn is not a new turn.
        report_kind(
            &mut screen,
            "claude",
            task_notification("second", &["agent"]),
        );
        assert_eq!(
            screen.get_pane_info(PANE).unwrap().agent.unwrap().state,
            AgentState::Idle
        );
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        assert!(replies(&messages, CLIENT).is_empty());
        assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
    }
}

#[test]
fn a_notification_can_start_before_the_previous_turn_is_confirmed() {
    let (mut screen, messages, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    finish_paste(&writes);
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    report_kind(
        &mut screen,
        "claude",
        task_notification("notification", &["bash"]),
    );
    assert!(
        replies(&messages, CLIENT).is_empty(),
        "the prompt is not interrupted"
    );
    report_kind(
        &mut screen,
        "claude",
        claude_stop("prompt", "Started.", true),
    );
    let completed = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(completed.outcome, AgentOutcome::Completed);
    assert_eq!(completed.message.as_deref(), Some("Started."));
    assert_eq!(
        completed.agents[0].agent.state,
        AgentState::Busy,
        "the notification turn is still running"
    );
    report_kind(
        &mut screen,
        "claude",
        claude_stop("notification", "BGDONE", false),
    );
    screen.agent_control(wait(AgentState::Idle), CLIENT);
    let done = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(done.outcome, AgentOutcome::Ok);
    assert_eq!(done.message.as_deref(), Some("BGDONE"));
    assert!(done.agents[0].ready);
}

#[test]
fn a_notification_confirmed_before_its_launching_turn_keeps_the_newer_registry_status() {
    // Detached observers can confirm the later notification turn first.
    let (mut screen, messages, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    finish_paste(&writes);
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    report_kind(
        &mut screen,
        "claude",
        task_notification("notification", &["bash"]),
    );
    report_kind(
        &mut screen,
        "claude",
        claude_stop("notification", "BGDONE", false),
    );
    report_kind(
        &mut screen,
        "claude",
        claude_stop("prompt", "Started.", true),
    );
    replies(&messages, CLIENT);
    let status = screen.agent_status_for_test();
    assert!(!status.background);
    assert!(!screen.claude_background_pending_for_test());
    screen.agent_control(wait(AgentState::Idle), CLIENT);
    let done = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(done.outcome, AgentOutcome::Ok);
    assert_eq!(done.message.as_deref(), Some("BGDONE"));
}

#[test]
fn interrupted_recovery_ends_an_interrupted_notification_turn() {
    let (mut screen, messages, writes) = setup("claude");
    complete_with_background(&mut screen, &messages, &writes, true);
    report_kind(
        &mut screen,
        "claude",
        task_notification("notification", &["bash"]),
    );
    send_esc(&mut screen, &messages);
    recover_after_quiet(&mut screen, interrupted_recover());
    let recovered = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(recovered.outcome, AgentOutcome::Ok);
    assert!(recovered.agents[0].ready);
    assert!(!screen.claude_background_pending_for_test());
    screen.agent_control(wait(AgentState::Idle), CLIENT);
    assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
}

#[test]
fn a_notification_never_consumes_a_typed_draft() {
    let (mut screen, messages, writes) = setup("claude");
    complete_with_background(&mut screen, &messages, &writes, true);
    screen.agent_write_for_test();
    // A person can type the same tags; the draft still advances the input revision.
    report_kind(
        &mut screen,
        "claude",
        task_notification("notification", &["bash"]),
    );
    report_kind(
        &mut screen,
        "claude",
        claude_stop("notification", "BGDONE", false),
    );
    let status = screen.agent_status_for_test();
    assert_eq!(status.agent.state, AgentState::Idle);
    assert!(!status.background);
    assert!(!status.ready);
    assert_eq!(
        status.not_ready_reason,
        Some(NotReadyReason::UnconsumedInput)
    );
    screen.agent_control(wait(AgentState::Idle), CLIENT);
    assert!(
        replies(&messages, CLIENT).is_empty(),
        "the draft still blocks readiness"
    );
}

#[test]
fn an_unknown_notification_after_interrupted_recovery_leaves_the_agent_ready() {
    // Claude 2.1.286 backgrounds a Bash command on Esc instead of killing it.
    let (mut screen, messages, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    finish_paste(&writes);
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    send_esc(&mut screen, &messages);
    screen.refresh_agent_controls();
    replies(&messages, CLIENT);
    finish_paste(&writes);
    recover_after_quiet(&mut screen, interrupted_recover());
    assert!(replies(&messages, CLIENT)[0].agents[0].ready);
    report_kind(
        &mut screen,
        "claude",
        task_notification("notification", &["escaped"]),
    );
    assert!(!screen.agent_status_for_test().ready);
    report_kind(
        &mut screen,
        "claude",
        claude_stop("notification", "I'll wait.", false),
    );
    let status = screen.agent_status_for_test();
    assert!(status.ready);
    assert!(!status.background);
}

#[test]
fn background_tracking_ends_with_recovery_and_the_conversation() {
    for end in ["recover", "clear", "session_end"] {
        let (mut screen, messages, writes) = setup("claude");
        complete_with_background(&mut screen, &messages, &writes, true);
        match end {
            "recover" => {
                screen.agent_control(recover(), CLIENT);
                assert_eq!(
                    replies(&messages, CLIENT)[0].outcome,
                    AgentOutcome::Rejected
                );
                send_esc(&mut screen, &messages);
                recover_after_quiet(&mut screen, interrupted_recover());
                assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
            },
            "clear" => {
                screen.agent_control(clear_prompt(), CLIENT);
                assert_eq!(
                    replies(&messages, CLIENT)[0].outcome,
                    AgentOutcome::Rejected
                );
                assert!(finish_paste(&writes).is_empty());
                report_kind(&mut screen, "claude", hook("SessionEnd"));
                report_kind(&mut screen, "claude", clear_start());
                replies(&messages, CLIENT);
            },
            _ => report_kind(&mut screen, "claude", hook("SessionEnd")),
        }
        if end != "session_end" {
            assert!(!screen.agent_status_for_test().background, "{end}");
            if end == "recover" {
                screen.agent_control(wait(AgentState::Idle), CLIENT);
                assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
            }
        }
        assert!(screen
            .agent_lifecycle
            .get(&PANE)
            .is_none_or(|lifecycle| lifecycle.claude_background_for_test().is_none()));
    }
}

#[test]
fn an_idle_wait_times_out_while_background_work_runs() {
    let (mut screen, messages, writes) = setup("claude");
    complete_with_background(&mut screen, &messages, &writes, true);
    screen.agent_control(wait(AgentState::Idle), CLIENT);
    assert!(replies(&messages, CLIENT).is_empty());
    screen.refresh_agent_controls_at(Instant::now() + Duration::from_secs(31));
    let timed_out = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(timed_out.outcome, AgentOutcome::TimedOut);
    assert_eq!(timed_out.agents[0].agent.state, AgentState::Busy);
    assert!(timed_out.agents[0].background);
}

#[test]
fn registry_idle_ends_only_the_background_work_of_its_own_turn() {
    for target in pane_targets() {
        let (mut screen, messages, writes) = setup_on_target("claude", target);
        complete_with_background(&mut screen, &messages, &writes, true);
        screen.agent_control(wait(AgentState::Idle), CLIENT);
        // Another turn's watch, a worker, or a replayed archive cannot end it.
        for stale in [
            background_idle("older"),
            HookEvent {
                agent_id: Some("worker".into()),
                ..background_idle("prompt")
            },
            HookEvent {
                archive_only: true,
                ..background_idle("prompt")
            },
            HookEvent {
                session_id: "other".into(),
                ..background_idle("prompt")
            },
        ] {
            report_kind(&mut screen, "claude", stale);
            assert!(replies(&messages, CLIENT).is_empty());
            assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Busy);
        }
        report_kind(&mut screen, "claude", background_idle("prompt"));
        let done = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(done.outcome, AgentOutcome::Ok);
        assert_eq!(done.message.as_deref(), Some("Started."));
        assert!(!done.agents[0].background);
        assert_eq!(done.agents[0].agent.state, AgentState::Idle);
        // A repeated report changes nothing.
        report_kind(&mut screen, "claude", background_idle("prompt"));
        assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
    }
}

#[test]
fn a_newer_turn_confirmation_replaces_older_background_status() {
    let (mut screen, messages, writes) = setup("claude");
    complete_with_background(&mut screen, &messages, &writes, true);
    report_kind(
        &mut screen,
        "claude",
        HookEvent {
            prompt_id: Some("next".into()),
            ..hook("UserPromptSubmit")
        },
    );
    // The older watch reporting idle cannot hide work the newer turn left running.
    report_kind(&mut screen, "claude", claude_stop("next", "NEXT", true));
    report_kind(&mut screen, "claude", background_idle("prompt"));
    let status = screen.agent_status_for_test();
    assert_eq!(status.agent.state, AgentState::Busy);
    assert!(status.background);
    // An older turn's late confirmation cannot restore its own background status either.
    report_kind(&mut screen, "claude", background_idle("next"));
    report_kind(
        &mut screen,
        "claude",
        claude_stop("prompt", "Started.", true),
    );
    assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
    // A confirmation without registry evidence degrades to hook-derived state.
    report_kind(
        &mut screen,
        "claude",
        HookEvent {
            prompt_id: Some("third".into()),
            ..hook("UserPromptSubmit")
        },
    );
    report_kind(
        &mut screen,
        "claude",
        HookEvent {
            claude_background: None,
            ..claude_stop("third", "THIRD", false)
        },
    );
    let status = screen.agent_status_for_test();
    assert_eq!(status.agent.state, AgentState::Idle);
    assert!(!status.background);
    replies(&messages, CLIENT);
}

#[test]
fn an_idle_wait_returns_the_last_completed_turn_and_a_new_prompt_clears_it() {
    let (mut screen, messages, writes) = setup("claude");
    complete_with_background(&mut screen, &messages, &writes, false);
    screen.agent_control(wait(AgentState::Idle), CLIENT);
    assert_eq!(
        replies(&messages, CLIENT)[0].message.as_deref(),
        Some("Started.")
    );
    screen.agent_control_with_clear(prompt(false), CLIENT, &writes);
    finish_paste(&writes);
    report_kind(
        &mut screen,
        "claude",
        HookEvent {
            prompt_id: Some("next".into()),
            ..hook("UserPromptSubmit")
        },
    );
    replies(&messages, CLIENT);
    let lifecycle = |screen: &Screen| {
        screen
            .agent_lifecycle
            .get(&PANE)
            .unwrap()
            .last_message_for_test()
            .map(str::to_owned)
    };
    assert_eq!(lifecycle(&screen), None);
    report_kind(&mut screen, "claude", claude_stop("next", "NEXT", false));
    screen.agent_control(wait(AgentState::Idle), CLIENT);
    assert_eq!(
        replies(&messages, CLIENT)[0].message.as_deref(),
        Some("NEXT")
    );
}

#[test]
fn a_failed_notification_confirmation_ends_only_that_turn() {
    let (mut screen, messages, writes) = setup("claude");
    complete_with_background(&mut screen, &messages, &writes, true);
    report_kind(
        &mut screen,
        "claude",
        task_notification("notification", &["bash"]),
    );
    report_kind(
        &mut screen,
        "claude",
        HookEvent {
            claude_turn_complete: Some(false),
            ..claude_stop("notification", "", false)
        },
    );
    // An observer timeout cannot certify that background processing finished.
    let status = screen.agent_status_for_test();
    assert!(status.background);
    assert_eq!(status.agent.state, AgentState::Busy);
    assert!(screen.claude_background_pending_for_test());
    send_esc(&mut screen, &messages);
    recover_after_quiet(&mut screen, interrupted_recover());
    let recovered = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(recovered.outcome, AgentOutcome::Ok);
    assert!(recovered.agents[0].ready);
}

fn muse_bootstrap(ready: bool) -> HookEvent {
    HookEvent {
        session_id: String::new(),
        muse_hooks_ready: Some(ready),
        ..hook("SessionStart")
    }
}

/// Muse speaks no session id until its first turn begins, so the launcher's verified
/// startup announcement plus the empty composer readies the pane unpinned; the first submit binds it. The
/// provider's deferred SessionStart is that same turn arriving, not a replacement.
#[test]
fn verified_muse_bootstrap_readies_an_unpinned_first_prompt() {
    let (mut screen, messages, writes) = setup("muse");
    screen.agent_control(wait(AgentState::Idle), CLIENT);
    muse_output(&mut screen, &muse_composer_frame(""));
    report_kind(&mut screen, "muse", muse_bootstrap(true));
    let ready = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(ready.outcome, AgentOutcome::Ok);
    assert!(ready.agents[0].ready);
    assert_eq!(ready.agents[0].not_ready_reason, None);
    assert!(ready.agents[0].status_available);
    assert!(ready.agents[0].prompt_supported);
    assert!(ready.agents[0].agent.session_id.is_none());
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
    report_kind(&mut screen, "muse", hook("SessionStart"));
    assert!(
        replies(&messages, CLIENT).is_empty(),
        "the deferred start must not end the submission that caused it"
    );
    report_kind(&mut screen, "muse", hook("UserPromptSubmit"));
    assert!(replies(&messages, CLIENT).is_empty());
    report_kind(&mut screen, "muse", hook("Stop"));
    let completed = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(completed.outcome, AgentOutcome::Completed);
    assert!(completed.acknowledged);
    assert_eq!(completed.turn_id.as_deref(), Some("turn"));
    assert_eq!(
        completed.agents[0].agent.session_id.as_deref(),
        Some("conversation")
    );
    // The pane stays pinned afterwards: the next prompt observes its own turn.
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    finish_paste(&writes);
    report_kind(&mut screen, "muse", hook("UserPromptSubmit"));
    report_kind(&mut screen, "muse", hook("Stop"));
    assert_eq!(
        replies(&messages, CLIENT).pop().unwrap().outcome,
        AgentOutcome::Completed
    );
}

#[test]
fn muse_first_prompt_replaces_a_visible_draft() {
    let (mut screen, messages, writes) = setup("muse");
    muse_output(&mut screen, &muse_composer_frame("unfinished draft"));
    report_kind(&mut screen, "muse", muse_bootstrap(true));
    assert!(!screen.agent_status_for_test().ready);
    screen.agent_control_with_clear(prompt(false), CLIENT, &writes);
    assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
    assert!(replies(&messages, CLIENT).is_empty());
    report_kind(&mut screen, "muse", hook("UserPromptSubmit"));
    assert_eq!(
        replies(&messages, CLIENT).pop().unwrap().outcome,
        AgentOutcome::Submitted
    );
}

#[test]
fn muse_bootstrap_without_verification_proves_nothing() {
    // An unmarked start with no session - however it arrived - claims no lifecycle.
    let (mut screen, messages, writes) = setup("muse");
    muse_output(&mut screen, &muse_composer_frame(""));
    report_kind(
        &mut screen,
        "muse",
        HookEvent {
            session_id: String::new(),
            ..hook("SessionStart")
        },
    );
    screen.agent_control(wait(AgentState::Idle), CLIENT);
    assert!(replies(&messages, CLIENT).is_empty());
    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let status = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(status.outcome, AgentOutcome::StatusUnavailable);
    assert!(!status.agents[0].ready);
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    assert_eq!(
        replies(&messages, CLIENT).pop().unwrap().outcome,
        AgentOutcome::StatusUnavailable
    );
    // A failed verification actively clears the bootstrap it replaces.
    report_kind(&mut screen, "muse", muse_bootstrap(true));
    screen.agent_control(wait(AgentState::Idle), CLIENT);
    assert_eq!(
        replies(&messages, CLIENT).pop().unwrap().outcome,
        AgentOutcome::Ok
    );
    report_kind(&mut screen, "muse", muse_bootstrap(false));
    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let status = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(status.outcome, AgentOutcome::StatusUnavailable);
    assert!(!status.agents[0].prompt_supported);
}

#[test]
fn muse_startup_keys_require_later_evidence_before_another_prompt() {
    for (keys, bytes) in [
        (vec![BareKey::Char('x')], b"x".as_slice()),
        (vec![BareKey::Down, BareKey::Enter], b"\x1b[B\r".as_slice()),
    ] {
        for manual_draft in [false, true] {
            let (mut screen, messages, writes) = setup("muse");
            report_kind(&mut screen, "muse", muse_bootstrap(true));
            screen.agent_control(send_keys(keys.clone()), CLIENT);
            let sent = replies(&messages, CLIENT).pop().unwrap();
            assert_eq!(sent.outcome, AgentOutcome::Ok);
            assert!(!sent.agents[0].ready);
            assert_eq!(finish_paste(&writes), bytes);
            screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
            assert_eq!(
                replies(&messages, CLIENT).pop().unwrap().outcome,
                AgentOutcome::Rejected
            );
            assert!(writes.try_recv().is_err());

            // Repeating the launcher verification cannot consume subsequent keys.
            report_kind(&mut screen, "muse", muse_bootstrap(true));
            let status = AgentRequest::Status {
                pane_id: PANE,
                expected_session_id: None,
            };
            screen.agent_control(status.clone(), CLIENT);
            assert!(!replies(&messages, CLIENT).pop().unwrap().agents[0].ready);
            if manual_draft {
                screen
                    .tab_containing_pane_mut(PANE)
                    .unwrap()
                    .write_to_active_terminal(&None, b"draft".to_vec(), false, CLIENT, None)
                    .unwrap();
                assert_eq!(finish_paste(&writes), b"draft");
            }
            // A repeated announcement cannot consume newer manual input either.
            report_kind(&mut screen, "muse", muse_bootstrap(true));
            muse_output(&mut screen, &muse_composer_frame(""));
            screen.agent_control(status, CLIENT);
            assert_eq!(
                replies(&messages, CLIENT).pop().unwrap().agents[0].ready,
                !manual_draft
            );
            screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
            assert!(replies(&messages, CLIENT).is_empty());
            assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
        }
    }
}

/// Sent text breaks provisional readiness, and recovery restores it: an
/// empty pin matches a pane that reports no session, and fails closed on one
/// that does.
#[test]
fn unpinned_muse_recovery_restores_readiness_after_a_cleared_draft() {
    let recover_as = |conversation: &str, confirm_empty: bool| AgentRequest::Recover {
        pane_id: PANE,
        expected_session_id: conversation.into(),
        confirm_empty,
        interrupted: false,
    };
    let (mut screen, messages, _writes) = setup("muse");
    muse_output(&mut screen, &muse_composer_frame(""));
    report_kind(&mut screen, "muse", muse_bootstrap(true));
    screen.agent_control(send_keys(vec![BareKey::Char('x')]), CLIENT);
    assert!(!replies(&messages, CLIENT).pop().unwrap().agents[0].ready);
    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    assert!(!replies(&messages, CLIENT).pop().unwrap().agents[0].ready);
    screen.agent_control(recover_as("", false), CLIENT);
    assert_eq!(
        replies(&messages, CLIENT).pop().unwrap().outcome,
        AgentOutcome::Rejected
    );
    screen.agent_control(send_keys(vec![BareKey::Backspace]), CLIENT);
    assert!(!replies(&messages, CLIENT).pop().unwrap().agents[0].ready);
    screen.agent_control(recover_as("", true), CLIENT);
    let recovered = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(recovered.outcome, AgentOutcome::Ok);
    assert!(recovered.agents[0].ready);
    assert!(screen.agent_dialog_inputs.is_empty());
    // Once the first turn binds, the empty pin no longer matches.
    report_kind(&mut screen, "muse", hook("SessionStart"));
    screen.agent_control(recover_as("", true), CLIENT);
    assert_eq!(
        replies(&messages, CLIENT).pop().unwrap().outcome,
        AgentOutcome::Rejected
    );
    screen.agent_control(recover_as("conversation", true), CLIENT);
    assert_eq!(
        replies(&messages, CLIENT).pop().unwrap().outcome,
        AgentOutcome::Ok
    );
}

#[test]
fn muse_bootstrap_cannot_overwrite_provider_state() {
    let (mut screen, messages, writes) = setup("muse");
    report_kind(&mut screen, "muse", hook("SessionStart"));
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    report_kind(&mut screen, "muse", hook("UserPromptSubmit"));
    let parent = screen.get_pane_info(PANE).unwrap().agent.unwrap();
    assert_eq!(parent.state, AgentState::Busy);
    let (to_plugin, plugins) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
    report_kind(&mut screen, "muse", muse_bootstrap(true));
    assert_eq!(screen.get_pane_info(PANE).unwrap().agent.unwrap(), parent);
    assert!(replies(&messages, CLIENT).is_empty());
    assert_eq!(screen.agent_controls.len(), 1);
    assert!(plugins
        .try_iter()
        .all(|(instruction, _)| !matches!(instruction, PluginInstruction::Update(_))));
    report_kind(&mut screen, "muse", hook("Stop"));
    assert_eq!(
        replies(&messages, CLIENT).pop().unwrap().outcome,
        AgentOutcome::Completed
    );
}

// Terminal sequences match Muse 1.4.2: full-width rules, prompt at column 1,
// and cursor at column 3 (or after pasted text). The fixture pane is 80 columns.
fn muse_composer_frame(text: &str) -> String {
    let rule = "─".repeat(80);
    format!(
        "\x1b[?25l\x1b[2J\x1b[H{rule}\r\n❯ {text}\r\n{rule}\x1b[2;{}H\x1b[?25h",
        3 + text.chars().count()
    )
}

fn muse_output(screen: &mut Screen, bytes: &str) {
    screen
        .handle_pty_bytes(
            TerminalId::new(7),
            bytes.as_bytes().to_vec(),
            &mut Default::default(),
        )
        .unwrap();
    screen.refresh_agent_controls();
}

#[test]
fn muse_bootstrap_requires_a_live_empty_composer_on_every_target() {
    for target in pane_targets() {
        let (mut screen, messages, writes) = setup_on_target("muse", target);
        report_kind(&mut screen, "muse", muse_bootstrap(true));
        for frame in [
            "".to_owned(),
            "\x1b[?2004h\x1b[2J\x1b[HDo you trust this workspace?\r\n❯ Yes\r\n  No".to_owned(),
            format!("{}\x1b[?25l", muse_composer_frame("")),
            format!("{}\x1b[2;1H", muse_composer_frame("")),
            format!("{}\x1b[3;1H", muse_composer_frame("")),
            format!("{}\x1b[3;1H\x1b[2K\x1b[2;3H", muse_composer_frame("")),
            format!("{}\x1b[1;1H\x1b[2K\x1b[2;3H", muse_composer_frame("")),
            format!("{}\x1b[4;1Hdialog", muse_composer_frame("")),
        ] {
            muse_output(&mut screen, &frame);
            screen.agent_control(
                AgentRequest::Status {
                    pane_id: PANE,
                    expected_session_id: None,
                },
                CLIENT,
            );
            let status = replies(&messages, CLIENT).pop().unwrap();
            assert!(status.agents[0].status_available);
            assert!(!status.agents[0].ready, "{frame:?}");
            assert_eq!(
                status.agents[0].not_ready_reason,
                Some(NotReadyReason::UnconsumedInput)
            );
            screen.agent_control(AgentRequest::List { project: None }, CLIENT);
            assert!(!replies(&messages, CLIENT).pop().unwrap().agents[0].ready);
            screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
            assert_eq!(
                replies(&messages, CLIENT).pop().unwrap().outcome,
                AgentOutcome::Rejected
            );
            assert!(finish_paste(&writes).is_empty());
        }
        screen.agent_control(wait(AgentState::Idle), CLIENT);
        assert!(replies(&messages, CLIENT).is_empty());
        muse_output(&mut screen, &muse_composer_frame(""));
        assert!(replies(&messages, CLIENT).pop().unwrap().agents[0].ready);
        // Scrolling shows historical content, even if it contains a composer.
        muse_output(
            &mut screen,
            &format!("{}{}", "\r\n".repeat(40), muse_composer_frame("")),
        );
        screen
            .tab_containing_pane_mut(PANE)
            .unwrap()
            .scroll_terminal_up(TerminalId::new(7))
            .unwrap();
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        assert_eq!(
            replies(&messages, CLIENT).pop().unwrap().outcome,
            AgentOutcome::Rejected
        );
        assert!(finish_paste(&writes).is_empty());
    }
}

#[test]
fn muse_first_enter_requires_the_composer_to_keep_focus() {
    for target in pane_targets() {
        for dialog in [false, true] {
            let (mut screen, messages, writes) = setup_on_target("muse", target.clone());
            report_kind(&mut screen, "muse", muse_bootstrap(true));
            muse_output(&mut screen, &muse_composer_frame(""));
            screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
            assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
            let now = Instant::now();
            screen.refresh_agent_controls_at(now);
            muse_output(
                &mut screen,
                &if dialog {
                    "\x1b[2J\x1b[HDo you trust this workspace?\r\n❯ Yes".to_owned()
                } else {
                    muse_composer_frame("review")
                },
            );
            screen.refresh_agent_controls_at(now + Duration::from_millis(101));
            if dialog {
                assert!(finish_paste(&writes).is_empty());
                assert_eq!(
                    replies(&messages, CLIENT).pop().unwrap().outcome,
                    AgentOutcome::Interrupted
                );
                assert_eq!(screen.agent_controls.len(), 1);
                muse_output(&mut screen, &muse_composer_frame(""));
                screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
                assert_eq!(
                    replies(&messages, CLIENT).pop().unwrap().outcome,
                    AgentOutcome::Rejected
                );
                assert!(finish_paste(&writes).is_empty());
            } else {
                assert_eq!(finish_paste(&writes), b"\r");
                report_kind(&mut screen, "muse", hook("SessionStart"));
                report_kind(&mut screen, "muse", hook("UserPromptSubmit"));
                report_kind(&mut screen, "muse", hook("Stop"));
                let completed = replies(&messages, CLIENT).pop().unwrap();
                assert_eq!(completed.outcome, AgentOutcome::Completed);
                assert!(completed.acknowledged);
            }
        }
    }
}

#[test]
fn agy_blocking_stop_and_continuation_cannot_finish_a_prompt() {
    for target in pane_targets() {
        let (mut screen, messages, writes) = setup_on_target("agy", target);
        report_kind(&mut screen, "agy", hook("SessionStart"));
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        finish_paste(&writes);
        report_kind(&mut screen, "agy", hook("UserPromptSubmit"));
        let mut stop = hook("Stop");
        stop.agy_turn_complete = None;
        stop.prompt_id = None;
        stop.last_assistant_message = Some("FIRST. BLOCKME".into());
        // Replay an old installed reporter's Idle state.
        let mut old_idle = screen.get_pane_info(PANE).unwrap().agent.unwrap();
        old_idle.state = AgentState::Idle;
        report_state(
            &mut screen,
            "agy",
            stop.clone(),
            AgentReportState::Set(old_idle),
        );
        assert!(replies(&messages, CLIENT).is_empty());
        screen.agent_control_with_clear(prompt(true), ClientId::new(2), &writes);
        let rejected = replies(&messages, ClientId::new(2)).pop().unwrap();
        assert_eq!(rejected.outcome, AgentOutcome::Rejected);
        assert!(!rejected.agents[0].ready);
        assert_eq!(rejected.agents[0].agent.state, AgentState::Busy);
        assert!(finish_paste(&writes).is_empty());
        report_kind(&mut screen, "agy", hook("PreInvocation"));
        report_kind(&mut screen, "agy", hook("PostToolUse"));
        let mut confirmation = hook("Stop");
        confirmation.prompt_id = Some("old".into());
        confirmation.last_assistant_message = Some("OLD".into());
        report_kind(&mut screen, "agy", confirmation.clone());
        assert!(replies(&messages, CLIENT).is_empty());
        confirmation.prompt_id = Some("prompt".into());
        confirmation.last_assistant_message = Some("SECOND".into());
        report_kind(&mut screen, "agy", confirmation.clone());
        let completed = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(completed.outcome, AgentOutcome::Completed);
        assert_eq!(completed.message.as_deref(), Some("SECOND"));
        assert!(completed.agents[0].ready);
        report_kind(&mut screen, "agy", confirmation);
        report_kind(&mut screen, "agy", hook("UserPromptSubmit"));
        report_kind(&mut screen, "agy", stop);
        assert!(replies(&messages, CLIENT).is_empty());
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        finish_paste(&writes);
        let mut next = hook("UserPromptSubmit");
        next.prompt_id = Some("next".into());
        report_kind(&mut screen, "agy", next);
        report_kind(&mut screen, "agy", hook("UserPromptSubmit"));
        report_kind(&mut screen, "agy", hook("Stop"));
        assert!(
            replies(&messages, CLIENT).is_empty(),
            "old turn cannot finish next prompt"
        );
    }
}

#[test]
fn agy_confirmation_failure_retains_the_uncertain_submission_claim() {
    let (mut screen, messages, writes) = setup("agy");
    report_kind(&mut screen, "agy", hook("SessionStart"));
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    finish_paste(&writes);
    report_kind(&mut screen, "agy", hook("UserPromptSubmit"));
    let mut unavailable = hook("Stop");
    unavailable.agy_turn_complete = Some(false);
    report_kind(&mut screen, "agy", unavailable);
    let response = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(response.outcome, AgentOutcome::StatusUnavailable);
    assert!(response.acknowledged);
    assert!(!response.agents[0].ready);
    assert_eq!(screen.agent_controls.len(), 1);
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    assert_eq!(
        replies(&messages, CLIENT)[0].outcome,
        AgentOutcome::Rejected
    );
    assert!(finish_paste(&writes).is_empty());
    // A second observer can supply valid late evidence after the first was unavailable.
    report_kind(&mut screen, "agy", hook("Stop"));
    assert!(screen.agent_controls.is_empty());
    assert!(
        replies(&messages, CLIENT).is_empty(),
        "the caller already received unavailable"
    );
    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    assert!(replies(&messages, CLIENT)[0].agents[0].ready);
}

#[test]
fn agy_subsequent_prompt_is_admitted_and_acknowledged() {
    let (mut screen, messages, writes) = setup("agy");
    report_kind(&mut screen, "agy", hook("SessionStart"));

    // Turn 1
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    finish_paste(&writes);
    let mut submit1 = hook("UserPromptSubmit");
    submit1.prompt_id = Some("conv:1".into());
    report_kind(&mut screen, "agy", submit1);

    let mut stop1 = hook("Stop");
    stop1.prompt_id = Some("conv:1".into());
    stop1.agy_turn_complete = Some(true);
    stop1.last_assistant_message = Some("turn 1 complete".into());
    report_kind(&mut screen, "agy", stop1);

    let reply1 = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(reply1.outcome, AgentOutcome::Completed);
    assert_eq!(reply1.message.as_deref(), Some("turn 1 complete"));
    assert!(reply1.agents[0].ready);

    // Turn 2: A different prompt ID in the same conversation must not be filtered out
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    finish_paste(&writes);
    let mut submit2 = hook("UserPromptSubmit");
    submit2.prompt_id = Some("conv:2".into());
    report_kind(&mut screen, "agy", submit2);

    let mut stop2 = hook("Stop");
    stop2.prompt_id = Some("conv:2".into());
    stop2.agy_turn_complete = Some(true);
    stop2.last_assistant_message = Some("turn 2 complete".into());
    report_kind(&mut screen, "agy", stop2);

    let reply2 = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(reply2.outcome, AgentOutcome::Completed);
    assert_eq!(reply2.message.as_deref(), Some("turn 2 complete"));
    assert!(reply2.agents[0].ready);
}

#[test]
fn agy_switched_conversation_is_admitted_and_acknowledged() {
    let (mut screen, messages, _writes) = setup("agy");

    // Turn 1 on conv-1
    let mut submit1 = hook("UserPromptSubmit");
    submit1.session_id = "conv-1".into();
    submit1.prompt_id = Some("conv-1:1".into());
    report_kind(&mut screen, "agy", submit1);

    let mut stop1 = hook("Stop");
    stop1.session_id = "conv-1".into();
    stop1.prompt_id = Some("conv-1:1".into());
    stop1.agy_turn_complete = Some(true);
    report_kind(&mut screen, "agy", stop1.clone());

    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let status1 = replies(&messages, CLIENT)
        .pop()
        .unwrap()
        .agents
        .pop()
        .unwrap();
    assert_eq!(status1.agent.session_id.as_deref(), Some("conv-1"));
    assert_eq!(status1.agent.state, AgentState::Idle);

    // Conversation switches to conv-2: new prompt establishes identity
    // (previously discarded by the conversation check in ignore_agy_confirmation)
    let mut submit2 = hook("UserPromptSubmit");
    submit2.session_id = "conv-2".into();
    submit2.prompt_id = Some("conv-2:1".into());
    assert!(!screen.ignore_agy_confirmation(PANE, &submit2));
    report_kind(&mut screen, "agy", submit2);

    let mut stop2 = hook("Stop");
    stop2.session_id = "conv-2".into();
    stop2.prompt_id = Some("conv-2:1".into());
    stop2.agy_turn_complete = Some(true);
    assert!(!screen.ignore_agy_confirmation(PANE, &stop2));
    report_kind(&mut screen, "agy", stop2);

    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let status2 = replies(&messages, CLIENT)
        .pop()
        .unwrap()
        .agents
        .pop()
        .unwrap();
    assert_eq!(status2.agent.session_id.as_deref(), Some("conv-2"));
    assert_eq!(status2.agent.state, AgentState::Idle);

    // Stale confirmation from old conv-1 is filtered out and ignored
    assert!(screen.ignore_agy_confirmation(PANE, &stop1));

    // Normal subsequent submission on conv-2 is admitted and acknowledged
    let mut submit3 = hook("UserPromptSubmit");
    submit3.session_id = "conv-2".into();
    submit3.prompt_id = Some("conv-2:2".into());
    assert!(!screen.ignore_agy_confirmation(PANE, &submit3));
    report_kind(&mut screen, "agy", submit3);

    let mut stop3 = hook("Stop");
    stop3.session_id = "conv-2".into();
    stop3.prompt_id = Some("conv-2:2".into());
    stop3.agy_turn_complete = Some(true);
    assert!(!screen.ignore_agy_confirmation(PANE, &stop3));
    report_kind(&mut screen, "agy", stop3);

    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let status3 = replies(&messages, CLIENT)
        .pop()
        .unwrap()
        .agents
        .pop()
        .unwrap();
    assert_eq!(status3.agent.session_id.as_deref(), Some("conv-2"));
    assert_eq!(status3.agent.state, AgentState::Idle);
}

#[test]
fn agy_confirmations_require_nonempty_matching_identities() {
    let (mut screen, messages, _writes) = setup("agy");
    report_kind(&mut screen, "agy", hook("UserPromptSubmit"));
    for (conversation, prompt) in [
        ("", Some("prompt")),
        ("conversation", None),
        ("conversation", Some("")),
        ("other", Some("prompt")),
        ("conversation", Some("stale")),
    ] {
        let mut confirmation = hook("Stop");
        confirmation.session_id = conversation.into();
        confirmation.prompt_id = prompt.map(str::to_owned);
        assert!(screen.ignore_agy_confirmation(PANE, &confirmation));
        report_kind(&mut screen, "agy", confirmation);
    }
    screen.agent_control(wait(AgentState::Idle), CLIENT);
    assert!(replies(&messages, CLIENT).is_empty());
    report_kind(&mut screen, "agy", hook("Stop"));
    let done = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(done.outcome, AgentOutcome::Ok);
    assert!(done.agents[0].ready);
    assert!(screen.ignore_agy_confirmation(PANE, &hook("Stop")));
}

#[test]
fn cli_pi_new_restores_readiness() {
    for target in pane_targets() {
        let (mut screen, messages, writes) = setup_on_target("pi", target);
        report_kind(&mut screen, "pi", hook("SessionStart"));
        screen.agent_control_with_clear(
            AgentRequest::Prompt {
                preserve_draft: false,
                submission_id: None,
                pane_id: PANE,
                expected_session_id: None,
                prompt: "/new".into(),
                wait: true,
                timeout_ms: 30_000,
            },
            CLIENT,
            &writes,
        );
        // Typed alone: an empty paste after it would discard it in Codex's paste-burst handling.
        assert_eq!(finish_paste(&writes), b"/new");
        let now = Instant::now();
        screen.refresh_agent_controls_at(now);
        screen.refresh_agent_controls_at(now + Duration::from_millis(501));
        assert_eq!(finish_paste(&writes), b"\r");
        assert!(screen.agent_clear_inputs.contains_key(&PANE));
        // The bridge reports a replaced thread as an end and a clear start.
        report_kind(&mut screen, "pi", hook("SessionEnd"));
        assert_eq!(
            replies(&messages, CLIENT)[0].outcome,
            AgentOutcome::TargetChanged
        );
        // A wait issued before the new conversation starts waits for it.
        screen.agent_control(wait(AgentState::Idle), CLIENT);
        assert!(replies(&messages, CLIENT).is_empty());
        report_kind(&mut screen, "pi", HookEvent { ..clear_start() });
        assert!(screen.agent_clear_inputs.is_empty());
        let waited = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(waited.outcome, AgentOutcome::Ok);
        assert_eq!(
            waited.agents[0].agent.session_id.as_deref(),
            Some("cleared-conversation")
        );
        assert!(waited.agents[0].ready);
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        assert!(replies(&messages, CLIENT).is_empty());
        finish_paste(&writes);
        for name in ["UserPromptSubmit", "Stop"] {
            report_kind(
                &mut screen,
                "pi",
                HookEvent {
                    session_id: "cleared-conversation".into(),
                    ..hook(name)
                },
            );
        }
        let completed = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(completed.outcome, AgentOutcome::Completed);
        assert!(completed.agents[0].ready);
    }
}

#[test]
fn pi_normalized_prompt_completes_and_allows_another_prompt() {
    let (mut screen, messages, writes) = setup("pi");
    report_kind(&mut screen, "pi", hook("SessionStart"));
    for (text, received) in [
        (
            "review\tthese\t\tfiles\nthen\treport",
            "review    these        files\nthen    report",
        ),
        ("\u{feff}review", "review"),
        (
            " \u{feff}\treview\tthese files\u{feff}\n",
            "review    these files",
        ),
        ("another prompt", "another prompt"),
    ] {
        screen.agent_control_with_clear(
            AgentRequest::Prompt {
                preserve_draft: false,
                submission_id: None,
                pane_id: PANE,
                expected_session_id: Some("conversation".into()),
                prompt: text.into(),
                wait: true,
                timeout_ms: 30_000,
            },
            CLIENT,
            &writes,
        );
        assert_eq!(
            finish_paste(&writes),
            format!("\x1b[200~{text}\x1b[201~").as_bytes()
        );
        let now = Instant::now();
        screen.refresh_agent_controls_at(now);
        screen.refresh_agent_controls_at(now + Duration::from_millis(100));
        assert_eq!(finish_paste(&writes), b"\r");
        report_kind(
            &mut screen,
            "pi",
            HookEvent {
                prompt: Some(received.into()),
                ..hook("UserPromptSubmit")
            },
        );
        assert!(replies(&messages, CLIENT).is_empty());
        report_kind(&mut screen, "pi", hook("Stop"));
        let result = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(result.outcome, AgentOutcome::Completed);
        assert!(result.acknowledged);
        assert!(result.agents[0].ready);
        assert!(screen.agent_controls.is_empty());
    }
}

#[test]
fn pi_rejects_commands_and_empty_text_after_trimming() {
    let (mut screen, messages, writes) = setup("pi");
    writes.try_iter().for_each(drop);
    report_kind(&mut screen, "pi", hook("SessionStart"));
    for text in [
        "\u{feff}!command",
        "\u{feff}/reload",
        "\u{feff}/new",
        "/new\u{feff}",
        "\u{feff}",
        " \u{feff}\t\u{feff}\n",
    ] {
        screen.agent_control_with_clear(
            AgentRequest::Prompt {
                preserve_draft: false,
                submission_id: None,
                pane_id: PANE,
                expected_session_id: None,
                prompt: text.into(),
                wait: false,
                timeout_ms: 30_000,
            },
            CLIENT,
            &writes,
        );
        let result = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(result.outcome, AgentOutcome::Rejected, "{text:?}");
        assert!(result.agents[0].ready);
        assert!(writes.try_iter().next().is_none());
        assert!(screen.agent_controls.is_empty());
    }
}

#[test]
fn pi_early_startup_dialog_consumes_only_its_answer() {
    for draft in [false, true] {
        let (mut screen, messages, writes) = setup("pi");
        if draft {
            screen
                .tab_containing_pane_mut(PANE)
                .unwrap()
                .write_to_active_terminal(&None, b"manual draft".to_vec(), false, CLIENT, None)
                .unwrap();
            writes.try_iter().for_each(drop);
        }
        // The extension announces startup before a dialog in an earlier startup handler.
        for name in ["SessionStart", "Elicitation"] {
            report_kind(
                &mut screen,
                "pi",
                HookEvent {
                    turn_id: None,
                    ..hook(name)
                },
            );
        }
        screen.agent_control(wait(AgentState::Idle), CLIENT);
        assert_eq!(
            replies(&messages, CLIENT)[0].outcome,
            AgentOutcome::NeedsInput
        );
        for key in [BareKey::Down, BareKey::Enter] {
            screen.agent_control(send_keys(vec![key]), CLIENT);
            assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
            assert_eq!(screen.agent_dialog_inputs.contains_key(&PANE), !draft);
        }
        finish_paste(&writes);
        report_kind(
            &mut screen,
            "pi",
            HookEvent {
                source: Some("idle".into()),
                turn_id: None,
                ..hook("ElicitationResult")
            },
        );
        // Pi now reaches Yinz's actual startup handler.
        report_kind(&mut screen, "pi", hook("SessionStart"));
        assert!(screen.agent_dialog_inputs.is_empty());
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        assert!(replies(&messages, CLIENT).is_empty());
        assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
    }
}

#[test]
fn pi_dialog_before_submit_keeps_claim_and_consumes_only_its_answer() {
    for wait_for_completion in [false, true] {
        let (mut screen, messages, writes) = setup("pi");
        report_kind(&mut screen, "pi", hook("SessionStart"));
        screen.agent_control_with_clear(prompt(wait_for_completion), CLIENT, &writes);
        finish_paste(&writes);
        let now = Instant::now();
        screen.refresh_agent_controls_at(now);
        screen.refresh_agent_controls_at(now + Duration::from_millis(100));
        assert_eq!(finish_paste(&writes), b"\r");
        report_kind(
            &mut screen,
            "pi",
            HookEvent {
                turn_id: None,
                ..hook("Elicitation")
            },
        );
        let result = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(result.outcome, AgentOutcome::NeedsInput);
        assert!(!result.acknowledged);
        assert_eq!(result.turn_id, None);
        assert!(!result.agents[0].ready);
        assert_eq!(screen.agent_controls.len(), 1);
        for key in [BareKey::Down, BareKey::Enter] {
            screen.agent_control(send_keys(vec![key]), ClientId::new(2));
            assert_eq!(
                replies(&messages, ClientId::new(2))[0].outcome,
                AgentOutcome::Ok
            );
            assert!(screen.agent_dialog_inputs.contains_key(&PANE));
        }
        report_kind(
            &mut screen,
            "pi",
            HookEvent {
                turn_id: None,
                source: Some("idle".into()),
                ..hook("ElicitationResult")
            },
        );
        // The idle dialog result cannot release the pending submission.
        screen.agent_control_with_clear(prompt(false), ClientId::new(2), &writes);
        let blocked = replies(&messages, ClientId::new(2)).pop().unwrap();
        assert_eq!(blocked.outcome, AgentOutcome::Rejected);
        assert_eq!(
            blocked.agents[0].not_ready_reason,
            Some(NotReadyReason::PendingSubmission)
        );
        report_kind(&mut screen, "pi", hook("UserPromptSubmit"));
        report_kind(
            &mut screen,
            "pi",
            HookEvent {
                turn_id: Some("stale".into()),
                ..hook("Stop")
            },
        );
        assert_eq!(screen.agent_controls.len(), 1);
        report_kind(&mut screen, "pi", hook("Stop"));
        assert!(screen.agent_controls.is_empty());
        assert!(
            replies(&messages, CLIENT).is_empty(),
            "the original caller was already answered"
        );
        screen.agent_control(wait(AgentState::Idle), CLIENT);
        let ready = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(ready.outcome, AgentOutcome::Ok);
        assert!(ready.agents[0].ready);
    }
}

#[test]
fn pi_unverified_dialog_keys_cannot_acquire_prompt_correlation() {
    for case in [
        "stale-turn",
        "empty-turn",
        "conversation",
        "kind",
        "pane",
        "nonhuman",
        "manual-before",
        "manual-after",
        "delivering",
        "stale-after-dialog",
    ] {
        let (mut screen, messages, writes) = setup("pi");
        report_kind(&mut screen, "pi", hook("SessionStart"));
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        if case != "delivering" {
            finish_paste(&writes);
            let now = Instant::now();
            screen.refresh_agent_controls_at(now);
            screen.refresh_agent_controls_at(now + Duration::from_millis(100));
            assert_eq!(finish_paste(&writes), b"\r");
        }
        if case == "manual-before" {
            screen.agent_write_for_test();
        }
        let mut dialog = HookEvent {
            turn_id: None,
            ..hook("Elicitation")
        };
        match case {
            "stale-turn" => dialog.turn_id = Some("stale".into()),
            "empty-turn" => dialog.turn_id = Some(String::new()),
            "conversation" => dialog.session_id = "other".into(),
            "nonhuman" => {
                dialog.hook_event_name = "PermissionRequest".into();
                dialog.approvals_reviewer = Some("auto_review".into());
            },
            _ => {},
        }
        if case == "pane" {
            let event = AgentEvent::from_hook("pi", TerminalId::new(8), &dialog, String::new());
            screen.agent_control_event(&event, Some(&dialog));
            let HookOutcome::Reported(waiting) = hook_outcome("pi", &dialog) else {
                panic!("dialog reports a waiting agent");
            };
            crate::screen::apply_pane_agent_report(&mut screen, PANE, Some(waiting)).unwrap();
        } else {
            report_kind(
                &mut screen,
                if case == "kind" { "codex" } else { "pi" },
                dialog,
            );
        }
        if case == "manual-after" {
            screen.agent_write_for_test();
        } else if case == "stale-after-dialog" {
            report_kind(&mut screen, "pi", hook("Elicitation"));
        }
        replies(&messages, CLIENT);
        screen.agent_control(send_keys(vec![BareKey::Enter]), ClientId::new(2));
        let response = replies(&messages, ClientId::new(2)).pop().unwrap();
        assert_eq!(
            response.outcome,
            if case == "kind" {
                AgentOutcome::StatusUnavailable
            } else {
                AgentOutcome::Ok
            },
            "{case}"
        );
        assert!(screen.agent_dialog_inputs.is_empty(), "{case}");
        finish_paste(&writes);
        report_kind(&mut screen, "pi", hook("UserPromptSubmit"));
        report_kind(&mut screen, "pi", hook("Stop"));
        assert!(!screen.agent_status_for_test().ready, "{case}");
    }
}

#[test]
fn pi_manual_terminal_input_interrupts_correlation_before_completion() {
    let (mut screen, messages, writes) = setup("pi");
    report_kind(&mut screen, "pi", hook("SessionStart"));
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    finish_paste(&writes);
    report_kind(&mut screen, "pi", hook("UserPromptSubmit"));
    screen
        .tab_containing_pane_mut(PANE)
        .unwrap()
        .write_to_active_terminal(&None, b"manual".to_vec(), false, CLIENT, None)
        .unwrap();
    report_kind(&mut screen, "pi", hook("Stop"));
    let result = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(result.outcome, AgentOutcome::Interrupted);
    assert!(result.acknowledged);
    assert!(screen.agent_controls.is_empty());
}

#[test]
fn pi_needs_hooks_and_restores_idle_after_an_idle_dialog_or_compaction() {
    let (mut screen, messages, _writes) = setup("pi");
    screen.agent_control(
        AgentRequest::Status {
            pane_id: PANE,
            expected_session_id: None,
        },
        CLIENT,
    );
    let status = replies(&messages, CLIENT).pop().unwrap();
    assert!(!status.agents[0].prompt_supported);
    assert!(!status.agents[0].ready);
    report_kind(&mut screen, "pi", hook("SessionStart"));
    for (start, end) in [
        ("Elicitation", "ElicitationResult"),
        ("PreCompact", "PostCompact"),
    ] {
        report_kind(&mut screen, "pi", hook(start));
        report_kind(
            &mut screen,
            "pi",
            HookEvent {
                source: Some("idle".into()),
                ..hook(end)
            },
        );
        screen.agent_control(wait(AgentState::Idle), CLIENT);
        let result = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(result.outcome, AgentOutcome::Ok);
        assert!(result.agents[0].ready);
    }
}

#[test]
fn a_codex_plan_prompt_keeps_the_turn_complete_and_the_next_hook_clears_waiting() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    finish_paste(&writes);
    report(&mut screen, hook("UserPromptSubmit"));
    report(&mut screen, hook("Stop"));
    let completed = replies(&messages, CLIENT);
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].outcome, AgentOutcome::Completed);
    assert_eq!(completed[0].turn_id.as_deref(), Some("turn"));
    screen.forward_desktop_notifications(
        vec![crate::panes::grid::PendingNotification::Osc9 {
            body: "Plan mode prompt: Implement this plan?".into(),
        }],
        TerminalId::new(7),
    );
    assert!(screen
        .get_pane_info(PANE)
        .unwrap()
        .agent
        .unwrap()
        .is_waiting_on_a_person());
    assert!(replies(&messages, CLIENT).is_empty());
    assert!(screen.agent_controls.is_empty());
    report(
        &mut screen,
        HookEvent {
            turn_id: Some("next".into()),
            ..hook("UserPromptSubmit")
        },
    );
    let agent = screen.get_pane_info(PANE).unwrap().agent.unwrap();
    assert_eq!(agent.state, AgentState::Busy);
    assert_eq!(agent.waiting_on, None);
    assert!(replies(&messages, CLIENT).is_empty());
}

fn identified_prompt(id: &str, wait: bool) -> AgentRequest {
    let mut request = prompt(wait);
    if let AgentRequest::Prompt { submission_id, .. } = &mut request {
        *submission_id = Some(id.into());
    }
    request
}

fn wait_submission(id: &str) -> AgentRequest {
    AgentRequest::WaitTurn {
        pane_id: PANE,
        expected_session_id: Some("conversation".into()),
        submission_id: id.into(),
        timeout_ms: 30_000,
    }
}

#[test]
fn submission_wait_preserves_failure_after_permission_and_later_turns() {
    for kind in ["codex", "claude", "pi"] {
        let (mut screen, messages, writes) = setup(kind);
        report_kind(&mut screen, kind, hook("SessionStart"));
        screen.agent_control_with_clear(identified_prompt("first", true), CLIENT, &writes);
        finish_paste(&writes);
        // Claude does not need a provider turn ID; the local receipt still identifies it.
        let turn_id = (kind != "claude").then(|| "turn".to_owned());
        report_kind(
            &mut screen,
            kind,
            HookEvent {
                turn_id: turn_id.clone(),
                ..hook("UserPromptSubmit")
            },
        );
        report_kind(&mut screen, kind, permission_wait(kind));
        let waiting = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(waiting.outcome, AgentOutcome::NeedsInput);
        assert_eq!(waiting.submission_id.as_deref(), Some("first"));
        screen.agent_control(wait_submission("first"), CLIENT);
        assert_eq!(
            replies(&messages, CLIENT)[0].outcome,
            AgentOutcome::NeedsInput
        );
        screen.agent_control(send_keys(vec![BareKey::Enter]), CLIENT);
        replies(&messages, CLIENT);
        assert_eq!(finish_paste(&writes), b"\r");
        screen.agent_control(wait_submission("first"), CLIENT);
        assert!(
            replies(&messages, CLIENT).is_empty(),
            "wait for the answered dialog to advance"
        );
        report_kind(
            &mut screen,
            kind,
            HookEvent {
                turn_id: turn_id.clone(),
                last_assistant_message: Some("provider failed".into()),
                ..hook("StopFailure")
            },
        );
        let failed = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(failed.outcome, AgentOutcome::Failed, "{kind}");
        assert_eq!(failed.submission_id.as_deref(), Some("first"));
        assert_eq!(failed.turn_id, turn_id);
        assert!(failed.acknowledged);
        assert_eq!(failed.message.as_deref(), Some("provider failed"));
        assert!(failed.agents[0].ready);

        screen.agent_control_with_clear(identified_prompt("second", true), CLIENT, &writes);
        finish_paste(&writes);
        for name in ["UserPromptSubmit", "Stop"] {
            report_kind(
                &mut screen,
                kind,
                HookEvent {
                    prompt_id: Some("second-prompt".into()),
                    turn_id: Some("second-turn".into()),
                    last_assistant_message: Some("second succeeded".into()),
                    ..hook(name)
                },
            );
        }
        assert_eq!(
            replies(&messages, CLIENT)[0].outcome,
            AgentOutcome::Completed
        );
        screen.agent_control(wait_submission("first"), CLIENT);
        let historical = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(historical.outcome, AgentOutcome::Failed);
        assert_eq!(historical.message, failed.message);
        assert_eq!(historical.turn_id, failed.turn_id);

        screen.agent_control_with_clear(identified_prompt("first", true), CLIENT, &writes);
        let duplicate = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(duplicate.outcome, AgentOutcome::Rejected);
        assert!(duplicate.message.unwrap().contains("--submission first"));
        assert!(
            finish_paste(&writes).is_empty(),
            "duplicate IDs must not send input"
        );
    }
}

#[test]
fn submission_wait_survives_timeout_and_disconnect_without_resubmitting() {
    for ending in ["before_ack", "after_ack", "disconnect", "manual_input"] {
        let (mut screen, messages, writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        screen.agent_control_with_clear(identified_prompt("receipt", true), CLIENT, &writes);
        finish_paste(&writes);
        screen.refresh_agent_controls();
        if ending != "before_ack" {
            report(&mut screen, hook("UserPromptSubmit"));
        }
        if ending == "disconnect" {
            screen.detach_agent_controls(CLIENT);
        } else if ending == "manual_input" {
            screen
                .tab_containing_pane_mut(PANE)
                .unwrap()
                .write_to_active_terminal(&None, b"draft".to_vec(), false, CLIENT, None)
                .unwrap();
            screen.refresh_agent_controls();
            assert_eq!(
                replies(&messages, CLIENT)[0].outcome,
                AgentOutcome::Interrupted
            );
        } else {
            screen.agent_controls[0].deadline = Some(Instant::now());
            screen.refresh_agent_controls();
            let timed_out = replies(&messages, CLIENT).pop().unwrap();
            assert_eq!(timed_out.outcome, AgentOutcome::TimedOut);
            assert_eq!(timed_out.submission_id.as_deref(), Some("receipt"));
            assert_eq!(timed_out.acknowledged, ending == "after_ack");
        }
        finish_paste(&writes);
        let observer = ClientId::new(2);
        screen.agent_control(wait_submission("receipt"), observer);
        assert!(replies(&messages, observer).is_empty(), "{ending}");
        if ending == "before_ack" {
            report(&mut screen, hook("UserPromptSubmit"));
        }
        report(
            &mut screen,
            HookEvent {
                last_assistant_message: Some("late completion".into()),
                ..hook("Stop")
            },
        );
        let completed = replies(&messages, observer).pop().unwrap();
        assert_eq!(completed.outcome, AgentOutcome::Completed, "{ending}");
        assert_eq!(completed.submission_id.as_deref(), Some("receipt"));
        assert!(completed.acknowledged);
        assert_eq!(completed.turn_id.as_deref(), Some("turn"));
        assert_eq!(completed.message.as_deref(), Some("late completion"));
        assert!(
            finish_paste(&writes).is_empty(),
            "observing must not resubmit input"
        );
    }
}

#[test]
fn submission_wait_returns_provider_interrupt_and_explicit_recovery() {
    for provider_interrupt in [false, true] {
        let (mut screen, messages, writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        screen.agent_control_with_clear(identified_prompt("receipt", true), CLIENT, &writes);
        finish_paste(&writes);
        if provider_interrupt {
            report(&mut screen, hook("UserPromptSubmit"));
            report(&mut screen, hook("Interrupt"));
            replies(&messages, CLIENT);
        } else {
            screen.refresh_agent_controls();
            screen.agent_controls[0].deadline = Some(Instant::now());
            screen.refresh_agent_controls();
            replies(&messages, CLIENT);
            screen.agent_control(recover(), CLIENT);
            assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
        }
        screen.agent_control(wait_submission("receipt"), CLIENT);
        let result = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(result.outcome, AgentOutcome::Interrupted);
        assert_eq!(result.acknowledged, provider_interrupt);
        assert_eq!(result.submission_id.as_deref(), Some("receipt"));
        if !provider_interrupt {
            assert!(result.message.unwrap().contains("not verified"));
        }
    }
}

#[test]
fn submission_history_is_bounded_and_closing_a_pane_releases_it() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    let limit = crate::screen::agent_control::SUBMISSION_HISTORY_LIMIT;
    for index in 0..=limit {
        screen.agent_control_with_clear(
            identified_prompt(&format!("receipt-{index}"), false),
            CLIENT,
            &writes,
        );
        finish_paste(&writes);
        for name in ["UserPromptSubmit", "Stop"] {
            report(
                &mut screen,
                HookEvent {
                    turn_id: Some(format!("turn-{index}")),
                    ..hook(name)
                },
            );
        }
        assert_eq!(
            replies(&messages, CLIENT)[0].outcome,
            AgentOutcome::Submitted
        );
    }
    assert_eq!(screen.agent_submissions[&PANE].len(), limit);
    screen.agent_control(wait_submission("receipt-0"), CLIENT);
    assert_eq!(
        replies(&messages, CLIENT)[0].outcome,
        AgentOutcome::StatusUnavailable
    );
    screen.agent_control(wait_submission(&format!("receipt-{limit}")), CLIENT);
    assert_eq!(
        replies(&messages, CLIENT)[0].outcome,
        AgentOutcome::Completed
    );
    screen.agent_control_with_clear(identified_prompt("pending", true), CLIENT, &writes);
    finish_paste(&writes);
    screen.agent_control(wait_submission("pending"), ClientId::new(2));
    screen.close_pane_explicitly(PANE, None);
    screen.refresh_agent_controls();
    assert_eq!(
        replies(&messages, ClientId::new(2))[0].outcome,
        AgentOutcome::TargetClosed
    );
    assert!(!screen.agent_submissions.contains_key(&PANE));
}

#[test]
fn submission_wait_checks_conversation_pins_and_unknown_ids() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    screen.agent_control_with_clear(identified_prompt("receipt", false), CLIENT, &writes);
    finish_paste(&writes);
    report(&mut screen, hook("UserPromptSubmit"));
    replies(&messages, CLIENT);
    for (id, conversation, outcome) in [
        ("unknown", "conversation", AgentOutcome::StatusUnavailable),
        ("receipt", "replacement", AgentOutcome::TargetChanged),
    ] {
        screen.agent_control(
            AgentRequest::WaitTurn {
                pane_id: PANE,
                expected_session_id: Some(conversation.into()),
                submission_id: id.into(),
                timeout_ms: 30_000,
            },
            CLIENT,
        );
        assert_eq!(replies(&messages, CLIENT)[0].outcome, outcome);
    }
    assert!(finish_paste(&writes).is_empty());
    report(&mut screen, hook("Stop"));
    report(
        &mut screen,
        HookEvent {
            session_id: "replacement".into(),
            ..hook("SessionStart")
        },
    );
    assert!(!screen.agent_submissions.contains_key(&PANE));
    screen.agent_control(
        AgentRequest::WaitTurn {
            pane_id: PANE,
            expected_session_id: None,
            submission_id: "receipt".into(),
            timeout_ms: 30_000,
        },
        CLIENT,
    );
    assert_eq!(
        replies(&messages, CLIENT)[0].outcome,
        AgentOutcome::StatusUnavailable
    );
}

#[test]
fn submission_ids_are_validated_before_terminal_input() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    for id in ["", "two words", "with/slash", &"x".repeat(129)] {
        screen.agent_control_with_clear(identified_prompt(id, true), CLIENT, &writes);
        assert_eq!(
            replies(&messages, CLIENT)[0].outcome,
            AgentOutcome::Rejected
        );
    }
    assert!(finish_paste(&writes).is_empty());
    assert!(screen.agent_submissions.is_empty());
}

#[test]
fn a_muse_submission_receipt_binds_with_the_first_conversation() {
    let (mut screen, messages, writes) = setup("muse");
    muse_output(&mut screen, &muse_composer_frame(""));
    report_kind(&mut screen, "muse", muse_bootstrap(true));
    screen.agent_control_with_clear(identified_prompt("first-muse", false), CLIENT, &writes);
    finish_paste(&writes);
    let observer = ClientId::new(2);
    screen.agent_control(
        AgentRequest::WaitTurn {
            pane_id: PANE,
            expected_session_id: None,
            submission_id: "first-muse".into(),
            timeout_ms: 30_000,
        },
        observer,
    );
    report_kind(&mut screen, "muse", hook("SessionStart"));
    // The provider has named the conversation but has not accepted the prompt yet.
    // A new observer can already use that conversation pin with the same receipt.
    let pinned_observer = ClientId::new(3);
    screen.agent_control(wait_submission("first-muse"), pinned_observer);
    assert!(replies(&messages, pinned_observer).is_empty());
    for name in ["UserPromptSubmit", "Stop"] {
        report_kind(&mut screen, "muse", hook(name));
    }
    assert_eq!(
        replies(&messages, CLIENT)[0].outcome,
        AgentOutcome::Submitted
    );
    let result = replies(&messages, observer).pop().unwrap();
    assert_eq!(result.outcome, AgentOutcome::Completed);
    assert!(result.acknowledged);
    assert_eq!(result.submission_id.as_deref(), Some("first-muse"));
    let pinned_results = replies(&messages, pinned_observer);
    assert_eq!(pinned_results.len(), 1);
    let pinned_result = &pinned_results[0];
    // Report age can advance between observers; compare the receipt and conversation.
    assert_eq!(pinned_result.outcome, result.outcome);
    assert_eq!(pinned_result.acknowledged, result.acknowledged);
    assert_eq!(pinned_result.submission_id, result.submission_id);
    assert_eq!(pinned_result.turn_id, result.turn_id);
    assert_eq!(
        pinned_result.agents[0].agent.session_id.as_deref(),
        Some("conversation")
    );
    screen.agent_control(wait_submission("first-muse"), CLIENT);
    assert_eq!(
        replies(&messages, CLIENT)[0].outcome,
        AgentOutcome::Completed
    );
}

#[test]
fn literal_claude_notification_xml_is_a_caller_turn() {
    let (mut screen, messages, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    let text = "<task-notification>Explain this XML tag</task-notification>";
    let mut request = identified_prompt("literal", true);
    if let AgentRequest::Prompt { prompt, .. } = &mut request {
        *prompt = text.into();
    }
    screen.agent_control_with_clear(request, CLIENT, &writes);
    finish_paste(&writes);
    report_kind(
        &mut screen,
        "claude",
        HookEvent {
            prompt: Some(text.into()),
            ..hook("UserPromptSubmit")
        },
    );
    report_kind(
        &mut screen,
        "claude",
        claude_stop("prompt", "It is XML.", false),
    );
    let completed = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(completed.outcome, AgentOutcome::Completed);
    assert!(completed.acknowledged);
    assert!(completed.agents[0].ready);
    assert_eq!(completed.message.as_deref(), Some("It is XML."));
    assert!(!screen.claude_background_pending_for_test());
}

#[test]
fn unpinned_muse_recovery_advice_quotes_the_empty_conversation() {
    let (mut screen, messages, writes) = setup("muse");
    muse_output(&mut screen, &muse_composer_frame(""));
    report_kind(&mut screen, "muse", muse_bootstrap(true));
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    finish_paste(&writes);
    screen.refresh_agent_controls();
    screen.agent_controls[0].deadline = Some(Instant::now());
    screen.refresh_agent_controls();
    let result = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(result.outcome, AgentOutcome::TimedOut);
    assert!(result.submission_id.is_some());
    assert!(result
        .message
        .unwrap()
        .contains("--conversation '' --confirm-empty"));
}

#[test]
fn plugin_agent_input_uses_declared_bytes_delay_and_correlated_hooks() {
    use yinz_types::agent::provider::{register, unregister_owner, AgentInput, AgentProvider};
    let kind = "screen-input-test";
    let owner = 70_101;
    let mut provider = AgentProvider {
        kind: kind.into(),
        executable: kind.into(),
        input: Some(AgentInput {
            clear_key: "Ctrl u".into(),
            submit_key: "Ctrl j".into(),
            submit_delay_ms: 375,
            paste_prefix: "Instructions: ".into(),
            newline: "\r".into(),
            command_prefixes: vec![":".into()],
        }),
        ..Default::default()
    };
    register(owner, provider.clone()).unwrap();
    let (mut screen, messages, writes) = setup(kind);
    report_kind(&mut screen, kind, hook("SessionStart"));
    screen.agent_control_with_clear(
        AgentRequest::Prompt {
            preserve_draft: false,
            pane_id: PANE,
            expected_session_id: Some("conversation".into()),
            prompt: "review\ncode".into(),
            submission_id: None,
            wait: true,
            timeout_ms: 30_000,
        },
        CLIENT,
        &writes,
    );
    let now = Instant::now();
    screen.refresh_agent_controls_at(now + Duration::from_secs(1));
    assert_eq!(
        finish_paste(&writes),
        b"Instructions: \x1b[200~review\rcode\x1b[201~"
    );
    screen.refresh_agent_controls_at(now + Duration::from_secs(1));
    // Replacing a descriptor cannot change input which is already in flight.
    provider.input.as_mut().unwrap().submit_key = "Enter".into();
    provider.input.as_mut().unwrap().submit_delay_ms = 0;
    provider.input.as_mut().unwrap().paste_prefix = "Changed: ".into();
    provider.input.as_mut().unwrap().newline = "\n".into();
    register(owner, provider).unwrap();
    screen.refresh_agent_controls_at(now + Duration::from_millis(1374));
    assert!(finish_paste(&writes).is_empty());
    screen.refresh_agent_controls_at(now + Duration::from_millis(1375));
    assert_eq!(finish_paste(&writes), b"\n");
    screen.refresh_agent_controls_at(now + Duration::from_secs(2));
    assert!(finish_paste(&writes).is_empty());
    assert!(replies(&messages, CLIENT).is_empty());
    report_kind(
        &mut screen,
        kind,
        HookEvent {
            prompt: Some("Instructions: review\rcode".into()),
            ..hook("UserPromptSubmit")
        },
    );
    report_kind(
        &mut screen,
        kind,
        HookEvent {
            turn_id: Some("old".into()),
            ..hook("Stop")
        },
    );
    assert!(replies(&messages, CLIENT).is_empty());
    report_kind(&mut screen, kind, hook("Stop"));
    let response = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(response.outcome, AgentOutcome::Completed);
    assert!(response.acknowledged);
    assert_eq!(response.turn_id.as_deref(), Some("turn"));
    assert!(response.agents[0].ready);
    unregister_owner(owner);
}

#[test]
fn plugin_agent_capability_and_command_prefixes_refuse_unsafe_prompt_input() {
    use yinz_types::agent::provider::{register, unregister_owner, AgentInput, AgentProvider};
    let kind = "screen-capability-test";
    let owner = 70_102;
    for input in [
        None,
        Some(AgentInput {
            command_prefixes: vec![":".into()],
            ..Default::default()
        }),
    ] {
        let unsupported = input.is_none();
        register(
            owner,
            AgentProvider {
                kind: kind.into(),
                executable: kind.into(),
                input,
                ..Default::default()
            },
        )
        .unwrap();
        let (mut screen, messages, writes) = setup(kind);
        report_kind(&mut screen, kind, hook("SessionStart"));
        screen.agent_control_with_clear(
            AgentRequest::Prompt {
                preserve_draft: false,
                pane_id: PANE,
                expected_session_id: Some("conversation".into()),
                prompt: ":reset".into(),
                submission_id: None,
                wait: true,
                timeout_ms: 30_000,
            },
            CLIENT,
            &writes,
        );
        let response = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(
            response.outcome,
            if unsupported {
                AgentOutcome::StatusUnavailable
            } else {
                AgentOutcome::Rejected
            }
        );
        assert!(finish_paste(&writes).is_empty());
        assert!(screen.agent_controls.is_empty());
    }
    unregister_owner(owner);
}

#[test]
fn plugin_agent_delayed_submit_preserves_host_input_and_identity_guards() {
    use yinz_types::agent::provider::{register, unregister_owner, AgentInput, AgentProvider};
    let kind = "screen-input-guards-test";
    let owner = 70_103;
    for reason in [
        "input",
        "failure",
        "ack",
        "replacement",
        "closed",
        "timeout",
        "unload",
    ] {
        register(
            owner,
            AgentProvider {
                kind: kind.into(),
                executable: kind.into(),
                input: Some(AgentInput::default()),
                ..Default::default()
            },
        )
        .unwrap();
        let (mut screen, messages, writes) = setup(kind);
        report_kind(&mut screen, kind, hook("SessionStart"));
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        let mut completion = writes
            .try_iter()
            .find_map(|(instruction, _)| match instruction {
                PtyWriteInstruction::Write(_, _, Some(completion)) => Some(completion),
                _ => None,
            })
            .unwrap();
        match reason {
            "input" => screen.agent_write_for_test(),
            "failure" => completion.set_error_message("partial write failed".into()),
            "ack" => report_kind(&mut screen, kind, hook("UserPromptSubmit")),
            "replacement" => {
                let tab = screen.tab_containing_pane_mut(PANE).unwrap();
                tab.hold_pane(PANE, Some(0), false, tab.pane_run_command(PANE).unwrap());
                screen.run_command_in_pane_with_id(
                    TerminalId::new(7),
                    RunCommand {
                        command: "bash".into(),
                        ..Default::default()
                    },
                    false,
                );
            },
            "closed" => screen.close_pane_explicitly(PANE, None),
            "timeout" => screen.agent_controls[0].deadline = Some(Instant::now()),
            "unload" => {
                unregister_owner(owner);
            },
            _ => unreachable!(),
        }
        finish_paste(&writes);
        drop(completion);
        let now = Instant::now();
        screen.refresh_agent_controls_at(now);
        screen.refresh_agent_controls_at(now + Duration::from_millis(101));
        assert!(finish_paste(&writes).is_empty(), "{reason}");
        if reason == "ack" {
            assert!(replies(&messages, CLIENT).is_empty());
        } else {
            assert_eq!(replies(&messages, CLIENT).len(), 1, "{reason}");
        }
    }
    unregister_owner(owner);
}

fn draft_request(screen: &Screen) -> AgentRequest {
    AgentRequest::ClearDraft {
        pane_id: PANE,
        expected_session_id: "conversation".into(),
        input_revision: screen.tabs[&TabId::new(0)].agent_input_revision(PANE),
        confirm_discard: true,
        interrupted: false,
        timeout_ms: 30_000,
    }
}

fn draft_frame(screen: &mut Screen, kind: &str, text: &str, cursor: usize, now: Instant) {
    let pane = screen.tabs[&TabId::new(0)].get_pane_with_id(PANE).unwrap();
    let border = "─".repeat(pane.get_content_columns());
    let border = if kind == "pi" {
        format!("\x1b[38;5;6m{border}\x1b[m")
    } else {
        border
    };
    // Historical labels must not affect either stage of composer verification.
    let history = "Previous response: [Image #1] [Pasted text #1] [Attached file]\r\n\r\n";
    let frame = match kind {
        "claude" => format!(
            "\x1b[2J\x1b[H{history}{border}\r\n❯ {text}\r\n{border}\r\n  auto mode on\x1b[4;{}H",
            cursor + 3
        ),
        "pi" => format!(
            "\x1b[2J\x1b[H{history}{border}\r\n{text}\r\n{border}\r\n/work\r\n0% (openai) gpt-5.5\x1b[4;{}H",
            cursor + 1
        ),
        _ => format!(
            "\x1b[2J\x1b[H{history}\r\n› {text}\r\n\r\n  GPT-6-Astra medium · /work\x1b[4;{}H",
            cursor + 3
        ),
    };
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(TerminalId::new(7), format!("{frame}\x1b[?25h").into_bytes())
        .unwrap();
    screen.pane_output_activity.insert(PANE, now);
}

#[test]
fn clear_draft_verifies_marker_then_stable_empty_before_recovery() {
    for kind in ["claude", "codex", "pi"] {
        let (mut screen, messages, writes) = setup(kind);
        report_kind(&mut screen, kind, hook("SessionStart"));
        let now = Instant::now();
        draft_frame(&mut screen, kind, "old draft", 3, now);
        screen.agent_control_at(
            draft_request(&screen),
            crate::screen::agent_control::Reply::Client(CLIENT),
            now,
        );
        let bytes = finish_paste(&writes);
        let marker = std::str::from_utf8(&bytes)
            .unwrap()
            .strip_prefix("\x1b[200~")
            .unwrap()
            .strip_suffix("\x1b[201~")
            .unwrap();
        screen.agent_control(
            AgentRequest::Status {
                pane_id: PANE,
                expected_session_id: None,
            },
            CLIENT,
        );
        assert_eq!(
            replies(&messages, CLIENT)[0].agents[0].not_ready_reason,
            Some(NotReadyReason::ClearingDraft)
        );
        // Delivery alone and a hidden cursor during redraw cannot authorize Ctrl+C.
        screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .handle_pty_bytes(TerminalId::new(7), b"\x1b[?25l".to_vec())
            .unwrap();
        screen.refresh_agent_controls_at(now + Duration::from_millis(1));
        assert!(finish_paste(&writes).is_empty());
        draft_frame(
            &mut screen,
            kind,
            &format!("old{marker} draft"),
            3 + marker.len(),
            now + Duration::from_millis(2),
        );
        screen.refresh_agent_controls_at(now + Duration::from_millis(3));
        assert_eq!(finish_paste(&writes), [3]);
        screen.refresh_agent_controls_at(now + Duration::from_millis(4));
        assert!(replies(&messages, CLIENT).is_empty());
        draft_frame(&mut screen, kind, "", 0, now + Duration::from_millis(5));
        screen.refresh_agent_controls_at(now + Duration::from_millis(6));
        screen.refresh_agent_controls_at(now + Duration::from_millis(255));
        assert!(replies(&messages, CLIENT).is_empty());
        screen.refresh_agent_controls_at(now + Duration::from_millis(256));
        let response = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(response.outcome, AgentOutcome::Ok, "{kind}");
        assert!(response.agents[0].ready);
        assert_eq!(
            response.agents[0].agent.session_id.as_deref(),
            Some("conversation")
        );
        screen.agent_control_with_clear(prompt(false), CLIENT, &writes);
        assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
    }
}

#[test]
fn clear_draft_rejects_guards_without_writing() {
    for reason in [
        "confirmation",
        "revision",
        "conversation",
        "timeout",
        "layout",
        "busy",
        "typing",
        "paste",
    ] {
        let (mut screen, messages, writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        let now = Instant::now();
        draft_frame(&mut screen, "codex", "draft", 5, now);
        let mut request = draft_request(&screen);
        match reason {
            "confirmation" => {
                if let AgentRequest::ClearDraft {
                    confirm_discard, ..
                } = &mut request
                {
                    *confirm_discard = false;
                }
            },
            "revision" => {
                if let AgentRequest::ClearDraft { input_revision, .. } = &mut request {
                    *input_revision += 1;
                }
            },
            "conversation" => {
                if let AgentRequest::ClearDraft {
                    expected_session_id,
                    ..
                } = &mut request
                {
                    *expected_session_id = "other".into();
                }
            },
            "timeout" => {
                if let AgentRequest::ClearDraft { timeout_ms, .. } = &mut request {
                    *timeout_ms = 0;
                }
            },
            "layout" | "paste" => {
                screen
                    .tabs
                    .get_mut(&TabId::new(0))
                    .unwrap()
                    .handle_pty_bytes(
                        TerminalId::new(7),
                        if reason == "layout" {
                            b"\x1b[?25l".to_vec()
                        } else {
                            b"\x1b[?2004l".to_vec()
                        },
                    )
                    .unwrap();
            },
            "busy" => report(&mut screen, hook("UserPromptSubmit")),
            "typing" => {
                screen.agent_control(send_keys(vec![BareKey::Char('x')]), CLIENT);
                replies(&messages, CLIENT);
                finish_paste(&writes);
                request = draft_request(&screen);
            },
            _ => unreachable!(),
        }
        screen.agent_control(request, CLIENT);
        assert_ne!(
            replies(&messages, CLIENT)[0].outcome,
            AgentOutcome::Ok,
            "{reason}"
        );
        assert!(finish_paste(&writes).is_empty(), "{reason}");
    }
}

#[test]
fn clear_draft_cancellation_drains_writes_and_preserves_new_input() {
    for race in ["typing", "report", "disconnect", "timeout", "replacement"] {
        let (mut screen, messages, writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        let now = Instant::now();
        draft_frame(&mut screen, "codex", "draft", 5, now);
        screen.agent_control_at(
            draft_request(&screen),
            crate::screen::agent_control::Reply::Client(CLIENT),
            now,
        );
        match race {
            "typing" => {
                screen.agent_control(send_keys(vec![BareKey::Char('x')]), ClientId::new(2));
            },
            "report" => report(&mut screen, hook("UserPromptSubmit")),
            "disconnect" => screen.detach_agent_controls(CLIENT),
            "replacement" => screen.invalidate_agent_lifecycle(PANE, "test"),
            _ => {},
        }
        let later = now + Duration::from_secs(if race == "timeout" { 31 } else { 1 });
        screen.refresh_agent_controls_at(later);
        assert!(
            screen.agent_draft_clears.contains_key(&PANE),
            "queued writes keep ownership: {race}"
        );
        screen.agent_control(recover(), ClientId::new(2));
        assert_eq!(
            replies(&messages, ClientId::new(2)).last().unwrap().outcome,
            AgentOutcome::Rejected
        );
        finish_paste(&writes);
        screen.refresh_agent_controls_at(later);
        assert!(screen.agent_draft_clears.is_empty());
        assert!(
            finish_paste(&writes).is_empty(),
            "no destructive stage after {race}"
        );
        assert_eq!(
            screen.agent_status_for_test().ready,
            matches!(race, "typing" | "disconnect" | "timeout"),
            "{race}"
        );
    }
}

#[test]
fn clear_draft_never_treats_suggestions_or_continuation_rows_as_empty() {
    for kind in ["claude", "codex", "pi"] {
        for text in [
            "Try writing a test",
            "[Pasted text #1]",
            "[Image #1]",
            "[Attached file]",
            "\r\n  remaining text",
        ] {
            let (mut screen, messages, writes) = setup(kind);
            report_kind(&mut screen, kind, hook("SessionStart"));
            let now = Instant::now();
            draft_frame(&mut screen, kind, text, 0, now);
            screen.agent_control_at(
                draft_request(&screen),
                crate::screen::agent_control::Reply::Client(CLIENT),
                now,
            );
            assert!(!finish_paste(&writes).is_empty());
            screen.refresh_agent_controls_at(now + Duration::from_secs(31));
            assert_eq!(
                replies(&messages, CLIENT)[0].outcome,
                AgentOutcome::TimedOut
            );
            // A later prompt can retry verified clearing; timeout did not prove emptiness.
            let status = screen.agent_status_for_test();
            assert!(status.clear_draft_supported);
            assert_eq!(status.ready, kind != "claude");
            if kind == "claude" {
                assert_eq!(
                    status.not_ready_reason,
                    Some(NotReadyReason::UnconsumedInput)
                );
            }
        }
    }
}

#[test]
fn clear_draft_attachment_areas_prevent_empty_verification() {
    for (kind, text, row, attachment) in [
        ("codex", "", 2, "  [Image #1]"),
        (
            "codex",
            "\x1b[2mAsk Codex to do anything\x1b[m",
            2,
            "  [Image #1]",
        ),
        ("claude", "", 7, "[Attached file]"),
        ("claude", "\x1b[2mTry \"a test\"\x1b[m", 7, "[Image #1]"),
        ("pi", "", 7, "[Attached file]"),
    ] {
        let (mut screen, messages, writes) = setup(kind);
        report_kind(&mut screen, kind, hook("SessionStart"));
        let now = Instant::now();
        draft_frame(&mut screen, kind, text, 0, now);
        screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .handle_pty_bytes(
                TerminalId::new(7),
                format!("\x1b7\x1b[{row};1H\x1b[2K{attachment}\x1b8").into_bytes(),
            )
            .unwrap();
        screen.agent_control_at(
            draft_request(&screen),
            crate::screen::agent_control::Reply::Client(CLIENT),
            now,
        );
        assert!(!finish_paste(&writes).is_empty(), "{kind}: {text}");
        screen.refresh_agent_controls_at(now + Duration::from_millis(250));
        assert!(replies(&messages, CLIENT).is_empty());
        assert!(!screen.agent_status_for_test().ready);
    }
}

#[test]
fn clear_draft_empty_composer_needs_stability_but_no_keys() {
    for kind in ["claude", "codex", "pi"] {
        let (mut screen, messages, writes) = setup(kind);
        report_kind(&mut screen, kind, hook("SessionStart"));
        let now = Instant::now();
        draft_frame(&mut screen, kind, "", 0, now);
        screen.agent_control_at(
            draft_request(&screen),
            crate::screen::agent_control::Reply::Client(CLIENT),
            now,
        );
        screen.refresh_agent_controls_at(now + Duration::from_millis(249));
        assert!(replies(&messages, CLIENT).is_empty());
        // A partial redraw temporarily restores draft content: restart the stability window.
        draft_frame(
            &mut screen,
            kind,
            "draft",
            0,
            now + Duration::from_millis(249),
        );
        screen.refresh_agent_controls_at(now + Duration::from_millis(250));
        draft_frame(&mut screen, kind, "", 0, now + Duration::from_millis(251));
        screen.refresh_agent_controls_at(now + Duration::from_millis(251));
        screen.refresh_agent_controls_at(now + Duration::from_millis(500));
        assert!(replies(&messages, CLIENT).is_empty());
        screen.refresh_agent_controls_at(now + Duration::from_millis(501));
        assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
        assert!(finish_paste(&writes).is_empty());
        assert!(screen.agent_status_for_test().ready);
    }
}

#[test]
fn clear_draft_partial_writer_failure_never_sends_ctrl_c() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    let now = Instant::now();
    draft_frame(&mut screen, "codex", "draft", 5, now);
    screen.agent_control_at(
        draft_request(&screen),
        crate::screen::agent_control::Reply::Client(CLIENT),
        now,
    );
    for (instruction, _) in writes.try_iter() {
        if let PtyWriteInstruction::Write(_, _, Some(mut completion)) = instruction {
            completion.set_error_message("partial write failed".into());
        }
    }
    screen.refresh_agent_controls_at(now + Duration::from_millis(1));
    let response = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(response.outcome, AgentOutcome::Failed);
    assert!(response.message.unwrap().contains("partial clearing"));
    assert!(response.agents[0].ready);
    assert!(finish_paste(&writes).is_empty());
}

#[test]
fn clear_draft_codex_placeholder_requires_captured_dim_style() {
    for dim in [false, true] {
        let (mut screen, messages, writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        let now = Instant::now();
        let text = if dim {
            "\x1b[2mAsk Codex to do anything\x1b[m"
        } else {
            "Ask Codex to do anything"
        };
        draft_frame(&mut screen, "codex", text, 0, now);
        screen.agent_control_at(
            draft_request(&screen),
            crate::screen::agent_control::Reply::Client(CLIENT),
            now,
        );
        let written = finish_paste(&writes);
        screen.refresh_agent_controls_at(now + Duration::from_millis(250));
        if dim {
            assert!(written.is_empty());
            assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
        } else {
            assert!(!written.is_empty(), "typed placeholder is a real draft");
            assert!(replies(&messages, CLIENT).is_empty());
        }
    }
}

fn captured_composer_frame(screen: &mut Screen, fixture: &serde_json::Value, now: Instant) {
    let cursor = fixture["cursor"]
        .as_array()
        .map(|pair| (pair[0].as_u64().unwrap(), pair[1].as_u64().unwrap()));
    let mut frame = format!(
        "\x1b[2J\x1b[H{}",
        fixture["ansi"].as_str().unwrap().replace('\n', "\r\n")
    );
    if let Some((x, y)) = cursor {
        frame += &format!("\x1b[{};{}H\x1b[?25h", y + 1, x + 1);
    } else {
        frame += "\x1b[?25l";
    }
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(TerminalId::new(7), frame.into_bytes())
        .unwrap();
    screen.pane_output_activity.insert(PANE, now);
}

#[test]
fn clear_draft_captured_default_composers_do_not_confuse_suggestions_with_input() {
    for raw in [
        include_str!("../fixtures/agent-composers/claude-empty.json"),
        include_str!("../fixtures/agent-composers/claude-colored-empty.json"),
        include_str!("../fixtures/agent-composers/codex-empty.json"),
        include_str!("../fixtures/agent-composers/codex-plan-empty.json"),
        include_str!("../fixtures/agent-composers/codex-plan-two-row-footer.json"),
        include_str!("../fixtures/agent-composers/codex-default-switched.json"),
        include_str!("../fixtures/agent-composers/pi-empty.json"),
        include_str!("../fixtures/agent-composers/claude-multiline-draft.json"),
        include_str!("../fixtures/agent-composers/codex-multiline-draft.json"),
        include_str!("../fixtures/agent-composers/codex-plan-multiline-draft.json"),
    ] {
        let fixture: serde_json::Value = serde_json::from_str(raw).unwrap();
        let kind = fixture["provider"].as_str().unwrap();
        let (mut screen, messages, writes) = setup_with_size(
            kind,
            PaneTarget::Local,
            Size {
                cols: fixture["width"].as_u64().unwrap() as usize,
                rows: (fixture["height"].as_u64().unwrap() as usize + 2) * 2,
            },
        );
        report_kind(&mut screen, kind, hook("SessionStart"));
        let now = Instant::now();
        captured_composer_frame(&mut screen, &fixture, now);
        screen.agent_control_at(
            draft_request(&screen),
            crate::screen::agent_control::Reply::Client(CLIENT),
            now,
        );
        let written = finish_paste(&writes);
        screen.refresh_agent_controls_at(now + Duration::from_millis(250));
        let responses = replies(&messages, CLIENT);
        match kind {
            _ if fixture["draft"] == true => {
                assert!(!written.is_empty(), "{kind}: {responses:?}");
                assert!(responses.is_empty());
            },
            "codex" | "claude" if kind == "codex" || fixture["color"] == true => {
                assert!(written.is_empty());
                assert_eq!(
                    responses[0].outcome,
                    AgentOutcome::Ok,
                    "{kind}: {responses:?}"
                );
            },
            "claude" => {
                assert!(!written.is_empty(), "{responses:?}");
                assert!(responses.is_empty());
            },
            "pi" => {
                assert!(written.is_empty());
                assert_eq!(responses[0].outcome, AgentOutcome::Rejected);
            },
            _ => unreachable!(),
        }
    }
}

#[test]
fn codex_collaboration_composers_clear_and_correlate_guarded_prompts() {
    for raw in [
        include_str!("../fixtures/agent-composers/codex-plan-empty.json"),
        include_str!("../fixtures/agent-composers/codex-plan-two-row-footer.json"),
        include_str!("../fixtures/agent-composers/codex-default-switched.json"),
    ] {
        let fixture: serde_json::Value = serde_json::from_str(raw).unwrap();
        let (mut screen, messages, writes) =
            setup_with_size("codex", PaneTarget::Local, Size { cols: 67, rows: 38 });
        report(&mut screen, hook("SessionStart"));
        // Mode changes are terminal input, with no lifecycle acknowledgement.
        screen.agent_control(
            AgentRequest::SendKeys {
                pane_id: PANE,
                expected_session_id: None,
                keys: vec![KeyWithModifier::new(BareKey::Tab).with_shift_modifier()],
            },
            ClientId::new(2),
        );
        finish_paste(&writes);
        let now = Instant::now();
        captured_composer_frame(&mut screen, &fixture, now);
        let status = screen.agent_status_for_test();
        assert!(status.ready && status.clear_draft_supported, "{raw}");
        screen.agent_control_at(
            identified_prompt("plan", true),
            crate::screen::agent_control::Reply::Client(CLIENT),
            now,
        );
        let bytes = finish_paste(&writes);
        let marker = std::str::from_utf8(&bytes)
            .unwrap()
            .strip_prefix("\x1b[200~")
            .unwrap()
            .strip_suffix("\x1b[201~")
            .unwrap();
        let mut marked = fixture.clone();
        marked["ansi"] = fixture["ansi"]
            .as_str()
            .unwrap()
            .replace("Ask Codex to do anything", marker)
            .into();
        marked["cursor"][0] = (2 + marker.len()).into();
        captured_composer_frame(&mut screen, &marked, now + Duration::from_millis(1));
        screen.refresh_agent_controls_at(now + Duration::from_millis(2));
        assert_eq!(finish_paste(&writes), [3]);
        captured_composer_frame(&mut screen, &fixture, now + Duration::from_millis(3));
        screen.refresh_agent_controls_at(now + Duration::from_millis(4));
        screen.refresh_agent_controls_at(now + Duration::from_millis(254));
        assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
        screen.refresh_agent_controls_at(now + Duration::from_millis(255));
        screen.refresh_agent_controls_at(now + Duration::from_secs(1));
        assert_eq!(finish_paste(&writes), b"\r");
        assert!(replies(&messages, CLIENT).is_empty());
        report(&mut screen, hook("UserPromptSubmit"));
        report(&mut screen, hook("Stop"));
        let result = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(result.outcome, AgentOutcome::Completed);
        assert!(result.acknowledged);
        assert_eq!(result.submission_id.as_deref(), Some("plan"));
        assert_eq!(result.turn_id.as_deref(), Some("turn"));
        screen.agent_control(wait_submission("plan"), CLIENT);
        assert_eq!(
            replies(&messages, CLIENT)[0].outcome,
            AgentOutcome::Completed
        );
    }
}

#[test]
fn codex_mode_footer_does_not_authorize_hidden_composers_or_extra_panels() {
    for invalid in ["hidden", "panel", "third-footer", "not-mode"] {
        let mut fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../fixtures/agent-composers/codex-plan-two-row-footer.json"
        ))
        .unwrap();
        match invalid {
            "hidden" => fixture["cursor"] = serde_json::Value::Null,
            "panel" => {
                fixture["ansi"] =
                    format!("{}\n  Continue? (y/n)", fixture["ansi"].as_str().unwrap()).into()
            },
            "third-footer" => {
                fixture["ansi"] = format!(
                    "{}\n  GPT-6-Astra medium · /work",
                    fixture["ansi"].as_str().unwrap()
                )
                .into()
            },
            "not-mode" => {
                fixture["ansi"] = fixture["ansi"]
                    .as_str()
                    .unwrap()
                    .replace("Plan mode", "unknown panel")
                    .into()
            },
            _ => unreachable!(),
        }
        let (mut screen, messages, writes) =
            setup_with_size("codex", PaneTarget::Local, Size { cols: 67, rows: 46 });
        report(&mut screen, hook("SessionStart"));
        let now = Instant::now();
        captured_composer_frame(&mut screen, &fixture, now);
        assert!(
            !screen.agent_status_for_test().clear_draft_supported,
            "{invalid}"
        );
        screen.agent_control_at(
            draft_request(&screen),
            crate::screen::agent_control::Reply::Client(CLIENT),
            now,
        );
        assert_eq!(
            replies(&messages, CLIENT)[0].outcome,
            AgentOutcome::Rejected,
            "{invalid}"
        );
        assert!(finish_paste(&writes).is_empty());
    }
}

#[test]
fn codex_startup_recovery_requires_the_already_bound_conversation() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    for (conversation, outcome) in [
        ("", AgentOutcome::Rejected),
        ("wrong", AgentOutcome::TargetChanged),
        ("conversation", AgentOutcome::Ok),
    ] {
        screen.agent_control(
            AgentRequest::Recover {
                pane_id: PANE,
                expected_session_id: conversation.into(),
                confirm_empty: true,
                interrupted: false,
            },
            CLIENT,
        );
        let response = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(response.outcome, outcome);
        if conversation.is_empty() {
            assert!(response
                .message
                .unwrap()
                .contains("already has a conversation"));
        }
    }
    assert!(finish_paste(&writes).is_empty());
}

#[test]
fn clear_draft_interrupted_recovery_finishes_the_abandoned_submission() {
    let (mut screen, messages, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    let mut request = prompt(true);
    if let AgentRequest::Prompt { submission_id, .. } = &mut request {
        *submission_id = Some("abandoned".into());
    }
    screen.agent_control_with_clear(request, CLIENT, &writes);
    finish_paste(&writes);
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    screen.agent_control(send_keys(vec![BareKey::Esc]), ClientId::new(2));
    finish_paste(&writes);
    screen.refresh_agent_controls();
    replies(&messages, CLIENT);
    let now = Instant::now() + Duration::from_secs(4);
    draft_frame(&mut screen, "claude", "", 0, now);
    let mut request = draft_request(&screen);
    screen.agent_control_at(
        request.clone(),
        crate::screen::agent_control::Reply::Client(CLIENT),
        now,
    );
    assert_eq!(
        replies(&messages, CLIENT)[0].outcome,
        AgentOutcome::Rejected
    );
    if let AgentRequest::ClearDraft { interrupted, .. } = &mut request {
        *interrupted = true;
    }
    screen.agent_control_at(
        request,
        crate::screen::agent_control::Reply::Client(CLIENT),
        now,
    );
    screen.refresh_agent_controls_at(now + Duration::from_millis(250));
    let result = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(result.outcome, AgentOutcome::Ok);
    assert!(result.agents[0].ready);
    assert_eq!(result.agents[0].agent.state, AgentState::Idle);
    screen.agent_control(
        AgentRequest::WaitTurn {
            pane_id: PANE,
            expected_session_id: Some("conversation".into()),
            submission_id: "abandoned".into(),
            timeout_ms: 1000,
        },
        CLIENT,
    );
    let result = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(result.outcome, AgentOutcome::Interrupted);
    assert!(result.message.unwrap().contains("not verified"));
    assert!(finish_paste(&writes).is_empty());
}

#[test]
fn clear_draft_pi_draft_rules_cannot_hide_earlier_input() {
    let (mut screen, messages, writes) = setup("pi");
    report_kind(&mut screen, "pi", hook("SessionStart"));
    let width = screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PANE)
        .unwrap()
        .get_content_columns();
    let now = Instant::now();
    draft_frame(
        &mut screen,
        "pi",
        &format!("draft\r\n{}\r\n", "─".repeat(width)),
        0,
        now,
    );
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(TerminalId::new(7), b"\x1b[6;1H".to_vec())
        .unwrap();
    screen.agent_control_at(
        draft_request(&screen),
        crate::screen::agent_control::Reply::Client(CLIENT),
        now,
    );
    assert_eq!(
        replies(&messages, CLIENT)[0].outcome,
        AgentOutcome::Rejected
    );
    assert!(finish_paste(&writes).is_empty());
}

#[path = "agent_events.rs"]
mod events;

#[test]
fn warped_codex_consumption_handles_parent_reports_and_input_before_restoration() {
    for consumed in [false, true] {
        for newer_input in [false, true] {
            for parent_hook in [None, Some("Stop"), Some("SessionStart")] {
                let (mut destination, _, _) = setup("codex");
                let HookOutcome::Reported(agent) = hook_outcome("codex", &hook("UserPromptSubmit"))
                else {
                    panic!("prompt submission reports a busy agent");
                };
                crate::screen::apply_pane_agent_report(&mut destination, PANE, Some(agent))
                    .unwrap();
                if newer_input {
                    destination.agent_write_for_test();
                }
                if let Some(parent_hook) = parent_hook {
                    report(&mut destination, hook(parent_hook));
                }
                destination.restore_warped_agent_input(
                    PANE,
                    yinz_types::data::WarpedAgentInput {
                        kind: "codex".into(),
                        conversation: "conversation".into(),
                        consumed,
                    },
                );
                if parent_hook.is_none() {
                    report(&mut destination, hook("Stop"));
                }
                assert_eq!(
                    destination.agent_status_for_test().ready,
                    consumed && !newer_input,
                    "consumed={consumed}, newer_input={newer_input}, parent_hook={parent_hook:?}"
                );
            }
        }
    }
}

#[test]
fn warped_codex_consumption_waits_for_parent_and_rejects_new_input() {
    for whole_tab in [false, true] {
        for rewarp in [false, true] {
            for parent_hook in ["Stop", "SessionStart"] {
                for case in [
                    "consumed",
                    "draft",
                    "unfinished_write",
                    "destination_input",
                    "conversation",
                    "unverified",
                ] {
                    // A new conversation's startup establishes its own empty-input lifecycle.
                    if case == "conversation" && parent_hook == "SessionStart" {
                        continue;
                    }
                    let (mut source, _, source_writes) = setup("codex");
                    report(&mut source, hook("SessionStart"));
                    source.agent_control_with_clear(prompt(false), CLIENT, &source_writes);
                    if case != "unfinished_write" {
                        finish_paste(&source_writes);
                        source.refresh_agent_controls();
                    }
                    report(&mut source, hook("UserPromptSubmit"));
                    if case == "draft" {
                        source.agent_write_for_test();
                    }
                    let before = source.warped_agent_input(PANE);
                    assert!(source
                        .take_released_pane_for_warp(
                            TerminalId::new(7),
                            Err("destination refused adoption".into())
                        )
                        .is_err());
                    assert_eq!(source.warped_agent_input(PANE), before);
                    let warp = |source: &mut Screen| {
                        let expected = source.warped_agent_input(PANE).unwrap();
                        let command = source
                            .tab_containing_pane(PANE)
                            .unwrap()
                            .pane_run_command(PANE)
                            .unwrap()
                            .clone();
                        let released = |id| {
                            Ok(released_for_warp(ReleasedPane::Local(ReleasedPtyHost {
                                host_id: yinz_types::internal_ids::SupervisorId::new(format!(
                                    "codex-warp-{id}"
                                )),
                                host_pid: ProcessId::new(1),
                                control_socket: Some(PathBuf::from(format!(
                                    "/tmp/codex-warp-{id}.sock"
                                ))),
                            })))
                        };
                        let payload = if whole_tab {
                            let (tab, terminals, prepared) = source
                                .check_tab_can_be_warped(Some(TabId::new(0)), CLIENT)
                                .unwrap();
                            let released =
                                terminals.into_iter().map(|id| (id, released(id))).collect();
                            source
                                .take_released_tab_for_warp(tab, released, prepared)
                                .unwrap()
                                .tiled_panes
                                .into_iter()
                                .find(|pane| pane.agent_input.is_some())
                                .unwrap()
                        } else {
                            *source
                                .take_released_pane_for_warp(
                                    TerminalId::new(7),
                                    released(TerminalId::new(7)),
                                )
                                .unwrap()
                        };
                        let evidence = payload.agent_input.unwrap();
                        assert_eq!(evidence, expected);
                        let (mut destination, messages, writes) = setup("codex");
                        assert_eq!(
                            destination
                                .tab_containing_pane(PANE)
                                .unwrap()
                                .pane_run_command(PANE),
                            Some(command)
                        );
                        let replay = crate::screen::warp_ops::warp_replay(
                            payload.contents,
                            payload.bracketed_paste,
                            vec![],
                        );
                        destination
                            .tab_containing_pane_mut(PANE)
                            .unwrap()
                            .handle_pty_bytes(TerminalId::new(7), replay)
                            .unwrap();
                        crate::screen::thread_main::apply_pane_agent_report(
                            &mut destination,
                            PANE,
                            payload.agent,
                        )
                        .unwrap();
                        destination.restore_warped_agent_input(PANE, evidence);
                        assert!(!destination.agent_status_for_test().status_available);
                        report(
                            &mut destination,
                            HookEvent {
                                agent_id: Some("worker".into()),
                                ..hook("Stop")
                            },
                        );
                        assert!(!destination.agent_status_for_test().status_available);
                        (destination, messages, writes)
                    };
                    let (mut destination, mut messages, mut writes) = warp(&mut source);
                    assert_eq!(
                        destination.warped_agent_input(PANE).unwrap().consumed,
                        !matches!(case, "draft" | "unfinished_write")
                    );
                    if case == "destination_input" {
                        destination.agent_write_for_test();
                    }
                    if rewarp {
                        assert_eq!(
                            destination.warped_agent_input(PANE).unwrap().consumed,
                            !matches!(case, "draft" | "unfinished_write" | "destination_input")
                        );
                        (destination, messages, writes) = warp(&mut destination);
                    }
                    let mut completion = hook(parent_hook);
                    if case == "conversation" {
                        completion.session_id = "different".into();
                    }
                    if case == "unverified" {
                        completion.codex_hooks_ready = Some(false);
                    }
                    report(&mut destination, completion);
                    assert_eq!(
                        destination.agent_status_for_test().ready,
                        case == "consumed",
                        "{case}, whole_tab={whole_tab}, rewarp={rewarp}, parent_hook={parent_hook}"
                    );
                    if matches!(case, "draft" | "unfinished_write" | "destination_input") {
                        report(&mut destination, hook("SessionStart"));
                        assert_eq!(
                            destination.agent_status_for_test().not_ready_reason,
                            Some(NotReadyReason::UnconsumedInput)
                        );
                    }
                    if case == "consumed" {
                        assert_eq!(
                            destination
                                .agent_status_for_test()
                                .agent
                                .session_id
                                .as_deref(),
                            Some("conversation")
                        );
                        destination.agent_control_with_clear(prompt(false), CLIENT, &writes);
                        assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
                        destination.refresh_agent_controls();
                        report(
                            &mut destination,
                            HookEvent {
                                turn_id: Some("next-turn".into()),
                                ..hook("UserPromptSubmit")
                            },
                        );
                        assert_eq!(
                            replies(&messages, CLIENT).pop().unwrap().outcome,
                            AgentOutcome::Submitted
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn warp_replay_restores_paste_mode_and_buffered_output_wins() {
    for enabled in [false, true] {
        for contents in [None, Some(String::new()), Some("screen\x1b[?2004h".into())] {
            for buffered in [None, Some(false), Some(true)] {
                let (mut screen, _, _) = setup("codex");
                let bytes = crate::screen::warp_ops::warp_replay(
                    contents.clone(),
                    enabled,
                    match buffered {
                        None => vec![],
                        Some(true) => b"\x1b[?2004h".to_vec(),
                        Some(false) => b"\x1b[?2004l".to_vec(),
                    },
                );
                screen
                    .tab_containing_pane_mut(PANE)
                    .unwrap()
                    .handle_pty_bytes(TerminalId::new(7), bytes)
                    .unwrap();
                assert_eq!(
                    screen
                        .tab_containing_pane(PANE)
                        .unwrap()
                        .get_pane_with_id(PANE)
                        .unwrap()
                        .supports_bracketed_paste(),
                    buffered.unwrap_or(enabled)
                );
            }
        }
    }
}

#[test]
fn prompt_automatically_replaces_idle_drafts() {
    for kind in ["claude", "codex", "pi", "agy", "muse"] {
        for draft in ["recent", "existing", "stale-empty", "unknown", "multiline"] {
            for preserve in [false, true] {
                let (mut screen, messages, writes) = setup(kind);
                report_kind(&mut screen, kind, hook("SessionStart"));
                let now = Instant::now();
                if matches!(draft, "recent" | "stale-empty" | "unknown") {
                    screen.agent_control(send_keys(vec![BareKey::Char('x')]), ClientId::new(2));
                    finish_paste(&writes);
                }
                if draft != "unknown" && matches!(kind, "claude" | "codex" | "pi") {
                    draft_frame(
                        &mut screen,
                        kind,
                        match draft {
                            "stale-empty" => "",
                            "multiline" => "old\nsecond line",
                            _ => "old",
                        },
                        0,
                        now,
                    );
                }
                let mut request = prompt(false);
                if let AgentRequest::Prompt { preserve_draft, .. } = &mut request {
                    *preserve_draft = preserve;
                }
                screen.agent_control_at(
                    request,
                    crate::screen::agent_control::Reply::Client(CLIENT),
                    now,
                );
                // No rendered acknowledgement of the marker or empty composer is needed.
                screen.refresh_agent_controls_at(now + Duration::from_millis(1));
                let bytes = finish_paste(&writes);
                assert!(
                    bytes.starts_with(b"\x1b[200~yz") && bytes.ends_with(b"\x1b[201~"),
                    "{kind}/{draft}: {bytes:?}, {:?}",
                    replies(&messages, CLIENT)
                );
                screen.refresh_agent_controls_at(now + Duration::from_millis(2));
                let now = if kind == "pi" {
                    assert!(finish_paste(&writes).is_empty());
                    screen.refresh_agent_controls_at(now + Duration::from_millis(601));
                    assert!(finish_paste(&writes).is_empty());
                    screen.refresh_agent_controls_at(now + Duration::from_millis(602));
                    now + Duration::from_millis(600)
                } else {
                    now
                };
                assert_eq!(finish_paste(&writes), [3]);
                screen.refresh_agent_controls_at(now + Duration::from_millis(3));
                screen.refresh_agent_controls_at(now + Duration::from_millis(252));
                assert!(
                    finish_paste(&writes).is_empty(),
                    "clear must settle before delivery"
                );
                screen.refresh_agent_controls_at(now + Duration::from_millis(253));
                assert_eq!(
                    finish_paste(&writes),
                    b"\x1b[200~review\x1b[201~",
                    "{kind}/{draft}"
                );
                assert!(replies(&messages, CLIENT).is_empty());
                screen.refresh_agent_controls_at(now + Duration::from_millis(254));
                screen.refresh_agent_controls_at(now + Duration::from_secs(1));
                assert_eq!(finish_paste(&writes), b"\r");
                screen.refresh_agent_controls_at(now + Duration::from_secs(2));
                assert!(finish_paste(&writes).is_empty());
                assert!(
                    replies(&messages, CLIENT).is_empty(),
                    "writes do not acknowledge a prompt"
                );
                report_kind(&mut screen, kind, hook("UserPromptSubmit"));
                let response = replies(&messages, CLIENT).pop().unwrap();
                assert_eq!(response.outcome, AgentOutcome::Submitted, "{kind}/{draft}");
                assert!(response.acknowledged);
            }
        }
    }
}

/// A freshly spawned agent reports its conversation first; the plugin keeping its run history
/// binds the derived `<kind>:<conversation>` run ID moments later, which can land while a prompt
/// is already preparing (Agenda 67: Claude prompts failed with `target_changed`).
#[test]
fn first_derived_run_id_binding_before_or_during_prompt_preparation_delivers() {
    for kind in ["codex", "claude"] {
        for binding in ["before", "marker", "clearing"] {
            let (mut screen, messages, writes) = setup(kind);
            report_kind(&mut screen, kind, hook("SessionStart"));
            let original = screen.get_pane_info(PANE).unwrap().agent.unwrap();
            assert_eq!(original.run_id, None);
            let mut bound = original.clone();
            bound.run_id = Some(format!("{kind}:conversation"));
            if binding == "before" {
                screen
                    .tab_containing_pane_mut(PANE)
                    .unwrap()
                    .set_pane_agent(PANE, Some(bound.clone()));
            }
            let now = Instant::now();
            screen.agent_control_at(
                prompt(false),
                crate::screen::agent_control::Reply::Client(CLIENT),
                now,
            );
            let marker = finish_paste(&writes);
            assert!(marker.starts_with(b"\x1b[200~yz") && marker.ends_with(b"\x1b[201~"));
            if binding == "clearing" {
                screen.refresh_agent_controls_at(now + Duration::from_millis(1));
                assert_eq!(finish_paste(&writes), [3]);
            }
            let revision = screen.tabs[&TabId::new(0)].agent_input_revision(PANE);
            screen
                .tab_containing_pane_mut(PANE)
                .unwrap()
                .set_pane_agent(PANE, Some(bound));
            assert_eq!(
                screen.tabs[&TabId::new(0)].agent_input_revision(PANE),
                revision
            );
            if binding != "clearing" {
                screen.refresh_agent_controls_at(now + Duration::from_millis(1));
                assert_eq!(finish_paste(&writes), [3], "{kind}/{binding}");
            }
            screen.refresh_agent_controls_at(now + Duration::from_millis(2));
            screen.refresh_agent_controls_at(now + Duration::from_millis(252));
            assert_eq!(
                finish_paste(&writes),
                b"\x1b[200~review\x1b[201~",
                "{kind}/{binding}: {:?}",
                replies(&messages, CLIENT)
            );
            screen.refresh_agent_controls_at(now + Duration::from_millis(253));
            screen.refresh_agent_controls_at(now + Duration::from_secs(1));
            assert_eq!(finish_paste(&writes), b"\r");
            report_kind(&mut screen, kind, hook("UserPromptSubmit"));
            let response = replies(&messages, CLIENT).pop().unwrap();
            assert_eq!(
                response.outcome,
                AgentOutcome::Submitted,
                "{kind}/{binding}"
            );
            assert_eq!(
                response.prompt_delivery,
                Some(yinz_types::agent::control::PromptDelivery::Started)
            );
            assert!(response.acknowledged);
        }
    }
}

#[test]
fn prompt_preparation_pins_identity_and_retains_unsent_failures() {
    use yinz_types::agent::control::PromptDelivery;
    for kind in ["codex", "claude"] {
        let other_kind = if kind == "codex" { "claude" } else { "codex" };
        for stage in ["marker", "clearing"] {
            for change in [
                // A run ID derived from some other conversation.
                "noncanonical",
                // Another provider's derivation for the same conversation.
                "foreign",
                // A fallback ID that is not derived from the conversation at all.
                "fallback",
                "replacement",
                "removal",
                "provider",
                "conversation",
                // A real replacement: new conversation with its own derived run ID.
                "rebound",
                "missing",
                "input",
            ] {
                let (mut screen, messages, writes) = setup(kind);
                report_kind(&mut screen, kind, hook("SessionStart"));
                let mut original = screen.get_pane_info(PANE).unwrap().agent.unwrap();
                if matches!(change, "replacement" | "removal") {
                    original.run_id = Some(format!("{kind}:conversation"));
                }
                screen
                    .tab_containing_pane_mut(PANE)
                    .unwrap()
                    .set_pane_agent(PANE, Some(original.clone()));
                let now = Instant::now();
                let mut request = prompt(false);
                if let AgentRequest::Prompt { submission_id, .. } = &mut request {
                    *submission_id = Some("identity-check".into());
                }
                screen.agent_control_at(
                    request,
                    crate::screen::agent_control::Reply::Client(CLIENT),
                    now,
                );
                finish_paste(&writes);
                if stage == "clearing" {
                    screen.refresh_agent_controls_at(now + Duration::from_millis(1));
                    assert_eq!(finish_paste(&writes), [3]);
                }
                let mut changed = original.clone();
                match change {
                    "noncanonical" | "replacement" => {
                        changed.run_id = Some(format!("{kind}:other"))
                    },
                    "foreign" => changed.run_id = Some(format!("{other_kind}:conversation")),
                    "fallback" => changed.run_id = Some(format!("{kind}:pane:7:1")),
                    "removal" => changed.run_id = None,
                    "provider" => changed.kind = other_kind.into(),
                    "conversation" => changed.session_id = Some("other".into()),
                    "rebound" => {
                        changed.session_id = Some("other".into());
                        changed.run_id = Some(format!("{kind}:other"));
                    },
                    "input" => {
                        screen.agent_control(send_keys(vec![BareKey::Char('x')]), ClientId::new(2))
                    },
                    _ => {},
                }
                screen
                    .tab_containing_pane_mut(PANE)
                    .unwrap()
                    .set_pane_agent(PANE, (change != "missing").then_some(changed));
                screen.refresh_agent_controls_at(now + Duration::from_millis(2));
                let response = replies(&messages, CLIENT).pop().unwrap();
                assert_eq!(
                    response.outcome,
                    match change {
                        "missing" => AgentOutcome::StatusUnavailable,
                        "input" => AgentOutcome::Interrupted,
                        _ => AgentOutcome::TargetChanged,
                    },
                    "{kind}/{stage}/{change}"
                );
                assert_eq!(response.prompt_delivery, Some(PromptDelivery::NotSent));
                assert!(!response.acknowledged);
                assert!(response.turn_id.is_none());
                // Restore status only to retrieve the retained result; never retry preparation.
                screen
                    .tab_containing_pane_mut(PANE)
                    .unwrap()
                    .set_pane_agent(PANE, Some(original));
                screen.agent_control(
                    AgentRequest::WaitTurn {
                        pane_id: PANE,
                        expected_session_id: Some("conversation".into()),
                        submission_id: "identity-check".into(),
                        timeout_ms: 1000,
                    },
                    CLIENT,
                );
                let retained = replies(&messages, CLIENT).pop().unwrap();
                assert_eq!(retained.outcome, response.outcome);
                assert_eq!(retained.prompt_delivery, response.prompt_delivery);
                screen.refresh_agent_controls_at(now + Duration::from_secs(1));
                let remaining = finish_paste(&writes);
                assert!(remaining.is_empty() || (change == "input" && remaining == b"x"));
            }
        }
    }
}

/// Once the derived run ID is accepted it is pinned: a later change to it still fails closed.
#[test]
fn accepted_derived_run_id_is_pinned_for_later_stages() {
    for kind in ["codex", "claude"] {
        for change in ["replacement", "removal"] {
            let (mut screen, messages, writes) = setup(kind);
            report_kind(&mut screen, kind, hook("SessionStart"));
            let original = screen.get_pane_info(PANE).unwrap().agent.unwrap();
            assert_eq!(original.run_id, None);
            let now = Instant::now();
            screen.agent_control_at(
                prompt(false),
                crate::screen::agent_control::Reply::Client(CLIENT),
                now,
            );
            finish_paste(&writes);
            let mut bound = original.clone();
            bound.run_id = Some(format!("{kind}:conversation"));
            screen
                .tab_containing_pane_mut(PANE)
                .unwrap()
                .set_pane_agent(PANE, Some(bound.clone()));
            screen.refresh_agent_controls_at(now + Duration::from_millis(1));
            assert_eq!(finish_paste(&writes), [3], "{kind}/{change}");
            let mut changed = bound;
            changed.run_id = (change == "replacement").then(|| format!("{kind}:other"));
            screen
                .tab_containing_pane_mut(PANE)
                .unwrap()
                .set_pane_agent(PANE, Some(changed));
            screen.refresh_agent_controls_at(now + Duration::from_millis(2));
            let response = replies(&messages, CLIENT).pop().unwrap();
            assert_eq!(
                response.outcome,
                AgentOutcome::TargetChanged,
                "{kind}/{change}"
            );
            screen.refresh_agent_controls_at(now + Duration::from_secs(1));
            assert!(finish_paste(&writes).is_empty(), "{kind}/{change}");
        }
    }
}

#[test]
fn pi_prompt_clear_waits_after_marker_delivery_and_honors_cancellation() {
    for cancellation in ["none", "typing", "timeout"] {
        let (mut screen, messages, writes) = setup("pi");
        report_kind(&mut screen, "pi", hook("SessionStart"));
        let now = Instant::now();
        let mut request = prompt(false);
        if cancellation == "timeout"
            && let AgentRequest::Prompt { timeout_ms, .. } = &mut request
        {
            *timeout_ms = 1500;
        }
        screen.agent_control_at(
            request,
            crate::screen::agent_control::Reply::Client(CLIENT),
            now,
        );
        // Queue time does not count: the marker is still awaiting writer acknowledgement.
        screen.refresh_agent_controls_at(now + Duration::from_millis(1000));
        let marker = finish_paste(&writes);
        assert!(marker.starts_with(b"\x1b[200~yz") && marker.ends_with(b"\x1b[201~"));
        screen.refresh_agent_controls_at(now + Duration::from_millis(1000));
        assert!(finish_paste(&writes).is_empty());
        if cancellation == "typing" {
            screen.agent_control(send_keys(vec![BareKey::Char('x')]), ClientId::new(2));
            assert_eq!(finish_paste(&writes), b"x");
        }
        screen.refresh_agent_controls_at(now + Duration::from_millis(1599));
        assert!(finish_paste(&writes).is_empty());
        screen.refresh_agent_controls_at(now + Duration::from_millis(1600));
        if cancellation == "none" {
            assert_eq!(finish_paste(&writes), [3]);
        } else {
            assert!(finish_paste(&writes).is_empty());
            assert!(screen.agent_draft_clears.is_empty());
            assert_eq!(
                replies(&messages, CLIENT).pop().unwrap().outcome,
                if cancellation == "typing" {
                    AgentOutcome::Interrupted
                } else {
                    AgentOutcome::TimedOut
                }
            );
        }
    }
}

#[test]
fn pi_prompt_clear_is_spaced_after_new_or_cancelled_clear() {
    for cancelled in [false, true] {
        let (mut screen, messages, writes) = setup("pi");
        report_kind(&mut screen, "pi", hook("SessionStart"));
        let now = Instant::now();
        let mut request = prompt(false);
        if let AgentRequest::Prompt { prompt, .. } = &mut request {
            *prompt = "/new".into();
        }
        screen.agent_control_at(
            request,
            crate::screen::agent_control::Reply::Client(CLIENT),
            now,
        );
        finish_paste(&writes);
        screen.refresh_agent_controls_at(now);
        let first_clear = now + Duration::from_millis(600);
        screen.refresh_agent_controls_at(first_clear);
        assert_eq!(finish_paste(&writes), [3]);
        let retry_at = if cancelled {
            screen.detach_agent_controls(CLIENT);
            screen.refresh_agent_controls_at(first_clear);
            assert!(finish_paste(&writes).is_empty());
            first_clear
        } else {
            screen.refresh_agent_controls_at(first_clear);
            screen.refresh_agent_controls_at(first_clear + Duration::from_millis(250));
            assert_eq!(finish_paste(&writes), b"/new");
            screen.refresh_agent_controls_at(first_clear + Duration::from_millis(250));
            let submitted = first_clear + Duration::from_millis(350);
            screen.refresh_agent_controls_at(submitted);
            assert_eq!(finish_paste(&writes), b"\r");
            screen.refresh_agent_controls_at(submitted);
            report_kind(&mut screen, "pi", hook("SessionEnd"));
            report_kind(&mut screen, "pi", clear_start());
            assert!(screen.agent_status_for_test().ready);
            submitted
        };
        replies(&messages, CLIENT);
        screen.agent_control_at(
            prompt(false),
            crate::screen::agent_control::Reply::Client(CLIENT),
            retry_at,
        );
        let marker = finish_paste(&writes);
        assert!(marker.starts_with(b"\x1b[200~yz") && marker.ends_with(b"\x1b[201~"));
        screen.refresh_agent_controls_at(retry_at);
        screen.refresh_agent_controls_at(retry_at + Duration::from_millis(599));
        assert!(finish_paste(&writes).is_empty());
        let second_clear = retry_at + Duration::from_millis(600);
        screen.refresh_agent_controls_at(second_clear);
        assert_eq!(finish_paste(&writes), [3]);
        assert!(second_clear.duration_since(first_clear) > Duration::from_millis(500));
        screen.refresh_agent_controls_at(second_clear);
        screen.refresh_agent_controls_at(second_clear + Duration::from_millis(250));
        assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
    }
}

#[test]
fn automatic_draft_clear_validates_before_writing() {
    for kind in ["claude", "codex", "pi"] {
        for invalid in [
            "empty",
            "control",
            "command",
            "timeout",
            "id",
            "conversation",
        ] {
            let (mut screen, messages, writes) = setup(kind);
            report_kind(&mut screen, kind, hook("SessionStart"));
            draft_frame(&mut screen, kind, "old", 3, Instant::now());
            let mut request = prompt(false);
            if let AgentRequest::Prompt {
                prompt,
                timeout_ms,
                submission_id,
                expected_session_id,
                ..
            } = &mut request
            {
                match invalid {
                    "empty" => *prompt = " ".into(),
                    "control" => *prompt = "\x03".into(),
                    "command" => *prompt = "/help".into(),
                    "timeout" => *timeout_ms = 0,
                    "id" => *submission_id = Some("".into()),
                    _ => *expected_session_id = Some("other".into()),
                }
            }
            screen.agent_control(request, CLIENT);
            assert_eq!(replies(&messages, CLIENT).len(), 1, "{kind}/{invalid}");
            assert!(finish_paste(&writes).is_empty(), "{kind}/{invalid}");
            assert!(screen.agent_draft_clears.is_empty());
        }
    }
}

#[test]
fn automatic_draft_clear_cancellation_retains_result_and_drains_writer() {
    for race in [
        "typing",
        "report",
        "disconnect",
        "timeout",
        "replacement",
        "writer",
    ] {
        let (mut screen, messages, writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        let now = Instant::now();
        draft_frame(&mut screen, "codex", "draft", 5, now);
        let mut request = prompt(false);
        if let AgentRequest::Prompt { submission_id, .. } = &mut request {
            *submission_id = Some("auto-clear".into());
        }
        screen.agent_control_at(
            request.clone(),
            crate::screen::agent_control::Reply::Client(CLIENT),
            now,
        );
        screen.agent_control(request, ClientId::new(2));
        assert_eq!(
            replies(&messages, ClientId::new(2))[0].outcome,
            AgentOutcome::Rejected
        );
        match race {
            "typing" => screen.agent_control(send_keys(vec![BareKey::Char('x')]), ClientId::new(2)),
            "report" => report(&mut screen, hook("UserPromptSubmit")),
            "disconnect" => screen.detach_agent_controls(CLIENT),
            "replacement" => screen.invalidate_agent_lifecycle(PANE, "test"),
            "writer" => {
                for (instruction, _) in writes.try_iter() {
                    if let PtyWriteInstruction::Write(_, _, Some(mut completion)) = instruction {
                        completion.set_error_message("test writer failure".into());
                    }
                }
            },
            _ => {},
        }
        let later = now + Duration::from_secs(if race == "timeout" { 31 } else { 1 });
        screen.refresh_agent_controls_at(later);
        if race != "writer" {
            assert!(screen.agent_draft_clears.contains_key(&PANE), "{race}");
            let status = screen.agent_status(PANE, None).unwrap();
            assert!(!status.ready);
            assert_eq!(status.not_ready_reason, Some(NotReadyReason::ClearingDraft));
        }
        finish_paste(&writes);
        screen.refresh_agent_controls_at(later);
        assert!(screen.agent_draft_clears.is_empty());
        assert!(finish_paste(&writes).is_empty(), "no delivery after {race}");
        if race != "replacement" {
            screen.agent_control(
                AgentRequest::WaitTurn {
                    pane_id: PANE,
                    expected_session_id: None,
                    submission_id: "auto-clear".into(),
                    timeout_ms: 1000,
                },
                ClientId::new(3),
            );
            let result = replies(&messages, ClientId::new(3)).pop().unwrap();
            assert!(!result.acknowledged);
            assert_eq!(
                result.prompt_delivery,
                Some(yinz_types::agent::control::PromptDelivery::NotSent)
            );
            assert_ne!(
                screen.agent_status(PANE, None).unwrap().not_ready_reason,
                Some(NotReadyReason::ClearingDraft)
            );
            assert_eq!(
                result.outcome,
                match race {
                    "timeout" => AgentOutcome::TimedOut,
                    "writer" => AgentOutcome::Failed,
                    _ => AgentOutcome::Interrupted,
                },
                "{race}"
            );
        }
    }
}

#[test]
fn automatic_draft_clear_and_prompt_share_the_deadline_and_submission() {
    let (mut screen, messages, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    let now = Instant::now();
    draft_frame(&mut screen, "claude", "old", 3, now);
    let mut request = prompt(false);
    if let AgentRequest::Prompt {
        submission_id,
        timeout_ms,
        ..
    } = &mut request
    {
        *submission_id = Some("shared-deadline".into());
        *timeout_ms = 500;
    }
    screen.agent_control_at(
        request.clone(),
        crate::screen::agent_control::Reply::Client(CLIENT),
        now,
    );
    let bytes = finish_paste(&writes);
    let marker = std::str::from_utf8(&bytes)
        .unwrap()
        .strip_prefix("\x1b[200~")
        .unwrap()
        .strip_suffix("\x1b[201~")
        .unwrap();
    draft_frame(
        &mut screen,
        "claude",
        marker,
        marker.len(),
        now + Duration::from_millis(1),
    );
    screen.refresh_agent_controls_at(now + Duration::from_millis(2));
    assert_eq!(finish_paste(&writes), [3]);
    draft_frame(&mut screen, "claude", "", 0, now + Duration::from_millis(3));
    screen.refresh_agent_controls_at(now + Duration::from_millis(4));
    screen.refresh_agent_controls_at(now + Duration::from_millis(254));
    assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
    screen.refresh_agent_controls_at(now + Duration::from_millis(255));
    screen.refresh_agent_controls_at(now + Duration::from_millis(600));
    let result = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(result.outcome, AgentOutcome::TimedOut);
    assert_eq!(result.submission_id.as_deref(), Some("shared-deadline"));
    assert!(!result.acknowledged);
    assert!(
        finish_paste(&writes).is_empty(),
        "deadline prevents Enter after clearing"
    );
    screen.agent_control(request, ClientId::new(2));
    let duplicate = replies(&messages, ClientId::new(2)).pop().unwrap();
    assert_eq!(duplicate.outcome, AgentOutcome::Rejected);
    assert!(duplicate.message.unwrap().contains("already retained"));
    assert!(finish_paste(&writes).is_empty());
}

#[test]
fn disconnect_before_prompt_delivery_finishes_cancels_enter() {
    for drained in [false, true] {
        let (mut screen, _messages, writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        screen.agent_control(prompt(false), CLIENT);
        if drained {
            finish_paste(&writes);
            screen.refresh_agent_controls();
        }
        screen.detach_agent_controls(CLIENT);
        finish_paste(&writes);
        screen.refresh_agent_controls_at(Instant::now() + Duration::from_secs(1));
        assert!(finish_paste(&writes).is_empty(), "disconnect cancels Enter");
        assert!(
            !screen.agent_status_for_test().ready,
            "unconsumed paste retains ownership"
        );
    }
}

fn visibility(suppressed: bool, conversation: Option<&str>) -> AgentRequest {
    let expected_session_id = conversation.map(str::to_owned);
    if suppressed {
        AgentRequest::Hide {
            pane_id: PANE,
            expected_session_id,
        }
    } else {
        AgentRequest::Show {
            pane_id: PANE,
            expected_session_id,
        }
    }
}

#[test]
fn suppressed_support_probe_confirms_the_exact_token() {
    let (mut screen, messages, _writes) = setup("bash");
    screen.agent_control(AgentRequest::SpawnSuppressedSupport, CLIENT);
    let response = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(response.outcome, AgentOutcome::Ok);
    assert_eq!(response.output.as_deref(), Some("agent-suppressed-v1"));
}

#[test]
fn agent_hide_rejects_the_last_selectable_pane_before_settlement() {
    for placement in [
        NewPanePlacement::default(),
        NewPanePlacement::Floating(None),
    ] {
        for companion in ["absent", "nonselectable", "pending", "closing", "overlay"] {
            for guarded in [false, true] {
                let (mut screen, messages, _writes) = setup_with_placement(
                    "codex",
                    PaneTarget::Local,
                    Size { cols: 80, rows: 24 },
                    placement.clone(),
                );
                report(&mut screen, hook("SessionStart"));
                let tab = screen.tab_containing_pane_mut(PANE).unwrap();
                let original = PaneId::Terminal(TerminalId::new(1));
                if companion == "nonselectable" {
                    tab.set_pane_selectable(original, false);
                } else {
                    tab.close_pane(original, false, None);
                }
                let other_tab = TabId::new(1);
                match companion {
                    "pending" => {
                        screen
                            .new_tab(other_tab, (vec![], vec![]), None, None)
                            .unwrap();
                        let tab = screen.tabs.get_mut(&other_tab).unwrap();
                        tab.new_pane(
                            PaneId::Terminal(TerminalId::new(9)),
                            None,
                            None,
                            false,
                            false,
                            NewPanePlacement::default(),
                            None,
                            None,
                        )
                        .unwrap();
                        assert!(tab.is_pending() && tab.has_selectable_panes());
                    },
                    "closing" => {
                        new_tab(&mut screen, 9, other_tab);
                        screen.deferred_tab_closes.insert(other_tab);
                    },
                    "overlay" => {
                        tab.new_pane(
                            PaneId::Terminal(TerminalId::new(9)),
                            None,
                            None,
                            false,
                            false,
                            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
                            Some(CLIENT),
                            None,
                        )
                        .unwrap();
                    },
                    _ => {},
                }
                screen
                    .focus_pane_with_id(PANE, false, false, CLIENT)
                    .unwrap();
                screen.set_quit_notice(PluginId::new(8), None, guarded);
                let (to_pty, pty) = channels::unbounded();
                screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));
                screen.tabs.get_mut(&TabId::new(0)).unwrap().senders.to_pty =
                    screen.bus.senders.to_pty.clone();

                for (conversation, outcome) in [
                    ("conversation", AgentOutcome::Rejected),
                    ("other", AgentOutcome::TargetChanged),
                ] {
                    screen.agent_control(visibility(true, Some(conversation)), CLIENT);
                    let response = replies(&messages, CLIENT).pop().unwrap();
                    assert_eq!(
                        response.outcome, outcome,
                        "{placement:?}, {companion}, guard {guarded}"
                    );
                    if outcome == AgentOutcome::Rejected {
                        assert_eq!(
                            response.message.as_deref(),
                            Some("cannot hide the session's last selectable pane")
                        );
                    }
                    screen.settle_tabs().unwrap();
                    let info = screen.get_pane_info(PANE).unwrap();
                    assert!(!info.is_suppressed && !info.exited);
                    assert_eq!(screen.active_tab_ids[&CLIENT], TabId::new(0));
                    assert_eq!(
                        screen.tabs[&TabId::new(0)].get_active_pane_id(CLIENT),
                        Some(PANE)
                    );
                    assert_eq!(
                        screen.tabs.len(),
                        if companion == "pending" { 2 } else { 1 }
                    );
                    assert!(screen.replacement_tab.is_none() && !screen.replacement_pending);
                    assert!(pty.try_iter().all(|(instruction, _)| match instruction {
                        PtyInstruction::ClosePane(id, _) => id != PANE,
                        PtyInstruction::CloseTab(ids) => !ids.contains(&PANE),
                        _ => true,
                    }));
                }
            }
        }
    }
}

#[test]
fn agent_hide_with_another_selectable_pane_survives_settlement() {
    for placement in [
        NewPanePlacement::default(),
        NewPanePlacement::Floating(None),
    ] {
        for destination in ["tiled", "floating", "tab"] {
            let (mut screen, messages, _writes) = setup_with_placement(
                "codex",
                PaneTarget::Local,
                Size { cols: 80, rows: 24 },
                placement.clone(),
            );
            report(&mut screen, hook("SessionStart"));
            let tab = screen.tab_containing_pane_mut(PANE).unwrap();
            tab.close_pane(PaneId::Terminal(TerminalId::new(1)), false, None);
            let survivor = PaneId::Terminal(TerminalId::new(9));
            let destination_tab = if destination == "tab" {
                new_tab(&mut screen, 9, TabId::new(1));
                TabId::new(1)
            } else {
                tab.new_pane(
                    survivor,
                    None,
                    None,
                    false,
                    false,
                    if destination == "floating" {
                        NewPanePlacement::Floating(None)
                    } else {
                        NewPanePlacement::default()
                    },
                    Some(CLIENT),
                    None,
                )
                .unwrap();
                TabId::new(0)
            };
            screen
                .focus_pane_with_id(PANE, false, false, CLIENT)
                .unwrap();
            let (to_pty, pty) = channels::unbounded();
            screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));
            screen.tabs.get_mut(&TabId::new(0)).unwrap().senders.to_pty =
                screen.bus.senders.to_pty.clone();
            for _ in 0..2 {
                screen.agent_control(visibility(true, Some("conversation")), CLIENT);
                let response = replies(&messages, CLIENT).pop().unwrap();
                assert_eq!(
                    response.outcome,
                    AgentOutcome::Ok,
                    "{placement:?}, {destination}"
                );
                screen.settle_tabs().unwrap();
                let info = screen.get_pane_info(PANE).unwrap();
                assert!(info.is_suppressed && !info.exited);
                assert!(info.agent.is_some());
                assert_eq!(screen.tab_id_containing_pane(PANE), Some(TabId::new(0)));
                assert_eq!(screen.active_tab_ids[&CLIENT], destination_tab);
                assert_eq!(
                    screen.tabs[&destination_tab].get_active_pane_id(CLIENT),
                    Some(survivor)
                );
                assert!(pty.try_recv().is_err());
            }
        }
    }
}

#[test]
fn agent_show_restores_an_overlay_hidden_through_the_screen_instruction() {
    let mut mock = MockScreen::new(Size { cols: 80, rows: 24 });
    let received = Watched::new(vec![]);
    let receiver = mock.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(received, ServerInstruction::KillSession, receiver);
    let screen_thread = mock.run(None, vec![]);
    let send = |instruction| mock.to_screen.send(instruction).unwrap();
    let pane_info = || {
        let (tx, rx) = channels::bounded(1);
        send(ScreenInstruction::GetPaneInfo {
            pane_id: PANE,
            response_channel: tx,
        });
        rx.recv_timeout(yinz_test_support::default_timeout())
            .unwrap()
            .unwrap()
    };
    let focused_pane = || {
        let (tx, rx) = channels::bounded(1);
        send(ScreenInstruction::GetFocusedPaneInfo {
            client_id: CLIENT,
            response_channel: tx,
        });
        rx.recv_timeout(yinz_test_support::default_timeout())
            .unwrap()
    };
    send(ScreenInstruction::NewPane(
        PANE,
        None,
        None,
        Some(Run::Command(RunCommand {
            command: PathBuf::from("env"),
            args: vec![
                format!("{}=1", yinz_types::agent::AGENT_PANE_ENV),
                "codex".into(),
            ],
            ..Default::default()
        })),
        NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(
            FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(2)),
                y: Some(PercentOrFixed::Fixed(3)),
                width: Some(PercentOrFixed::Fixed(40)),
                height: Some(PercentOrFixed::Fixed(12)),
                ..Default::default()
            },
        ))),
        false,
        ClientTabIndexOrPaneId::ClientId(CLIENT),
        None,
        false,
    ));
    send(ScreenInstruction::FocusPaneWithId(
        PaneId::Terminal(TerminalId::new(0)),
        false,
        false,
        CLIENT,
        None,
    ));
    let before = pane_info();
    assert!(!before.is_suppressed);
    let focus = focused_pane();
    assert_eq!(
        focus,
        yinz_types::data::GetFocusedPaneInfoResponse::Ok {
            tab_id: TabId::new(0),
            pane_id: PaneId::Terminal(TerminalId::new(0)),
        }
    );
    send(ScreenInstruction::SuppressPane(PANE, CLIENT));
    assert!(pane_info().is_suppressed);

    let (tx, rx) = std::sync::mpsc::channel();
    send(ScreenInstruction::AgentControlReply(
        visibility(false, None),
        tx,
    ));
    let response = rx
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap();
    assert_eq!(response.outcome, AgentOutcome::Ok, "{response:?}");
    assert!(!response.agents[0].is_suppressed);
    assert_eq!(pane_info(), before);
    assert_eq!(focused_pane(), focus);
    mock.teardown(vec![screen_thread, server_thread]);
}

#[test]
fn agent_overlay_hide_show_preserves_ownership_placement_and_client_focus() {
    let placement = OverlayPlacement::dismissible(Some(FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(2)),
        y: Some(PercentOrFixed::Fixed(3)),
        width: Some(PercentOrFixed::Fixed(40)),
        height: Some(PercentOrFixed::Fixed(12)),
        ..Default::default()
    }));
    let (mut screen, messages, _writes) = setup_with_placement(
        "codex",
        PaneTarget::Local,
        Size { cols: 80, rows: 24 },
        NewPanePlacement::Overlay(placement.clone()),
    );
    report(&mut screen, hook("SessionStart"));
    let other = ClientId::new(2);
    screen.add_client(other).unwrap();
    let tab = screen.tab_containing_pane_mut(PANE).unwrap();
    let shell = PaneId::Terminal(TerminalId::new(1));
    for client in [CLIENT, other] {
        tab.focus_pane_with_id(shell, false, false, client).unwrap();
    }
    let geom = tab.get_pane_with_id(PANE).unwrap().position_and_size();
    let active_tabs = screen.active_tab_ids.clone();
    for suppressed in [true, true, false, false, true, false] {
        screen.agent_control(visibility(suppressed, Some("conversation")), CLIENT);
        let response = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(response.outcome, AgentOutcome::Ok, "{response:?}");
        assert_eq!(response.agents[0].is_suppressed, suppressed);
        let tab = screen.tab_containing_pane(PANE).unwrap();
        assert!(tab.pane_is_overlay(PANE));
        assert_eq!(tab.overlays_are_rendered(), !suppressed);
        assert_eq!(tab.overlay_placement_for(PANE), Some(placement.clone()));
        assert_eq!(
            tab.get_pane_with_id(PANE).unwrap().position_and_size(),
            geom
        );
        assert_eq!(screen.active_tab_ids, active_tabs);
        for client in [CLIENT, other] {
            assert_eq!(tab.get_active_pane_id(client), Some(shell));
        }
    }
}

#[test]
fn agent_show_rejects_hidden_modals_without_changing_client_focus_or_tabs() {
    for separate_tabs in [false, true] {
        let (mut screen, messages, _writes) = setup_with_placement(
            "codex",
            PaneTarget::Local,
            Size { cols: 80, rows: 24 },
            NewPanePlacement::Overlay(OverlayPlacement::modal(Some(
                FloatingPaneCoordinates::default(),
            ))),
        );
        report(&mut screen, hook("SessionStart"));
        let other = ClientId::new(2);
        screen.add_client(other).unwrap();
        if separate_tabs {
            new_tab(&mut screen, 9, TabId::new(1));
        }
        for hidden in [false, true] {
            if hidden {
                screen.agent_control(visibility(true, Some("conversation")), CLIENT);
                assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
            }
            let before = screen.get_pane_info(PANE).unwrap();
            assert_eq!(before.is_suppressed, hidden);
            let placement = screen
                .tab_containing_pane(PANE)
                .unwrap()
                .overlay_placement_for(PANE);
            let active_tabs = screen.active_tab_ids.clone();
            let focus = [CLIENT, other].map(|client| screen.active_pane_id(client));
            assert_eq!(
                focus,
                [
                    Some(if separate_tabs {
                        PaneId::Terminal(TerminalId::new(9))
                    } else if hidden {
                        PaneId::Terminal(TerminalId::new(1))
                    } else {
                        PANE
                    }),
                    Some(if hidden {
                        PaneId::Terminal(TerminalId::new(1))
                    } else {
                        PANE
                    }),
                ]
            );

            screen.agent_control(visibility(false, Some("conversation")), CLIENT);
            let response = replies(&messages, CLIENT).pop().unwrap();
            assert_eq!(
                response.outcome,
                if hidden {
                    AgentOutcome::Rejected
                } else {
                    AgentOutcome::Ok
                },
                "{response:?}"
            );
            assert_eq!(response.agents[0].is_suppressed, hidden);
            if hidden {
                assert_eq!(
                    response.message.as_deref(),
                    Some("cannot show a hidden modal overlay without changing focus; use overlay navigation to show it")
                );
            }
            for settle in [false, true] {
                if settle {
                    screen.settle_tabs().unwrap();
                }
                assert_eq!(screen.get_pane_info(PANE).unwrap(), before);
                let tab = screen.tab_containing_pane(PANE).unwrap();
                assert_eq!(tab.overlay_placement_for(PANE), placement);
                assert_eq!(tab.overlays_are_rendered(), !hidden);
                assert_eq!(screen.active_tab_ids, active_tabs);
                assert_eq!(
                    [CLIENT, other].map(|client| screen.active_pane_id(client)),
                    focus
                );
            }
        }
    }
}

#[test]
fn agent_hide_retained_multipane_requires_a_destination_outside_its_container() {
    use yinz_types::multipane::{MultipaneDirection, MultipaneSplit};

    for destination in ["absent", "tiled", "floating", "tab"] {
        let (mut screen, messages, _writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        let children = [PaneId::Terminal(TerminalId::new(1)), PANE];
        let placement = OverlayPlacement::dismissible(Some(FloatingPaneCoordinates::default()));
        let tab = screen.tab_containing_pane_mut(PANE).unwrap();
        tab.create_multipane(
            children,
            MultipaneSplit {
                direction: MultipaneDirection::LeftRight,
                ratio: 5000,
            },
            0,
            placement.clone(),
        )
        .unwrap();
        assert_eq!(tab.get_selectable_tiled_panes_count(), 0);
        assert!(tab.has_selectable_panes());
        let container = tab.multipane_info(PANE).unwrap();
        let survivor = PaneId::Terminal(TerminalId::new(9));
        let destination_tab = if destination == "tab" {
            new_tab(&mut screen, 9, TabId::new(1));
            TabId::new(1)
        } else {
            if destination != "absent" {
                tab.new_pane(
                    survivor,
                    None,
                    None,
                    false,
                    false,
                    if destination == "floating" {
                        NewPanePlacement::Floating(None)
                    } else {
                        NewPanePlacement::default()
                    },
                    Some(CLIENT),
                    None,
                )
                .unwrap();
            }
            tab.focus_pane_with_id(PANE, false, false, CLIENT).unwrap();
            TabId::new(0)
        };
        let (to_pty, pty) = channels::unbounded();
        screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));
        screen.tabs.get_mut(&TabId::new(0)).unwrap().senders.to_pty =
            screen.bus.senders.to_pty.clone();
        let hidden = destination != "absent";
        for _ in 0..2 {
            screen.agent_control(visibility(true, Some("conversation")), CLIENT);
            let response = replies(&messages, CLIENT).pop().unwrap();
            assert_eq!(
                response.outcome,
                if hidden {
                    AgentOutcome::Ok
                } else {
                    AgentOutcome::Rejected
                },
                "{destination}: {response:?}"
            );
            assert_eq!(response.agents[0].is_suppressed, hidden);
            if !hidden {
                assert_eq!(
                    response.message.as_deref(),
                    Some("cannot hide the session's last selectable pane")
                );
            }
            screen.settle_tabs().unwrap();
            let tab = screen.tab_containing_pane(PANE).unwrap();
            assert_eq!(tab.id, TabId::new(0));
            assert_eq!(tab.multipane_info(PANE), Some(container.clone()));
            for child in children {
                let info = tab.get_pane_info(child).unwrap();
                assert_eq!(info.is_suppressed, hidden, "{destination}: {child}");
                assert!(!info.exited);
                assert_eq!(tab.overlay_placement_for(child), Some(placement.clone()));
            }
            assert_eq!(screen.active_tab_ids[&CLIENT], destination_tab);
            assert_eq!(
                screen.active_pane_id(CLIENT),
                Some(if hidden { survivor } else { PANE })
            );
            assert_eq!(screen.tabs.len(), if destination == "tab" { 2 } else { 1 });
            assert!(screen.replacement_tab.is_none() && !screen.replacement_pending);
            assert!(pty.try_recv().is_err());
        }
    }
}

#[test]
fn agent_hide_checks_the_stack_sibling_that_suppression_will_promote() {
    let previous = PaneId::Terminal(TerminalId::new(1));
    let next = PaneId::Terminal(TerminalId::new(8));
    for has_next in [false, true] {
        for selectable in [false, true] {
            let (mut screen, messages, _writes) = setup_with_placement(
                "codex",
                PaneTarget::Local,
                Size { cols: 80, rows: 24 },
                NewPanePlacement::Stacked {
                    pane_id_to_stack_under: Some(previous),
                    borderless: None,
                },
            );
            report(&mut screen, hook("SessionStart"));
            *screen.stacked_pane_list.borrow_mut() = true;
            let tab = screen.tab_containing_pane_mut(PANE).unwrap();
            tab.sync_stacked_pane_list_mode();
            if has_next {
                tab.new_pane(
                    next,
                    None,
                    None,
                    false,
                    true,
                    NewPanePlacement::Stacked {
                        pane_id_to_stack_under: Some(PANE),
                        borderless: None,
                    },
                    Some(CLIENT),
                    None,
                )
                .unwrap();
            }
            tab.focus_pane_with_id(PANE, false, false, CLIENT).unwrap();
            let promoted = if has_next { next } else { previous };
            tab.get_pane_with_id_mut(promoted)
                .unwrap()
                .set_selectable(selectable);
            assert!(tab.pane_is_hidden_stack_list_member(&promoted));
            assert_eq!(tab.get_selectable_tiled_panes_count(), 1);
            let (to_pty, pty) = channels::unbounded();
            tab.senders.to_pty = Some(SenderWithContext::new(to_pty));
            screen.bus.senders.to_pty = tab.senders.to_pty.clone();

            for _ in 0..2 {
                screen.agent_control(visibility(true, Some("conversation")), CLIENT);
                let response = replies(&messages, CLIENT).pop().unwrap();
                assert_eq!(
                    response.outcome,
                    if selectable {
                        AgentOutcome::Ok
                    } else {
                        AgentOutcome::Rejected
                    },
                    "next {has_next}, selectable {selectable}: {response:?}"
                );
                screen.settle_tabs().unwrap();
                let tab = screen.tab_containing_pane(PANE).unwrap();
                assert_eq!(tab.get_pane_info(PANE).unwrap().is_suppressed, selectable);
                assert_eq!(
                    tab.get_pane_info(promoted).unwrap().is_suppressed,
                    !selectable
                );
                assert_eq!(
                    tab.get_active_pane_id(CLIENT),
                    Some(if selectable { promoted } else { PANE })
                );
                assert_eq!(tab.get_all_pane_ids().len(), if has_next { 3 } else { 2 });
                assert_eq!(screen.tabs.len(), 1);
                assert_eq!(screen.active_tab_ids[&CLIENT], tab.id);
                assert!(screen.replacement_tab.is_none() && !screen.replacement_pending);
                assert!(pty.try_recv().is_err());
            }
        }
    }
}

#[test]
fn agent_hide_already_suppressed_without_a_selectable_destination_is_idempotent() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    let tab = screen.tab_containing_pane_mut(PANE).unwrap();
    tab.close_pane(PaneId::Terminal(TerminalId::new(1)), false, None);
    tab.suppress_pane(PANE, None);
    assert!(!tab.has_selectable_panes());
    for (conversation, outcome) in [
        ("conversation", AgentOutcome::Ok),
        ("other", AgentOutcome::TargetChanged),
        ("conversation", AgentOutcome::Ok),
    ] {
        screen.agent_control(visibility(true, Some(conversation)), CLIENT);
        assert_eq!(replies(&messages, CLIENT)[0].outcome, outcome);
        assert!(screen.get_pane_info(PANE).unwrap().is_suppressed);
    }
}

#[test]
fn agent_show_hide_preserve_focus_and_validate_live_and_held_conversations() {
    for exited in [false, true] {
        for clear_record in [false, true] {
            let (mut screen, messages, _writes) = setup("codex");
            report(&mut screen, hook("SessionStart"));
            if exited {
                if clear_record {
                    report(&mut screen, hook("SessionEnd"));
                }
                let tab = screen.tab_containing_pane_mut(PANE).unwrap();
                tab.hold_pane(PANE, Some(0), false, tab.pane_run_command(PANE).unwrap());
            }
            new_tab(&mut screen, 9, TabId::new(1));
            let active_tabs = screen.active_tab_ids.clone();
            for suppressed in [true, true, false, false, true] {
                let before = screen.get_pane_info(PANE).unwrap().is_suppressed;
                screen.agent_control(visibility(suppressed, Some("other")), CLIENT);
                assert_eq!(
                    replies(&messages, CLIENT)[0].outcome,
                    AgentOutcome::TargetChanged
                );
                assert_eq!(screen.get_pane_info(PANE).unwrap().is_suppressed, before);
                let focus = screen.tabs[&TabId::new(1)].get_active_pane_id(CLIENT);
                screen.agent_control(visibility(suppressed, Some("conversation")), CLIENT);
                let response = replies(&messages, CLIENT).pop().unwrap();
                assert_eq!(response.outcome, AgentOutcome::Ok, "{response:?}");
                let info = screen.get_pane_info(PANE).unwrap();
                assert_eq!(info.is_suppressed, suppressed);
                assert!(!info.is_floating);
                assert_eq!(screen.tab_id_containing_pane(PANE), Some(TabId::new(0)));
                assert_eq!(screen.active_tab_ids, active_tabs);
                assert_eq!(
                    screen.tabs[&TabId::new(1)].get_active_pane_id(CLIENT),
                    focus
                );
            }
        }
    }
}

#[test]
fn agent_show_rejects_suppressed_pane_tabs_without_changing_ownership() {
    use yinz_types::pane_tabs::{PaneTabDescriptor, PaneTabsLayout};

    for hide_group in [false, true] {
        let (mut screen, messages, _writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        let tab = screen.tab_containing_pane_mut(PANE).unwrap();
        let (sender, _plugins) = channels::unbounded();
        tab.senders
            .replace_to_plugin(SenderWithContext::new(sender));
        let info = tab
            .mount_pane_tabs(
                PANE,
                PaneTabsLayout {
                    active: 0,
                    tabs: ["Conversation", "Overview"]
                        .map(|label| PaneTabDescriptor {
                            label: label.into(),
                            location: "yinz:fixture-plugin-for-tests".into(),
                            configuration: BTreeMap::new(),
                            initial_cwd: None,
                        })
                        .to_vec(),
                },
                0,
                CLIENT,
            )
            .unwrap();
        tab.select_pane_tab(info.id, 1, CLIENT).unwrap();
        let overview = PluginId::new(100);
        let run = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
            "yinz:fixture-plugin-for-tests",
            &None,
            None,
            None,
        )
        .unwrap();
        tab.pane_tab_loaded(info.id, 1, 1, Ok((overview, run)))
            .unwrap();
        if hide_group {
            tab.select_pane_tab(info.id, 0, CLIENT).unwrap();
            tab.suppress_pane(PANE, None);
        }
        assert!(tab.get_pane_info(PANE).unwrap().is_suppressed);
        let before = tab.pane_tabs_info(info.id).unwrap();
        let focus = tab.get_active_pane_id(CLIENT);
        let active_tabs = screen.active_tab_ids.clone();

        for (conversation, outcome) in [
            ("other", AgentOutcome::TargetChanged),
            ("conversation", AgentOutcome::Rejected),
        ] {
            screen.agent_control(visibility(false, Some(conversation)), CLIENT);
            let response = replies(&messages, CLIENT).pop().unwrap();
            assert_eq!(response.outcome, outcome, "{response:?}");
            assert!(response.agents[0].is_suppressed);
            if outcome == AgentOutcome::Rejected {
                assert!(response.message.unwrap().contains("pane-tab"));
            }
            let tab = screen.tab_containing_pane(PANE).unwrap();
            assert_eq!(tab.pane_tabs_info(info.id).unwrap(), before);
            assert_eq!(tab.pane_tabs_of(PANE), Some(info.id));
            assert_eq!(tab.pane_tabs_of(PaneId::Plugin(overview)), Some(info.id));
            assert_eq!(tab.get_active_pane_id(CLIENT), focus);
            assert_eq!(screen.active_tab_ids, active_tabs);
        }

        let tab = screen.tab_containing_pane_mut(PANE).unwrap();
        tab.select_pane_tab(info.id, 0, CLIENT).unwrap();
        assert_eq!(tab.pane_tabs_info(info.id).unwrap().pane_id, PANE);
        if !hide_group {
            assert!(!tab.get_pane_info(PANE).unwrap().is_suppressed);
            for _ in 0..2 {
                screen.agent_control(visibility(false, None), CLIENT);
                assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
            }
        }
    }
}

#[test]
fn agent_show_rejects_hidden_stack_members_without_changing_client_focus() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    *screen.stacked_pane_list.borrow_mut() = true;
    let visible = PaneId::Terminal(TerminalId::new(8));
    screen
        .tab_containing_pane_mut(PANE)
        .unwrap()
        .new_pane(
            visible,
            None,
            None,
            false,
            true,
            NewPanePlacement::Stacked {
                pane_id_to_stack_under: Some(PANE),
                borderless: None,
            },
            Some(CLIENT),
            None,
        )
        .unwrap();
    let other = ClientId::new(2);
    screen.add_client(other).unwrap();
    let active_tabs = screen.active_tab_ids.clone();
    let tab = screen.tab_containing_pane_mut(PANE).unwrap();
    for client in [CLIENT, other] {
        tab.focus_pane_with_id(visible, false, false, client)
            .unwrap();
    }
    assert!(tab.pane_is_hidden_stack_list_member(&PANE));
    assert!(tab.get_pane_info(PANE).unwrap().is_suppressed);

    screen.agent_control(visibility(false, Some("conversation")), CLIENT);
    let response = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(response.outcome, AgentOutcome::Rejected, "{response:?}");
    assert!(response.message.unwrap().contains("stack"));
    assert!(response.agents[0].is_suppressed);
    assert_eq!(screen.active_tab_ids, active_tabs);
    let tab = screen.tab_containing_pane_mut(PANE).unwrap();
    assert!(tab.pane_is_hidden_stack_list_member(&PANE));
    assert!(tab.pane_is_stack_list_member(&visible));
    assert!(!tab.get_pane_info(visible).unwrap().is_suppressed);
    for client in [CLIENT, other] {
        assert_eq!(tab.get_active_pane_id(client), Some(visible));
    }
    assert!(tab.focus_hidden_stack_list_member(PANE, CLIENT));
    for _ in 0..2 {
        screen.agent_control(visibility(false, None), CLIENT);
        assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
    }
}

#[test]
fn agent_visibility_rejects_missing_and_non_agent_targets() {
    for missing in [false, true] {
        let (mut screen, messages, _writes) = setup("bash");
        if missing {
            screen.close_pane_explicitly(PANE, None);
        }
        for suppressed in [false, true] {
            for pin in [None, Some("conversation")] {
                screen.agent_control(visibility(suppressed, pin), CLIENT);
                assert_eq!(
                    replies(&messages, CLIENT)[0].outcome,
                    match (missing, pin.is_some()) {
                        (true, false) => AgentOutcome::TargetNotFound,
                        (true, true) => AgentOutcome::TargetClosed,
                        (false, false) => AgentOutcome::NotAgent,
                        (false, true) => AgentOutcome::TargetChanged,
                    }
                );
            }
        }
    }
}

#[test]
fn agent_show_without_room_keeps_the_running_pane_suppressed() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    screen.agent_control(visibility(true, None), CLIENT);
    replies(&messages, CLIENT);
    screen.resize_to_screen(Size { cols: 8, rows: 3 }).unwrap();
    screen.agent_control(visibility(false, None), CLIENT);
    let response = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(response.outcome, AgentOutcome::Rejected, "{response:?}");
    assert!(response.message.unwrap().contains("no room"));
    assert!(response.agents[0].is_suppressed);
    assert!(response.agents[0].ready);
    assert!(!screen.get_pane_info(PANE).unwrap().exited);
    screen
        .resize_to_screen(Size { cols: 80, rows: 24 })
        .unwrap();
    screen.agent_control(visibility(false, None), CLIENT);
    assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
    assert!(!screen.get_pane_info(PANE).unwrap().is_suppressed);
}

#[test]
fn suppressed_agents_support_status_list_prompts_reads_keys_recovery_and_restart() {
    for target in pane_targets() {
        let (mut screen, messages, writes) = setup_on_target("codex", target);
        screen.agent_control(visibility(true, None), CLIENT);
        assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
        report(&mut screen, hook("SessionStart"));
        for request in [
            AgentRequest::List { project: None },
            AgentRequest::Status {
                pane_id: PANE,
                expected_session_id: Some("conversation".into()),
            },
        ] {
            screen.agent_control(request, CLIENT);
            let response = replies(&messages, CLIENT).pop().unwrap();
            assert_eq!(response.outcome, AgentOutcome::Ok);
            assert_eq!(response.agents.len(), 1);
            assert!(response.agents[0].is_suppressed && response.agents[0].ready);
        }
        screen.agent_control_with_clear(prompt(false), CLIENT, &writes);
        assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
        assert!(replies(&messages, CLIENT).is_empty());
        report(&mut screen, hook("UserPromptSubmit"));
        let submitted = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(submitted.outcome, AgentOutcome::Submitted);
        assert!(submitted.acknowledged);
        screen.agent_control(
            AgentRequest::WaitTurn {
                pane_id: PANE,
                expected_session_id: Some("conversation".into()),
                submission_id: submitted.submission_id.unwrap(),
                timeout_ms: 30_000,
            },
            CLIENT,
        );
        report(&mut screen, hook("Stop"));
        assert_eq!(
            replies(&messages, CLIENT)[0].outcome,
            AgentOutcome::Completed
        );
        screen
            .tab_containing_pane_mut(PANE)
            .unwrap()
            .handle_pty_bytes(TerminalId::new(7), b"hidden result".to_vec())
            .unwrap();
        screen.agent_control(
            AgentRequest::Read {
                pane_id: PANE,
                expected_session_id: None,
                lines: 80,
            },
            CLIENT,
        );
        assert_eq!(
            replies(&messages, CLIENT)[0].output.as_deref(),
            Some("hidden result")
        );
        screen.agent_control(send_keys(vec![BareKey::Esc]), CLIENT);
        assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
        assert!(finish_paste(&writes).ends_with(b"\x1b"));
        screen.agent_control(recover(), CLIENT);
        assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
        report(&mut screen, hook("SessionEnd"));
        let tab = screen.tab_containing_pane_mut(PANE).unwrap();
        tab.hold_pane(PANE, Some(0), false, tab.pane_run_command(PANE).unwrap());
        screen.agent_control(
            AgentRequest::Restart {
                pane_id: PANE,
                prompt: None,
                session: yinz_types::agent::control::AgentRestartSession::Fresh,
            },
            CLIENT,
        );
        let response = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(response.outcome, AgentOutcome::Ok, "{response:?}");
        assert_eq!(response.restarted.unwrap().pane_id, PANE);
        assert!(screen.get_pane_info(PANE).unwrap().is_suppressed);
    }
}

#[test]
fn live_hook_recency_is_shared_repeated_and_excludes_replay_and_display_updates() {
    let (mut screen, _, _) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    let first = screen.get_pane_info(PANE).unwrap().last_hook_at_ms.unwrap();
    for name in [
        "UserPromptSubmit",
        "UserPromptSubmit",
        "PreCompact",
        "SubagentStart",
    ] {
        // A sentinel avoids relying on clock resolution or sleeping between identical hooks.
        screen.with_pane(PANE, |pane| pane.set_last_hook_at_ms(1));
        report(&mut screen, hook(name));
        let info = screen.get_pane_info(PANE).unwrap();
        let at = info.last_hook_at_ms.unwrap();
        assert!(at >= first);
        let cached = screen.peer_sessions_cache[&screen.session_name]
            .panes
            .panes
            .values()
            .flatten()
            .find(|pane| pane.id == PANE)
            .unwrap();
        assert_eq!(cached.last_hook_at_ms, Some(at));
    }
    let at = screen.get_pane_info(PANE).unwrap().last_hook_at_ms;
    report(
        &mut screen,
        HookEvent {
            archive_only: true,
            ..hook("PreCompact")
        },
    );
    screen.with_pane(PANE, |pane| {
        pane.update_name("changed title");
    });
    screen
        .tab_containing_pane_mut(PANE)
        .unwrap()
        .handle_pty_bytes(TerminalId::new(7), b"ordinary output".to_vec())
        .unwrap();
    screen.render().unwrap();
    assert_eq!(screen.get_pane_info(PANE).unwrap().last_hook_at_ms, at);
    let mut replacement = screen.get_pane_info(PANE).unwrap().agent.unwrap();
    replacement.session_id = Some("replacement".into());
    crate::screen::apply_pane_agent_report(&mut screen, PANE, Some(replacement)).unwrap();
    assert_eq!(screen.get_pane_info(PANE).unwrap().last_hook_at_ms, None);
    report(&mut screen, hook("PreCompact"));
    assert!(screen
        .get_pane_info(PANE)
        .unwrap()
        .last_hook_at_ms
        .is_some());
    crate::screen::apply_pane_agent_report(&mut screen, PANE, None).unwrap();
    assert_eq!(screen.get_pane_info(PANE).unwrap().last_hook_at_ms, None);
}

#[test]
fn hook_flash_restarts_for_identical_and_stateless_reports_and_expires_at_150_ms() {
    let (mut screen, _, _) = setup("codex");
    let start = Instant::now();
    screen.pinned_now = Some(start);
    report(&mut screen, hook("SessionStart"));
    let pane = screen.get_pane_info(PANE).unwrap();
    assert!(screen.pane_title_hook_is_flashing(&pane, start));
    assert!(!screen.agent_hook_is_flashing(&pane, start));
    for (offset, name) in [
        (0, "UserPromptSubmit"),
        (100, "UserPromptSubmit"),
        (200, "PreCompact"),
    ] {
        let now = start + Duration::from_millis(offset);
        screen.pinned_now = Some(now);
        report(&mut screen, hook(name));
        let pane = screen.get_pane_info(PANE).unwrap();
        assert!(screen.agent_hook_is_flashing(&pane, now));
        assert!(screen.agent_hook_is_flashing(&pane, now + Duration::from_millis(149)));
        assert!(!screen.agent_hook_is_flashing(&pane, now + Duration::from_millis(150)));
    }
    screen.reap_agent_hook_flashes(start + Duration::from_millis(350));
    assert!(screen.agent_hook_flashes.is_empty());
}

#[test]
fn hook_flash_includes_all_title_states_but_excludes_replay_ignored_and_mismatched_reports() {
    for state in [AgentState::Idle, AgentState::Waiting, AgentState::Exited] {
        let (mut screen, _, _) = setup("codex");
        report(&mut screen, hook("UserPromptSubmit"));
        let mut agent = screen.get_pane_info(PANE).unwrap().agent.unwrap();
        agent.state = state;
        report_state(
            &mut screen,
            "codex",
            hook("PreCompact"),
            AgentReportState::Set(agent),
        );
        let now = screen.frame_now();
        let pane = screen.get_pane_info(PANE).unwrap();
        assert!(screen.pane_title_hook_is_flashing(&pane, now), "{state:?}");
        assert!(!screen.agent_hook_is_flashing(&pane, now), "{state:?}");
    }
    let (mut screen, _, _) = setup("agy");
    let start = Instant::now();
    screen.pinned_now = Some(start);
    report_kind(&mut screen, "agy", hook("UserPromptSubmit"));
    screen.pinned_now = Some(start + Duration::from_millis(100));
    for event in [
        HookEvent {
            archive_only: true,
            ..hook("PreCompact")
        },
        HookEvent {
            prompt_id: Some("stale".into()),
            ..hook("Stop")
        },
        HookEvent {
            session_id: "other".into(),
            ..hook("PreCompact")
        },
    ] {
        if event.hook_event_name == "Stop" {
            assert!(screen.ignore_agy_confirmation(PANE, &event));
        }
        report_state(&mut screen, "agy", event, AgentReportState::Unchanged);
        let pane = screen.get_pane_info(PANE).unwrap();
        assert!(screen.agent_hook_is_flashing(&pane, start + Duration::from_millis(149)));
        assert!(!screen.agent_hook_is_flashing(&pane, start + Duration::from_millis(150)));
    }
}

#[test]
fn hook_flash_is_cleared_on_replacement_clear_and_pane_removal() {
    for change in ["kind", "session", "run", "clear", "pane"] {
        let (mut screen, _, _) = setup("codex");
        screen.pinned_now = Some(Instant::now());
        report(&mut screen, hook("UserPromptSubmit"));
        assert!(!screen.agent_hook_flashes.is_empty());
        let mut agent = screen.get_pane_info(PANE).unwrap().agent.unwrap();
        match change {
            "kind" => agent.kind = "claude".into(),
            "session" => agent.session_id = Some("replacement".into()),
            "run" => agent.run_id = Some("replacement".into()),
            "pane" => screen.close_pane_explicitly(PANE, None),
            _ => {},
        }
        if change != "pane" {
            crate::screen::apply_pane_agent_report(
                &mut screen,
                PANE,
                (change != "clear").then_some(agent),
            )
            .unwrap();
        }
        assert!(screen.agent_hook_flashes.is_empty(), "{change}");
    }
}

#[test]
fn hook_flash_clearing_hook_flashes_only_remaining_title_and_expires() {
    let (mut screen, _, _) = setup("codex");
    screen
        .tab_containing_pane_mut(PANE)
        .unwrap()
        .set_pane_frames(PaneFrameStyle::Full);
    let start = Instant::now();
    screen.pinned_now = Some(start);
    report(&mut screen, hook("UserPromptSubmit"));
    report(&mut screen, hook("SessionEnd"));
    let pane = screen.get_pane_info(PANE).unwrap();
    assert!(pane.agent.is_none());
    assert!(screen.pane_title_hook_is_flashing(&pane, start));
    assert!(!screen.agent_hook_is_flashing(&pane, start));
    assert!(screen.pane_title_hook_flashes_are_visible(start));
    screen.render_to_clients().unwrap();
    let title = screen
        .tab_containing_pane(PANE)
        .unwrap()
        .get_pane_with_id(PANE)
        .unwrap()
        .agent_title();
    assert_eq!(title.state, None);
    assert!(title.hook_flash);
    let expired = start + Duration::from_millis(150);
    screen.pinned_now = Some(expired);
    screen.on_animation_tick(expired).unwrap();
    screen.render_to_clients().unwrap();
    assert!(screen.agent_hook_flashes.is_empty());
    assert!(
        !screen
            .tab_containing_pane(PANE)
            .unwrap()
            .get_pane_with_id(PANE)
            .unwrap()
            .agent_title()
            .hook_flash
    );
}

#[test]
fn hook_flash_title_clock_with_palette_closed_and_animations_disabled() {
    use yinz_types::animation::FRAME_PERIOD;
    for floating in [false, true] {
        for enabled in [false, true] {
            for focused in [false, true] {
                let placement = if floating {
                    NewPanePlacement::Floating(None)
                } else {
                    NewPanePlacement::default()
                };
                let size = Size { cols: 80, rows: 24 };
                let (mut screen, _, _) =
                    setup_with_placement("codex", PaneTarget::Local, size, placement);
                let tab = screen.tab_containing_pane_mut(PANE).unwrap();
                tab.set_pane_frames(PaneFrameStyle::Full);
                if !focused {
                    let placement = if floating {
                        NewPanePlacement::Floating(None)
                    } else {
                        NewPanePlacement::default()
                    };
                    tab.new_pane(
                        PaneId::Terminal(TerminalId::new(8)),
                        None,
                        None,
                        false,
                        true,
                        placement,
                        Some(CLIENT),
                        None,
                    )
                    .unwrap();
                }
                tab.settle_overlay_ripples();
                screen.set_pane_animations(enabled);
                let (sender, receiver) = channels::unbounded();
                screen.bus.senders.to_server = Some(SenderWithContext::new(sender));
                let mut grid = grid_for_render_snapshot(size);
                let start = Instant::now();
                screen.pinned_now = Some(start);
                report(&mut screen, hook("SessionStart"));
                assert!(screen.command_palettes.is_empty());
                render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
                assert_eq!(
                    screen.animation_clock_period(start),
                    enabled.then_some(FRAME_PERIOD),
                    "floating={floating}, focused={focused}, enabled={enabled}"
                );
                let colors = if focused {
                    screen.style.colors.text_selected
                } else {
                    screen.style.colors.text_unselected
                };
                let marker_color = |grid: &crate::panes::grid::Grid| {
                    grid.as_character_lines()
                        .into_iter()
                        .flatten()
                        .find(|cell| cell.character == '○')
                        .unwrap()
                        .styles
                        .foreground
                };
                assert_eq!(
                    marker_color(&grid),
                    Some(
                        if enabled {
                            colors.emphasis_0
                        } else {
                            colors.base
                        }
                        .into()
                    )
                );
                screen.pinned_now = Some(start + Duration::from_millis(150));
                screen.on_animation_tick(screen.frame_now()).unwrap();
                render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
                assert_eq!(screen.animation_clock_period(screen.frame_now()), None);
                assert_eq!(marker_color(&grid), Some(colors.base.into()));
            }
        }
    }
}

#[test]
fn hook_flash_pane_tab_repaints_when_terminal_is_hidden_behind_plugin_tab() {
    use yinz_types::pane_tabs::{PaneTabDescriptor, PaneTabsLayout};
    for plugin_selected in [false, true] {
        let size = Size { cols: 80, rows: 24 };
        let (mut screen, _, _) = setup("codex");
        let tab = screen.tab_containing_pane_mut(PANE).unwrap();
        tab.set_pane_frames(PaneFrameStyle::Full);
        let (sender, _plugins) = channels::unbounded();
        tab.senders
            .replace_to_plugin(SenderWithContext::new(sender));
        let info = tab
            .mount_pane_tabs(
                PANE,
                PaneTabsLayout {
                    active: 0,
                    tabs: ["Conversation", "Overview"]
                        .map(|label| PaneTabDescriptor {
                            label: label.into(),
                            location: "yinz:fixture-plugin-for-tests".into(),
                            configuration: BTreeMap::new(),
                            initial_cwd: None,
                        })
                        .to_vec(),
                },
                0,
                CLIENT,
            )
            .unwrap();
        if plugin_selected {
            tab.select_pane_tab(info.id, 1, CLIENT).unwrap();
            let run = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
                "yinz:fixture-plugin-for-tests",
                &None,
                None,
                None,
            )
            .unwrap();
            tab.pane_tab_loaded(info.id, 1, 1, Ok((PluginId::new(100), run)))
                .unwrap();
        }
        tab.settle_overlay_ripples();
        let (sender, receiver) = channels::unbounded();
        screen.bus.senders.to_server = Some(SenderWithContext::new(sender));
        let mut grid = grid_for_render_snapshot(size);
        let start = Instant::now();
        screen.pinned_now = Some(start);
        report(&mut screen, hook("SessionStart"));
        render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert!(screen.pane_title_hook_flashes_are_visible(start));
        let color = |grid: &crate::panes::grid::Grid| {
            grid.as_character_lines()
                .into_iter()
                .find(|row| {
                    row.iter()
                        .map(|ch| ch.character)
                        .collect::<String>()
                        .contains("Conversation")
                })
                .unwrap()
                .into_iter()
                .find(|ch| ch.character == '○')
                .unwrap()
                .styles
                .foreground
        };
        assert_eq!(
            color(&grid),
            Some(screen.style.colors.text_unselected.emphasis_0.into())
        );
        screen.pinned_now = Some(start + Duration::from_millis(150));
        screen.on_animation_tick(screen.frame_now()).unwrap();
        render_frame_into(&mut screen, &receiver, CLIENT, &mut grid);
        assert_eq!(screen.animation_clock_period(screen.frame_now()), None);
        assert_eq!(
            color(&grid),
            Some(screen.style.colors.text_unselected.base.into())
        );
        assert_eq!(
            screen
                .tab_containing_pane(PANE)
                .unwrap()
                .pane_tabs_info(info.id)
                .unwrap()
                .tabs[0]
                .descriptor
                .label,
            "Conversation"
        );
    }
}

#[test]
fn hook_flash_idle_hooks_restart_title_deadline_without_arming_palette() {
    let (mut screen, _, _) = setup("codex");
    let start = Instant::now();
    screen.pinned_now = Some(start);
    report(&mut screen, hook("SessionStart"));
    for offset in [100, 200] {
        let now = start + Duration::from_millis(offset);
        screen.pinned_now = Some(now);
        report_state(
            &mut screen,
            "codex",
            hook("PreCompact"),
            AgentReportState::Unchanged,
        );
        let pane = screen.get_pane_info(PANE).unwrap();
        assert_eq!(pane.agent.as_ref().unwrap().state, AgentState::Idle);
        assert!(screen.pane_title_hook_is_flashing(&pane, now + Duration::from_millis(149)));
        assert!(!screen.pane_title_hook_is_flashing(&pane, now + Duration::from_millis(150)));
        assert!(!screen.agent_hook_is_flashing(&pane, now));
    }
    let mut agent = screen.get_pane_info(PANE).unwrap().agent.unwrap();
    agent.state = AgentState::Busy;
    crate::screen::apply_pane_agent_report(&mut screen, PANE, Some(agent)).unwrap();
    let pane = screen.get_pane_info(PANE).unwrap();
    assert!(screen.pane_title_hook_is_flashing(&pane, screen.frame_now()));
    assert!(!screen.agent_hook_is_flashing(&pane, screen.frame_now()));
    report_state(
        &mut screen,
        "codex",
        hook("PreCompact"),
        AgentReportState::Unchanged,
    );
    assert!(screen.agent_hook_is_flashing(&screen.get_pane_info(PANE).unwrap(), screen.frame_now()));
    for state in [AgentState::Idle, AgentState::Busy] {
        let mut agent = screen.get_pane_info(PANE).unwrap().agent.unwrap();
        agent.state = state;
        crate::screen::apply_pane_agent_report(&mut screen, PANE, Some(agent)).unwrap();
        let pane = screen.get_pane_info(PANE).unwrap();
        assert!(screen.pane_title_hook_is_flashing(&pane, screen.frame_now()));
        assert!(!screen.agent_hook_is_flashing(&pane, screen.frame_now()));
    }
}

fn claude_worker(name: &str, id: &str) -> HookEvent {
    HookEvent {
        agent_id: Some(id.into()),
        tool_use_id: matches!(name, "PreToolUse" | "PostToolUse").then(|| format!("input-{id}")),
        ..hook(name)
    }
}

fn claude_worker_input(name: &str, task: &str, tool: Option<&str>) -> HookEvent {
    HookEvent {
        tool_name: Some("AskUserQuestion".into()),
        tool_use_id: tool.map(str::to_owned),
        ..claude_worker(name, task)
    }
}

#[test]
fn claude_worker_waits_release_only_the_matching_tool() {
    for (asking, release) in [
        ("PreToolUse", "PostToolUse"),
        ("Elicitation", "ElicitationResult"),
    ] {
        let (mut screen, messages, _) = setup("claude");
        let task = "worker";
        report_kind(&mut screen, "claude", hook("SessionStart"));
        report_kind(&mut screen, "claude", claude_worker("SubagentStart", task));
        report_kind(
            &mut screen,
            "claude",
            claude_worker_input(asking, task, Some("question-1")),
        );
        report_kind(
            &mut screen,
            "claude",
            claude_worker_input(release, task, Some("question-1")),
        );
        report_kind(
            &mut screen,
            "claude",
            claude_worker_input(asking, task, Some("question-2")),
        );
        for event in [
            claude_worker("SubagentStop", task),
            // A delayed older wait cannot replace the newer pending tool.
            claude_worker_input(asking, task, Some("question-1")),
            claude_worker_input(release, task, Some("question-1")),
            claude_worker_input(release, task, None),
            HookEvent {
                session_id: "foreign".into(),
                ..claude_worker_input(release, task, Some("question-2"))
            },
            HookEvent {
                archive_only: true,
                ..claude_worker_input(release, task, Some("question-2"))
            },
            // A release received before its wait must suppress that late wait.
            claude_worker_input(asking, task, Some("question-1")),
        ] {
            report_kind(&mut screen, "claude", event);
            assert_eq!(
                screen.agent_status_for_test().agent.state,
                AgentState::Waiting
            );
            assert!(screen.agent_worker_waits[&PANE].waits.contains_key(task));
        }
        report_kind(
            &mut screen,
            "claude",
            claude_worker_input(release, task, Some("question-2")),
        );
        assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
        // A reordered wait must not recreate Waiting.
        report_kind(
            &mut screen,
            "claude",
            claude_worker_input(asking, task, Some("question-2")),
        );
        assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
        screen.agent_control(wait(AgentState::Idle), CLIENT);
        assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
    }
}

#[test]
fn claude_late_wait_after_completion_can_release_but_missing_identity_stays_conservative() {
    for tool in [None, Some("late-question")] {
        let (mut screen, _, _) = setup("claude");
        report_kind(&mut screen, "claude", hook("SessionStart"));
        report_kind(
            &mut screen,
            "claude",
            claude_worker("SubagentStart", "worker"),
        );
        report_kind(
            &mut screen,
            "claude",
            claude_worker("SubagentStop", "worker"),
        );
        report_kind(
            &mut screen,
            "claude",
            claude_worker_input("PreToolUse", "worker", tool),
        );
        assert_eq!(
            screen.agent_status_for_test().agent.state,
            AgentState::Waiting
        );
        report_kind(
            &mut screen,
            "claude",
            claude_worker_input("PostToolUse", "worker", tool),
        );
        assert_eq!(
            screen.agent_status_for_test().agent.state,
            if tool.is_some() {
                AgentState::Idle
            } else {
                AgentState::Waiting
            }
        );
    }
}

#[test]
fn background_completion_preserves_draft_revision_and_independent_readiness() {
    let (mut screen, _, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    finish_paste(&writes);
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    report_kind(&mut screen, "claude", claude_stop("prompt", "done", true));
    screen.agent_write_for_test();
    assert_eq!(
        screen.agent_status_for_test().not_ready_reason,
        Some(NotReadyReason::UnconsumedInput),
        "draft input before render"
    );
    draft_frame(
        &mut screen,
        "claude",
        "unsubmitted draft",
        17,
        Instant::now(),
    );
    let revision = screen.tabs[&TabId::new(0)].agent_input_revision(PANE);
    assert_eq!(
        screen.agent_status_for_test().not_ready_reason,
        Some(NotReadyReason::UnconsumedInput),
        "draft input after render"
    );
    report_kind(&mut screen, "claude", background_idle("prompt"));
    let status = screen.agent_status_for_test();
    assert_eq!(status.agent.state, AgentState::Idle);
    assert!(!status.background);
    assert_eq!(
        status.not_ready_reason,
        Some(NotReadyReason::UnconsumedInput)
    );
    assert_eq!(
        screen.tabs[&TabId::new(0)].agent_input_revision(PANE),
        revision
    );
    assert!(screen.tabs[&TabId::new(0)]
        .get_pane_with_id(PANE)
        .unwrap()
        .dump_screen(false, Some(CLIENT))
        .contains("unsubmitted draft"));
}

#[test]
fn claude_worker_stop_archives_without_publishing_idle_or_clearing_waits() {
    let (mut screen, _, _) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    report_kind(
        &mut screen,
        "claude",
        claude_worker("SubagentStart", "worker"),
    );
    report_kind(
        &mut screen,
        "claude",
        HookEvent {
            tool_name: Some("AskUserQuestion".into()),
            ..claude_worker("PreToolUse", "worker")
        },
    );
    let (sender, plugins) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
    let mut stop = claude_worker("SubagentStop", "worker");
    stop.agent_type = Some("reviewer".into());
    stop.last_assistant_message = Some("worker report".into());
    report_kind(&mut screen, "claude", stop);
    assert_eq!(
        screen.agent_status_for_test().agent.state,
        AgentState::Waiting
    );
    assert!(screen.agent_worker_waits[&PANE]
        .waits
        .contains_key("worker"));
    let events: Vec<_> = plugins
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            PluginInstruction::Update(updates) => updates
                .into_iter()
                .map(|(_, _, event)| event)
                .collect::<Vec<_>>(),
            _ => vec![],
        })
        .collect();
    assert!(!events
        .iter()
        .any(|event| matches!(event, Event::PaneUpdate(_))));
    let archived = events
        .into_iter()
        .find_map(|event| match event {
            Event::AgentEvent(event) => {
                Some(serde_json::from_str::<HookEvent>(&event.raw).unwrap())
            },
            _ => None,
        })
        .unwrap();
    assert!(archived.archive_only);
    assert_eq!(
        archived.last_assistant_message.as_deref(),
        Some("worker report")
    );
    assert_eq!(archived.agent_type.as_deref(), Some("reviewer"));
}

fn steering_prompt(id: &str) -> AgentRequest {
    AgentRequest::Prompt {
        submission_id: Some(id.into()),
        prompt: "steer".into(),
        preserve_draft: false,
        pane_id: PANE,
        expected_session_id: Some("conversation".into()),
        wait: true,
        timeout_ms: 30_000,
    }
}

fn steering_ack(id: &str) -> HookEvent {
    HookEvent {
        prompt_id: Some(id.into()),
        prompt: Some("steer".into()),
        ..hook("UserPromptSubmit")
    }
}

fn deliver_steering(
    screen: &mut Screen,
    writes: &Receiver<(PtyWriteInstruction, ErrorContext)>,
    id: &str,
    client: ClientId,
) {
    draft_frame(screen, "claude", "", 0, Instant::now());
    screen.agent_control_with_clear(steering_prompt(id), client, writes);
    assert_eq!(
        finish_paste(writes),
        b"\x1b[200~steer\x1b[201~",
        "steering preparation must deliver the requested text"
    );
    let now = Instant::now();
    screen.refresh_agent_controls_at(now);
    screen.refresh_agent_controls_at(now + Duration::from_millis(600));
    assert_eq!(finish_paste(writes), b"\r");
}

#[test]
fn claude_background_and_notifications_publish_aggregate_activity_without_idle_edges() {
    for target in pane_targets() {
        let (mut screen, messages, writes) = setup_on_target("claude", target.clone());
        report_kind(&mut screen, "claude", hook("SessionStart"));
        screen.agent_control_with_clear(identified_prompt("parent", true), CLIENT, &writes);
        finish_paste(&writes);
        report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
        for id in ["one", "two"] {
            report_kind(&mut screen, "claude", claude_worker("SubagentStart", id));
        }
        let (sender, plugins) = channels::unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
        screen.background_plugin_subscriptions.insert(
            (PluginId::new(99), CLIENT),
            HashSet::from([EventType::PaneUpdate]),
        );
        report_kind(&mut screen, "claude", claude_stop("prompt", "parent", true));
        assert_eq!(
            replies(&messages, CLIENT)[0].agents[0].agent.state,
            AgentState::Busy
        );
        for id in ["one", "two"] {
            report_kind(&mut screen, "claude", claude_worker("SubagentStop", id));
            assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Busy);
        }
        report_kind(
            &mut screen,
            "claude",
            task_notification("notification", &["one"]),
        );
        // The launching turn's watch reports idle only after the notification turn.
        report_kind(&mut screen, "claude", background_idle("prompt"));
        assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Busy);
        assert!(plugins.try_iter().all(|(instruction, _)| {
            let PluginInstruction::Update(updates) = instruction else { return true };
            updates.into_iter().all(|(_, _, event)| {
                !matches!(event, Event::PaneUpdate(manifest) if manifest.panes.values().flatten()
                    .any(|pane| pane.id == PANE && pane.agent.as_ref().is_some_and(|a| a.state == AgentState::Idle)))
            })
        }), "parent completion must never publish an idle pane while work remains");
        screen.agent_control(wait(AgentState::Idle), CLIENT);
        assert!(replies(&messages, CLIENT).is_empty());
        report_kind(
            &mut screen,
            "claude",
            claude_stop("notification", "finished", false),
        );
        assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
        assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
        // An uncorrelated question may belong to a resumption not yet persisted.
        // Keep that input wait conservative until the actual tool response arrives.
        report_kind(
            &mut screen,
            "claude",
            HookEvent {
                tool_name: Some("AskUserQuestion".into()),
                ..claude_worker("PreToolUse", "one")
            },
        );
        assert_eq!(
            screen.agent_status_for_test().agent.state,
            AgentState::Waiting
        );
    }
}

#[test]
fn claude_worker_waits_and_foreign_events_preserve_the_underlying_parent() {
    for parent_done in [false, true] {
        let (mut screen, _, _) = setup("claude");
        report_kind(&mut screen, "claude", hook("SessionStart"));
        report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
        report_kind(
            &mut screen,
            "claude",
            claude_worker("SubagentStart", "worker"),
        );
        if parent_done {
            // The worker outlives the turn, so the registry still shows it working.
            report_kind(&mut screen, "claude", claude_stop("prompt", "done", true));
        }
        report_kind(
            &mut screen,
            "claude",
            HookEvent {
                tool_name: Some("AskUserQuestion".into()),
                ..claude_worker("PreToolUse", "worker")
            },
        );
        assert_eq!(
            screen.agent_status_for_test().agent.state,
            AgentState::Waiting
        );
        assert!(!screen.agent_status_for_test().ready);
        report_kind(
            &mut screen,
            "claude",
            HookEvent {
                session_id: "foreign".into(),
                ..claude_worker("SubagentStop", "worker")
            },
        );
        assert_eq!(
            screen.agent_status_for_test().agent.state,
            AgentState::Waiting
        );
        report_kind(
            &mut screen,
            "claude",
            claude_worker("PostToolUse", "worker"),
        );
        assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Busy);
        report_kind(
            &mut screen,
            "claude",
            claude_worker("SubagentStop", "worker"),
        );
        assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Busy);
        if parent_done {
            report_kind(&mut screen, "claude", background_idle("prompt"));
            assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
        }
        report_kind(
            &mut screen,
            "claude",
            claude_worker("SubagentStart", "next"),
        );
        report_kind(
            &mut screen,
            "claude",
            HookEvent {
                session_id: "replacement".into(),
                ..hook("SessionStart")
            },
        );
        report_kind(
            &mut screen,
            "claude",
            claude_worker("SubagentStart", "stale"),
        );
        assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
    }
}

#[test]
fn claude_sequential_steering_keeps_each_completion_and_ignores_old_acknowledgements() {
    for target in pane_targets() {
        for managed in [false, true] {
            let (mut screen, messages, writes) = setup_on_target("claude", target.clone());
            report_kind(&mut screen, "claude", hook("SessionStart"));
            if managed {
                screen.agent_control_with_clear(identified_prompt("parent", true), CLIENT, &writes);
                finish_paste(&writes);
            }
            report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
            deliver_steering(&mut screen, &writes, "one", ClientId::new(2));
            screen.agent_control(steering_prompt("blocked"), ClientId::new(3));
            assert_eq!(
                replies(&messages, ClientId::new(3))[0].outcome,
                AgentOutcome::Rejected
            );
            report_kind(&mut screen, "claude", steering_ack("one"));
            assert!(
                replies(&messages, CLIENT).is_empty(),
                "steering must preserve the parent's waiter"
            );
            deliver_steering(&mut screen, &writes, "two", ClientId::new(3));
            report_kind(&mut screen, "claude", steering_ack("one"));
            assert_eq!(
                screen.agent_status_for_test().not_ready_reason,
                Some(NotReadyReason::PendingSubmission)
            );
            report_kind(
                &mut screen,
                "claude",
                claude_stop("one", "first steering", false),
            );
            let first = replies(&messages, ClientId::new(2));
            assert_eq!(first[0].submission_id.as_deref(), Some("one"));
            assert_eq!(first[0].outcome, AgentOutcome::Completed);
            assert!(
                !first[0].agents[0].ready,
                "the next submission still owns the composer"
            );
            assert!(replies(&messages, ClientId::new(3)).is_empty());
            report_kind(&mut screen, "claude", steering_ack("two"));
            // Latest confirmation can beat the first parent confirmation.
            report_kind(
                &mut screen,
                "claude",
                claude_stop("two", "latest answer", false),
            );
            let second = replies(&messages, ClientId::new(3));
            assert_eq!(second[0].submission_id.as_deref(), Some("two"));
            assert_eq!(second[0].message.as_deref(), Some("latest answer"));
            report_kind(
                &mut screen,
                "claude",
                claude_stop("prompt", "parent answer", false),
            );
            let parent = replies(&messages, CLIENT);
            if managed {
                assert_eq!(parent.len(), 1);
                assert_eq!(parent[0].outcome, AgentOutcome::Completed);
                assert_eq!(parent[0].submission_id.as_deref(), Some("parent"));
            } else {
                assert!(parent.is_empty());
            }
            assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
            screen.agent_control(wait(AgentState::Idle), CLIENT);
            assert_eq!(
                replies(&messages, CLIENT)[0].message.as_deref(),
                Some("latest answer")
            );
        }
    }
}

#[test]
fn claude_preserving_steering_rejects_unverified_drafts_and_invalid_targets() {
    for case in [
        "draft",
        "dialog",
        "hidden",
        "clear",
        "input",
        "conversation",
    ] {
        let (mut screen, messages, writes) = setup("claude");
        report_kind(&mut screen, "claude", hook("SessionStart"));
        screen.agent_control_with_clear(
            identified_prompt("parent", false),
            ClientId::new(9),
            &writes,
        );
        finish_paste(&writes);
        report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
        draft_frame(
            &mut screen,
            "claude",
            if case == "draft" { "draft" } else { "" },
            if case == "draft" { 5 } else { 0 },
            Instant::now(),
        );
        match case {
            "dialog" => report_kind(&mut screen, "claude", claude_permission_prompt()),
            "hidden" => {
                screen
                    .tabs
                    .get_mut(&TabId::new(0))
                    .unwrap()
                    .handle_pty_bytes(TerminalId::new(7), b"\x1b[?25l".to_vec())
                    .unwrap();
            },
            _ => {},
        }
        let mut request = if case == "clear" {
            clear_prompt()
        } else if case == "conversation" {
            let mut request = steering_prompt("one");
            if let AgentRequest::Prompt {
                expected_session_id,
                ..
            } = &mut request
            {
                *expected_session_id = Some("foreign".into());
            }
            request
        } else {
            steering_prompt("one")
        };
        if let AgentRequest::Prompt { preserve_draft, .. } = &mut request {
            *preserve_draft = true;
        }
        screen.agent_control(request, CLIENT);
        if case == "input" {
            assert_eq!(finish_paste(&writes), b"\x1b[200~steer\x1b[201~");
            screen.agent_control(send_keys(vec![BareKey::Char('x')]), ClientId::new(2));
            screen.refresh_agent_controls_at(Instant::now() + Duration::from_secs(1));
            assert_eq!(finish_paste(&writes), b"x");
            assert_eq!(
                replies(&messages, CLIENT)[0].outcome,
                AgentOutcome::Interrupted
            );
        } else {
            assert!(finish_paste(&writes).is_empty(), "{case}");
            assert!(
                matches!(
                    replies(&messages, CLIENT)[0].outcome,
                    AgentOutcome::Rejected | AgentOutcome::TargetChanged
                ),
                "{case}"
            );
        }
    }
}

#[test]
fn claude_steering_timeout_or_delivery_failure_does_not_finish_the_parent() {
    for failure in [false, true] {
        let (mut screen, messages, writes) = setup("claude");
        report_kind(&mut screen, "claude", hook("SessionStart"));
        screen.agent_control_with_clear(identified_prompt("parent", true), CLIENT, &writes);
        finish_paste(&writes);
        report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
        draft_frame(&mut screen, "claude", "", 0, Instant::now());
        let mut request = steering_prompt("one");
        if let AgentRequest::Prompt { timeout_ms, .. } = &mut request {
            *timeout_ms = 10_000;
        }
        screen.agent_control_with_clear(request, ClientId::new(2), &writes);
        if failure {
            for (instruction, _) in writes.try_iter() {
                if let PtyWriteInstruction::Write(_, _, Some(mut completion)) = instruction {
                    completion.set_error_message("failed steering paste".into());
                }
            }
            screen.refresh_agent_controls();
        } else {
            finish_paste(&writes);
            screen.refresh_agent_controls_at(Instant::now() + Duration::from_secs(11));
        }
        let response = replies(&messages, ClientId::new(2));
        assert_eq!(
            response[0].outcome,
            if failure {
                AgentOutcome::Failed
            } else {
                AgentOutcome::TimedOut
            }
        );
        assert!(!response[0].acknowledged);
        report_kind(
            &mut screen,
            "claude",
            claude_stop("prompt", "parent answer", false),
        );
        let parent = replies(&messages, CLIENT);
        assert_eq!(parent.last().unwrap().outcome, AgentOutcome::Completed);
        assert!(!screen.agent_status_for_test().ready);
        // A late acknowledgement belongs only to the steering receipt.
        report_kind(&mut screen, "claude", steering_ack("one"));
        report_kind(
            &mut screen,
            "claude",
            claude_stop("one", "steering answer", false),
        );
        assert!(replies(&messages, ClientId::new(2)).is_empty());
        screen.agent_control(wait_submission("one"), ClientId::new(2));
        let result = replies(&messages, ClientId::new(2));
        assert_eq!(result[0].outcome, AgentOutcome::Completed);
        assert_eq!(result[0].message.as_deref(), Some("steering answer"));
    }
}

#[test]
fn claude_acknowledged_timeout_keeps_its_result_without_blocking_steering() {
    let (mut screen, messages, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    let mut request = identified_prompt("parent", true);
    if let AgentRequest::Prompt { timeout_ms, .. } = &mut request {
        *timeout_ms = 1_000;
    }
    screen.agent_control_with_clear(request, CLIENT, &writes);
    finish_paste(&writes);
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    screen.refresh_agent_controls_at(Instant::now() + Duration::from_secs(2));
    let timeout = replies(&messages, CLIENT);
    assert_eq!(timeout[0].outcome, AgentOutcome::TimedOut);
    assert!(timeout[0].acknowledged);
    deliver_steering(&mut screen, &writes, "next", ClientId::new(2));
    report_kind(&mut screen, "claude", steering_ack("next"));
    report_kind(
        &mut screen,
        "claude",
        claude_stop("prompt", "parent answer", false),
    );
    assert!(replies(&messages, ClientId::new(2)).is_empty());
    screen.agent_control(wait_submission("parent"), CLIENT);
    assert_eq!(
        replies(&messages, CLIENT)[0].message.as_deref(),
        Some("parent answer")
    );
    report_kind(
        &mut screen,
        "claude",
        claude_stop("next", "steered answer", false),
    );
    assert_eq!(
        replies(&messages, ClientId::new(2))[0].outcome,
        AgentOutcome::Completed
    );
}

#[test]
fn claude_background_notifications_cannot_acknowledge_steering_or_hide_permission_waits() {
    let (mut screen, messages, writes) = setup("claude");
    complete_with_background(&mut screen, &messages, &writes, true);
    deliver_steering(&mut screen, &writes, "next", CLIENT);
    report_kind(
        &mut screen,
        "claude",
        task_notification("notification", &["bash"]),
    );
    report_kind(
        &mut screen,
        "claude",
        HookEvent {
            prompt_id: Some("notification".into()),
            ..claude_permission_prompt()
        },
    );
    assert_eq!(
        screen.agent_status_for_test().agent.state,
        AgentState::Waiting
    );
    report_kind(
        &mut screen,
        "claude",
        HookEvent {
            prompt_id: Some("notification".into()),
            ..hook("PostToolUse")
        },
    );
    assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Busy);
    report_kind(
        &mut screen,
        "claude",
        claude_stop("notification", "background answer", false),
    );
    assert!(replies(&messages, CLIENT).is_empty());
    assert_eq!(
        screen.agent_status_for_test().not_ready_reason,
        Some(NotReadyReason::PendingSubmission)
    );
    report_kind(&mut screen, "claude", steering_ack("next"));
    report_kind(
        &mut screen,
        "claude",
        claude_stop("next", "steered answer", false),
    );
    let result = replies(&messages, CLIENT);
    assert_eq!(result[0].outcome, AgentOutcome::Completed);
    assert_eq!(result[0].message.as_deref(), Some("steered answer"));
    assert!(!result[0].background);
    assert!(!result[0].agents[0].background);
    assert_eq!(result[0].agents[0].agent.state, AgentState::Idle);
}

#[test]
fn claude_steering_attempts_enter_even_if_the_composer_becomes_hidden() {
    let (mut screen, messages, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    draft_frame(&mut screen, "claude", "", 0, Instant::now());
    screen.agent_control_with_clear(steering_prompt("next"), CLIENT, &writes);
    finish_paste(&writes);
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .handle_pty_bytes(
            TerminalId::new(7),
            b"\x1b[2J\x1b[HAllow this tool?".to_vec(),
        )
        .unwrap();
    let now = Instant::now();
    screen.refresh_agent_controls_at(now);
    screen.refresh_agent_controls_at(now + Duration::from_secs(1));
    assert_eq!(finish_paste(&writes), b"\r");
    assert!(replies(&messages, CLIENT).is_empty());
    screen.refresh_agent_controls_at(now + Duration::from_secs(31));
    assert_eq!(
        replies(&messages, CLIENT)[0].outcome,
        AgentOutcome::TimedOut
    );
}

#[test]
fn claude_late_uncertainty_is_retained_when_a_newer_steering_turn_completes() {
    let (mut screen, messages, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    screen.agent_control_with_clear(identified_prompt("parent", true), CLIENT, &writes);
    finish_paste(&writes);
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    deliver_steering(&mut screen, &writes, "next", ClientId::new(2));
    report_kind(&mut screen, "claude", steering_ack("next"));
    draft_frame(&mut screen, "claude", "", 0, Instant::now());
    report_kind(
        &mut screen,
        "claude",
        HookEvent {
            claude_turn_complete: Some(false),
            ..claude_stop("prompt", "", false)
        },
    );
    assert_eq!(
        replies(&messages, CLIENT)[0].outcome,
        AgentOutcome::StatusUnavailable
    );
    report_kind(
        &mut screen,
        "claude",
        claude_stop("next", "next answer", false),
    );
    let completed = replies(&messages, ClientId::new(2));
    assert_eq!(completed[0].outcome, AgentOutcome::Completed);
    assert_eq!(
        completed[0].agents[0].not_ready_reason,
        Some(NotReadyReason::UnresolvedTurn)
    );
    assert!(!screen.agent_status_for_test().ready);
    report_kind(
        &mut screen,
        "claude",
        claude_stop("prompt", "parent answer", false),
    );
    assert!(screen.agent_status_for_test().ready);
}

#[test]
fn recent_submission_results_survive_older_unresolved_steering() {
    let (mut screen, messages, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    screen.agent_control_with_clear(identified_prompt("parent", true), CLIENT, &writes);
    finish_paste(&writes);
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    let limit = crate::screen::agent_control::SUBMISSION_HISTORY_LIMIT;
    for index in 1..limit {
        let id = format!("old-{index}");
        deliver_steering(&mut screen, &writes, &id, CLIENT);
        report_kind(&mut screen, "claude", steering_ack(&id));
    }
    deliver_steering(&mut screen, &writes, "recent", CLIENT);
    report_kind(&mut screen, "claude", steering_ack("recent"));
    report_kind(
        &mut screen,
        "claude",
        claude_stop("recent", "recent answer", false),
    );
    replies(&messages, CLIENT);
    for index in 0..limit {
        let id = format!("new-{index}");
        deliver_steering(&mut screen, &writes, &id, CLIENT);
        report_kind(&mut screen, "claude", steering_ack(&id));
        screen.agent_control(wait_submission("recent"), ClientId::new(2));
        let result = replies(&messages, ClientId::new(2));
        assert_eq!(
            result[0].outcome,
            if index + 1 < limit {
                AgentOutcome::Completed
            } else {
                AgentOutcome::StatusUnavailable
            }
        );
        if index + 1 < limit {
            assert_eq!(result[0].message.as_deref(), Some("recent answer"));
        }
    }
    // Resolving an older claim prunes it immediately, without waiting for another admission.
    let before = screen.agent_submissions[&PANE].len();
    report_kind(&mut screen, "claude", claude_stop("old-1", "late", false));
    assert_eq!(screen.agent_submissions[&PANE].len(), before - 1);
}

#[test]
fn claude_overlapping_failures_only_finish_the_correlated_submission() {
    for older in [false, true] {
        let (mut screen, messages, writes) = setup("claude");
        report_kind(&mut screen, "claude", hook("SessionStart"));
        screen.agent_control_with_clear(identified_prompt("parent", true), CLIENT, &writes);
        finish_paste(&writes);
        report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
        deliver_steering(&mut screen, &writes, "next", ClientId::new(2));
        report_kind(&mut screen, "claude", steering_ack("next"));
        report_kind(
            &mut screen,
            "claude",
            HookEvent {
                prompt_id: older.then(|| "prompt".into()),
                ..hook("StopFailure")
            },
        );
        let (failed, remaining, prompt_id, submission_id) = if older {
            (CLIENT, ClientId::new(2), "next", "next")
        } else {
            (ClientId::new(2), CLIENT, "prompt", "parent")
        };
        assert_eq!(replies(&messages, failed)[0].outcome, AgentOutcome::Failed);
        assert!(replies(&messages, remaining).is_empty());
        report_kind(
            &mut screen,
            "claude",
            claude_stop(prompt_id, "success", false),
        );
        let completed = replies(&messages, remaining);
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].outcome, AgentOutcome::Completed);
        assert_eq!(completed[0].submission_id.as_deref(), Some(submission_id));
        screen.agent_control(wait_submission(submission_id), remaining);
        assert_eq!(
            replies(&messages, remaining)[0].outcome,
            AgentOutcome::Completed
        );
    }
}

#[test]
fn claude_uncorrelatable_failure_leaves_older_submissions_unresolved() {
    let (mut screen, messages, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    screen.agent_control_with_clear(identified_prompt("parent", true), CLIENT, &writes);
    finish_paste(&writes);
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    // An incomplete report of a newer human turn leaves no current prompt identity.
    for name in ["UserPromptSubmit", "StopFailure"] {
        report_kind(
            &mut screen,
            "claude",
            HookEvent {
                prompt_id: None,
                ..hook(name)
            },
        );
    }
    assert!(replies(&messages, CLIENT).is_empty());
    report_kind(
        &mut screen,
        "claude",
        claude_stop("prompt", "success", false),
    );
    assert_eq!(
        replies(&messages, CLIENT)[0].outcome,
        AgentOutcome::Completed
    );
}

#[test]
fn claude_resumed_workers_can_request_input_again() {
    for target in pane_targets() {
        let (mut screen, messages, _) = setup_on_target("claude", target);
        report_kind(&mut screen, "claude", hook("SessionStart"));
        let mut question = HookEvent {
            tool_name: Some("AskUserQuestion".into()),
            ..claude_worker("PreToolUse", "worker")
        };
        for run in ["first", "resumed"] {
            question.tool_use_id = Some(format!("input-{run}"));
            for _ in 0..2 {
                report_kind(
                    &mut screen,
                    "claude",
                    claude_worker("SubagentStart", "worker"),
                );
            }
            screen.agent_control(wait(AgentState::Waiting), CLIENT);
            report_kind(&mut screen, "claude", question.clone());
            assert_eq!(
                screen.agent_status_for_test().agent.state,
                AgentState::Waiting
            );
            assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
            report_kind(
                &mut screen,
                "claude",
                claude_worker("SubagentStart", "worker"),
            );
            assert_eq!(
                screen.agent_status_for_test().agent.state,
                AgentState::Waiting
            );
            report_kind(
                &mut screen,
                "claude",
                HookEvent {
                    tool_use_id: question.tool_use_id.clone(),
                    ..claude_worker("PostToolUse", "worker")
                },
            );
            for _ in 0..2 {
                report_kind(
                    &mut screen,
                    "claude",
                    claude_worker("SubagentStop", "worker"),
                );
                assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
            }
            for stale in [
                HookEvent {
                    tool_use_id: question.tool_use_id.clone(),
                    ..claude_worker("PostToolUse", "worker")
                },
                HookEvent {
                    archive_only: true,
                    ..claude_worker("SubagentStart", "worker")
                },
                HookEvent {
                    session_id: "foreign".into(),
                    ..claude_worker("SubagentStart", "worker")
                },
            ] {
                report_kind(&mut screen, "claude", stale);
                assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
            }
        }
    }
}

#[test]
fn old_submission_waiters_survive_completion_and_newer_admissions() {
    for release in ["complete", "disconnect", "timeout"] {
        let (mut screen, messages, writes) = setup("claude");
        report_kind(&mut screen, "claude", hook("SessionStart"));
        screen.agent_control_with_clear(identified_prompt("parent", false), CLIENT, &writes);
        finish_paste(&writes);
        report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
        replies(&messages, CLIENT);
        for client in [2, 3] {
            screen.agent_control(wait_submission("parent"), ClientId::new(client));
        }
        let limit = crate::screen::agent_control::SUBMISSION_HISTORY_LIMIT;
        for index in 0..=limit {
            let id = format!("next-{index}");
            deliver_steering(&mut screen, &writes, &id, CLIENT);
            report_kind(&mut screen, "claude", steering_ack(&id));
            report_kind(&mut screen, "claude", claude_stop(&id, "newer", false));
            replies(&messages, CLIENT);
        }
        assert_eq!(screen.agent_submissions[&PANE].len(), limit + 1);
        assert!(replies(&messages, ClientId::new(2)).is_empty());
        if release == "timeout" {
            screen.refresh_agent_controls_at(Instant::now() + Duration::from_secs(31));
            for client in [2, 3] {
                assert_eq!(
                    replies(&messages, ClientId::new(client))[0].outcome,
                    AgentOutcome::TimedOut
                );
            }
        }
        // Separate observation from waiter delivery, as on the screen thread, so an
        // admission or disconnect can happen while the completed result is protected.
        let stop = claude_stop("prompt", "old answer", true);
        let event = AgentEvent::from_hook(
            "claude",
            TerminalId::new(7),
            &stop,
            serde_json::to_string(&stop).unwrap(),
        );
        screen.agent_control_event(&event, Some(&stop));
        if release == "complete" {
            deliver_steering(&mut screen, &writes, "after-completion", CLIENT);
            for client in [2, 3] {
                let result = replies(&messages, ClientId::new(client));
                assert_eq!(result.len(), 1);
                assert_eq!(result[0].outcome, AgentOutcome::Completed);
                assert_eq!(result[0].submission_id.as_deref(), Some("parent"));
                assert_eq!(result[0].message.as_deref(), Some("old answer"));
                assert!(result[0].background);
            }
        } else if release == "disconnect" {
            assert_eq!(screen.agent_submissions[&PANE].len(), limit + 1);
            screen.detach_agent_controls(ClientId::new(2));
            assert_eq!(screen.agent_submissions[&PANE].len(), limit + 1);
            screen.detach_agent_controls(ClientId::new(3));
            assert!(replies(&messages, ClientId::new(2)).is_empty());
            assert!(replies(&messages, ClientId::new(3)).is_empty());
        }
        assert_eq!(screen.agent_submissions[&PANE].len(), limit);
        screen.agent_control(wait_submission("parent"), ClientId::new(4));
        assert_eq!(
            replies(&messages, ClientId::new(4))[0].outcome,
            AgentOutcome::StatusUnavailable
        );
    }
}

#[test]
fn claude_submission_background_is_independent_of_the_pane() {
    for next_background in [false, true] {
        let (mut screen, messages, writes) = setup("claude");
        report_kind(&mut screen, "claude", hook("SessionStart"));
        screen.agent_control_with_clear(identified_prompt("parent", true), CLIENT, &writes);
        finish_paste(&writes);
        report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
        deliver_steering(&mut screen, &writes, "next", ClientId::new(2));
        report_kind(&mut screen, "claude", steering_ack("next"));
        for (submission, prompt, client, background) in [
            ("parent", "prompt", CLIENT, true),
            ("next", "next", ClientId::new(2), next_background),
        ] {
            screen.agent_control(wait_submission(submission), ClientId::new(3));
            report_kind(
                &mut screen,
                "claude",
                claude_stop(prompt, submission, background),
            );
            for recipient in [client, ClientId::new(3)] {
                let result = replies(&messages, recipient);
                assert_eq!(result.len(), 1);
                assert_eq!(result[0].outcome, AgentOutcome::Completed);
                assert_eq!(result[0].background, background);
            }
        }
        // The newest confirmation carries the registry's current status for the pane.
        assert_eq!(screen.agent_status_for_test().background, next_background);
        if next_background {
            screen.agent_control(wait(AgentState::Busy), CLIENT);
            let pane_wait = replies(&messages, CLIENT);
            assert_eq!(pane_wait[0].outcome, AgentOutcome::Ok);
            assert!(pane_wait[0].background);
            report_kind(
                &mut screen,
                "claude",
                task_notification("notification", &["task"]),
            );
            report_kind(
                &mut screen,
                "claude",
                claude_stop("notification", "finished tasks", false),
            );
        }
        assert!(!screen.agent_status_for_test().background);
        for (submission, background) in [("parent", true), ("next", next_background)] {
            screen.agent_control(wait_submission(submission), CLIENT);
            let retained = replies(&messages, CLIENT);
            assert_eq!(retained[0].outcome, AgentOutcome::Completed);
            assert_eq!(retained[0].background, background);
            assert!(!retained[0].agents[0].background);
        }
    }
}

#[test]
fn notification_permission_wait_clears_only_after_delivered_enter() {
    for cli in [false, true] {
        let (mut screen, messages, writes) = setup("claude");
        complete_with_background(&mut screen, &messages, &writes, true);
        report_kind(
            &mut screen,
            "claude",
            task_notification("notification", &["bash"]),
        );
        report_kind(
            &mut screen,
            "claude",
            HookEvent {
                prompt_id: Some("notification".into()),
                ..claude_permission_prompt()
            },
        );
        assert_eq!(
            screen.agent_status_for_test().agent.state,
            AgentState::Waiting
        );
        permission_key(&mut screen, KeyWithModifier::new(BareKey::Down), cli);
        assert_eq!(
            screen.agent_status_for_test().agent.state,
            AgentState::Waiting
        );
        screen
            .tab_containing_pane_mut(PANE)
            .unwrap()
            .get_pane_with_id_mut(PANE)
            .unwrap()
            .set_suspended(true);
        permission_key(&mut screen, KeyWithModifier::new(BareKey::Enter), cli);
        assert_eq!(
            screen.agent_status_for_test().agent.state,
            AgentState::Waiting
        );
        screen
            .tab_containing_pane_mut(PANE)
            .unwrap()
            .get_pane_with_id_mut(PANE)
            .unwrap()
            .set_suspended(false);
        let worker = HookEvent {
            agent_id: Some("worker".into()),
            tool_use_id: Some("worker-question".into()),
            tool_name: Some("AskUserQuestion".into()),
            ..hook("PreToolUse")
        };
        if cli {
            report_kind(&mut screen, "claude", worker.clone());
        }
        permission_key(&mut screen, KeyWithModifier::new(BareKey::Enter), cli);
        if cli {
            assert_eq!(
                screen.agent_status_for_test().agent.state,
                AgentState::Waiting
            );
            report_kind(
                &mut screen,
                "claude",
                HookEvent {
                    hook_event_name: "PostToolUse".into(),
                    ..worker
                },
            );
        }
        assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Busy);
        // The notification is still running, and the parent's completion stays intact.
        assert_eq!(
            screen.agent_lifecycle[&PANE].last_message_for_test(),
            Some("Started.")
        );
        report_kind(
            &mut screen,
            "claude",
            claude_stop("notification", "finished", false),
        );
        assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
    }
}

fn claude_lifecycle(screen: &Screen) -> yinz_types::agent::tracking::LifecycleState {
    screen.agent_lifecycle[&PANE].handoff_for_test()
}

fn claude_background_permission_request(name: &str) -> HookEvent {
    HookEvent {
        tool_name: Some("Bash".into()),
        tool_use_id: Some("worker-bash".into()),
        ..claude_worker(name, "worker")
    }
}

fn claude_background_permission_wait(screen: &mut Screen, caller_id: bool) {
    report_kind(
        screen,
        "claude",
        claude_background_permission_request("PermissionRequest"),
    );
    report_kind(
        screen,
        "claude",
        HookEvent {
            prompt_id: caller_id.then(|| "prompt".into()),
            ..claude_permission_prompt()
        },
    );
    assert!(claude_lifecycle(screen).claude_notifications.is_empty());
    assert_eq!(
        screen.agent_status_for_test().agent.state,
        AgentState::Waiting
    );
}

#[test]
fn claude_background_permission_approval_restores_completed_parent_and_finishes_idle_wait() {
    for cli in [false, true] {
        for caller_id in [false, true] {
            for more_background in [false, true] {
                let (mut screen, messages, writes) = setup("claude");
                complete_with_background(&mut screen, &messages, &writes, true);
                report_kind(
                    &mut screen,
                    "claude",
                    claude_worker("SubagentStart", "worker"),
                );
                let (sender, plugins) = channels::unbounded();
                screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
                screen.background_plugin_subscriptions.insert(
                    (PluginId::new(99), CLIENT),
                    HashSet::from([EventType::PaneUpdate]),
                );
                claude_background_permission_wait(&mut screen, caller_id);
                let previous_consumption = claude_lifecycle(&screen).input_revision;
                permission_key(&mut screen, KeyWithModifier::new(BareKey::Enter), cli);
                assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Busy);
                let consumption = if cli {
                    Some(screen.tabs[&TabId::new(0)].agent_input_revision(PANE))
                } else {
                    previous_consumption
                };
                let lifecycle = &claude_lifecycle(&screen);
                let parent = lifecycle.claude.parent.as_ref().unwrap();
                assert_eq!(
                    parent.state,
                    AgentState::Idle,
                    "cli={cli} caller_id={caller_id}"
                );
                assert_eq!(parent.detail, None);
                assert_eq!(parent.waiting_on, None);
                assert!(!parent.wait_needs_a_person);
                assert_eq!(lifecycle.claude_confirmation, Some(true));
                assert!(screen.agent_status_for_test().background);
                assert!(screen.claude_permission_waits.is_empty());
                // Terminal Enter has no CLI consumption receipt; observe the empty composer
                // so the idle wait also has independent evidence of input readiness.
                draft_frame(&mut screen, "claude", "", 0, Instant::now());
                let waiter = ClientId::new(2);
                screen.agent_control(wait(AgentState::Idle), waiter);
                assert!(replies(&messages, waiter).is_empty());
                for name in ["PreToolUse", "PostToolUse", "SubagentStop"] {
                    report_kind(
                        &mut screen,
                        "claude",
                        claude_background_permission_request(name),
                    );
                    assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Busy);
                    assert_eq!(
                        claude_lifecycle(&screen)
                            .claude
                            .parent
                            .as_ref()
                            .unwrap()
                            .state,
                        AgentState::Idle
                    );
                }
                report_kind(
                    &mut screen,
                    "claude",
                    task_notification("notification", &["worker"]),
                );
                // Registry idle cannot finish an in-flight notification turn.
                report_kind(&mut screen, "claude", background_idle("prompt"));
                assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Busy);
                assert!(replies(&messages, waiter).is_empty());
                let published: Vec<_> = plugins
                    .try_iter()
                    .flat_map(|(instruction, _)| {
                        let PluginInstruction::Update(updates) = instruction else {
                            return vec![];
                        };
                        updates
                    })
                    .filter_map(|(_, _, event)| {
                        let Event::PaneUpdate(manifest) = event else {
                            return None;
                        };
                        manifest
                            .panes
                            .values()
                            .flatten()
                            .find(|pane| pane.id == PANE)
                            .and_then(|pane| pane.agent.as_ref().map(|agent| agent.state))
                    })
                    .collect();
                assert!(published.contains(&AgentState::Waiting));
                assert!(published.contains(&AgentState::Busy));
                assert!(
                    !published.contains(&AgentState::Idle),
                    "permission approval must keep publishing aggregate activity"
                );
                report_kind(
                    &mut screen,
                    "claude",
                    claude_stop("notification", "Worker finished.", more_background),
                );
                if more_background {
                    assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Busy);
                    assert!(screen.agent_status_for_test().background);
                    assert!(replies(&messages, waiter).is_empty());
                    report_kind(&mut screen, "claude", background_idle("notification"));
                }
                let done = replies(&messages, waiter);
                assert_eq!(done.len(), 1);
                assert_eq!(done[0].outcome, AgentOutcome::Ok);
                assert_eq!(done[0].message.as_deref(), Some("Worker finished."));
                assert!(!done[0].background);
                assert_eq!(done[0].agents[0].agent.state, AgentState::Idle);
                assert_eq!(claude_lifecycle(&screen).input_revision, consumption);
                assert!(plugins.try_iter().any(|(instruction, _)| {
                    let PluginInstruction::Update(updates) = instruction else { return false };
                    updates.into_iter().any(|(_, _, event)| {
                        matches!(event, Event::PaneUpdate(manifest) if manifest.panes.values().flatten()
                            .any(|pane| pane.id == PANE && pane.agent.as_ref().is_some_and(|a| a.state == AgentState::Idle)))
                    })
                }));
            }
        }
    }
}

#[test]
fn claude_background_permission_repeated_dialogs_require_new_enter_and_preserve_drafts() {
    for cli in [false, true] {
        for early_enter in [false, true] {
            let (mut screen, messages, writes) = setup("claude");
            complete_with_background(&mut screen, &messages, &writes, true);
            if early_enter {
                permission_key(&mut screen, KeyWithModifier::new(BareKey::Enter), cli);
            }
            for _ in 0..2 {
                // Repeated Notifications and an earlier Enter cannot answer a new dialog.
                for _ in 0..2 {
                    claude_background_permission_wait(&mut screen, false);
                }
                permission_key(&mut screen, KeyWithModifier::new(BareKey::Down), cli);
                assert_eq!(
                    screen.agent_status_for_test().agent.state,
                    AgentState::Waiting
                );
                permission_key(&mut screen, KeyWithModifier::new(BareKey::Enter), cli);
                assert_eq!(
                    claude_lifecycle(&screen)
                        .claude
                        .parent
                        .as_ref()
                        .unwrap()
                        .state,
                    AgentState::Idle
                );
                assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Busy);
            }
            // Activity completion must not consume a draft typed after permission approval.
            let consumption = screen
                .agent_dialog_inputs
                .get(&PANE)
                .map(|input| input.input_revision)
                .or(claude_lifecycle(&screen).input_revision);
            screen.agent_write_for_test();
            draft_frame(
                &mut screen,
                "claude",
                "unsubmitted draft",
                17,
                Instant::now(),
            );
            let revision = screen.tabs[&TabId::new(0)].agent_input_revision(PANE);
            report_kind(
                &mut screen,
                "claude",
                claude_background_permission_request("PostToolUse"),
            );
            report_kind(
                &mut screen,
                "claude",
                claude_worker("SubagentStop", "worker"),
            );
            report_kind(
                &mut screen,
                "claude",
                task_notification("notification", &["worker"]),
            );
            report_kind(
                &mut screen,
                "claude",
                claude_stop("notification", "finished", false),
            );
            let status = screen.agent_status_for_test();
            assert_eq!(status.agent.state, AgentState::Idle);
            assert!(!status.background);
            assert_eq!(
                status.not_ready_reason,
                Some(NotReadyReason::UnconsumedInput)
            );
            assert_eq!(
                screen.tabs[&TabId::new(0)].agent_input_revision(PANE),
                revision
            );
            assert_eq!(claude_lifecycle(&screen).input_revision, consumption);
            assert!(screen.tabs[&TabId::new(0)]
                .get_pane_with_id(PANE)
                .unwrap()
                .dump_screen(false, Some(CLIENT))
                .contains("unsubmitted draft"));
        }
    }
}

#[test]
fn claude_background_permission_unconfirmed_callers_remain_busy_after_approval() {
    for confirmation in [None, Some(false)] {
        for cli in [false, true] {
            let (mut screen, _, _) = setup("claude");
            report_kind(&mut screen, "claude", hook("SessionStart"));
            report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
            if let Some(complete) = confirmation {
                report_kind(
                    &mut screen,
                    "claude",
                    HookEvent {
                        claude_turn_complete: Some(complete),
                        ..claude_stop("prompt", "still working", true)
                    },
                );
            }
            claude_background_permission_wait(&mut screen, true);
            permission_key(&mut screen, KeyWithModifier::new(BareKey::Enter), cli);
            assert_eq!(claude_lifecycle(&screen).claude_confirmation, confirmation);
            assert_eq!(
                claude_lifecycle(&screen)
                    .claude
                    .parent
                    .as_ref()
                    .unwrap()
                    .state,
                AgentState::Busy
            );
            assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Busy);
            report_kind(
                &mut screen,
                "claude",
                claude_stop("prompt", "finished", false),
            );
            assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
        }
    }
}

#[test]
fn claude_background_permission_new_callers_invalidate_old_completion_evidence() {
    for recovered in [false, true] {
        for cli in [false, true] {
            let (mut screen, messages, writes) = setup("claude");
            complete_with_background(&mut screen, &messages, &writes, true);
            claude_background_permission_wait(&mut screen, false);
            if !recovered {
                permission_key(&mut screen, KeyWithModifier::new(BareKey::Enter), cli);
                report_kind(
                    &mut screen,
                    "claude",
                    task_notification("notification", &["worker"]),
                );
            }
            report_kind(
                &mut screen,
                "claude",
                if recovered {
                    recovered_prompt("newer", "prompt")
                } else {
                    HookEvent {
                        prompt_id: Some("newer".into()),
                        ..hook("UserPromptSubmit")
                    }
                },
            );
            assert_eq!(claude_lifecycle(&screen).claude_confirmation, None);
            if recovered {
                assert_eq!(
                    screen.agent_status_for_test().agent.state,
                    AgentState::Waiting
                );
                assert!(screen.claude_permission_waits.contains_key(&PANE));
            } else {
                report_kind(
                    &mut screen,
                    "claude",
                    HookEvent {
                        prompt_id: Some("newer".into()),
                        ..claude_permission_prompt()
                    },
                );
            }
            permission_key(&mut screen, KeyWithModifier::new(BareKey::Enter), cli);
            assert_eq!(
                claude_lifecycle(&screen)
                    .claude
                    .parent
                    .as_ref()
                    .unwrap()
                    .state,
                AgentState::Busy
            );
            if recovered {
                report_kind(
                    &mut screen,
                    "claude",
                    task_notification("notification", &["worker"]),
                );
            }
            for stale in [
                background_idle("prompt"),
                claude_stop("notification", "older answer", false),
                claude_stop("prompt", "Started.", false),
            ] {
                report_kind(&mut screen, "claude", stale);
                assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Busy);
                assert_eq!(claude_lifecycle(&screen).claude_confirmation, None);
                assert_eq!(
                    claude_lifecycle(&screen)
                        .claude
                        .parent
                        .as_ref()
                        .unwrap()
                        .state,
                    AgentState::Busy
                );
            }
            report_kind(
                &mut screen,
                "claude",
                claude_stop("newer", "new answer", false),
            );
            assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
            assert!(!screen.agent_status_for_test().background);
        }
    }
}

#[test]
fn claude_background_permission_approval_preserves_concurrent_worker_tools_after_stop() {
    for cli in [false, true] {
        let (mut screen, messages, writes) = setup("claude");
        complete_with_background(&mut screen, &messages, &writes, true);
        claude_background_permission_wait(&mut screen, false);
        for (worker, tool) in [
            ("one", "question-1"),
            ("one", "question-2"),
            ("two", "question-3"),
        ] {
            report_kind(
                &mut screen,
                "claude",
                claude_worker_input("PreToolUse", worker, Some(tool)),
            );
        }
        permission_key(&mut screen, KeyWithModifier::new(BareKey::Enter), cli);
        assert_eq!(
            claude_lifecycle(&screen)
                .claude
                .parent
                .as_ref()
                .unwrap()
                .state,
            AgentState::Idle
        );
        assert_eq!(
            screen.agent_worker_waits[&PANE].parent.state,
            AgentState::Idle
        );
        assert_eq!(
            screen.agent_status_for_test().agent.state,
            AgentState::Waiting
        );
        draft_frame(&mut screen, "claude", "", 0, Instant::now());
        report_kind(
            &mut screen,
            "claude",
            task_notification("notification", &["worker"]),
        );
        report_kind(
            &mut screen,
            "claude",
            claude_stop("notification", "finished", false),
        );
        assert!(!screen.agent_status_for_test().background);
        let waiter = ClientId::new(2);
        for event in [
            claude_worker("SubagentStop", "one"),
            claude_worker("SubagentStop", "two"),
            claude_background_permission_request("PostToolUse"),
            claude_worker_input("PostToolUse", "one", Some("unrelated")),
            claude_worker_input("PostToolUse", "one", Some("question-1")),
            claude_worker_input("PostToolUse", "two", Some("question-3")),
        ] {
            report_kind(&mut screen, "claude", event);
            assert_eq!(
                screen.agent_status_for_test().agent.state,
                AgentState::Waiting
            );
        }
        assert!(screen.agent_worker_waits[&PANE].waits.contains_key("one"));
        assert!(!screen.agent_worker_waits[&PANE].waits.contains_key("two"));
        report_kind(
            &mut screen,
            "claude",
            claude_worker_input("PostToolUse", "one", Some("question-2")),
        );
        assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
        screen.agent_control(wait(AgentState::Idle), waiter);
        let done = replies(&messages, waiter);
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].message.as_deref(), Some("finished"));
    }
}

#[test]
fn claude_steering_delivery_is_bounded_during_continuous_background_output() {
    let (mut screen, messages, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    screen.agent_control_with_clear(identified_prompt("parent", true), CLIENT, &writes);
    finish_paste(&writes);
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    report_kind(
        &mut screen,
        "claude",
        claude_worker("SubagentStart", "worker"),
    );
    screen.agent_control(steering_prompt("next"), ClientId::new(2));
    assert!(finish_paste(&writes).starts_with(b"\x1b[200~yz"));
    let now = Instant::now();
    let mut delivered = Vec::new();
    // Advance a controlled clock; output is never quiet for the old 100 ms debounce.
    for tick in 0..=30 {
        let at = now + Duration::from_millis(tick * 50);
        screen.pane_output_activity.insert(PANE, at);
        if tick == 2 {
            report_kind(
                &mut screen,
                "claude",
                claude_worker("PostToolUse", "worker"),
            );
        }
        screen.refresh_agent_controls_at(at);
        delivered.extend(finish_paste(&writes));
    }
    assert_eq!(delivered, b"\x03\x1b[200~steer\x1b[201~\r");
    assert!(replies(&messages, CLIENT).is_empty());
    assert!(replies(&messages, ClientId::new(2)).is_empty());
    report_kind(&mut screen, "claude", steering_ack("next"));
    // The registry still shows the worker running when the newest turn ends.
    report_kind(
        &mut screen,
        "claude",
        claude_stop("next", "next answer", true),
    );
    assert_eq!(
        replies(&messages, ClientId::new(2))[0].outcome,
        AgentOutcome::Completed
    );
    report_kind(
        &mut screen,
        "claude",
        claude_stop("prompt", "parent answer", false),
    );
    assert_eq!(
        replies(&messages, CLIENT)[0].outcome,
        AgentOutcome::Completed
    );
    assert_eq!(
        pane_state(&screen),
        AgentState::Busy,
        "worker remains active"
    );
}

#[test]
fn claude_steering_replaces_unconsumed_input_despite_unrelated_output() {
    let (mut screen, messages, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    screen.agent_control_with_clear(identified_prompt("parent", false), CLIENT, &writes);
    finish_paste(&writes);
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    replies(&messages, CLIENT);
    draft_frame(&mut screen, "claude", "", 0, Instant::now());
    screen.agent_control(send_keys(vec![BareKey::Char('x')]), ClientId::new(9));
    assert_eq!(finish_paste(&writes), b"x");
    // The terminal still shows the old empty editor. A spinner is not an input receipt.
    screen
        .pane_output_activity
        .insert(PANE, Instant::now() + Duration::from_millis(1));
    assert_eq!(
        screen.agent_status_for_test().not_ready_reason,
        Some(NotReadyReason::UnconsumedInput)
    );
    let mut preserving = steering_prompt("preserve");
    if let AgentRequest::Prompt { preserve_draft, .. } = &mut preserving {
        *preserve_draft = true;
    }
    screen.agent_control(preserving, ClientId::new(2));
    assert_eq!(
        replies(&messages, ClientId::new(2))[0].outcome,
        AgentOutcome::Rejected
    );
    assert!(finish_paste(&writes).is_empty());
    screen.agent_control_with_clear(steering_prompt("replace"), ClientId::new(2), &writes);
    assert_eq!(finish_paste(&writes), b"\x1b[200~steer\x1b[201~");
    let now = Instant::now();
    screen.refresh_agent_controls_at(now);
    screen.refresh_agent_controls_at(now + Duration::from_millis(600));
    assert_eq!(finish_paste(&writes), b"\r");
    report_kind(&mut screen, "claude", steering_ack("replace"));
    report_kind(
        &mut screen,
        "claude",
        claude_stop("replace", "replaced", false),
    );
    assert_eq!(
        replies(&messages, ClientId::new(2))[0].outcome,
        AgentOutcome::Completed
    );
}

#[test]
fn claude_steering_preparation_stops_when_permission_or_question_hooks_arrive() {
    for waiting_hook in [
        claude_permission_prompt(),
        HookEvent {
            tool_name: Some("AskUserQuestion".into()),
            ..hook("PreToolUse")
        },
    ] {
        for stage in ["marker", "clear", "settling"] {
            let (mut screen, messages, writes) = setup("claude");
            report_kind(&mut screen, "claude", hook("SessionStart"));
            screen.agent_control_with_clear(identified_prompt("parent", false), CLIENT, &writes);
            finish_paste(&writes);
            report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
            assert_eq!(
                replies(&messages, CLIENT)[0].outcome,
                AgentOutcome::Submitted
            );

            screen.agent_control(steering_prompt("next"), ClientId::new(2));
            if stage != "marker" {
                assert!(finish_paste(&writes).starts_with(b"\x1b[200~yz"));
                screen.refresh_agent_controls();
            }
            // Keep the marker or clear acknowledgement pending across the hook.
            let mut queued = writes.try_iter().collect::<Vec<_>>();
            assert_eq!(queued.len(), 1);
            let PtyWriteInstruction::Write(bytes, _, Some(_)) = &queued[0].0 else {
                panic!("preparation must have a pending write");
            };
            if stage != "marker" {
                assert_eq!(bytes, b"\x03");
            } else {
                assert!(bytes.starts_with(b"\x1b[200~yz"));
            }
            if stage == "settling" {
                // Acknowledge clearing before the hook; the report's initial refresh
                // starts the settling interval, with no prompt bytes delivered yet.
                queued.clear();
            }

            report_kind(&mut screen, "claude", waiting_hook.clone());
            assert_eq!(
                screen.agent_status_for_test().agent.state,
                AgentState::Waiting
            );
            let result = replies(&messages, ClientId::new(2));
            assert_eq!(result.len(), 1, "{waiting_hook:?}, stage={stage}");
            assert_eq!(result[0].outcome, AgentOutcome::Interrupted);
            assert_eq!(result[0].submission_id.as_deref(), Some("next"));
            assert!(!result[0].acknowledged);
            assert!(result[0]
                .message
                .as_ref()
                .unwrap()
                .contains("prompt was not sent"));
            assert!(finish_paste(&writes).is_empty());

            drop(queued);
            let now = Instant::now();
            for tick in 0..=3 {
                screen.refresh_agent_controls_at(now + Duration::from_millis(tick * 600));
                assert!(
                    finish_paste(&writes).is_empty(),
                    "no preparation, paste, or Enter after waiting"
                );
            }
            assert!(screen.agent_draft_clears.is_empty());
            assert!(replies(&messages, ClientId::new(2)).is_empty());

            report_kind(&mut screen, "claude", hook("PostToolUse"));
            report_kind(
                &mut screen,
                "claude",
                claude_stop("prompt", "parent answer", false),
            );
            screen.agent_control(wait_submission("parent"), CLIENT);
            let parent = replies(&messages, CLIENT);
            assert_eq!(parent.len(), 1);
            assert_eq!(parent[0].outcome, AgentOutcome::Completed);
            assert_eq!(parent[0].message.as_deref(), Some("parent answer"));
            assert!(finish_paste(&writes).is_empty());
        }
    }
}

#[test]
fn claude_steering_preparation_reports_failures_without_sending_the_prompt() {
    for failure in ["writer", "input", "conversation", "timeout"] {
        let (mut screen, messages, writes) = setup("claude");
        report_kind(&mut screen, "claude", hook("SessionStart"));
        report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
        screen.agent_control(steering_prompt("next"), CLIENT);
        for (instruction, _) in writes.try_iter() {
            if let PtyWriteInstruction::Write(bytes, _, Some(mut completion)) = instruction {
                assert!(bytes.starts_with(b"\x1b[200~yz"));
                if failure == "writer" {
                    completion.set_error_message("marker delivery failed".into());
                }
            }
        }
        match failure {
            "input" => {
                screen.agent_control(send_keys(vec![BareKey::Char('x')]), ClientId::new(2));
                assert_eq!(finish_paste(&writes), b"x");
            },
            "conversation" => report_kind(
                &mut screen,
                "claude",
                HookEvent {
                    session_id: "replacement".into(),
                    ..hook("SessionStart")
                },
            ),
            _ => {},
        }
        screen.refresh_agent_controls_at(
            Instant::now() + Duration::from_secs(if failure == "timeout" { 31 } else { 1 }),
        );
        // Reports drain ready controls before applying their new identity. The clear
        // can already be queued, but replacement must prevent the prompt and Enter.
        assert_eq!(
            finish_paste(&writes),
            if failure == "conversation" {
                vec![3]
            } else {
                vec![]
            },
            "{failure}"
        );
        screen.refresh_agent_controls();
        let result = replies(&messages, CLIENT);
        assert_eq!(result.len(), 1);
        assert!(!result[0].acknowledged);
        assert_eq!(
            result[0].outcome,
            match failure {
                "writer" => AgentOutcome::Failed,
                "input" => AgentOutcome::Interrupted,
                "conversation" => AgentOutcome::TargetChanged,
                _ => AgentOutcome::TimedOut,
            }
        );
        assert!(result[0]
            .message
            .as_ref()
            .unwrap()
            .contains("prompt was not sent"));
    }
}

#[test]
fn claude_terminal_hooks_retire_detached_confirmations_without_turn_ids() {
    for terminal_hook in ["StopFailure", "Interrupt"] {
        for older in [false, true] {
            let (mut screen, messages, writes) = setup("claude");
            report_kind(&mut screen, "claude", hook("SessionStart"));
            screen.agent_control_with_clear(identified_prompt("parent", true), CLIENT, &writes);
            finish_paste(&writes);
            report_kind(
                &mut screen,
                "claude",
                HookEvent {
                    turn_id: None,
                    ..hook("UserPromptSubmit")
                },
            );
            if older {
                deliver_steering(&mut screen, &writes, "next", ClientId::new(2));
                report_kind(&mut screen, "claude", steering_ack("next"));
            }
            report_kind(
                &mut screen,
                "claude",
                HookEvent {
                    prompt_id: older.then(|| "prompt".into()),
                    turn_id: None,
                    ..hook(terminal_hook)
                },
            );
            assert_eq!(
                replies(&messages, CLIENT)[0].outcome,
                if terminal_hook == "StopFailure" {
                    AgentOutcome::Failed
                } else {
                    AgentOutcome::Interrupted
                }
            );
            for complete in [false, true] {
                report_kind(
                    &mut screen,
                    "claude",
                    HookEvent {
                        turn_id: None,
                        claude_turn_complete: Some(complete),
                        ..claude_stop("prompt", "stale", true)
                    },
                );
                assert_eq!(
                    pane_state(&screen),
                    if older {
                        AgentState::Busy
                    } else {
                        AgentState::Idle
                    }
                );
                assert!(!screen.agent_status_for_test().background);
            }
            if older {
                assert!(replies(&messages, ClientId::new(2)).is_empty());
                report_kind(&mut screen, "claude", claude_stop("next", "success", false));
                assert_eq!(
                    replies(&messages, ClientId::new(2))[0].outcome,
                    AgentOutcome::Completed
                );
            }
            if terminal_hook == "StopFailure" {
                screen.agent_control(wait(AgentState::Idle), CLIENT);
                assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
            }
        }
    }
}

#[test]
fn claude_steering_attempts_replacement_for_full_and_hidden_composers() {
    for hidden in [false, true] {
        let (mut screen, messages, writes) = setup("claude");
        report_kind(&mut screen, "claude", hook("SessionStart"));
        report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
        draft_frame(&mut screen, "claude", "old draft", 9, Instant::now());
        if hidden {
            screen
                .tabs
                .get_mut(&TabId::new(0))
                .unwrap()
                .handle_pty_bytes(TerminalId::new(7), b"\x1b[?25l".to_vec())
                .unwrap();
        }
        screen.agent_control_with_clear(steering_prompt("next"), CLIENT, &writes);
        assert_eq!(finish_paste(&writes), b"\x1b[200~steer\x1b[201~");
        let now = Instant::now();
        screen.refresh_agent_controls_at(now);
        screen.refresh_agent_controls_at(now + Duration::from_millis(600));
        assert_eq!(finish_paste(&writes), b"\r");
        assert!(replies(&messages, CLIENT).is_empty());
        report_kind(&mut screen, "claude", steering_ack("next"));
        report_kind(
            &mut screen,
            "claude",
            claude_stop("next", "delivered", false),
        );
        assert_eq!(
            replies(&messages, CLIENT)[0].outcome,
            AgentOutcome::Completed
        );
    }
}

#[path = "agent_tracking_handoff.rs"]
mod tracking_handoff;

#[test]
fn canonical_binding_during_preparation_is_pinned_for_later_checks() {
    for replacement in [None, Some("codex:replacement")] {
        let (mut screen, messages, writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        let now = Instant::now();
        screen.agent_control_at(
            prompt(false),
            crate::screen::agent_control::Reply::Client(CLIENT),
            now,
        );
        finish_paste(&writes);
        let mut agent = screen.get_pane_info(PANE).unwrap().agent.unwrap();
        agent.run_id = Some("codex:conversation".into());
        screen
            .tab_containing_pane_mut(PANE)
            .unwrap()
            .set_pane_agent(PANE, Some(agent.clone()));
        screen.refresh_agent_controls_at(now + Duration::from_millis(1));
        assert_eq!(finish_paste(&writes), [3]);
        assert!(replies(&messages, CLIENT).is_empty());
        agent.run_id = replacement.map(str::to_owned);
        screen
            .tab_containing_pane_mut(PANE)
            .unwrap()
            .set_pane_agent(PANE, Some(agent));
        screen.refresh_agent_controls_at(now + Duration::from_millis(2));
        let response = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(response.outcome, AgentOutcome::TargetChanged);
        assert_eq!(
            response.prompt_delivery,
            Some(yinz_types::agent::control::PromptDelivery::NotSent)
        );
        assert!(finish_paste(&writes).is_empty());
    }
}

#[test]
fn attempted_prompt_writes_never_gain_unsent_evidence() {
    use yinz_types::agent::control::PromptDelivery;
    for failure in ["queue", "writer"] {
        let (mut screen, messages, writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        let now = Instant::now();
        let mut request = prompt(false);
        if let AgentRequest::Prompt { submission_id, .. } = &mut request {
            *submission_id = Some("failed-write".into());
        }
        screen.agent_control_at(
            request,
            crate::screen::agent_control::Reply::Client(CLIENT),
            now,
        );
        finish_paste(&writes);
        screen.refresh_agent_controls_at(now + Duration::from_millis(1));
        assert_eq!(finish_paste(&writes), [3]);
        screen.refresh_agent_controls_at(now + Duration::from_millis(2));
        if failure == "queue" {
            let (sender, receiver) = channels::unbounded();
            drop(receiver);
            let senders = &mut screen.tab_containing_pane_mut(PANE).unwrap().senders;
            senders.should_silently_fail = false;
            senders.to_pty_writer = Some(SenderWithContext::new(sender));
        }
        screen.refresh_agent_controls_at(now + Duration::from_millis(252));
        if failure == "writer" {
            for (instruction, _) in writes.try_iter() {
                if let PtyWriteInstruction::Write(_, _, Some(mut completion)) = instruction {
                    completion.set_error_message("prompt write failed".into());
                }
            }
            screen.refresh_agent_controls_at(now + Duration::from_millis(253));
        }
        let response = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(response.outcome, AgentOutcome::Failed);
        assert_eq!(response.prompt_delivery, Some(PromptDelivery::Started));
        assert!(!response.acknowledged);
        screen.agent_control_at(
            AgentRequest::WaitTurn {
                pane_id: PANE,
                expected_session_id: Some("conversation".into()),
                submission_id: "failed-write".into(),
                timeout_ms: 1000,
            },
            crate::screen::agent_control::Reply::Client(CLIENT),
            now + Duration::from_secs(1),
        );
        screen.refresh_agent_controls_at(now + Duration::from_secs(3));
        let retained = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(retained.prompt_delivery, Some(PromptDelivery::Started));
    }
}

#[test]
fn controller_worker_stack_shares_region_and_survives_root_close() {
    use yinz_types::agent::control::AgentPlacement;
    let (mut screen, _, _) = setup("bash");
    let owner = PANE;
    let tab_id = screen.tab_id_containing_pane(owner).unwrap();
    let mut workers = vec![];
    for id in [88, 89, 90] {
        let pane = PaneId::Terminal(TerminalId::new(id));
        let destination = screen
            .resolve_agent_tab(AgentPlacement::ControllerStack(owner), None, CLIENT)
            .unwrap();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        let mut completion = crate::route::NotificationEnd::new(tx);
        completion.agent_tab = Some(destination);
        screen
            .place_new_pane(
                pane,
                None,
                None,
                None,
                NewPanePlacement::default(),
                false,
                crate::pty::ClientTabIndexOrPaneId::TabIndexNoFocus(tab_id),
                Some(completion),
                false,
                &mut Default::default(),
            )
            .unwrap();
        assert!(rx.try_recv().unwrap().error_message.is_none());
        workers.push(pane);
        assert_eq!(screen.tabs[&tab_id].get_active_pane_id(CLIENT), Some(owner));
    }
    let tab = &screen.tabs[&tab_id];
    let first = tab
        .get_pane_with_id(workers[0])
        .unwrap()
        .position_and_size();
    for worker in &workers[1..] {
        let geom = tab.get_pane_with_id(*worker).unwrap().position_and_size();
        assert!(first.stacked.is_some());
        assert_eq!(geom.stacked, first.stacked);
        assert_eq!(geom.x, first.x);
        assert_eq!(geom.cols, first.cols);
    }
    screen.close_pane_explicitly(workers[0], None);
    assert_eq!(screen.agent_worker_stacks[&(owner, tab_id)], workers[1..]);
    for worker in &workers[1..] {
        screen.close_pane_explicitly(*worker, None);
    }
    assert!(!screen.agent_worker_stacks.contains_key(&(owner, tab_id)));
    assert!(screen.tab_containing_pane(owner).is_some());
}

#[test]
fn completed_worker_close_requires_latest_accepted_submission() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    finish_paste(&writes);
    report(&mut screen, hook("UserPromptSubmit"));
    report(&mut screen, hook("Stop"));
    let submission = screen.agent_submissions[&PANE].back().unwrap().id.clone();
    replies(&messages, CLIENT);
    screen.agent_control(
        AgentRequest::CloseCompleted {
            pane_id: PANE,
            expected_session_id: Some("conversation".into()),
            submission_id: "wrong".into(),
        },
        CLIENT,
    );
    assert_eq!(
        replies(&messages, CLIENT).pop().unwrap().outcome,
        AgentOutcome::Rejected
    );
    assert!(screen.tab_containing_pane(PANE).is_some());
    screen.agent_control(
        AgentRequest::CloseCompleted {
            pane_id: PANE,
            expected_session_id: Some("conversation".into()),
            submission_id: submission,
        },
        CLIENT,
    );
    assert_eq!(
        replies(&messages, CLIENT).pop().unwrap().outcome,
        AgentOutcome::Ok
    );
    assert!(screen.tab_containing_pane(PANE).is_none());
}

#[test]
fn completed_worker_close_preserves_subsequent_draft() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    finish_paste(&writes);
    report(&mut screen, hook("UserPromptSubmit"));
    report(&mut screen, hook("Stop"));
    let submission = screen.agent_submissions[&PANE].back().unwrap().id.clone();
    replies(&messages, CLIENT);
    screen
        .tab_containing_pane_mut(PANE)
        .unwrap()
        .write_to_active_terminal(&None, b"keep this draft".to_vec(), false, CLIENT, None)
        .unwrap();
    let revision = screen
        .tab_containing_pane(PANE)
        .unwrap()
        .agent_input_revision(PANE);
    screen.agent_control(
        AgentRequest::CloseCompleted {
            pane_id: PANE,
            expected_session_id: Some("conversation".into()),
            submission_id: "wrong".into(),
        },
        CLIENT,
    );
    assert_eq!(
        replies(&messages, CLIENT).pop().unwrap().outcome,
        AgentOutcome::Rejected
    );
    assert!(screen.tab_containing_pane(PANE).is_some());
    screen.agent_control(
        AgentRequest::CloseCompleted {
            pane_id: PANE,
            expected_session_id: Some("conversation".into()),
            submission_id: submission,
        },
        CLIENT,
    );
    assert_eq!(
        replies(&messages, CLIENT).pop().unwrap().outcome,
        AgentOutcome::Rejected
    );
    assert!(screen.tab_containing_pane(PANE).is_some());
    assert_eq!(
        screen
            .tab_containing_pane(PANE)
            .unwrap()
            .agent_input_revision(PANE),
        revision
    );
}

#[test]
fn controller_worker_stack_avoids_unrelated_stack() {
    use yinz_types::agent::control::AgentPlacement;
    let (mut screen, _, _) = setup("bash");
    let owner = PANE;
    let tab_id = screen.tab_id_containing_pane(owner).unwrap();
    let unrelated = PaneId::Terminal(TerminalId::new(87));
    screen
        .place_new_pane(
            unrelated,
            None,
            None,
            None,
            NewPanePlacement::Stacked {
                pane_id_to_stack_under: Some(owner),
                borderless: None,
            },
            false,
            crate::pty::ClientTabIndexOrPaneId::TabIndexNoFocus(tab_id),
            None,
            false,
            &mut Default::default(),
        )
        .unwrap();
    let mut workers = vec![];
    for id in [88, 89, 90] {
        let pane = PaneId::Terminal(TerminalId::new(id));
        let destination = screen
            .resolve_agent_tab(AgentPlacement::ControllerStack(owner), None, CLIENT)
            .unwrap();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        let mut completion = crate::route::NotificationEnd::new(tx);
        completion.agent_tab = Some(destination);
        screen
            .place_new_pane(
                pane,
                None,
                None,
                None,
                NewPanePlacement::default(),
                false,
                crate::pty::ClientTabIndexOrPaneId::TabIndexNoFocus(tab_id),
                Some(completion),
                false,
                &mut Default::default(),
            )
            .unwrap();
        assert!(rx.try_recv().unwrap().error_message.is_none());
        workers.push(pane);
        assert_eq!(screen.tabs[&tab_id].get_active_pane_id(CLIENT), Some(owner));
    }
    let tab = &screen.tabs[&tab_id];
    let first = tab
        .get_pane_with_id(workers[0])
        .unwrap()
        .position_and_size();
    assert_ne!(
        first.stacked,
        tab.get_pane_with_id(unrelated)
            .unwrap()
            .position_and_size()
            .stacked
    );
    for worker in &workers[1..] {
        let geom = tab.get_pane_with_id(*worker).unwrap().position_and_size();
        assert!(first.stacked.is_some());
        assert_eq!(geom.stacked, first.stacked);
        assert_eq!(geom.x, first.x);
        assert_eq!(geom.cols, first.cols);
    }
    screen.close_pane_explicitly(workers[0], None);
    assert_eq!(screen.agent_worker_stacks[&(owner, tab_id)], workers[1..]);
    for worker in &workers[1..] {
        screen.close_pane_explicitly(*worker, None);
    }
    assert!(!screen.agent_worker_stacks.contains_key(&(owner, tab_id)));
    assert!(screen.tab_containing_pane(owner).is_some());
}

#[test]
fn controller_worker_stack_rejects_only_stack_space() {
    use yinz_types::agent::control::AgentPlacement;
    let (mut screen, _, _) = setup_with_size("bash", PaneTarget::Local, Size { cols: 20, rows: 8 });
    let owner = PANE;
    let tab_id = screen.tab_id_containing_pane(owner).unwrap();
    let unrelated = PaneId::Terminal(TerminalId::new(87));
    screen
        .place_new_pane(
            unrelated,
            None,
            None,
            None,
            NewPanePlacement::Stacked {
                pane_id_to_stack_under: Some(owner),
                borderless: None,
            },
            false,
            crate::pty::ClientTabIndexOrPaneId::TabIndexNoFocus(tab_id),
            None,
            false,
            &mut Default::default(),
        )
        .unwrap();
    screen.resize_to_screen(Size { cols: 6, rows: 5 }).unwrap();
    let pane = PaneId::Terminal(TerminalId::new(88));
    let destination = screen
        .resolve_agent_tab(AgentPlacement::ControllerStack(owner), None, CLIENT)
        .unwrap();
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    let mut completion = crate::route::NotificationEnd::new(tx);
    completion.agent_tab = Some(destination);
    screen
        .place_new_pane(
            pane,
            None,
            None,
            None,
            NewPanePlacement::default(),
            false,
            crate::pty::ClientTabIndexOrPaneId::TabIndexNoFocus(tab_id),
            Some(completion),
            false,
            &mut Default::default(),
        )
        .unwrap_err();
    assert!(rx.try_recv().unwrap().error_message.is_some());
    assert!(!screen
        .agent_worker_stacks
        .get(&(owner, tab_id))
        .is_some_and(|members| members.contains(&pane)));
    assert!(screen.get_pane_info(pane).is_none());
}

#[test]
fn completed_worker_close_preserves_related_panes() {
    use yinz_types::multipane::{MultipaneDirection, MultipaneSplit};
    use yinz_types::pane_tabs::{PaneTabDescriptor, PaneTabsLayout};

    for composition in ["editor", "multipane", "pane_tabs"] {
        let (mut screen, messages, writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        finish_paste(&writes);
        report(&mut screen, hook("UserPromptSubmit"));
        report(&mut screen, hook("Stop"));
        let submission = screen.agent_submissions[&PANE].back().unwrap().id.clone();
        replies(&messages, CLIENT);
        let tab = screen.tab_containing_pane_mut(PANE).unwrap();
        let editor = PaneId::Terminal(TerminalId::new(99));
        let (sender, _plugins) = channels::unbounded();
        tab.senders
            .replace_to_plugin(SenderWithContext::new(sender));
        match composition {
            "editor" => tab.replace_pane_with_editor_pane(editor, PANE).unwrap(),
            "multipane" => {
                tab.create_multipane(
                    [PaneId::Terminal(TerminalId::new(1)), PANE],
                    MultipaneSplit {
                        direction: MultipaneDirection::LeftRight,
                        ratio: 5000,
                    },
                    0,
                    OverlayPlacement::dismissible(Some(FloatingPaneCoordinates::default())),
                )
                .unwrap();
            },
            _ => {
                tab.mount_pane_tabs(
                    PANE,
                    PaneTabsLayout {
                        active: 0,
                        tabs: ["Conversation", "Overview"]
                            .map(|label| PaneTabDescriptor {
                                label: label.into(),
                                location: "yinz:fixture-plugin-for-tests".into(),
                                configuration: BTreeMap::new(),
                                initial_cwd: None,
                            })
                            .to_vec(),
                    },
                    0,
                    CLIENT,
                )
                .unwrap();
            },
        }
        screen.agent_control(
            AgentRequest::CloseCompleted {
                pane_id: PANE,
                expected_session_id: Some("conversation".into()),
                submission_id: submission,
            },
            CLIENT,
        );
        let response = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(response.outcome, AgentOutcome::Rejected, "{composition}");
        assert_eq!(
            response.message.as_deref(),
            Some("worker cleanup would affect other panes")
        );
        let tab = screen.tab_containing_pane(PANE).unwrap();
        match composition {
            "editor" => assert!(tab.has_pane_with_pid(&editor)),
            "multipane" => assert!(tab.multipane_info(PANE).is_some()),
            _ => assert!(tab.pane_tabs_of(PANE).is_some()),
        }
    }
}

#[test]
fn controller_worker_stack_skips_hidden_workers() {
    use yinz_types::agent::control::AgentPlacement;

    for remaining_worker in [false, true] {
        for list_mode in [false, true] {
            let (mut screen, _, _) = setup("bash");
            *screen.stacked_pane_list.borrow_mut() = list_mode;
            let tab_id = screen.tab_id_containing_pane(PANE).unwrap();
            let first = PaneId::Terminal(TerminalId::new(88));
            let second = PaneId::Terminal(TerminalId::new(89));
            for id in [88, 89, 90] {
                if id == 89 && !remaining_worker {
                    continue;
                }
                if id == 90 {
                    screen
                        .tabs
                        .get_mut(&tab_id)
                        .unwrap()
                        .suppress_pane(first, Some(CLIENT));
                }
                let destination = screen
                    .resolve_agent_tab(AgentPlacement::ControllerStack(PANE), None, CLIENT)
                    .unwrap();
                let (tx, mut rx) = tokio::sync::oneshot::channel();
                let mut completion = crate::route::NotificationEnd::new(tx);
                completion.agent_tab = Some(destination);
                screen
                    .place_new_pane(
                        PaneId::Terminal(TerminalId::new(id)),
                        None,
                        None,
                        None,
                        NewPanePlacement::default(),
                        false,
                        crate::pty::ClientTabIndexOrPaneId::TabIndexNoFocus(tab_id),
                        Some(completion),
                        false,
                        &mut Default::default(),
                    )
                    .unwrap();
                assert!(rx.try_recv().unwrap().error_message.is_none());
            }
            let tab = &screen.tabs[&tab_id];
            let newest = PaneId::Terminal(TerminalId::new(90));
            assert!(tab.can_anchor_tiled_stack(newest));
            assert!(!tab.can_anchor_tiled_stack(first));
            if remaining_worker {
                assert!(tab.can_anchor_tiled_stack(second));
                let a = tab.get_pane_with_id(second).unwrap().position_and_size();
                let b = tab.get_pane_with_id(newest).unwrap().position_and_size();
                assert_eq!((a.x, a.cols), (b.x, b.cols));
            }
        }
    }
}

#[test]
fn worker_stack_anchor_requires_a_tiled_region() {
    for floating in [false, true] {
        let placement = if floating {
            NewPanePlacement::Floating(None)
        } else {
            NewPanePlacement::default()
        };
        let (mut screen, _, _) = setup_with_placement(
            "bash",
            PaneTarget::Local,
            Size { cols: 80, rows: 24 },
            placement,
        );
        *screen.stacked_pane_list.borrow_mut() = true;
        let tab = screen.tab_containing_pane_mut(PANE).unwrap();
        assert_eq!(tab.can_anchor_tiled_stack(PANE), !floating);
        tab.new_pane(
            PaneId::Terminal(TerminalId::new(88)),
            None,
            None,
            false,
            true, // Focus the new member so the original anchor is hidden.
            NewPanePlacement::Stacked {
                pane_id_to_stack_under: Some(PANE),
                borderless: None,
            },
            Some(CLIENT),
            None,
        )
        .unwrap();
        assert_eq!(tab.can_anchor_tiled_stack(PANE), !floating);
        if !floating {
            assert!(tab.pane_is_hidden_stack_list_member(&PANE));
        }
    }
    let (mut screen, _, _) = setup("bash");
    let tab = screen.tab_containing_pane_mut(PANE).unwrap();
    tab.replace_pane_with_editor_pane(PaneId::Terminal(TerminalId::new(99)), PANE)
        .unwrap();
    assert!(!tab.can_anchor_tiled_stack(PANE));
}

#[test]
fn controller_worker_stack_preserves_focused_member() {
    use yinz_types::agent::control::AgentPlacement;

    for list_mode in [false, true] {
        for focus_worker in [false, true] {
            let (mut screen, _, _) = setup("bash");
            *screen.stacked_pane_list.borrow_mut() = list_mode;
            let tab_id = screen.tab_id_containing_pane(PANE).unwrap();
            let focused = PaneId::Terminal(TerminalId::new(89));
            for id in [88, 89, 90] {
                if id == 90 && focus_worker {
                    screen
                        .tabs
                        .get_mut(&tab_id)
                        .unwrap()
                        .focus_pane_with_id(focused, false, false, CLIENT)
                        .unwrap();
                }
                let destination = screen
                    .resolve_agent_tab(AgentPlacement::ControllerStack(PANE), None, CLIENT)
                    .unwrap();
                let (tx, mut rx) = tokio::sync::oneshot::channel();
                let mut completion = crate::route::NotificationEnd::new(tx);
                completion.agent_tab = Some(destination);
                screen
                    .place_new_pane(
                        PaneId::Terminal(TerminalId::new(id)),
                        None,
                        None,
                        None,
                        NewPanePlacement::default(),
                        false,
                        crate::pty::ClientTabIndexOrPaneId::TabIndexNoFocus(tab_id),
                        Some(completion),
                        false,
                        &mut Default::default(),
                    )
                    .unwrap();
                assert!(rx.try_recv().unwrap().error_message.is_none());
                screen.render().unwrap();
            }
            let expected = if focus_worker { focused } else { PANE };
            assert_eq!(
                screen.tabs[&tab_id].get_active_pane_id(CLIENT),
                Some(expected)
            );
            assert!(!screen.get_pane_info(expected).unwrap().is_suppressed);
            if list_mode {
                assert!(!screen.tabs[&tab_id]
                    .get_pane_with_id(expected)
                    .unwrap()
                    .position_and_size()
                    .rows
                    .is_fixed());
            }
        }
    }
}

#[test]
fn controller_worker_stack_respects_first_worker_pane_limit() {
    use yinz_types::agent::control::AgentPlacement;

    for limit in [1, 2] {
        let (mut screen, _, _) = setup_with_pane_limit(
            "bash",
            PaneTarget::Local,
            Size { cols: 80, rows: 24 },
            NewPanePlacement::default(),
            Some(limit),
        );
        let tab_id = screen.tab_id_containing_pane(PANE).unwrap();
        let worker = PaneId::Terminal(TerminalId::new(88));
        let destination = screen
            .resolve_agent_tab(AgentPlacement::ControllerStack(PANE), None, CLIENT)
            .unwrap();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        let mut completion = crate::route::NotificationEnd::new(tx);
        completion.agent_tab = Some(destination);
        let result = screen.place_new_pane(
            worker,
            None,
            None,
            None,
            NewPanePlacement::default(),
            false,
            crate::pty::ClientTabIndexOrPaneId::TabIndexNoFocus(tab_id),
            Some(completion),
            false,
            &mut Default::default(),
        );
        assert_eq!(result.is_err(), limit == 1);
        assert_eq!(rx.try_recv().unwrap().error_message.is_some(), limit == 1);
        assert_eq!(screen.tabs[&tab_id].get_tiled_pane_ids().len(), limit);
        assert_eq!(screen.tabs[&tab_id].get_active_pane_id(CLIENT), Some(PANE));
        assert!(screen.get_pane_info(PANE).is_some());
        assert_eq!(screen.get_pane_info(worker).is_some(), limit == 2);
        assert_eq!(
            screen
                .agent_worker_stacks
                .get(&(PANE, tab_id))
                .is_some_and(|members| members.contains(&worker)),
            limit == 2
        );
    }
}

#[test]
fn controller_worker_stack_respects_subsequent_worker_pane_limit() {
    use yinz_types::agent::control::AgentPlacement;

    for limit in [2, 3] {
        let (mut screen, _, _) = setup_with_pane_limit(
            "bash",
            PaneTarget::Local,
            Size { cols: 80, rows: 24 },
            NewPanePlacement::default(),
            Some(limit),
        );
        let tab_id = screen.tab_id_containing_pane(PANE).unwrap();
        let (to_pty, pty) = channels::unbounded();
        screen.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));
        screen.tabs.get_mut(&tab_id).unwrap().senders.to_pty = screen.bus.senders.to_pty.clone();
        let mut workers = vec![];
        for index in 0..limit {
            let worker = PaneId::Terminal(TerminalId::new(88 + index as u32));
            let destination = screen
                .resolve_agent_tab(AgentPlacement::ControllerStack(PANE), None, CLIENT)
                .unwrap();
            let (tx, mut rx) = tokio::sync::oneshot::channel();
            let mut completion = crate::route::NotificationEnd::new(tx);
            completion.agent_tab = Some(destination);
            let result = screen.place_new_pane(
                worker,
                None,
                None,
                None,
                NewPanePlacement::default(),
                false,
                crate::pty::ClientTabIndexOrPaneId::TabIndexNoFocus(tab_id),
                Some(completion),
                false,
                &mut Default::default(),
            );
            let at_capacity = index == limit - 1;
            assert_eq!(result.is_err(), at_capacity);
            let completion = rx.try_recv().unwrap();
            if at_capacity {
                assert!(completion.error_message.unwrap().contains("PaneLimit"));
                assert!(screen.get_pane_info(worker).is_none());
            } else {
                assert!(completion.error_message.is_none());
                workers.push(worker);
            }
            assert_eq!(screen.agent_worker_stacks[&(PANE, tab_id)], workers);
            assert_eq!(screen.tabs[&tab_id].get_active_pane_id(CLIENT), Some(PANE));
            assert!(screen.get_pane_info(PANE).is_some());
            for existing in &workers {
                assert!(screen.get_pane_info(*existing).is_some());
            }
            assert!(pty.try_iter().all(|(instruction, _)| match instruction {
                PtyInstruction::ClosePane(id, _) => id != PANE && !workers.contains(&id),
                PtyInstruction::CloseTab(ids) => {
                    !ids.contains(&PANE) && workers.iter().all(|id| !ids.contains(id))
                },
                _ => true,
            }));
        }
    }
}

fn recovered_prompt(id: &str, predecessor: &str) -> HookEvent {
    HookEvent {
        hook_event_name: "UserPromptSubmit".into(),
        session_id: "conversation".into(),
        prompt_id: Some(id.into()),
        claude_recovered_after: Some(predecessor.into()),
        ..Default::default()
    }
}

#[test]
fn claude_queued_prompt_recovery_finishes_the_missed_hook_fixture() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../yinz-process/src/unit/fixtures/claude-missed-queued-prompt.json"
    ))
    .unwrap();
    for background in [false, true] {
        let (mut screen, _, _) = setup("claude");
        for (index, raw) in fixture["reports"].as_array().unwrap().iter().enumerate() {
            let mut hook: HookEvent = serde_json::from_value(raw.clone()).unwrap();
            if background && index == 5 {
                hook.claude_background = Some(true);
            }
            report_kind(&mut screen, "claude", hook);
            let expected = if index == 1 || (index == 5 && !background) {
                AgentState::Idle
            } else {
                AgentState::Busy
            };
            assert_eq!(
                screen.agent_status_for_test().agent.state,
                expected,
                "report {index}, background {background}"
            );
        }
        if background {
            report_kind(
                &mut screen,
                "claude",
                HookEvent {
                    claude_recovered_after: Some("prompt".into()),
                    ..background_idle("queued")
                },
            );
            assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
        }
        let lifecycle = screen
            .capture_agent_tracking(PANE)
            .unwrap()
            .lifecycle
            .unwrap();
        assert_eq!(lifecycle.claude_prompt_id.as_deref(), Some("queued"));
        assert_eq!(
            lifecycle.last_message.as_deref(),
            Some("Second task finished.")
        );
    }
}

#[test]
fn claude_queued_prompt_recovery_deduplicates_native_hook_races_and_late_replay() {
    for native_first in [false, true] {
        let (mut screen, _, _) = setup("claude");
        report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
        report_kind(&mut screen, "claude", claude_stop("prompt", "first", false));
        let native = steering_ack("queued");
        let recovered = recovered_prompt("queued", "prompt");
        let (first, second) = if native_first {
            (native, recovered.clone())
        } else {
            (recovered.clone(), native)
        };
        report_kind(&mut screen, "claude", first);
        report_kind(&mut screen, "claude", claude_stop("queued", "final", false));
        for duplicate in [second, recovered.clone(), recovered] {
            report_kind(&mut screen, "claude", duplicate);
            assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
            let lifecycle = screen
                .capture_agent_tracking(PANE)
                .unwrap()
                .lifecycle
                .unwrap();
            assert_eq!(lifecycle.last_message.as_deref(), Some("final"));
            assert_eq!(lifecycle.claude.turns.len(), 2);
        }
    }
}

#[test]
fn claude_queued_prompt_recovery_rejects_stale_conversations_predecessors_and_identities() {
    for change in [
        "newer_prompt",
        "conversation",
        "empty",
        "missing",
        "predecessor",
        "worker",
    ] {
        let (mut screen, _, _) = setup("claude");
        report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
        report_kind(
            &mut screen,
            "claude",
            claude_stop("prompt", "original", false),
        );
        let mut recovered = recovered_prompt("queued", "prompt");
        match change {
            "newer_prompt" => {
                report_kind(&mut screen, "claude", steering_ack("newer"));
                report_kind(&mut screen, "claude", claude_stop("newer", "newest", false));
            },
            "conversation" => report_kind(
                &mut screen,
                "claude",
                HookEvent {
                    session_id: "replacement".into(),
                    ..hook("SessionStart")
                },
            ),
            "empty" => recovered.prompt_id = Some(" ".into()),
            "missing" => recovered.prompt_id = None,
            "predecessor" => recovered.claude_recovered_after = Some("wrong".into()),
            "worker" => recovered.agent_id = Some("worker".into()),
            _ => unreachable!(),
        }
        let before = screen
            .capture_agent_tracking(PANE)
            .unwrap()
            .lifecycle
            .unwrap();
        report_kind(&mut screen, "claude", recovered.clone());
        recovered.hook_event_name = "Stop".into();
        recovered.claude_turn_complete = Some(true);
        recovered.last_assistant_message = Some("stale response".into());
        report_kind(&mut screen, "claude", recovered);
        let after = screen
            .capture_agent_tracking(PANE)
            .unwrap()
            .lifecycle
            .unwrap();
        assert_eq!(after.claude_prompt_id, before.claude_prompt_id, "{change}");
        assert_eq!(after.last_message, before.last_message, "{change}");
        assert_eq!(after.conversation, before.conversation, "{change}");
        assert_eq!(
            after.claude.turns.len(),
            before.claude.turns.len(),
            "{change}"
        );
        assert_eq!(
            screen.agent_status_for_test().agent.state,
            AgentState::Idle,
            "{change}"
        );
    }
}

#[test]
fn claude_queued_prompt_recovery_cannot_acknowledge_pending_input_or_consume_a_draft() {
    for newer_draft in [false, true] {
        let (mut screen, messages, writes) = setup("claude");
        report_kind(&mut screen, "claude", hook("SessionStart"));
        screen.agent_control_with_clear(identified_prompt("initial", true), CLIENT, &writes);
        finish_paste(&writes);
        report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
        report_kind(&mut screen, "claude", claude_stop("prompt", "first", false));
        replies(&messages, CLIENT);
        if newer_draft {
            screen.agent_write_for_test();
            draft_frame(&mut screen, "claude", "new draft", 9, Instant::now());
        } else {
            deliver_steering(&mut screen, &writes, "pending", CLIENT);
        }
        let revision = screen.tabs[&TabId::new(0)].agent_input_revision(PANE);
        // Even identical prose is not correlation evidence in a recovered report.
        let mut recovered = recovered_prompt("queued", "prompt");
        recovered.prompt = Some("steer".into());
        report_kind(&mut screen, "claude", recovered.clone());
        recovered.hook_event_name = "Stop".into();
        recovered.claude_turn_complete = Some(true);
        recovered.last_assistant_message = Some("recovered response".into());
        report_kind(&mut screen, "claude", recovered);
        let responses = replies(&messages, CLIENT);
        assert!(responses.is_empty(), "{responses:?}");
        if !newer_draft {
            let pending = screen.agent_submissions[&PANE]
                .iter()
                .find(|submission| submission.id == "pending")
                .unwrap();
            assert!(!pending.acknowledged);
            assert!(pending.result.is_none());
        }
        assert_eq!(
            screen.tabs[&TabId::new(0)].agent_input_revision(PANE),
            revision
        );
        assert_ne!(
            screen
                .capture_agent_tracking(PANE)
                .unwrap()
                .lifecycle
                .unwrap()
                .input_revision,
            Some(revision)
        );
        if newer_draft {
            assert_eq!(
                screen.agent_status_for_test().not_ready_reason,
                Some(NotReadyReason::UnconsumedInput)
            );
            assert!(screen.tabs[&TabId::new(0)]
                .get_pane_with_id(PANE)
                .unwrap()
                .dump_screen(false, Some(CLIENT))
                .contains("new draft"));
        }
    }
}

#[test]
fn claude_queued_prompt_recovery_preserves_permission_wait_and_event_state() {
    let (mut screen, _, _) = setup("claude");
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    report_kind(&mut screen, "claude", claude_stop("prompt", "first", false));
    report_kind(&mut screen, "claude", claude_permission_prompt());
    let before = screen.get_pane_info(PANE).unwrap().agent.unwrap();
    let revision = screen.claude_permission_waits[&PANE].input_revision;
    let (sender, plugins) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
    report_kind(&mut screen, "claude", recovered_prompt("queued", "prompt"));
    assert_eq!(screen.get_pane_info(PANE).unwrap().agent.unwrap(), before);
    assert_eq!(
        screen.claude_permission_waits[&PANE].input_revision,
        revision
    );
    assert_eq!(
        screen.claude_permission_waits[&PANE].conversation,
        "conversation"
    );
    let lifecycle = screen
        .capture_agent_tracking(PANE)
        .unwrap()
        .lifecycle
        .unwrap();
    assert_eq!(lifecycle.claude_prompt_id.as_deref(), Some("queued"));
    assert_eq!(lifecycle.claude.parent.unwrap().state, AgentState::Waiting);
    let events: Vec<_> = plugins
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            PluginInstruction::Update(updates) => updates,
            _ => vec![],
        })
        .filter_map(|(_, _, event)| match event {
            Event::AgentEvent(event) => Some(event),
            _ => None,
        })
        .collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].state, Some(AgentState::Waiting));
}

#[test]
fn claude_queued_prompt_recovery_native_evidence_correlates_once_without_reopening() {
    for native_first in [false, true] {
        for late in [false, true] {
            for matches in [false, true] {
                let (mut screen, messages, writes) = setup("claude");
                report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
                report_kind(&mut screen, "claude", claude_stop("prompt", "first", false));
                deliver_steering(&mut screen, &writes, "pending", CLIENT);
                replies(&messages, CLIENT);
                let (sender, plugins) = channels::unbounded();
                screen.bus.senders.to_plugin = Some(SenderWithContext::new(sender));
                let mut native = steering_ack("queued");
                if !matches {
                    native.prompt = Some("different input".into());
                }
                let recovered = recovered_prompt("queued", "prompt");
                let (first, second) = if native_first {
                    (native.clone(), recovered)
                } else {
                    (recovered, native.clone())
                };
                report_kind(&mut screen, "claude", first);
                if !native_first {
                    assert!(
                        !screen.agent_submissions[&PANE]
                            .iter()
                            .find(|s| s.id == "pending")
                            .unwrap()
                            .acknowledged
                    );
                }
                if late {
                    report_kind(&mut screen, "claude", claude_stop("queued", "final", false));
                }
                report_kind(&mut screen, "claude", second);
                report_kind(&mut screen, "claude", native.clone());
                report_kind(&mut screen, "claude", native);
                assert_eq!(
                    screen.agent_submissions[&PANE]
                        .iter()
                        .find(|s| s.id == "pending")
                        .unwrap()
                        .acknowledged,
                    matches
                );
                if !late {
                    report_kind(&mut screen, "claude", claude_stop("queued", "final", false));
                    let responses = replies(&messages, CLIENT);
                    assert_eq!(
                        responses.len(),
                        1,
                        "native_first={native_first} late={late} matches={matches} {responses:?}"
                    );
                    assert_eq!(responses[0].acknowledged, matches);
                    assert_eq!(
                        responses[0].outcome,
                        if matches {
                            AgentOutcome::Completed
                        } else {
                            AgentOutcome::Interrupted
                        },
                        "native_first={native_first} late={late} matches={matches} {responses:?}"
                    );
                }
                // Capturing a handoff retires pending callers, so inspect it only after
                // checking the submission replies above.
                let lifecycle = screen
                    .capture_agent_tracking(PANE)
                    .unwrap()
                    .lifecycle
                    .unwrap();
                assert_eq!(lifecycle.claude.turns.len(), 2);
                assert_eq!(lifecycle.claude.sequence, 2);
                assert!(!lifecycle.claude.turns["queued"].awaiting_native_submit);
                assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
                assert_eq!(lifecycle.claude_confirmation, Some(true));
                assert_eq!(lifecycle.last_message.as_deref(), Some("final"));
                let accepts = plugins.try_iter().flat_map(|(instruction, _)| match instruction {
                    PluginInstruction::Update(updates) => updates,
                    _ => vec![],
                }).filter(|(_, _, event)| matches!(event, Event::AgentEvent(event) if event.hook == "UserPromptSubmit")).count();
                assert_eq!(accepts, 1);
            }
        }
    }
}

#[test]
fn claude_queued_prompt_recovery_native_replay_cannot_acknowledge_later_identical_input() {
    for native_first in [false, true] {
        let (mut screen, messages, writes) = setup("claude");
        report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
        let native = steering_ack("queued");
        let recovered = recovered_prompt("queued", "prompt");
        let reports = if native_first {
            [native.clone(), recovered]
        } else {
            [recovered, native.clone()]
        };
        for report in reports {
            report_kind(&mut screen, "claude", report);
        }
        report_kind(&mut screen, "claude", claude_stop("queued", "final", false));
        deliver_steering(&mut screen, &writes, "later", CLIENT);
        replies(&messages, CLIENT);
        report_kind(&mut screen, "claude", native);
        let later = screen.agent_submissions[&PANE]
            .iter()
            .find(|s| s.id == "later")
            .unwrap();
        assert!(!later.acknowledged);
        assert!(later.result.is_none());
        assert!(replies(&messages, CLIENT).is_empty());
    }
}

fn launch_transaction_worker(
    screen: &mut Screen,
    id: u32,
) -> (
    std::sync::Arc<yinz_bus::agent_launch::LaunchGate>,
    crate::route::ActionCompletionResult,
) {
    launch_transaction_worker_after_resolve(screen, id, |_| {})
}

fn launch_transaction_worker_after_resolve(
    screen: &mut Screen,
    id: u32,
    before_admission: impl FnOnce(&mut Screen),
) -> (
    std::sync::Arc<yinz_bus::agent_launch::LaunchGate>,
    crate::route::ActionCompletionResult,
) {
    let terminal = TerminalId::new(id);
    let gate = std::sync::Arc::new(yinz_bus::agent_launch::LaunchGate::new(
        format!("test.{id}"),
        false,
    ));
    gate.allocated(terminal);
    screen
        .bus
        .senders
        .launches
        .gates
        .lock()
        .unwrap()
        .insert(terminal, gate.clone());
    let destination = screen
        .resolve_agent_tab(
            yinz_types::agent::control::AgentPlacement::ControllerStack(PANE),
            None,
            CLIENT,
        )
        .unwrap();
    let tab = destination.tab_id;
    before_admission(screen);
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    let mut completion = crate::route::NotificationEnd::new(tx);
    completion.agent_tab = Some(destination);
    completion.launch = Some(gate.clone());
    let _ = screen.place_new_pane(
        PaneId::Terminal(terminal),
        None,
        None,
        None,
        NewPanePlacement::default(),
        false,
        crate::pty::ClientTabIndexOrPaneId::TabIndexNoFocus(tab),
        Some(completion),
        false,
        &mut Default::default(),
    );
    (gate, rx.try_recv().unwrap())
}

#[test]
fn launch_transaction_destination_rejection_has_typed_reason_and_no_side_effects() {
    use yinz_types::agent::{control::AgentPlacement, launch::LaunchReason};
    for (placement, expected) in [
        (
            AgentPlacement::ControllerStack(PaneId::Terminal(TerminalId::new(999))),
            LaunchReason::ControllerClosed,
        ),
        (
            AgentPlacement::Tab(TabId::new(999)),
            LaunchReason::DestinationClosed,
        ),
    ] {
        let (mut screen, _, _) = setup("bash");
        let before = screen.tabs.keys().copied().collect::<Vec<_>>();
        let focus = screen.active_tab_ids.clone();
        let gate = yinz_bus::agent_launch::LaunchGate::new("test.destination".into(), false);
        assert!(screen
            .resolve_launch_tab(placement, None, CLIENT, None, &gate)
            .is_err());
        assert_eq!(gate.snapshot().revoked, Some(expected));
        assert!(gate.snapshot().destination.is_none());
        assert_eq!(screen.tabs.keys().copied().collect::<Vec<_>>(), before);
        assert_eq!(screen.active_tab_ids, focus);
    }
}

#[test]
fn launch_transaction_controller_move_follows_before_admission_and_revokes_after_it() {
    for move_before_admission in [false, true] {
        let (mut screen, _, _) = setup("bash");
        new_tab(&mut screen, 2, TabId::new(1));
        let move_controller = |screen: &mut Screen| {
            let moved = screen
                .break_multiple_panes_to_tab_with_id(vec![PANE], TabId::new(1), false, CLIENT)
                .unwrap();
            assert!(moved.succeeded(), "{moved:?}");
        };
        let (gate, result) = launch_transaction_worker_after_resolve(&mut screen, 88, |screen| {
            if move_before_admission {
                move_controller(screen);
            }
        });
        assert!(result.error_message.is_none(), "{result:?}");
        let slot = gate.snapshot().slot.unwrap();
        assert_eq!(slot.tab_id, TabId::new(usize::from(move_before_admission)));
        if !move_before_admission {
            move_controller(&mut screen);
        }
        screen.revalidate_launches();
        if move_before_admission {
            assert!(screen.validate_launch_slot(TerminalId::new(88)).is_ok());
            assert!(gate.claim());
        } else {
            assert_eq!(
                gate.snapshot().revoked,
                Some(yinz_types::agent::launch::LaunchReason::ControllerMoved)
            );
            assert!(!gate.claim());
        }
        assert_eq!(
            screen.tab_id_containing_pane(PaneId::Terminal(TerminalId::new(88))),
            Some(slot.tab_id)
        );
    }
}

#[test]
fn launch_transaction_floating_controller_admits_first_worker_in_empty_tiled_layer() {
    for list_mode in [false, true] {
        let (mut screen, _, _) = setup_with_placement(
            "bash",
            PaneTarget::Local,
            Size { cols: 80, rows: 24 },
            NewPanePlacement::Floating(None),
        );
        *screen.stacked_pane_list.borrow_mut() = list_mode;
        screen.close_pane_explicitly(PaneId::Terminal(TerminalId::new(1)), None);
        let tab_id = screen.tab_id_containing_pane(PANE).unwrap();
        assert!(screen.tabs[&tab_id].get_tiled_pane_ids().is_empty());
        let controller = screen.tabs[&tab_id]
            .get_pane_with_id(PANE)
            .unwrap()
            .position_and_size();
        for id in [88, 89] {
            let (gate, result) = launch_transaction_worker(&mut screen, id);
            assert!(result.error_message.is_none(), "{result:?}");
            let slot = gate.snapshot().slot.unwrap();
            assert!(screen.tabs[&tab_id].can_anchor_tiled_stack(slot.pane_id));
            assert!(screen.validate_launch_slot(TerminalId::new(id)).is_ok());
            assert_eq!(screen.tabs[&tab_id].get_active_pane_id(CLIENT), Some(PANE));
            assert_eq!(
                screen.tabs[&tab_id]
                    .get_pane_with_id(PANE)
                    .unwrap()
                    .position_and_size(),
                controller
            );
        }
        let workers = [88, 89].map(|id| PaneId::Terminal(TerminalId::new(id)));
        assert_eq!(screen.agent_worker_stacks[&(PANE, tab_id)], workers);
        let mut members = screen
            .tabs
            .get_mut(&tab_id)
            .unwrap()
            .worker_region_members(workers[0]);
        members.sort();
        assert_eq!(members, workers);
    }
}

#[test]
fn launch_transaction_empty_tiled_worker_checks_capacity_and_size() {
    use yinz_types::agent::launch::LaunchReason;

    for (limit, size, expected) in [
        (
            Some(0),
            Size { cols: 80, rows: 24 },
            LaunchReason::PaneLimit,
        ),
        (
            None,
            Size { cols: 4, rows: 24 },
            LaunchReason::InsufficientRoom,
        ),
        (
            None,
            Size { cols: 80, rows: 4 },
            LaunchReason::InsufficientRoom,
        ),
    ] {
        let (mut screen, _, _) = setup_with_pane_limit(
            "bash",
            PaneTarget::Local,
            Size { cols: 80, rows: 24 },
            NewPanePlacement::Floating(None),
            limit,
        );
        if limit.is_none() {
            screen.close_pane_explicitly(PaneId::Terminal(TerminalId::new(1)), None);
        }
        screen.resize_to_screen(size).unwrap();
        let tab_id = screen.tab_id_containing_pane(PANE).unwrap();
        let focus = screen.tabs[&tab_id].get_active_pane_id(CLIENT);
        let (gate, result) = launch_transaction_worker(&mut screen, 88);
        assert!(result.error_message.is_some());
        assert_eq!(gate.snapshot().revoked, Some(expected));
        assert!(gate.snapshot().slot.is_none());
        assert!(screen.tabs[&tab_id].get_tiled_pane_ids().is_empty());
        assert_eq!(screen.tabs[&tab_id].get_active_pane_id(CLIENT), focus);
        assert!(screen.get_pane_info(PANE).is_some());
        assert!(screen
            .get_pane_info(PaneId::Terminal(TerminalId::new(88)))
            .is_none());
    }
}

#[test]
fn launch_transaction_floating_and_overlay_controllers_keep_both_clients_focus() {
    for placement in [
        NewPanePlacement::Floating(None),
        NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
    ] {
        let (mut screen, _, _) = setup_with_placement(
            "bash",
            PaneTarget::Local,
            Size {
                cols: 105,
                rows: 59,
            },
            placement,
        );
        let other = ClientId::new(2);
        screen.add_client(other).unwrap();
        let tab_id = screen.tab_id_containing_pane(PANE).unwrap();
        let focus = [CLIENT, other].map(|client| screen.tabs[&tab_id].get_active_pane_id(client));
        let active_tabs = screen.active_tab_ids.clone();
        let controller = screen.tabs[&tab_id]
            .get_pane_with_id(PANE)
            .unwrap()
            .position_and_size();
        let overlays = screen.tabs[&tab_id].overlays_are_rendered();
        for id in [88, 89] {
            let (gate, result) = launch_transaction_worker(&mut screen, id);
            assert!(result.error_message.is_none(), "{result:?}");
            let slot = gate.snapshot().slot.unwrap();
            assert_eq!(slot.tab_id, tab_id);
            assert!(screen.validate_launch_slot(TerminalId::new(id)).is_ok());
            assert_eq!(
                [CLIENT, other].map(|client| screen.tabs[&tab_id].get_active_pane_id(client)),
                focus
            );
            assert_eq!(screen.active_tab_ids, active_tabs);
            assert_eq!(
                screen.tabs[&tab_id]
                    .get_pane_with_id(PANE)
                    .unwrap()
                    .position_and_size(),
                controller
            );
            assert_eq!(screen.tabs[&tab_id].overlays_are_rendered(), overlays);
            assert!(!screen
                .tabs
                .get_mut(&tab_id)
                .unwrap()
                .worker_region_members(slot.pane_id)
                .contains(&PANE));
        }
    }
}

#[test]
fn launch_transaction_splits_only_controller_stack_in_incident_geometry() {
    for list_mode in [false, true] {
        let (mut screen, _, _) = setup_with_size(
            "bash",
            PaneTarget::Local,
            Size {
                cols: 105,
                rows: 59,
            },
        );
        *screen.stacked_pane_list.borrow_mut() = list_mode;
        screen.close_pane_explicitly(PaneId::Terminal(TerminalId::new(1)), None);
        let tab_id = screen.tab_id_containing_pane(PANE).unwrap();
        let old = PaneId::Terminal(TerminalId::new(87));
        screen
            .tabs
            .get_mut(&tab_id)
            .unwrap()
            .new_pane(
                old,
                None,
                None,
                false,
                false,
                NewPanePlacement::Stacked {
                    pane_id_to_stack_under: Some(PANE),
                    borderless: None,
                },
                None,
                None,
            )
            .unwrap();
        assert_eq!(
            screen
                .tabs
                .get_mut(&tab_id)
                .unwrap()
                .worker_region_members(PANE)
                .len(),
            2
        );
        for id in [88, 89] {
            let (gate, result) = launch_transaction_worker(&mut screen, id);
            assert!(result.error_message.is_none(), "{result:?}");
            let slot = gate.snapshot().slot.unwrap();
            assert!(slot.worker_region.is_some());
            let size = screen.validate_launch_slot(TerminalId::new(id)).unwrap();
            assert!(size.cols > 1 && size.rows > 1, "{size:?}");
            assert_eq!(screen.tabs[&tab_id].get_active_pane_id(CLIENT), Some(PANE));
            assert_eq!(
                screen
                    .tabs
                    .get_mut(&tab_id)
                    .unwrap()
                    .worker_region_members(PANE)
                    .len(),
                2
            );
            assert!(screen
                .tabs
                .get_mut(&tab_id)
                .unwrap()
                .worker_region_members(slot.pane_id)
                .iter()
                .all(|id| ![PANE, old].contains(id)));
        }
    }
}

#[test]
fn launch_transaction_rejected_admission_keeps_layout_and_has_no_hidden_orphan() {
    for (limit, fullscreen) in [(None, false), (Some(1), false), (None, true)] {
        let (mut screen, _, _) = setup_with_pane_limit(
            "bash",
            PaneTarget::Local,
            Size { cols: 80, rows: 24 },
            NewPanePlacement::default(),
            limit,
        );
        if !fullscreen {
            screen.close_pane_explicitly(PaneId::Terminal(TerminalId::new(1)), None);
        }
        screen.resize_to_screen(Size { cols: 6, rows: 5 }).unwrap();
        let tab = screen.tab_id_containing_pane(PANE).unwrap();
        if fullscreen {
            screen
                .tabs
                .get_mut(&tab)
                .unwrap()
                .toggle_pane_fullscreen(PANE);
        }
        assert_eq!(screen.tabs[&tab].is_fullscreen_active(), fullscreen);
        let geom = screen.tabs[&tab]
            .get_pane_with_id(PANE)
            .unwrap()
            .position_and_size();
        let (gate, result) = launch_transaction_worker(&mut screen, 88);
        assert!(result.error_message.is_some());
        assert!(gate.snapshot().slot.is_none());
        assert!(!gate.claim());
        assert_eq!(
            gate.snapshot().revoked,
            Some(if limit.is_some() {
                yinz_types::agent::launch::LaunchReason::PaneLimit
            } else {
                yinz_types::agent::launch::LaunchReason::InsufficientRoom
            })
        );
        assert!(screen
            .get_pane_info(PaneId::Terminal(TerminalId::new(88)))
            .is_none());
        assert_eq!(
            screen.tabs[&tab]
                .get_pane_with_id(PANE)
                .unwrap()
                .position_and_size(),
            geom
        );
        assert_eq!(screen.tabs[&tab].get_active_pane_id(CLIENT), Some(PANE));
        assert_eq!(screen.tabs[&tab].is_fullscreen_active(), fullscreen);
    }
}

#[test]
fn launch_transaction_region_survives_root_close_and_hidden_list_members() {
    for list_mode in [false, true] {
        let (mut screen, _, _) = setup("bash");
        *screen.stacked_pane_list.borrow_mut() = list_mode;
        let (first, _) = launch_transaction_worker(&mut screen, 88);
        let (second, result) = launch_transaction_worker(&mut screen, 89);
        assert!(result.error_message.is_none(), "{result:?}");
        let region = second.snapshot().slot.unwrap().worker_region;
        assert_eq!(first.snapshot().slot.unwrap().worker_region, region);
        screen.close_pane_explicitly(PaneId::Terminal(TerminalId::new(88)), None);
        screen.revalidate_launches();
        assert!(screen.validate_launch_slot(TerminalId::new(89)).is_ok());
        let (third, result) = launch_transaction_worker(&mut screen, 90);
        assert!(result.error_message.is_none(), "{result:?}");
        assert_eq!(third.snapshot().slot.unwrap().worker_region, region);
        assert_eq!(
            screen.tabs[&TabId::new(0)].get_active_pane_id(CLIENT),
            Some(PANE)
        );
    }
}

#[test]
fn launch_transaction_dissolved_group_cannot_revive_completed_region_ownership() {
    let (mut screen, _, _) = setup("bash");
    let (first, _) = launch_transaction_worker(&mut screen, 88);
    let (second, result) = launch_transaction_worker(&mut screen, 89);
    assert!(result.error_message.is_none());
    for gate in [&first, &second] {
        assert!(gate.claim());
        gate.running();
        gate.registered();
        assert!(gate.handoff());
    }
    let key = (PANE, TabId::new(0));
    let original_region = second.snapshot().slot.unwrap().worker_region.unwrap();
    let mut original_geoms = Vec::new();
    for id in [88, 89] {
        let pane = screen
            .tabs
            .get_mut(&key.1)
            .unwrap()
            .get_pane_with_id_mut(PaneId::Terminal(TerminalId::new(id)))
            .unwrap();
        let original = pane.position_and_size();
        original_geoms.push((id, original));
        let mut standalone = original;
        standalone.stacked = None;
        pane.set_geom(standalone);
    }
    screen.revalidate_launches();
    assert!(!screen.agent_worker_regions.contains_key(&key));
    // Even identical panes and recycled classic stack IDs cannot resurrect retired ownership.
    for (id, original) in original_geoms {
        screen
            .tabs
            .get_mut(&key.1)
            .unwrap()
            .get_pane_with_id_mut(PaneId::Terminal(TerminalId::new(id)))
            .unwrap()
            .set_geom(original);
    }
    let (third, result) = launch_transaction_worker(&mut screen, 90);
    assert!(result.error_message.is_none(), "{result:?}");
    assert_ne!(
        third.snapshot().slot.unwrap().worker_region,
        Some(original_region)
    );
    assert_eq!(screen.tabs[&key.1].get_active_pane_id(CLIENT), Some(PANE));
}

#[test]
fn launch_transaction_only_confirmed_warp_departure_retires_worker_ownership() {
    for whole_tab in [false, true] {
        let (mut screen, _, _) = setup("bash");
        let terminal = TerminalId::new(88);
        let pane = PaneId::Terminal(terminal);
        let (gate, result) = launch_transaction_worker(&mut screen, terminal.get());
        assert!(result.error_message.is_none());
        assert!(gate.claim());
        gate.running();
        gate.registered();
        assert!(gate.handoff());
        let released = |id| {
            Ok(released_for_warp(ReleasedPane::Local(ReleasedPtyHost {
                host_id: yinz_types::internal_ids::SupervisorId::new(format!("worker-{id}")),
                host_pid: ProcessId::new(1),
                control_socket: Some(PathBuf::from(format!("/tmp/worker-{id}.sock"))),
            })))
        };
        if whole_tab {
            let (tab, _, prepared) = screen
                .check_tab_can_be_warped(Some(TabId::new(0)), CLIENT)
                .unwrap();
            assert!(screen
                .take_released_tab_for_warp(
                    tab,
                    vec![(terminal, Err("release failed".into()))],
                    prepared
                )
                .is_err());
        } else {
            assert!(screen
                .take_released_pane_for_warp(terminal, Err("release failed".into()))
                .is_err());
        }
        assert!(!gate.snapshot().departed);
        assert!(screen.get_pane_info(pane).is_some());
        if whole_tab {
            let (tab, terminals, prepared) = screen
                .check_tab_can_be_warped(Some(TabId::new(0)), CLIENT)
                .unwrap();
            let addresses = terminals.into_iter().map(|id| (id, released(id))).collect();
            screen
                .take_released_tab_for_warp(tab, addresses, prepared)
                .unwrap();
        } else {
            screen
                .take_released_pane_for_warp(terminal, released(terminal))
                .unwrap();
        }
        assert!(screen.get_pane_info(pane).is_none());
        let state = gate.snapshot();
        assert!(state.departed);
        assert!(!state.exited && !state.cleanup_started && !state.cleanup_confirmed);
    }
}

#[test]
fn launch_transaction_controller_close_revokes_pending_but_preserves_committed_workers() {
    for committed in [false, true] {
        let (mut screen, _, _) = setup("bash");
        let (gate, result) = launch_transaction_worker(&mut screen, 88);
        assert!(result.error_message.is_none());
        if committed {
            assert!(gate.claim());
            gate.running();
            gate.registered();
            assert!(gate.handoff());
        }
        screen.close_pane_explicitly(PANE, None);
        screen.revalidate_launches();
        assert_eq!(gate.snapshot().revoked.is_none(), committed);
        if !committed {
            assert!(!gate.claim());
        } else {
            assert!(screen
                .get_pane_info(PaneId::Terminal(TerminalId::new(88)))
                .is_some());
        }
    }
}

#[test]
fn launch_transaction_pending_placeholders_serialize_capacity_and_first_region_creation() {
    for limit in [2, 3] {
        let (mut screen, _, _) = setup_with_pane_limit(
            "bash",
            PaneTarget::Local,
            Size {
                cols: 105,
                rows: 59,
            },
            NewPanePlacement::default(),
            Some(limit),
        );
        let (first, result) = launch_transaction_worker(&mut screen, 88);
        assert!(result.error_message.is_none());
        let (second, result) = launch_transaction_worker(&mut screen, 89);
        assert_eq!(result.error_message.is_none(), limit == 3);
        if limit == 3 {
            assert_eq!(
                first.snapshot().slot.unwrap().worker_region,
                second.snapshot().slot.unwrap().worker_region
            );
            assert!(
                screen
                    .validate_launch_slot(TerminalId::new(88))
                    .unwrap()
                    .rows
                    > 1
            );
        } else {
            assert!(!second.claim());
            assert_eq!(
                second.snapshot().revoked,
                Some(yinz_types::agent::launch::LaunchReason::PaneLimit)
            );
        }
    }
}

#[test]
fn launch_transaction_changed_group_or_tab_incarnation_cannot_authorize_start() {
    for reused_tab in [false, true] {
        let (mut screen, _, _) = setup("bash");
        let (gate, result) = launch_transaction_worker(&mut screen, 88);
        assert!(result.error_message.is_none());
        let slot = gate.snapshot().slot.unwrap();
        if reused_tab {
            let old = screen.tabs.remove(&slot.tab_id).unwrap();
            new_tab(&mut screen, 1, slot.tab_id);
            assert_ne!(
                screen.tabs[&slot.tab_id].agent_tab_incarnation,
                old.agent_tab_incarnation
            );
        } else {
            screen
                .tabs
                .get_mut(&slot.tab_id)
                .unwrap()
                .new_pane(
                    PaneId::Terminal(TerminalId::new(99)),
                    None,
                    None,
                    false,
                    false,
                    NewPanePlacement::Stacked {
                        pane_id_to_stack_under: Some(slot.pane_id),
                        borderless: None,
                    },
                    None,
                    None,
                )
                .unwrap();
        }
        screen.revalidate_launches();
        assert!(gate.snapshot().revoked.is_some());
        assert!(!gate.claim());
    }
}

fn notification_source() -> yinz_types::agent::claude_notifications::Source {
    yinz_types::agent::claude_notifications::Source {
        pid: 123,
        started: 456,
    }
}

fn sourced_stop(prompt: &str, text: &str) -> HookEvent {
    HookEvent {
        claude_notifications: Some(yinz_types::agent::claude_notifications::Evidence {
            source: Some(notification_source()),
            ..Default::default()
        }),
        ..claude_stop(prompt, text, false)
    }
}

fn lose_notification_observation(screen: &mut Screen, loss: &str) {
    match loss {
        "budget" => {
            let mut state = claude_lifecycle(screen);
            state.claude.obligations.launches[0].remaining_ms = 1;
            screen.agent_lifecycle.insert(PANE, state.into());
            screen.refresh_agent_controls_at(Instant::now() + Duration::from_millis(10));
        },
        "capacity" => {
            report_kind(
                screen,
                "claude",
                native_background_launch("overflow", "overflow-tool", "Bash"),
            );
            assert!(claude_lifecycle(screen).claude.obligations.disabled);
        },
        _ => {
            let mut failed = sourced_stop(
                if loss == "launch" { "prompt" } else { "notice" },
                "not an answer",
            );
            failed.claude_turn_complete = Some(false);
            failed.claude_notifications.as_mut().unwrap().unavailable = true;
            if loss == "source" {
                failed.prompt_id = None;
            }
            report_kind(screen, "claude", failed);
        },
    }
}

fn fill_notification_capacity(screen: &mut Screen) {
    // Fill before any wait is established. Only the final report should trigger
    // observation loss; earlier tool progress is ordinary native hook fallback.
    for i in 1..yinz_types::agent::claude_notifications::MAX_FACTS {
        report_kind(
            screen,
            "claude",
            native_background_launch(&format!("task-{i}"), &format!("tool-{i}"), "Bash"),
        );
    }
}

#[test]
fn unavailable_notification_observation_preserves_explicit_wait_until_native_release() {
    // R69-5: input ownership is independent of failed processing observation,
    // including the direct, attributed, source-wide, expiry and capacity paths.
    for loss in ["notice", "launch", "source", "budget", "capacity"] {
        for needs_person in [false, true] {
            let (mut screen, messages, _writes) = setup("claude");
            report_kind(&mut screen, "claude", hook("SessionStart"));
            report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
            report_kind(
                &mut screen,
                "claude",
                native_background_launch("bash", "tool", "Bash"),
            );
            if loss == "capacity" {
                fill_notification_capacity(&mut screen);
            }
            report_kind(&mut screen, "claude", sourced_stop("prompt", "LAUNCH"));
            report_kind(
                &mut screen,
                "claude",
                task_notification("notice", &["bash"]),
            );
            report_kind(
                &mut screen,
                "claude",
                identified_notice("notice", "bash", "tool", "run"),
            );
            let waiting_hook = HookEvent {
                prompt_id: Some("notice".into()),
                tool_use_id: Some("permission-tool".into()),
                ..claude_permission_prompt()
            };
            let HookOutcome::Reported(mut waiting) = hook_outcome("claude", &waiting_hook) else {
                panic!("expected wait")
            };
            // State reports can also carry an explicit wait on a worker. Preserve
            // the complete fact, not just person waits or permission revisions.
            waiting.wait_needs_a_person = needs_person;
            waiting.waiting_on = Some(if needs_person { "permission" } else { "worker" }.into());
            report_state(
                &mut screen,
                "claude",
                waiting_hook,
                AgentReportState::Set(waiting.clone()),
            );
            let before = claude_lifecycle(&screen).claude_notifications["notice"].clone();
            lose_notification_observation(&mut screen, loss);
            let state = claude_lifecycle(&screen);
            let after = state
                .claude_notifications
                .get("notice")
                .unwrap_or_else(|| panic!("lost wait: {loss}, needs_person={needs_person}"));
            assert!(after.observation_unavailable, "{loss}");
            assert_eq!(after.waiting, before.waiting, "{loss}");
            assert_eq!(
                after.permission_revision, before.permission_revision,
                "{loss}"
            );
            assert_eq!(screen.agent_status_for_test().agent, waiting, "{loss}");
            assert_eq!(state.last_message.as_deref(), Some("LAUNCH"));
            assert!(!state.claude.obligations.active());
            assert!(state.claude.obligations.launches.iter().all(|o| !o.settled));
            draft_frame(&mut screen, "claude", "", 0, Instant::now());
            screen.agent_control(wait(AgentState::Idle), CLIENT);
            let response = replies(&messages, CLIENT);
            if needs_person {
                assert_eq!(response.len(), 1);
                assert_eq!(response[0].outcome, AgentOutcome::NeedsInput);
                assert_ne!(response[0].message.as_deref(), Some("LAUNCH"));
            } else {
                assert!(response.is_empty(), "a worker wait stays pending: {loss}");
            }
            // The same real native release still finds the notification owner.
            report_kind(
                &mut screen,
                "claude",
                HookEvent {
                    prompt_id: Some("notice".into()),
                    tool_use_id: Some("permission-tool".into()),
                    ..hook("PostToolUse")
                },
            );
            assert!(claude_lifecycle(&screen).claude_notifications.is_empty());
            // Later normal use proves no failed processing latch was retained.
            report_kind(&mut screen, "claude", steering_ack("later"));
            report_kind(&mut screen, "claude", sourced_stop("later", "LATER"));
            screen.agent_control(wait(AgentState::Idle), ClientId::new(3));
            assert_eq!(
                replies(&messages, ClientId::new(3))[0].message.as_deref(),
                Some("LATER")
            );
        }
    }
}

#[test]
fn unavailable_notification_observation_preserves_worker_tool_wait_ownership() {
    for loss in ["notice", "launch", "source", "budget", "capacity"] {
        let (mut screen, _, _writes) = setup("claude");
        report_kind(&mut screen, "claude", hook("SessionStart"));
        report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
        report_kind(
            &mut screen,
            "claude",
            native_background_launch("bash", "tool", "Bash"),
        );
        if loss == "capacity" {
            fill_notification_capacity(&mut screen);
        }
        report_kind(&mut screen, "claude", sourced_stop("prompt", "LAUNCH"));
        report_kind(
            &mut screen,
            "claude",
            task_notification("notice", &["bash"]),
        );
        report_kind(
            &mut screen,
            "claude",
            identified_notice("notice", "bash", "tool", "run"),
        );
        report_kind(
            &mut screen,
            "claude",
            claude_worker_input("PreToolUse", "worker", Some("question")),
        );
        lose_notification_observation(&mut screen, loss);
        assert_eq!(
            screen.agent_status_for_test().agent.state,
            AgentState::Waiting
        );
        assert!(screen.agent_status_for_test().agent.wait_needs_a_person);
        assert!(claude_lifecycle(&screen).claude.worker_wait_tools["worker"].contains("question"));
        report_kind(
            &mut screen,
            "claude",
            claude_worker_input("PostToolUse", "worker", Some("unrelated")),
        );
        assert_eq!(
            screen.agent_status_for_test().agent.state,
            AgentState::Waiting
        );
        report_kind(
            &mut screen,
            "claude",
            claude_worker_input("PostToolUse", "worker", Some("question")),
        );
        assert!(screen
            .agent_worker_waits
            .get(&PANE)
            .is_none_or(|w| w.waits.is_empty()));
        report_kind(&mut screen, "claude", steering_ack("later"));
        report_kind(&mut screen, "claude", sourced_stop("later", "LATER"));
        assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
    }
}

#[test]
fn unavailable_identified_notification_releases_only_its_processing_lifetime() {
    // R69-3: registry-only turn_over ends the observer without proving an answer.
    for concurrent in [false, true] {
        for budget in [false, true] {
            let (mut screen, messages, writes) = setup("claude");
            report_kind(&mut screen, "claude", hook("SessionStart"));
            screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
            finish_paste(&writes);
            report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
            report_kind(
                &mut screen,
                "claude",
                native_background_launch("bash", "tool", "Bash"),
            );
            if concurrent {
                report_kind(
                    &mut screen,
                    "claude",
                    native_background_launch("agent", "agent-tool", "Agent"),
                );
            }
            report_kind(&mut screen, "claude", sourced_stop("prompt", "LAUNCH"));
            let original = replies(&messages, CLIENT).pop().unwrap();
            assert_eq!(original.outcome, AgentOutcome::Completed);
            assert_eq!(original.message.as_deref(), Some("LAUNCH"));
            report_kind(
                &mut screen,
                "claude",
                task_notification("notice", &["bash"]),
            );
            let accepted = identified_notice("notice", "bash", "tool", "run");
            if budget || concurrent {
                report_kind(&mut screen, "claude", accepted.clone());
            }
            // The standalone failure also carries the first structural join in its Stop.
            draft_frame(&mut screen, "claude", "", 0, Instant::now());
            screen.agent_control(wait(AgentState::Idle), CLIENT);
            if budget {
                // Exhaust only this observation, leaving an independent task live.
                let mut state = claude_lifecycle(&screen);
                state.claude.obligations.launches[0].remaining_ms = 1;
                // The remaining budget and pending notice survive the handoff JSON route.
                let state = serde_json::from_str::<yinz_types::agent::tracking::LifecycleState>(
                    &serde_json::to_string(&state).unwrap(),
                )
                .unwrap();
                screen.agent_lifecycle.insert(PANE, state.into());
                screen.refresh_agent_controls_at(Instant::now() + Duration::from_millis(10));
            } else {
                if !concurrent {
                    // A prior observer could report no answer without scoped loss.
                    // The second observer's same false confirmation is deduplicated,
                    // but its new unavailable evidence must still publish cleanup.
                    let mut unknown = sourced_stop("notice", "");
                    unknown.claude_turn_complete = Some(false);
                    report_kind(&mut screen, "claude", unknown);
                    assert!(screen.claude_background_pending_for_test());
                }
                let mut failed = sourced_stop("notice", "not a confirmed answer");
                failed.claude_turn_complete = Some(false);
                failed.claude_notifications = accepted.claude_notifications.clone();
                failed.claude_notifications.as_mut().unwrap().unavailable = true;
                report_kind(&mut screen, "claude", failed);
            }
            let lifecycle = claude_lifecycle(&screen);
            assert!(!lifecycle.claude_notifications.contains_key("notice"));
            assert!(lifecycle.claude.obligations.launches[0].unavailable);
            assert!(!lifecycle.claude.obligations.launches[0].settled);
            assert_eq!(
                screen.agent_status_for_test().agent.state,
                if concurrent {
                    AgentState::Busy
                } else {
                    AgentState::Idle
                }
            );
            assert_eq!(screen.agent_status_for_test().background, concurrent);
            assert_eq!(
                lifecycle.last_message.as_deref(),
                Some("LAUNCH"),
                "unavailable is not an answer"
            );
            assert_eq!(replies(&messages, CLIENT).is_empty(), concurrent);
            // A second observer can still supply a real answer after observation loss.
            let mut late = sourced_stop("notice", "REAL_ANSWER");
            late.claude_notifications = accepted.claude_notifications;
            report_kind(&mut screen, "claude", late.clone());
            assert_eq!(
                screen.agent_lifecycle[&PANE].last_message_for_test(),
                Some("REAL_ANSWER")
            );
            // A later human turn finishes normally without erasing independent work.
            report_kind(&mut screen, "claude", steering_ack("later"));
            report_kind(&mut screen, "claude", sourced_stop("later", "LATER"));
            assert_eq!(screen.agent_status_for_test().background, concurrent);
            report_kind(&mut screen, "claude", late);
            assert_eq!(
                screen.agent_lifecycle[&PANE].last_message_for_test(),
                Some("LATER")
            );
            if concurrent {
                report_kind(
                    &mut screen,
                    "claude",
                    task_notification("other", &["agent"]),
                );
                let mut answer = sourced_stop("other", "FINAL_OTHER");
                answer.claude_notifications =
                    identified_notice("other", "agent", "agent-tool", "other-run")
                        .claude_notifications;
                report_kind(&mut screen, "claude", answer);
                assert_eq!(
                    replies(&messages, CLIENT).pop().unwrap().message.as_deref(),
                    Some("FINAL_OTHER")
                );
            }
            assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
            assert!(!claude_lifecycle(&screen).claude.obligations.launches[0].settled);
        }
    }
}

#[test]
fn ordinary_answer_history_cannot_disable_later_notification_tracking() {
    // R69-4: neither an empty adapter nor one with independent work may accumulate
    // an unbounded dependency on unrelated foreground answers.
    for concurrent in [false, true] {
        let (mut screen, messages, writes) = setup("claude");
        report_kind(&mut screen, "claude", hook("SessionStart"));
        if concurrent {
            report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
            report_kind(
                &mut screen,
                "claude",
                native_background_launch("independent", "independent-tool", "Agent"),
            );
            report_kind(&mut screen, "claude", sourced_stop("prompt", "ORIGINAL"));
        }
        for i in 0..80 {
            let id = format!("ordinary-{i}");
            report_kind(&mut screen, "claude", steering_ack(&id));
            report_kind(&mut screen, "claude", sourced_stop(&id, "ordinary"));
            assert_eq!(screen.agent_status_for_test().background, concurrent);
            assert!(!claude_lifecycle(&screen).claude.obligations.disabled);
        }
        // The real submission result remains independent of aggregate idle.
        draft_frame(&mut screen, "claude", "", 0, Instant::now());
        screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
        finish_paste(&writes);
        let mut submit = hook("UserPromptSubmit");
        submit.prompt_id = Some("new-launch".into());
        report_kind(&mut screen, "claude", submit);
        let mut launch = native_background_launch("new-task", "new-tool", "Bash");
        launch.prompt_id = Some("new-launch".into());
        launch.claude_notifications.as_mut().unwrap().launches[0].prompt = "new-launch".into();
        report_kind(&mut screen, "claude", launch);
        report_kind(
            &mut screen,
            "claude",
            sourced_stop("new-launch", "LAUNCH_NEW"),
        );
        assert_eq!(
            replies(&messages, CLIENT).pop().unwrap().message.as_deref(),
            Some("LAUNCH_NEW")
        );
        draft_frame(&mut screen, "claude", "", 0, Instant::now());
        screen.agent_control(wait(AgentState::Idle), CLIENT);
        assert!(replies(&messages, CLIENT).is_empty());
        // Answer-before-acceptance remains a dependency through another >64 answers.
        report_kind(
            &mut screen,
            "claude",
            task_notification("notice", &["new-task"]),
        );
        report_kind(&mut screen, "claude", sourced_stop("notice", "NEW_ANSWER"));
        for i in 80..160 {
            let id = format!("ordinary-{i}");
            report_kind(&mut screen, "claude", steering_ack(&id));
            report_kind(&mut screen, "claude", sourced_stop(&id, "newer ordinary"));
            assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Busy);
            assert!(!claude_lifecycle(&screen).claude.obligations.disabled);
        }
        report_kind(
            &mut screen,
            "claude",
            identified_notice("notice", "new-task", "new-tool", "new-run"),
        );
        assert_eq!(
            screen.agent_lifecycle[&PANE].last_message_for_test(),
            Some("newer ordinary")
        );
        if concurrent {
            assert!(replies(&messages, CLIENT).is_empty());
            report_kind(
                &mut screen,
                "claude",
                task_notification("other", &["independent"]),
            );
            report_kind(
                &mut screen,
                "claude",
                identified_notice(
                    "other",
                    "independent",
                    "independent-tool",
                    "independent-run",
                ),
            );
            report_kind(&mut screen, "claude", sourced_stop("other", "FINAL"));
        }
        let expected = if concurrent {
            "FINAL"
        } else {
            "newer ordinary"
        };
        assert_eq!(
            replies(&messages, CLIENT).pop().unwrap().message.as_deref(),
            Some(expected)
        );
        screen.agent_control(wait(AgentState::Idle), CLIENT);
        assert_eq!(
            replies(&messages, CLIENT).pop().unwrap().message.as_deref(),
            Some(expected)
        );
    }
}

#[test]
fn unrelated_answer_pruning_preserves_reordered_cancellation_and_run_guards() {
    let (mut screen, _, _) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    report_kind(
        &mut screen,
        "claude",
        native_background_launch("bash", "tool", "Bash"),
    );
    report_kind(&mut screen, "claude", sourced_stop("prompt", "LAUNCH"));
    for i in 0..80 {
        let id = format!("unrelated-{i}");
        report_kind(&mut screen, "claude", steering_ack(&id));
        report_kind(&mut screen, "claude", sourced_stop(&id, "ordinary"));
    }
    report_kind(&mut screen, "claude", steering_ack("cancel"));
    report_kind(&mut screen, "claude", sourced_stop("cancel", "CANCELLED"));
    assert!(screen.claude_background_pending_for_test());
    // A reordered positive cancellation joins the retained confirmed answer.
    let (mut cancel, _) = yinz_runtime::agent::hooks::parse_hook_event(
        "claude",
        serde_json::json!({
            "hook_event_name":"PostToolUse","session_id":"conversation","prompt_id":"cancel",
            "tool_use_id":"cancel-tool","tool_name":"TaskStop","tool_input":{"task_id":"bash"},
            "tool_response":{"task_id":"bash","task_type":"local_bash","command":"private"}
        })
        .to_string(),
    )
    .unwrap();
    cancel.claude_notifications.as_mut().unwrap().source = Some(notification_source());
    report_kind(&mut screen, "claude", cancel.clone());
    assert!(!screen.claude_background_pending_for_test());
    assert_eq!(
        screen.agent_lifecycle[&PANE].last_message_for_test(),
        Some("CANCELLED")
    );
    report_kind(&mut screen, "claude", steering_ack("new"));
    let mut launch = native_background_launch("bash", "new-tool", "Bash");
    launch.prompt_id = Some("new".into());
    launch.claude_notifications.as_mut().unwrap().launches[0].prompt = "new".into();
    report_kind(&mut screen, "claude", launch);
    report_kind(&mut screen, "claude", sourced_stop("new", "NEW_LAUNCH"));
    report_kind(&mut screen, "claude", cancel);
    assert!(
        screen.claude_background_pending_for_test(),
        "old cancellation cannot bind reused task"
    );
    report_kind(
        &mut screen,
        "claude",
        task_notification("notice", &["bash"]),
    );
    let mut answer = sourced_stop("notice", "FINAL");
    answer.claude_notifications =
        identified_notice("notice", "bash", "new-tool", "new-run").claude_notifications;
    report_kind(&mut screen, "claude", answer);
    assert!(!screen.claude_background_pending_for_test());
    assert!(!claude_lifecycle(&screen).claude.obligations.disabled);
    assert_eq!(
        screen.agent_lifecycle[&PANE].last_message_for_test(),
        Some("FINAL")
    );
}

#[test]
fn identified_capacity_loss_does_not_orphan_an_accepted_notification() {
    let (mut screen, _, _) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    for i in 0..yinz_types::agent::claude_notifications::MAX_FACTS {
        report_kind(
            &mut screen,
            "claude",
            native_background_launch(&format!("task-{i}"), &format!("tool-{i}"), "Bash"),
        );
    }
    report_kind(&mut screen, "claude", sourced_stop("prompt", "LAUNCH"));
    report_kind(
        &mut screen,
        "claude",
        task_notification("notice", &["task-0"]),
    );
    report_kind(
        &mut screen,
        "claude",
        identified_notice("notice", "task-0", "tool-0", "run"),
    );
    report_kind(
        &mut screen,
        "claude",
        native_background_launch("overflow", "overflow-tool", "Bash"),
    );
    let lifecycle = claude_lifecycle(&screen);
    assert!(lifecycle.claude.obligations.disabled);
    assert!(lifecycle.claude_notifications.is_empty());
    assert_eq!(lifecycle.last_message.as_deref(), Some("LAUNCH"));
    // Fresh tool progress retains the hook fallback until a later confirmed turn.
    assert!(!screen.agent_status_for_test().background);
    report_kind(&mut screen, "claude", steering_ack("later"));
    report_kind(&mut screen, "claude", sourced_stop("later", "LATER"));
    assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
    // After adapter capacity loss, ordinary accepted-turn tracking still supplies
    // the hook fallback. A future notice is not itself a new observation failure.
    report_kind(
        &mut screen,
        "claude",
        task_notification("future", &["future-task"]),
    );
    report_kind(
        &mut screen,
        "claude",
        identified_notice("future", "future-task", "future-tool", "future-run"),
    );
    assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Busy);
    let mut failed = sourced_stop("future", "not an answer");
    failed.claude_turn_complete = Some(false);
    failed.claude_notifications.as_mut().unwrap().unavailable = true;
    report_kind(&mut screen, "claude", failed);
    assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
    assert_eq!(
        screen.agent_lifecycle[&PANE].last_message_for_test(),
        Some("LATER")
    );
}

fn native_background_launch(task: &str, tool: &str, kind: &str) -> HookEvent {
    let raw = serde_json::json!({"hook_event_name":"PostToolUse", "session_id":"conversation", "prompt_id":"prompt",
        "tool_name":kind,"tool_use_id":tool,"tool_response":if kind == "Bash" {
            serde_json::json!({"backgroundTaskId":task})
        } else { serde_json::json!({"isAsync":true,"status":"async_launched","agentId":task}) }});
    let (mut event, _) =
        yinz_runtime::agent::hooks::parse_hook_event("claude", raw.to_string()).unwrap();
    event.claude_notifications.as_mut().unwrap().source = Some(notification_source());
    event
}

fn identified_notice(prompt: &str, task: &str, tool: &str, run: &str) -> HookEvent {
    use yinz_types::agent::claude_notifications::{Evidence, Notice};
    HookEvent {
        hook_event_name: "NotificationAccepted".into(),
        session_id: "conversation".into(),
        prompt_id: Some(prompt.into()),
        claude_notifications: Some(Evidence {
            source: Some(notification_source()),
            notice: Some(Notice {
                task: task.into(),
                tool: tool.into(),
                run: run.into(),
                prompt: prompt.into(),
                row: format!("row-{prompt}"),
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[test]
fn identified_launches_survive_every_sampled_idle_until_their_correlated_answers() {
    for answer_before_acceptance in [false, true] {
        for working in [false, true] {
            let (mut screen, messages, writes) = setup("claude");
            report_kind(&mut screen, "claude", hook("SessionStart"));
            screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
            finish_paste(&writes);
            report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
            for (task, tool, kind) in [
                ("bash", "bash-tool", "Bash"),
                ("agent", "agent-tool", "Agent"),
            ] {
                report_kind(
                    &mut screen,
                    "claude",
                    native_background_launch(task, tool, kind),
                );
            }
            report_kind(
                &mut screen,
                "claude",
                claude_stop("prompt", "LAUNCH_MIX", working),
            );
            let launched = replies(&messages, CLIENT).pop().unwrap();
            assert_eq!(launched.message.as_deref(), Some("LAUNCH_MIX"));
            assert_eq!(launched.outcome, AgentOutcome::Completed);
            draft_frame(&mut screen, "claude", "", 0, Instant::now());
            screen.agent_control(wait(AgentState::Idle), CLIENT);
            for (prompt, task, tool, run, text) in [
                ("first", "bash", "bash-tool", "run1", "FIRST"),
                ("second", "agent", "agent-tool", "run2", "FINAL_MIX"),
            ] {
                // A registry clear before acceptance and arbitrary journal delay provide no disposition.
                report_kind(&mut screen, "claude", background_idle("prompt"));
                assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Busy);
                assert!(screen.agent_status_for_test().background);
                assert!(replies(&messages, CLIENT).is_empty());
                report_kind(&mut screen, "claude", task_notification(prompt, &[task]));
                let notice = identified_notice(prompt, task, tool, run);
                let mut stop = claude_stop(prompt, text, false);
                // Observer failure/early-interruption cannot fabricate a successful answer.
                if !answer_before_acceptance {
                    report_kind(&mut screen, "claude", notice.clone());
                }
                stop.claude_notifications =
                    Some(yinz_types::agent::claude_notifications::Evidence {
                        source: Some(notification_source()),
                        ..Default::default()
                    });
                report_kind(&mut screen, "claude", stop);
                if answer_before_acceptance {
                    report_kind(&mut screen, "claude", notice);
                }
                if prompt == "first" {
                    assert!(replies(&messages, CLIENT).is_empty());
                }
            }
            let done = replies(&messages, CLIENT).pop().unwrap();
            assert_eq!(done.message.as_deref(), Some("FINAL_MIX"));
            assert_eq!(done.agents[0].agent.state, AgentState::Idle);
            screen.agent_control(wait(AgentState::Idle), CLIENT);
            assert_eq!(
                replies(&messages, CLIENT).pop().unwrap().message.as_deref(),
                Some("FINAL_MIX")
            );
            // Duplicate run and acquisition cannot reopen settled work.
            report_kind(
                &mut screen,
                "claude",
                identified_notice("first", "bash", "bash-tool", "run1"),
            );
            report_kind(
                &mut screen,
                "claude",
                native_background_launch("bash", "bash-tool", "Bash"),
            );
            assert!(!screen.claude_background_pending_for_test());
        }
    }
}

#[test]
fn exact_local_cancellation_waits_for_cancelling_answer_and_budget_loss_is_not_settlement() {
    for cancel in [true, false] {
        let (mut screen, messages, _writes) = setup("claude");
        report_kind(&mut screen, "claude", hook("SessionStart"));
        report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
        report_kind(
            &mut screen,
            "claude",
            native_background_launch("bash", "tool", "Bash"),
        );
        report_kind(
            &mut screen,
            "claude",
            claude_stop("prompt", "LAUNCH", false),
        );
        draft_frame(&mut screen, "claude", "", 0, Instant::now());
        screen.agent_control(wait(AgentState::Idle), CLIENT);
        if cancel {
            let mut submit = hook("UserPromptSubmit");
            submit.prompt_id = Some("cancel".into());
            report_kind(&mut screen, "claude", submit);
            let raw = serde_json::json!({"hook_event_name":"PostToolUse","session_id":"conversation","prompt_id":"cancel",
                "tool_use_id":"cancel-tool","tool_name":"TaskStop","tool_input":{"task_id":"bash"},
                "tool_response":{"task_id":"bash","task_type":"local_bash","command":"private"}});
            let (mut post, _) =
                yinz_runtime::agent::hooks::parse_hook_event("claude", raw.to_string()).unwrap();
            post.claude_notifications.as_mut().unwrap().source = Some(notification_source());
            report_kind(&mut screen, "claude", post);
            assert!(replies(&messages, CLIENT).is_empty());
            let mut stop = claude_stop("cancel", "CANCELLED", false);
            stop.claude_notifications = Some(yinz_types::agent::claude_notifications::Evidence {
                source: Some(notification_source()),
                ..Default::default()
            });
            report_kind(&mut screen, "claude", stop);
            assert_eq!(
                replies(&messages, CLIENT).pop().unwrap().message.as_deref(),
                Some("CANCELLED")
            );
        } else {
            screen.refresh_agent_controls_at(
                Instant::now() + Duration::from_millis(yinz_types::agent::control::MAX_TIMEOUT_MS),
            );
            let state = screen
                .agent_lifecycle
                .get(&PANE)
                .unwrap()
                .handoff_for_test();
            assert!(state.claude.obligations.launches[0].unavailable);
            assert!(!state.claude.obligations.launches[0].settled);
            assert!(!screen.claude_background_pending_for_test());
        }
    }
}

#[test]
fn unrelated_scope_old_runs_and_reused_tasks_cannot_settle_new_launches() {
    let (mut screen, _, _) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    report_kind(
        &mut screen,
        "claude",
        native_background_launch("bash", "tool", "Bash"),
    );
    report_kind(
        &mut screen,
        "claude",
        claude_stop("prompt", "LAUNCH", false),
    );
    for mutate in ["worker", "archive", "foreign", "old-process", "wrong-tool"] {
        let mut notice = identified_notice("notice", "bash", "tool", "run");
        match mutate {
            "worker" => notice.agent_id = Some("worker".into()),
            "archive" => notice.archive_only = true,
            "foreign" => notice.session_id = "foreign".into(),
            "old-process" => {
                notice
                    .claude_notifications
                    .as_mut()
                    .unwrap()
                    .source
                    .as_mut()
                    .unwrap()
                    .started -= 1
            },
            _ => {
                notice
                    .claude_notifications
                    .as_mut()
                    .unwrap()
                    .notice
                    .as_mut()
                    .unwrap()
                    .tool = "wrong-tool".into()
            },
        }
        report_kind(&mut screen, "claude", notice);
        assert!(screen.claude_background_pending_for_test());
    }
    // A journal delay longer than the historical debounce still has no clearing transition.
    screen.refresh_agent_controls_at(Instant::now() + Duration::from_secs(30));
    assert!(screen.claude_background_pending_for_test());
    report_kind(
        &mut screen,
        "claude",
        task_notification("notice", &["bash"]),
    );
    let accepted = identified_notice("notice", "bash", "tool", "actual-run");
    report_kind(&mut screen, "claude", accepted.clone());
    let mut stop = claude_stop("notice", "FINAL", false);
    stop.claude_notifications = accepted.claude_notifications;
    report_kind(&mut screen, "claude", stop);
    assert!(!screen.claude_background_pending_for_test());
    // Reusing a task with a different positively acquired tool is a separate obligation.
    let mut submit = hook("UserPromptSubmit");
    submit.prompt_id = Some("new".into());
    report_kind(&mut screen, "claude", submit);
    let mut launch = native_background_launch("bash", "new-tool", "Bash");
    launch.prompt_id = Some("new".into());
    launch.claude_notifications.as_mut().unwrap().launches[0].prompt = "new".into();
    report_kind(&mut screen, "claude", launch);
    report_kind(
        &mut screen,
        "claude",
        identified_notice("notice", "bash", "tool", "actual-run"),
    );
    assert!(screen.claude_background_pending_for_test());
    let state = screen
        .agent_lifecycle
        .get(&PANE)
        .unwrap()
        .handoff_for_test();
    assert!(state.claude.obligations.launches[0].settled);
    assert!(!state.claude.obligations.launches[1].settled);
}

#[test]
fn unsupported_cancellation_degrades_only_unbound_observation_without_settling() {
    for bound in [false, true] {
        let (mut screen, _, _) = setup("claude");
        report_kind(&mut screen, "claude", hook("SessionStart"));
        report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
        report_kind(
            &mut screen,
            "claude",
            native_background_launch("agent", "tool", "Agent"),
        );
        report_kind(
            &mut screen,
            "claude",
            claude_stop("prompt", "LAUNCH", false),
        );
        if bound {
            report_kind(
                &mut screen,
                "claude",
                task_notification("notice", &["agent"]),
            );
            report_kind(
                &mut screen,
                "claude",
                identified_notice("notice", "agent", "tool", "run"),
            );
        }
        let (mut unsupported,_)=yinz_runtime::agent::hooks::parse_hook_event("claude",serde_json::json!({
            "hook_event_name":"PostToolUse","session_id":"conversation","prompt_id":"cancel","tool_use_id":"cancel-tool",
            "tool_name":"TaskStop","tool_input":{"task_id":"agent"},"tool_response":{"task_id":"agent","task_type":"local_agent","command":"private"}
        }).to_string()).unwrap();
        unsupported.claude_notifications.as_mut().unwrap().source = Some(notification_source());
        report_kind(&mut screen, "claude", unsupported);
        let state = screen
            .agent_lifecycle
            .get(&PANE)
            .unwrap()
            .handoff_for_test();
        assert_eq!(state.claude.obligations.launches[0].unavailable, !bound);
        assert!(!state.claude.obligations.launches[0].settled);
    }
}

fn with_notification_source(
    mut event: HookEvent,
    source: yinz_types::agent::claude_notifications::Source,
) -> HookEvent {
    event
        .claude_notifications
        .get_or_insert_with(Default::default)
        .source = Some(source);
    event
}

fn replacement_source() -> yinz_types::agent::claude_notifications::Source {
    yinz_types::agent::claude_notifications::Source {
        pid: 124,
        started: 457,
    }
}

fn provider_resume(source: yinz_types::agent::claude_notifications::Source) -> HookEvent {
    let (event, _) = yinz_runtime::agent::hooks::parse_hook_event(
        "claude",
        serde_json::json!({
            "hook_event_name": "SessionStart", "session_id": "conversation", "source": "resume"
        })
        .to_string(),
    )
    .unwrap();
    with_notification_source(event, source)
}

#[test]
fn claude_provider_resume_retires_old_observation_and_preserves_completed_submission() {
    let (mut screen, messages, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    screen.agent_control_with_clear(identified_prompt("original", true), CLIENT, &writes);
    finish_paste(&writes);
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    report_kind(
        &mut screen,
        "claude",
        native_background_launch("bash", "tool", "Bash"),
    );
    report_kind(&mut screen, "claude", sourced_stop("prompt", "LAUNCH"));
    assert_eq!(
        replies(&messages, CLIENT).pop().unwrap().message.as_deref(),
        Some("LAUNCH")
    );
    let original = screen.agent_submissions[&PANE].clone();
    draft_frame(&mut screen, "claude", "", 0, Instant::now());
    screen.agent_control(wait(AgentState::Idle), CLIENT);
    assert!(replies(&messages, CLIENT).is_empty());

    report_kind(&mut screen, "claude", provider_resume(replacement_source()));
    assert_eq!(
        replies(&messages, CLIENT).pop().unwrap().outcome,
        AgentOutcome::TargetChanged
    );
    assert!(!screen.claude_background_pending_for_test());
    assert_eq!(screen.agent_submissions[&PANE], original);
    assert_eq!(claude_lifecycle(&screen).last_message, None);
    assert_eq!(
        claude_lifecycle(&screen).claude.obligations.source,
        Some(replacement_source())
    );
    report_kind(
        &mut screen,
        "claude",
        with_notification_source(steering_ack("replacement"), replacement_source()),
    );
    screen.agent_control(wait(AgentState::Idle), CLIENT);
    report_kind(
        &mut screen,
        "claude",
        with_notification_source(
            claude_stop("replacement", "NEW", false),
            replacement_source(),
        ),
    );
    let done = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(done.outcome, AgentOutcome::Ok);
    assert_eq!(done.message.as_deref(), Some("NEW"));
    assert_eq!(done.agents[0].agent.state, AgentState::Idle);
    assert!(screen.agent_status_for_test().ready);
    screen.agent_control(wait(AgentState::Idle), CLIENT);
    assert_eq!(
        replies(&messages, CLIENT).pop().unwrap().message.as_deref(),
        Some("NEW")
    );
    recover_after_quiet(&mut screen, recover());
    assert_eq!(
        replies(&messages, CLIENT).pop().unwrap().outcome,
        AgentOutcome::Ok
    );
    screen.agent_control(
        AgentRequest::WaitTurn {
            pane_id: PANE,
            expected_session_id: None,
            submission_id: "original".into(),
            timeout_ms: 30_000,
        },
        CLIENT,
    );
    let original_result = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(original_result.outcome, AgentOutcome::Completed);
    assert_eq!(original_result.message.as_deref(), Some("LAUNCH"));
}

#[test]
fn claude_provider_source_adoption_rejects_unrelated_callbacks_and_old_replay() {
    let (mut screen, _, _) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    report_kind(
        &mut screen,
        "claude",
        native_background_launch("bash", "tool", "Bash"),
    );
    report_kind(&mut screen, "claude", sourced_stop("prompt", "LAUNCH"));
    let before = claude_lifecycle(&screen);
    for event in [
        sourced_stop("prompt", "UNRELATED"),
        identified_notice("notice", "bash", "tool", "run"),
        task_notification("notice", &["bash"]),
        background_idle("prompt"),
        hook("SessionEnd"),
        HookEvent {
            source: Some("compact".into()),
            ..hook("SessionStart")
        },
        HookEvent {
            archive_only: true,
            ..provider_resume(replacement_source())
        },
        HookEvent {
            agent_id: Some("worker".into()),
            ..provider_resume(replacement_source())
        },
    ] {
        report_kind(
            &mut screen,
            "claude",
            with_notification_source(event, replacement_source()),
        );
        let after = claude_lifecycle(&screen);
        assert_eq!(
            after.claude.obligations.source,
            before.claude.obligations.source
        );
        assert_eq!(after.claude.turns, before.claude.turns);
        assert_eq!(after.last_message, before.last_message);
        assert!(screen.claude_background_pending_for_test());
    }
    report_kind(&mut screen, "claude", provider_resume(replacement_source()));
    report_kind(
        &mut screen,
        "claude",
        with_notification_source(steering_ack("new"), replacement_source()),
    );
    report_kind(
        &mut screen,
        "claude",
        with_notification_source(claude_stop("new", "NEW", false), replacement_source()),
    );
    let current = claude_lifecycle(&screen);
    for event in [
        provider_resume(notification_source()),
        native_background_launch("bash", "tool", "Bash"),
        task_notification("notice", &["bash"]),
        identified_notice("notice", "bash", "tool", "run"),
        sourced_stop("new", "OLD"),
        hook("SessionEnd"),
    ] {
        report_kind(
            &mut screen,
            "claude",
            with_notification_source(event, notification_source()),
        );
        let after = claude_lifecycle(&screen);
        assert_eq!(after.claude, current.claude);
        assert_eq!(after.last_message.as_deref(), Some("NEW"));
        assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
    }
    // The replacement can reuse a task/tool/run from the retired incarnation.
    report_kind(
        &mut screen,
        "claude",
        with_notification_source(steering_ack("prompt"), replacement_source()),
    );
    report_kind(
        &mut screen,
        "claude",
        with_notification_source(
            native_background_launch("bash", "tool", "Bash"),
            replacement_source(),
        ),
    );
    report_kind(
        &mut screen,
        "claude",
        with_notification_source(sourced_stop("prompt", "NEW_LAUNCH"), replacement_source()),
    );
    report_kind(
        &mut screen,
        "claude",
        with_notification_source(task_notification("notice", &["bash"]), replacement_source()),
    );
    let notice = with_notification_source(
        identified_notice("notice", "bash", "tool", "run"),
        replacement_source(),
    );
    report_kind(&mut screen, "claude", notice.clone());
    // Even an old-source answer naming the current prompt cannot settle it.
    report_kind(&mut screen, "claude", sourced_stop("notice", "OLD"));
    assert!(screen.claude_background_pending_for_test());
    let mut answer =
        with_notification_source(sourced_stop("notice", "FINAL_NEW"), replacement_source());
    answer.claude_notifications = notice.claude_notifications;
    report_kind(&mut screen, "claude", answer);
    assert!(!screen.claude_background_pending_for_test());
    assert_eq!(
        claude_lifecycle(&screen).last_message.as_deref(),
        Some("FINAL_NEW")
    );
}

#[test]
fn claude_same_source_resume_preserves_available_work_and_native_waits() {
    for worker in [false, true] {
        let (mut screen, _, _) = setup("claude");
        report_kind(&mut screen, "claude", hook("SessionStart"));
        report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
        report_kind(
            &mut screen,
            "claude",
            native_background_launch("bash", "tool", "Bash"),
        );
        report_kind(&mut screen, "claude", sourced_stop("prompt", "LAUNCH"));
        report_kind(
            &mut screen,
            "claude",
            task_notification("notice", &["bash"]),
        );
        report_kind(
            &mut screen,
            "claude",
            identified_notice("notice", "bash", "tool", "run"),
        );
        let waiting = if worker {
            claude_worker_input("PreToolUse", "worker", Some("question"))
        } else {
            HookEvent {
                prompt_id: Some("notice".into()),
                ..claude_permission_prompt()
            }
        };
        report_kind(
            &mut screen,
            "claude",
            with_notification_source(waiting, notification_source()),
        );
        let before = claude_lifecycle(&screen);
        report_kind(
            &mut screen,
            "claude",
            provider_resume(notification_source()),
        );
        assert!(screen.claude_background_pending_for_test());
        assert_eq!(
            screen.agent_status_for_test().agent.state,
            AgentState::Waiting
        );
        assert_eq!(
            claude_lifecycle(&screen).claude_notifications,
            before.claude_notifications
        );
        assert_eq!(
            claude_lifecycle(&screen).claude.worker_wait_tools,
            before.claude.worker_wait_tools
        );
        report_kind(&mut screen, "claude", provider_resume(replacement_source()));
        assert!(!screen.claude_background_pending_for_test());
        assert!(claude_lifecycle(&screen).claude_notifications.is_empty());
        assert!(claude_lifecycle(&screen)
            .claude
            .worker_wait_tools
            .is_empty());
        assert!(!screen.agent_worker_waits.contains_key(&PANE));
        assert!(!screen.claude_permission_waits.contains_key(&PANE));
        assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
    }
}

#[test]
fn claude_provider_replacement_cannot_complete_an_unresolved_submission() {
    let (mut screen, messages, writes) = setup("claude");
    report_kind(
        &mut screen,
        "claude",
        provider_resume(notification_source()),
    );
    screen.agent_control_with_clear(identified_prompt("pending", true), CLIENT, &writes);
    finish_paste(&writes);
    report_kind(
        &mut screen,
        "claude",
        with_notification_source(hook("UserPromptSubmit"), notification_source()),
    );
    report_kind(&mut screen, "claude", provider_resume(replacement_source()));
    assert_eq!(
        replies(&messages, CLIENT).pop().unwrap().outcome,
        AgentOutcome::TargetChanged
    );
    let result = screen.agent_submissions[&PANE]
        .back()
        .unwrap()
        .result
        .clone()
        .unwrap();
    assert_eq!(result.outcome, AgentOutcome::TargetChanged);
    assert_eq!(result.message, None);
    report_kind(
        &mut screen,
        "claude",
        with_notification_source(steering_ack("new"), replacement_source()),
    );
    report_kind(
        &mut screen,
        "claude",
        with_notification_source(claude_stop("new", "NEW", false), replacement_source()),
    );
    assert_eq!(
        screen.agent_submissions[&PANE]
            .back()
            .unwrap()
            .result
            .as_ref(),
        Some(&result)
    );
    // A newer local positive launch retains the previously supported replacement route.
    let third = yinz_types::agent::claude_notifications::Source {
        pid: 125,
        started: 458,
    };
    report_kind(
        &mut screen,
        "claude",
        with_notification_source(
            native_background_launch("next", "next-tool", "Bash"),
            third.clone(),
        ),
    );
    report_kind(
        &mut screen,
        "claude",
        with_notification_source(sourced_stop("prompt", "NEXT"), third),
    );
    assert!(screen.claude_background_pending_for_test());
    assert_eq!(
        claude_lifecycle(&screen).last_message.as_deref(),
        Some("NEXT")
    );
}

#[test]
fn claude_provider_adoption_requires_live_local_native_authority() {
    for invalid in [
        "older",
        "same-start",
        "unavailable",
        "compact",
        "confirmation",
        "remote",
        "held",
        "foreign",
    ] {
        let target = if invalid == "remote" {
            PaneTarget::ssh("test-host").unwrap()
        } else {
            PaneTarget::Local
        };
        let (mut screen, _, _) = setup_on_target("claude", target);
        report_kind(&mut screen, "claude", hook("SessionStart"));
        report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
        report_kind(
            &mut screen,
            "claude",
            native_background_launch("bash", "tool", "Bash"),
        );
        report_kind(&mut screen, "claude", sourced_stop("prompt", "LAUNCH"));
        let mut start = provider_resume(replacement_source());
        match invalid {
            "older" => {
                start
                    .claude_notifications
                    .as_mut()
                    .unwrap()
                    .source
                    .as_mut()
                    .unwrap()
                    .started = 455
            },
            "same-start" => {
                start
                    .claude_notifications
                    .as_mut()
                    .unwrap()
                    .source
                    .as_mut()
                    .unwrap()
                    .started = 456
            },
            "unavailable" => start.claude_notifications.as_mut().unwrap().unavailable = true,
            "compact" => start.source = Some("compact".into()),
            "confirmation" => start.claude_turn_complete = Some(true),
            "held" => screen
                .tab_containing_pane_mut(PANE)
                .unwrap()
                .get_pane_with_id_mut(PANE)
                .unwrap()
                .hold(Some(0), true, RunCommand::default()),
            "foreign" => {
                // A foreign detached notice cannot claim this lifecycle.
                start = identified_notice("notice", "bash", "tool", "run");
                start.session_id = "foreign".into();
                start = with_notification_source(start, replacement_source());
            },
            _ => {},
        }
        report_kind(&mut screen, "claude", start);
        assert_eq!(
            claude_lifecycle(&screen).claude.obligations.source,
            Some(notification_source()),
            "{invalid}"
        );
        assert!(screen.claude_background_pending_for_test(), "{invalid}");
    }
}

#[test]
fn claude_same_source_resume_does_not_answer_the_current_parent_turn_or_permission() {
    for permission in [false, true] {
        let (mut screen, _, _) = setup("claude");
        report_kind(
            &mut screen,
            "claude",
            provider_resume(notification_source()),
        );
        report_kind(
            &mut screen,
            "claude",
            with_notification_source(hook("UserPromptSubmit"), notification_source()),
        );
        if permission {
            report_kind(
                &mut screen,
                "claude",
                with_notification_source(claude_permission_prompt(), notification_source()),
            );
        }
        let before = screen.agent_status_for_test().agent;
        report_kind(
            &mut screen,
            "claude",
            provider_resume(notification_source()),
        );
        assert_eq!(screen.agent_status_for_test().agent, before);
        assert_eq!(
            screen.claude_permission_waits.contains_key(&PANE),
            permission
        );
        report_kind(&mut screen, "claude", provider_resume(replacement_source()));
        assert_eq!(screen.agent_status_for_test().agent.state, AgentState::Idle);
        assert!(!screen.claude_permission_waits.contains_key(&PANE));
    }
}
