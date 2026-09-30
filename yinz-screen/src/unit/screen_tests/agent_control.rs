use super::*;
use crate::screen::thread_main::apply_queued_agent_report;
use std::time::{Duration, Instant};
use yinz_types::agent::control::{AgentOutcome, AgentRequest, AgentResponse, NotReadyReason};
use yinz_types::agent::report::{AgentReport, AgentReportState};
use yinz_types::agent::{hook_outcome, AgentEvent, AgentState, HookEvent, HookOutcome};
use yinz_types::command::PaneTarget;
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
            target,
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

fn pane_targets() -> [PaneTarget; 3] {
    [
        PaneTarget::Local,
        PaneTarget::ssh("test-host").unwrap(),
        PaneTarget::firecracker("test-vm", None, None).unwrap(),
    ]
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
        prompt_id: Some("prompt".into()),
        claude_turn_complete: (name == "Stop").then_some(true),
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

fn send_keys(keys: Vec<BareKey>) -> AgentRequest {
    AgentRequest::SendKeys {
        pane_id: PANE,
        expected_session_id: None,
        keys: keys.into_iter().map(KeyWithModifier::new).collect(),
    }
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
    for kind in ["claude", "codex"] {
        for split in [false, true] {
            let (mut screen, messages, writes) = setup(kind);
            screen
                .tab_containing_pane_mut(PANE)
                .unwrap()
                .handle_pty_bytes(TerminalId::new(7), b"\x1b[?2004l".to_vec())
                .unwrap();
            screen.agent_control(wait(AgentState::Idle), CLIENT);
            let batches = if split {
                vec![vec![BareKey::Down], vec![BareKey::Enter]]
            } else {
                vec![vec![BareKey::Down, BareKey::Enter]]
            };
            for keys in batches {
                screen.agent_control(send_keys(keys), ClientId::new(2));
                let result = replies(&messages, ClientId::new(2)).pop().unwrap();
                assert_eq!(result.outcome, AgentOutcome::Ok, "{kind}");
                assert!(!result.agents[0].status_available);
                assert!(!result.agents[0].ready);
            }
            assert_eq!(finish_paste(&writes), b"\x1b[B\r");
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
            screen.agent_control(prompt(false), CLIENT);
            assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
        }
    }
}

#[test]
fn permission_keys_preserve_the_accepted_prompt_until_progress_and_completion() {
    for kind in ["claude", "codex"] {
        for split in [false, true] {
            for progress in [false, true] {
                let (mut screen, messages, writes) = setup(kind);
                report_kind(&mut screen, kind, hook("SessionStart"));
                screen.agent_control(prompt(true), CLIENT);
                finish_paste(&writes);
                report_kind(&mut screen, kind, hook("UserPromptSubmit"));
                report_kind(&mut screen, kind, hook("PermissionRequest"));
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
                report_kind(&mut screen, kind, hook("Stop"));
                let result = replies(&messages, CLIENT).pop().unwrap();
                assert_eq!(
                    result.outcome,
                    AgentOutcome::Ok,
                    "{kind}, {split}, {progress}"
                );
                assert!(result.agents[0].ready);
                assert!(screen.agent_controls.is_empty());
                screen.agent_control(prompt(true), CLIENT);
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
    for kind in ["claude", "codex"] {
        let (mut screen, messages, writes) = setup(kind);
        report_kind(&mut screen, kind, hook("SessionStart"));
        screen.agent_control(prompt(false), CLIENT);
        finish_paste(&writes);
        report_kind(&mut screen, kind, hook("UserPromptSubmit"));
        replies(&messages, CLIENT);
        report_kind(&mut screen, kind, hook("PermissionRequest"));
        screen.agent_control(send_keys(vec![BareKey::Down]), CLIENT);
        assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
        screen.agent_control(wait(AgentState::Idle), CLIENT);
        assert!(replies(&messages, CLIENT).is_empty());
        report_kind(&mut screen, kind, hook("PermissionRequest"));
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
    for kind in ["claude", "codex"] {
        let (mut screen, messages, writes) = setup(kind);
        report_kind(&mut screen, kind, hook("SessionStart"));
        screen.agent_control(prompt(true), CLIENT);
        finish_paste(&writes);
        report_kind(&mut screen, kind, hook("UserPromptSubmit"));
        // A state-only update can expose the dialog before its hook finishes the prompt wait.
        let HookOutcome::Reported(waiting) = hook_outcome(kind, &hook("PermissionRequest")) else {
            panic!("permission request reports a waiting agent");
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
            "invalidate" => screen.invalidate_agent_lifecycle(PANE),
            "close" => screen.close_pane_explicitly(PANE, None),
            "restart" => {
                report(&mut screen, hook("SessionStart"));
                report(&mut screen, hook("Stop"));
                screen.agent_control(prompt(false), CLIENT);
                let result = replies(&messages, CLIENT).pop().unwrap();
                assert_eq!(result.outcome, AgentOutcome::Rejected);
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
    for kind in ["claude", "codex"] {
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
                    report_kind(&mut screen, kind, hook("PermissionRequest"));
                }
                if interference == "before" {
                    screen
                        .tab_containing_pane_mut(PANE)
                        .unwrap()
                        .write_key_to_pane_id(PANE, KeyWithModifier::new(BareKey::Char('x')))
                        .unwrap();
                }
                screen.agent_control(send_keys(vec![BareKey::Down]), CLIENT);
                assert_eq!(replies(&messages, CLIENT)[0].outcome, AgentOutcome::Ok);
                if matches!(interference, "after" | "between") {
                    screen
                        .tab_containing_pane_mut(PANE)
                        .unwrap()
                        .write_key_to_pane_id(PANE, KeyWithModifier::new(BareKey::Char('x')))
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
                        screen.invalidate_agent_lifecycle(PANE);
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
                screen.agent_control(prompt(false), CLIENT);
                let result = replies(&messages, CLIENT).pop().unwrap();
                assert_eq!(
                    result.outcome,
                    AgentOutcome::Rejected,
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
        screen.agent_control(prompt(true), CLIENT);
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
        screen.agent_control(prompt(false), ClientId::new(2));
        assert!(!replies(&messages, ClientId::new(2))[0].agents[0].ready);
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
        screen.agent_control(prompt(true), ClientId::new(2));
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

        screen.agent_control(prompt(false), CLIENT);
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
    for kind in ["claude", "codex"] {
        for replacement in [false, true] {
            let (mut screen, messages, _writes) = setup(kind);
            // Codex's bridge announces the conversation when the UI selects it; Claude reports
            // its startup once per hook registration.
            report_kind(&mut screen, kind, hook("SessionStart"));
            screen.agent_control(prompt(true), CLIENT);
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
    for kind in ["claude", "codex"] {
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
            screen.agent_control(prompt(true), CLIENT);
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
    assert_eq!(input, b"\x1b[200~review\x1b[201~");
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
        finish_paste(&_writes);
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
                Some(NotReadyReason::PendingSubmission)
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
    }
}

#[test]
fn manual_terminal_input_interrupts_correlation_before_a_fast_completion() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    screen.agent_control(prompt(true), CLIENT);
    finish_paste(&_writes);
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
        AgentOutcome::NotAgent
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
    finish_paste(&_writes);
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
    screen.agent_control(clear_prompt(), CLIENT);
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
            screen.agent_control(request, CLIENT);
            let result = replies(&messages, CLIENT).pop().unwrap();
            assert_eq!(result.outcome, AgentOutcome::Ok);
            assert_eq!(
                result.agents[0].agent.session_id.as_deref(),
                Some("cleared-conversation")
            );
            assert_eq!(result.agents[0].agent.state, AgentState::Idle);
            assert!(result.agents[0].ready);
        }
        screen.agent_control(prompt(true), CLIENT);
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
        screen.agent_control(prompt(true), CLIENT);
        let result = replies(&messages, CLIENT).pop().unwrap();
        assert_eq!(result.outcome, AgentOutcome::Rejected, "{draft_at}");
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
                    pane_id: PANE,
                    expected_session_id: None,
                    prompt: "/clear extra".into(),
                    wait: true,
                    timeout_ms: 30_000,
                }
            } else {
                clear_prompt()
            };
            screen.agent_control(request, CLIENT);
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
            "invalidate" => screen.invalidate_agent_lifecycle(PANE),
            "closed" => screen.close_pane_explicitly(PANE, None),
            "process" => screen.run_command_in_pane_with_id(
                TerminalId::new(7),
                RunCommand {
                    command: "claude".into(),
                    ..Default::default()
                },
                false,
            ),
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
            assert!(!result.agents[0].ready, "{reason}");
        }
    }
}

#[test]
fn paste_completion_and_delay_gate_enter_without_sleeping() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    screen.agent_control(prompt(true), CLIENT);
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
        "ack",
        "replacement",
        "closed",
        "timeout",
    ] {
        let (mut screen, messages, writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        screen.agent_control(prompt(true), CLIENT);
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
            "ack" => report(&mut screen, hook("UserPromptSubmit")),
            "replacement" => screen.run_command_in_pane_with_id(
                TerminalId::new(7),
                RunCommand {
                    command: "codex".into(),
                    ..Default::default()
                },
                false,
            ),
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
            assert_eq!(replies(&messages, CLIENT).len(), 1, "{reason}");
        }
    }
}

impl Screen {
    fn agent_write_for_test(&mut self) {
        self.tab_containing_pane_mut(PANE)
            .unwrap()
            .write_key_to_pane_id(PANE, KeyWithModifier::new(BareKey::Char('x')))
            .unwrap();
    }
}

#[test]
fn uncertain_submission_requires_explicit_recovery_and_replies_once() {
    for target in pane_targets() {
        let (mut screen, messages, writes) = setup_on_target("codex", target);
        report(&mut screen, hook("SessionStart"));
        screen.agent_control(prompt(true), CLIENT);
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
        screen.agent_control(prompt(true), ClientId::new(2));
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
            screen.agent_control(request, ClientId::new(2));
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
        screen.agent_control(prompt(true), CLIENT);
        assert!(replies(&messages, CLIENT).is_empty());
        assert_eq!(finish_paste(&writes), b"\x1b[200~review\x1b[201~");
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
            screen.agent_control(prompt(true), CLIENT);
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
        screen.agent_control(prompt(true), CLIENT);
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
        screen.agent_control(prompt(true), CLIENT);
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
            screen.agent_control(prompt(true), CLIENT);
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
    screen.agent_control(prompt(true), CLIENT);
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
    screen.agent_control(prompt(true), CLIENT);
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
    screen.agent_control(prompt(false), CLIENT);
    finish_paste(&writes);
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    replies(&messages, CLIENT);
    let mut permission = hook("PermissionRequest");
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
    screen.agent_control(prompt(true), CLIENT);
    finish_paste(&writes);
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    let message = |screen: &mut Screen, request| {
        screen.agent_control(request, ClientId::new(3));
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
    screen.agent_control(
        AgentRequest::Prompt {
            pane_id: PANE,
            expected_session_id: None,
            prompt: text.clone(),
            wait: true,
            timeout_ms: 30_000,
        },
        CLIENT,
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
    screen.agent_control(prompt(false), CLIENT);
    assert_eq!(
        finish_paste(&writes),
        b"\x1b[200~review\x1b[201~",
        "a short prompt is pasted alone"
    );
}

#[test]
fn commands_are_rejected_before_claiming_or_writing_input() {
    for kind in ["claude", "codex"] {
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
        {
            let (mut screen, messages, writes) = setup(kind);
            report_kind(&mut screen, kind, hook("SessionStart"));
            screen.agent_control(
                AgentRequest::Prompt {
                    pane_id: PANE,
                    expected_session_id: None,
                    prompt: command.into(),
                    wait: true,
                    timeout_ms: 30_000,
                },
                CLIENT,
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
                    screen.agent_control_with_reply(
                        request,
                        crate::screen::agent_control::Reply::Host(sender.clone()),
                    );
                } else {
                    screen.agent_control(request, CLIENT);
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
    let (mut screen, messages, _writes) = setup("codex");
    screen.agent_control(wait(AgentState::Idle), CLIENT);
    let mut bootstrap = hook("SessionStart");
    bootstrap.codex_hooks_ready = Some(true);
    report(&mut screen, bootstrap);
    let ready = replies(&messages, CLIENT).pop().unwrap();
    assert_eq!(ready.outcome, AgentOutcome::Ok);
    assert!(ready.agents[0].ready);
    screen.agent_control(prompt(true), CLIENT);
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
        let (mut screen, messages, _writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        screen.agent_control(prompt(true), CLIENT);
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
        let (mut screen, messages, _writes) = setup("codex");
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
        screen.agent_control(prompt(true), CLIENT);
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
    screen.agent_control(prompt(true), CLIENT);
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

    screen.agent_control(prompt(true), CLIENT);
    finish_paste(&writes);
    report(&mut screen, turn_hook("UserPromptSubmit", Some("t2")));
    report(&mut screen, turn_hook("PostToolUse", Some("t2")));
    assert_eq!(pane_state(&screen), AgentState::Busy);
    report(&mut screen, turn_hook("Stop", Some("t2")));
    let done = replies(&messages, CLIENT);
    assert_eq!(done[0].outcome, AgentOutcome::Completed);
    assert!(done[0].agents[0].ready);
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
            screen.agent_control(
                AgentRequest::Prompt {
                    pane_id: PANE,
                    expected_session_id: None,
                    prompt: text.clone(),
                    wait: true,
                    timeout_ms: 30_000,
                },
                CLIENT,
            );
            let written = finish_paste(&writes);
            assert!(written
                .windows(text.len())
                .any(|part| part == text.as_bytes()));
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
        screen.agent_control(prompt(true), CLIENT);
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
        screen.agent_control(prompt(true), ClientId::new(2));
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
        screen.agent_control(prompt(true), CLIENT);
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
fn claude_confirmation_failure_retains_the_uncertain_submission_claim() {
    let (mut screen, messages, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    screen.agent_control(prompt(true), CLIENT);
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
    screen.agent_control(prompt(true), CLIENT);
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
        (
            Some(NotReadyReason::UnconsumedInput),
            "unconsumed terminal input",
        ),
        (None, ""),
    ] {
        let (mut screen, messages, _writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        if matches!(
            reason,
            Some(
                NotReadyReason::PromptUnsupported
                    | NotReadyReason::NotIdle
                    | NotReadyReason::PendingSubmission
            )
        ) {
            screen.agent_control(prompt(true), ClientId::new(2));
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
            screen.agent_control(request, CLIENT);
            let status = replies(&messages, CLIENT).pop().unwrap().agents.remove(0);
            assert_eq!(status.not_ready_reason, reason);
            assert_eq!(status.ready, reason.is_none());
        }
        if reason.is_some() {
            screen.agent_control(prompt(true), CLIENT);
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
            screen.agent_control(prompt(true), CLIENT);
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
            screen.agent_control(prompt(true), CLIENT);
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
            screen.agent_control(prompt(true), CLIENT);
            if manual {
                assert_eq!(
                    replies(&messages, CLIENT)[0].outcome,
                    AgentOutcome::Rejected
                );
            } else {
                assert!(replies(&messages, CLIENT).is_empty());
                assert_eq!(screen.agent_controls.len(), 1);
            }
        }
    }
}

#[test]
fn lost_or_exited_targets_never_recommend_recovery() {
    for change in ["closed", "replaced", "exited", "session-ended"] {
        for acknowledged in [false, true] {
            let (mut screen, messages, writes) = setup("codex");
            report(&mut screen, hook("SessionStart"));
            screen.agent_control(prompt(true), CLIENT);
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
