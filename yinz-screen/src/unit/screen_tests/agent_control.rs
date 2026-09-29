use super::*;
use crate::screen::thread_main::apply_queued_agent_report;
use std::time::Instant;
use yinz_types::agent::control::{AgentOutcome, AgentRequest, AgentResponse};
use yinz_types::agent::report::{AgentReport, AgentReportState};
use yinz_types::agent::{hook_outcome, AgentEvent, AgentState, HookEvent, HookOutcome};

const PANE: PaneId = PaneId::Terminal(TerminalId::new(7));
const CLIENT: ClientId = ClientId::new(1);

fn setup(
    command: &str,
) -> (
    Screen,
    CapturedClientMessages,
    Receiver<(PtyWriteInstruction, ErrorContext)>,
) {
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
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
        Some(Run::Command(RunCommand {
            command: PathBuf::from("env"),
            args: vec![
                format!("{}=1", yinz_types::agent::AGENT_PANE_ENV),
                command.into(),
            ],
            cwd: Some(PathBuf::from("/work")),
            ..Default::default()
        })),
        false,
        true,
        NewPanePlacement::default(),
        Some(CLIENT),
        None,
    )
    .unwrap();
    tab.handle_pty_bytes(TerminalId::new(7), b"\x1b[?2004h".to_vec())
        .unwrap();
    (screen, messages, writes)
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

fn hook(name: &str) -> HookEvent {
    HookEvent {
        hook_event_name: name.into(),
        session_id: "conversation".into(),
        turn_id: Some("turn".into()),
        source: (name == "SessionStart").then(|| "startup".into()),
        prompt: (name == "UserPromptSubmit").then(|| "review".into()),
        ..Default::default()
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

#[test]
fn respawn_restores_readiness_without_consuming_a_new_draft() {
    for kind in ["claude", "codex"] {
        for keep_scrollback in [false, true] {
            for replace in [false, true] {
                for draft in [false, true] {
                    let (mut screen, messages, writes) = setup(kind);
                    report_kind(&mut screen, kind, hook("SessionStart"));
                    screen.agent_control(prompt(true), CLIENT);
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
                    screen.agent_control(prompt(false), CLIENT);
                    if draft {
                        let result = replies(&messages, CLIENT).pop().unwrap();
                        assert_eq!(result.outcome, AgentOutcome::Rejected);
                        assert!(!result.agents[0].ready);
                        assert!(writes.try_iter().all(|(instruction, _)| !matches!(
                            instruction,
                            PtyWriteInstruction::Write(..)
                        )));
                    } else {
                        assert!(replies(&messages, CLIENT).is_empty());
                        assert!(writes.try_iter().any(|(instruction, _)| matches!(
                            instruction,
                            PtyWriteInstruction::Write(..)
                        )));
                        let mut accepted = hook("UserPromptSubmit");
                        accepted.session_id = "new-conversation".into();
                        report_kind(&mut screen, kind, accepted);
                        assert_eq!(
                            replies(&messages, CLIENT)[0].outcome,
                            AgentOutcome::Submitted
                        );
                    }
                }
            }
        }
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
        pane.scroll_up(scroll, CLIENT);
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
fn opencode_idle_waits_observe_lifecycle_without_prompt_support() {
    for initially_busy in [false, true] {
        let (mut screen, messages, writes) = setup("opencode");
        report_kind(
            &mut screen,
            "opencode",
            hook(if initially_busy {
                "UserPromptSubmit"
            } else {
                "SessionStart"
            }),
        );
        screen.agent_control(wait(AgentState::Idle), CLIENT);
        if initially_busy {
            assert!(replies(&messages, CLIENT).is_empty());
            report_kind(&mut screen, "opencode", hook("Stop"));
        }
        let result = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(result.outcome, AgentOutcome::Ok);
        assert!(result.agents[0].status_available);
        assert!(!result.agents[0].ready);
        assert!(!result.agents[0].prompt_supported);
        screen.agent_control(prompt(true), CLIENT);
        assert_eq!(
            replies(&messages, CLIENT)[0].outcome,
            AgentOutcome::StatusUnavailable
        );
        assert!(writes
            .try_iter()
            .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))));
    }
}

#[test]
fn opencode_idle_wait_requires_matching_lifecycle_evidence() {
    for evidence in ["missing", "state-only", "wrong-kind", "wrong-conversation"] {
        let (mut screen, messages, _writes) = setup("opencode");
        if evidence.starts_with("wrong-") {
            let kind = if evidence == "wrong-kind" {
                "codex"
            } else {
                "opencode"
            };
            report_kind(&mut screen, kind, hook("SessionStart"));
        }
        if evidence != "missing" {
            let mut startup = hook("SessionStart");
            if evidence == "wrong-conversation" {
                startup.session_id = "other".into();
            }
            let HookOutcome::Reported(agent) = hook_outcome("opencode", &startup) else {
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
}

#[test]
fn supported_idle_waits_require_readiness_even_without_prompt_support() {
    for kind in ["claude", "codex"] {
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
}

#[test]
fn startup_wait_requires_lifecycle_evidence_and_binds_the_first_conversation() {
    let (mut screen, messages, _writes) = setup("codex");
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
    screen.agent_control(
        AgentRequest::Wait {
            pane_id: PANE,
            expected_session_id: None,
            until: AgentState::Idle,
            timeout_ms: 30_000,
        },
        CLIENT,
    );
    assert!(replies(&messages, CLIENT).is_empty());
    screen.agent_control(prompt(true), ClientId::new(2));
    assert_eq!(
        replies(&messages, ClientId::new(2))[0].outcome,
        AgentOutcome::StatusUnavailable
    );
    report(&mut screen, hook("SessionStart"));
    let result = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(result.outcome, AgentOutcome::Ok);
    assert!(result.agents[0].ready);
    assert!(result.agents[0].prompt_supported);
    assert_eq!(
        result.agents[0].agent.session_id.as_deref(),
        Some("conversation")
    );
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
            screen.agent_control(prompt(true), CLIENT);
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
            screen.agent_control(prompt(true), CLIENT);
            assert_eq!(
                replies(&messages, CLIENT)[0].outcome,
                AgentOutcome::Rejected
            );
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
            screen.agent_control(prompt(true), CLIENT);
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
            screen.agent_control(prompt(true), ClientId::new(2));
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
    let (mut screen, messages, _writes) = setup("codex");
    let HookOutcome::Reported(agent) = hook_outcome("codex", &hook("SessionStart")) else {
        panic!("startup reports an idle agent");
    };
    crate::screen::apply_pane_agent_report(&mut screen, PANE, Some(agent)).unwrap();
    report(&mut screen, hook("SubagentStart"));
    screen.agent_control(prompt(true), CLIENT);
    assert_eq!(
        replies(&messages, CLIENT)[0].outcome,
        AgentOutcome::StatusUnavailable
    );
}

#[test]
fn prompt_claim_precedes_input_and_acknowledgement_does_not_release_it() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    screen.agent_control(prompt(false), CLIENT);
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
    assert_eq!(input, b"\x1b[200~review\x1b[201~\r");
    screen.agent_control(prompt(true), ClientId::new(2));
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
    screen.agent_control(prompt(true), ClientId::new(2));
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
        let (mut screen, messages, _writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        screen.agent_control(prompt(true), CLIENT);
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
        assert_eq!(result.message.as_deref(), Some("result"));
        assert!(result.acknowledged);
        assert_eq!(result.agents[0].agent.state, AgentState::Idle);
        if outcome == AgentOutcome::Completed {
            assert!(result.agents[0].ready);
        }
        assert!(screen.agent_controls.is_empty());
    }
}

#[test]
fn manual_terminal_input_interrupts_correlation_before_a_fast_completion() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    screen.agent_control(prompt(true), CLIENT);
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
        let (mut screen, messages, _writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        screen.agent_control(prompt(true), CLIENT);
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
fn automatic_approval_and_compaction_continue_until_a_person_is_needed() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    screen.agent_control(prompt(true), CLIENT);
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
        AgentOutcome::TargetChanged
    );
    assert!(writes
        .try_iter()
        .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))));
}

#[test]
fn moving_an_agent_to_another_tab_preserves_pending_prompt_correlation() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    screen.agent_control(prompt(true), CLIENT);
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
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    screen.agent_control(prompt(true), CLIENT);
    screen.agent_controls[0].deadline = Some(Instant::now());
    screen.refresh_agent_controls();
    let timeout = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(timeout.outcome, AgentOutcome::TimedOut);
    assert!(!timeout.acknowledged);
    assert!(!timeout.agents[0].ready);
    assert_eq!(screen.agent_controls.len(), 1);
    assert!(screen.agent_control_deadline().is_none());

    screen.agent_control(prompt(true), ClientId::new(2));
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

    screen.agent_control(prompt(true), ClientId::new(2));
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
        screen.agent_control(prompt(true), CLIENT);
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
        screen.agent_control(prompt(true), CLIENT);
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
        screen.agent_control(prompt(true), ClientId::new(2));
        assert_eq!(
            replies(&messages, ClientId::new(2))[0].outcome,
            AgentOutcome::Rejected
        );
        report(&mut screen, hook("UserPromptSubmit"));
        report(&mut screen, hook("UserPromptSubmit"));
        report(&mut screen, hook("Stop"));
        assert!(replies(&messages, CLIENT).is_empty());
        assert!(screen.agent_controls.is_empty());

        screen.agent_control(prompt(true), ClientId::new(2));
        let result = replies(&messages, ClientId::new(2)).pop().unwrap();
        assert_eq!(result.outcome, AgentOutcome::Rejected);
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
        screen.agent_control(prompt(true), CLIENT);
        let result = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(result.outcome, AgentOutcome::Rejected);
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
        screen.agent_control(prompt(true), CLIENT);
        let result = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(result.outcome, AgentOutcome::Rejected, "{first_hook}");
        assert!(result.agents[0].status_available);
        assert!(!result.agents[0].ready);
        assert!(writes
            .try_iter()
            .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))));
    }
}

#[test]
fn a_manual_draft_invalidates_idle_readiness_before_a_cli_prompt() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    screen
        .tab_containing_pane_mut(PANE)
        .unwrap()
        .write_to_active_terminal(&None, b"manual draft".to_vec(), false, CLIENT, None)
        .unwrap();
    writes.try_iter().for_each(drop);
    screen.agent_control(prompt(true), CLIENT);
    let result = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(result.outcome, AgentOutcome::Rejected);
    assert!(!result.agents[0].ready);
    assert!(result.agents[0].status_available);
    assert!(writes
        .try_iter()
        .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))));
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
    screen.agent_control(prompt(true), CLIENT);
    let result = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(result.outcome, AgentOutcome::Rejected);
    assert_eq!(result.agents[0].agent.state, AgentState::Idle);
    assert!(!result.agents[0].ready);
    assert!(writes
        .try_iter()
        .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))));

    // An uncorrelated acceptance cannot prove which input revision was consumed.
    report(&mut screen, hook("UserPromptSubmit"));
    report(&mut screen, hook("Stop"));
    screen.agent_control(prompt(true), CLIENT);
    let result = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(result.outcome, AgentOutcome::Rejected);
    assert!(!result.agents[0].ready);
    assert!(screen.agent_controls.is_empty());
    assert!(writes
        .try_iter()
        .all(|(instruction, _)| !matches!(instruction, PtyWriteInstruction::Write(..))));
}
