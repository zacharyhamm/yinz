use super::*;

fn setup_tracking(
    kind: &str,
) -> (
    Screen,
    CapturedClientMessages,
    Receiver<(PtyWriteInstruction, ErrorContext)>,
) {
    if kind == "opencode" {
        yinz_types::agent::provider::register(
            71009,
            yinz_types::agent::provider::AgentProvider {
                kind: kind.into(),
                executable: kind.into(),
                ..Default::default()
            },
        )
        .unwrap();
    }
    setup(kind)
}

fn transfer(source: &mut Screen, kind: &str) -> Screen {
    let handoff = source.capture_agent_tracking(PANE).unwrap();
    // Exercise the wire representation, including the deliberate absence of process clocks.
    let json = serde_json::to_string(&handoff).unwrap();
    assert!(!json.contains("last_report"));
    let decoded = serde_json::from_str(&json).unwrap();
    assert_eq!(handoff, decoded);
    let (mut destination, _, _) = setup_tracking(kind);
    destination
        .pending_agent_tracking
        .insert(PANE, (Some(handoff.agent.clone()), Some(decoded)));
    destination.restore_staged_agent_tracking(PANE);
    destination
}

#[test]
fn warp_tracking_preserves_provider_states_and_completion_correlation() {
    for kind in [
        "claude", "codex", "agy", "muse", "pi", "opencode", "cursor", "qwen", "gemini", "aider",
        "cline",
    ] {
        for state in ["busy", "idle", "waiting", "interrupted"] {
            let (mut source, _, _) = setup_tracking(kind);
            report_kind(&mut source, kind, hook("SessionStart"));
            report_kind(&mut source, kind, hook("UserPromptSubmit"));
            match state {
                "idle" => report_kind(&mut source, kind, hook("Stop")),
                "waiting" => report_kind(&mut source, kind, permission_wait(kind)),
                "interrupted" => report_kind(&mut source, kind, hook("Interrupt")),
                _ => {},
            }
            let before = source.agent_status_for_test();
            let expected = source.capture_agent_tracking(PANE).unwrap();
            let mut destination = transfer(&mut source, kind);
            let after = destination.agent_status_for_test();
            assert_eq!(after.agent, before.agent, "{kind}/{state}");
            assert_eq!(
                after.status_available, before.status_available,
                "{kind}/{state}"
            );
            assert_eq!(
                after.not_ready_reason, before.not_ready_reason,
                "{kind}/{state}"
            );
            assert_eq!(
                destination.capture_agent_tracking(PANE).unwrap().lifecycle,
                expected.lifecycle,
                "{kind}/{state}"
            );
            // Repeated transfers and a rollback use exactly this same snapshot.
            destination = transfer(&mut destination, kind);
            if state == "interrupted" {
                report_kind(&mut destination, kind, hook("PreToolUse"));
                report_kind(&mut source, kind, hook("PreToolUse"));
                assert_eq!(
                    destination.agent_status_for_test().agent,
                    source.agent_status_for_test().agent,
                    "late tool {kind}"
                );
            }
            report_kind(&mut destination, kind, hook("Stop"));
            report_kind(&mut source, kind, hook("Stop"));
            assert_eq!(
                destination.agent_status_for_test().agent,
                source.agent_status_for_test().agent,
                "{kind}/{state}"
            );
            let completed = destination.capture_agent_tracking(PANE).unwrap().lifecycle;
            report_kind(&mut destination, kind, hook("Stop"));
            assert_eq!(
                destination.capture_agent_tracking(PANE).unwrap().lifecycle,
                completed,
                "duplicate {kind}/{state}"
            );
        }
    }
}

#[test]
fn warp_tracking_preserves_provisional_muse_and_rejects_mismatched_identity() {
    let (mut source, _, _) = setup_tracking("muse");
    muse_output(&mut source, &muse_composer_frame(""));
    report_kind(&mut source, "muse", muse_bootstrap(true));
    let mut destination = transfer(&mut source, "muse");
    assert!(destination.agent_status_for_test().status_available);
    assert!(destination
        .agent_status_for_test()
        .agent
        .session_id
        .is_none());
    report_kind(&mut destination, "muse", hook("UserPromptSubmit"));
    assert!(destination.has_agent_lifecycle(PANE, "muse", "conversation"));
    let original = destination.capture_agent_tracking(PANE).unwrap();
    for mismatch in ["kind", "conversation", "process"] {
        let mut handoff = original.clone();
        match mismatch {
            "kind" => handoff.lifecycle.as_mut().unwrap().kind = "claude".into(),
            "conversation" => handoff.lifecycle.as_mut().unwrap().conversation = "other".into(),
            _ => {},
        }
        let (mut rejected, _, _) = setup_tracking(if mismatch == "process" {
            "bash"
        } else {
            "muse"
        });
        rejected
            .pending_agent_tracking
            .insert(PANE, (Some(handoff.agent.clone()), Some(handoff)));
        rejected.restore_staged_agent_tracking(PANE);
        assert!(
            !rejected.has_agent_lifecycle(PANE, "muse", "conversation"),
            "{mismatch}"
        );
    }
}

#[test]
fn warp_tracking_keeps_overlapping_claude_turns_workers_and_background_tasks() {
    let (mut source, messages, writes) = setup_tracking("claude");
    complete_with_background(&mut source, &messages, &writes, &["task"]);
    report_kind(
        &mut source,
        "claude",
        task_notification("notification", &["task"]),
    );
    report_kind(
        &mut source,
        "claude",
        HookEvent {
            prompt_id: Some("new-prompt".into()),
            ..hook("UserPromptSubmit")
        },
    );
    report_kind(
        &mut source,
        "claude",
        HookEvent {
            agent_id: Some("worker".into()),
            ..hook("PreToolUse")
        },
    );
    report_kind(
        &mut source,
        "claude",
        HookEvent {
            agent_id: Some("worker".into()),
            ..permission_wait("claude")
        },
    );
    let expected = source.capture_agent_tracking(PANE).unwrap();
    let mut destination = transfer(&mut source, "claude");
    let restored = destination.capture_agent_tracking(PANE).unwrap();
    assert_eq!(restored.worker_waits, expected.worker_waits);
    assert_eq!(restored.lifecycle, expected.lifecycle);
    for completion in [
        claude_stop("unrelated", "ignored", &[]),
        claude_stop("notification", "background done", &[]),
        claude_stop("new-prompt", "finished", &[]),
    ] {
        report_kind(&mut source, "claude", completion.clone());
        report_kind(&mut destination, "claude", completion);
        assert_eq!(
            destination.agent_status_for_test().agent,
            source.agent_status_for_test().agent
        );
        assert_eq!(
            destination.capture_agent_tracking(PANE).unwrap().lifecycle,
            source.capture_agent_tracking(PANE).unwrap().lifecycle
        );
    }
}

#[test]
fn warp_tracking_rebases_consumed_draft_and_permission_revisions() {
    for kind in ["claude", "codex", "agy", "muse", "pi", "opencode"] {
        for draft in [false, true] {
            let (mut source, _, _) = setup_tracking(kind);
            report_kind(&mut source, kind, hook("SessionStart"));
            if draft {
                source.agent_write_for_test();
            }
            let mut destination = transfer(&mut source, kind);
            report_kind(&mut destination, kind, hook("SessionStart"));
            assert_eq!(
                destination.agent_status_for_test().not_ready_reason,
                source.agent_status_for_test().not_ready_reason,
                "{kind} draft={draft}"
            );
            if draft {
                assert!(!destination.agent_status_for_test().ready, "{kind}");
            }
        }
    }
    let (mut source, _, _) = setup_tracking("claude");
    report_kind(&mut source, "claude", hook("SessionStart"));
    source.agent_write_for_test();
    report_kind(&mut source, "claude", claude_permission_prompt());
    let mut destination = transfer(&mut source, "claude");
    assert_eq!(
        destination
            .capture_agent_tracking(PANE)
            .unwrap()
            .permission_wait
            .unwrap()
            .input_revision,
        0
    );
    destination.agent_write_for_test();
    let mut again = transfer(&mut destination, "claude");
    assert_ne!(
        again
            .capture_agent_tracking(PANE)
            .unwrap()
            .permission_wait
            .unwrap()
            .input_revision,
        0
    );
}

#[test]
fn warp_tracking_finalizes_requests_and_retains_launch_and_submission_history() {
    let (mut source, messages, writes) = setup_tracking("codex");
    report(&mut source, hook("SessionStart"));
    source.agent_launches.insert(
        PANE,
        yinz_types::agent::SpawnedAgent {
            pane_id: PANE,
            branch: Some("task-branch".into()),
            ..Default::default()
        },
    );
    let mut request = prompt(true);
    if let AgentRequest::Prompt { submission_id, .. } = &mut request {
        *submission_id = Some("warp-request".into());
    }
    source.agent_control_with_clear(request, CLIENT, &writes);
    finish_paste(&writes);
    report(&mut source, hook("UserPromptSubmit"));
    let mut destination = transfer(&mut source, "codex");
    assert_eq!(
        replies(&messages, CLIENT).last().unwrap().outcome,
        AgentOutcome::TargetChanged
    );
    let snapshot = destination.capture_agent_tracking(PANE).unwrap();
    assert_eq!(
        snapshot.submissions[0].result.as_ref().unwrap().outcome,
        AgentOutcome::TargetChanged
    );
    assert_eq!(
        snapshot.launch.unwrap().branch.as_deref(),
        Some("task-branch")
    );
    assert!(source.agent_controls.is_empty());
    assert!(destination.agent_controls.is_empty());
    let (reply, result) = std::sync::mpsc::channel();
    destination.agent_control_at(
        AgentRequest::WaitTurn {
            pane_id: PANE,
            expected_session_id: Some("conversation".into()),
            submission_id: "warp-request".into(),
            timeout_ms: 30_000,
        },
        crate::screen::agent_control::Reply::Host(reply),
        Instant::now(),
    );
    assert_eq!(
        result.try_recv().unwrap().outcome,
        AgentOutcome::TargetChanged
    );
}

#[test]
fn warp_tracking_restores_before_a_completion_queued_before_creation() {
    for kind in ["claude", "agy", "codex", "muse", "pi", "opencode"] {
        let (mut source, _, _) = setup_tracking(kind);
        report_kind(&mut source, kind, hook("SessionStart"));
        report_kind(&mut source, kind, hook("UserPromptSubmit"));
        let handoff = source.capture_agent_tracking(PANE).unwrap();
        let (mut destination, _, _) = setup_tracking(kind);
        let command = destination
            .tab_containing_pane(PANE)
            .unwrap()
            .pane_run_command(PANE)
            .unwrap();
        destination
            .tab_containing_pane_mut(PANE)
            .unwrap()
            .extract_warp_panes(&[PANE]);
        destination
            .pending_agent_tracking
            .insert(PANE, (Some(handoff.agent.clone()), Some(handoff)));
        let mut pending = crate::screen::thread_main::PendingEvents::default();
        let completed = hook("Stop");
        let HookOutcome::Reported(agent) = hook_outcome(kind, &completed) else {
            panic!("completion must report");
        };
        pending.waiting_for_pane.insert(
            PANE,
            vec![ScreenInstruction::AgentReport(
                AgentReport {
                    report_id: "queued-completion".into(),
                    pane_id: PANE,
                    state: AgentReportState::Set(agent),
                    event: Some(AgentEvent::from_hook(
                        kind,
                        TerminalId::new(7),
                        &completed,
                        serde_json::to_string(&completed).unwrap(),
                    )),
                    trace: false,
                    process_id: ProcessId::new(42),
                    kind: kind.into(),
                },
                Instant::now(),
            )],
        );
        destination
            .place_new_pane(
                PANE,
                None,
                None,
                Some(Run::Command(command)),
                NewPanePlacement::default(),
                false,
                crate::pty::ClientTabIndexOrPaneId::TabIndex(TabId::new(0)),
                None,
                false,
                &mut pending,
            )
            .unwrap();
        assert_eq!(
            destination.agent_status_for_test().agent.state,
            AgentState::Idle,
            "{kind}"
        );
        assert!(
            destination.agent_status_for_test().status_available,
            "{kind}"
        );
        assert!(!destination.pending_agent_tracking.contains_key(&PANE));
        // A later adoption acknowledgement cannot reapply the old Busy snapshot.
        destination.restore_staged_agent_tracking(PANE);
        assert_eq!(
            destination.agent_status_for_test().agent.state,
            AgentState::Idle
        );
    }
}

#[test]
fn warp_tracking_rejects_an_incompatible_destination_before_release() {
    for whole_tab in [false, true] {
        let (mut source, _, _) = setup_tracking("claude");
        report_kind(&mut source, "claude", hook("SessionStart"));
        let (to_pty, pty) = channels::unbounded();
        source.bus.senders.to_pty = Some(SenderWithContext::new(to_pty));
        let deadline = Instant::now() + Duration::from_secs(30);
        let reason = if whole_tab {
            let (tx, rx) = crossbeam::channel::bounded(1);
            source
                .warp_tab(true, true, false, Some(TabId::new(0)), CLIENT, tx, deadline)
                .unwrap();
            rx.try_recv().unwrap().unwrap_err()
        } else {
            let (tx, rx) = yinz_bus::warp::pane_warp_reply();
            source
                .warp_pane(
                    true,
                    true,
                    false,
                    Some(PANE),
                    "destination".into(),
                    yinz_types::data::WarpPlacement::NewTab,
                    CLIENT,
                    tx,
                    deadline,
                )
                .unwrap();
            rx.try_recv().unwrap().unwrap_err()
        };
        assert!(reason.contains("agent tracking"), "{reason}");
        assert!(pty.try_iter().next().is_none());
        assert!(source.has_agent_lifecycle(PANE, "claude", "conversation"));
    }
}

#[test]
fn warp_tracking_keeps_unknown_input_unknown_without_a_lifecycle() {
    for kind in ["claude", "codex", "agy", "muse", "pi", "opencode"] {
        for event in ["SessionStart", "UserPromptSubmit", "PreInvocation"] {
            let (mut source, _, _) = setup_tracking(kind);
            let HookOutcome::Reported(agent) = hook_outcome(kind, &hook("UserPromptSubmit")) else {
                panic!("reported agent");
            };
            crate::screen::apply_pane_agent_report(&mut source, PANE, Some(agent)).unwrap();
            source.agent_write_for_test();
            let mut destination = transfer(&mut source, kind);
            assert_eq!(
                destination
                    .capture_agent_tracking(PANE)
                    .unwrap()
                    .input_consumed,
                None
            );
            destination = transfer(&mut destination, kind);
            assert_eq!(
                destination
                    .capture_agent_tracking(PANE)
                    .unwrap()
                    .input_consumed,
                None
            );
            report_kind(&mut destination, kind, hook(event));
            let snapshot = destination.capture_agent_tracking(PANE).unwrap();
            assert!(
                snapshot
                    .lifecycle
                    .is_none_or(|l| l.input_revision.is_none()),
                "{kind}/{event}"
            );
        }
    }
}

#[test]
fn warp_tracking_does_not_resume_partial_prompt_delivery() {
    let (mut source, _, writes) = setup_tracking("codex");
    report(&mut source, hook("SessionStart"));
    source.agent_control_with_clear(prompt(true), CLIENT, &writes);
    // The writer has not acknowledged the paste and no submit key may be resumed.
    let mut destination = transfer(&mut source, "codex");
    assert!(destination.agent_controls.is_empty());
    assert_eq!(
        destination
            .capture_agent_tracking(PANE)
            .unwrap()
            .input_consumed,
        None
    );
    report(&mut destination, hook("Stop"));
    assert!(!destination.agent_status_for_test().ready);
}
