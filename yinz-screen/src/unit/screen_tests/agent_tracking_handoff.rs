use super::*;
use yinz_types::input::hooks::HookPoint;

/// Drive the screen side of adoption, including the layout reply and held-state replay.
fn adopt_tracking(
    source: &mut Screen,
    path: &str,
    fail_layout: bool,
    include_tracking: bool,
) -> (Screen, Receiver<(PluginInstruction, ErrorContext)>) {
    use yinz_types::data::{WarpHiddenPane, WarpPlacement, WarpRelationships, WarpedTab};

    let command = source
        .tab_containing_pane(PANE)
        .unwrap()
        .pane_run_command(PANE)
        .unwrap();
    let held = source
        .tab_containing_pane(PANE)
        .unwrap()
        .get_pane_with_id(PANE)
        .unwrap()
        .held_for_warp();
    let payload = WarpedPane {
        agent: source.get_pane_info(PANE).unwrap().agent,
        agent_tracking: include_tracking
            .then(|| source.capture_agent_tracking(PANE))
            .flatten(),
        held: held.clone(),
        ..Default::default()
    };
    let (mut destination, _, _) = setup("bash");
    destination
        .tab_containing_pane_mut(PANE)
        .unwrap()
        .extract_warp_panes(&[PANE]);
    let (to_plugin, plugins) = channels::unbounded();
    destination.bus.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
    let mut pending = crate::screen::thread_main::PendingEvents::default();
    if path == "pane" {
        let (to_screen, instructions) = channels::unbounded();
        destination.bus.senders.to_screen = Some(SenderWithContext::new(to_screen));
        destination
            .place_adopted_pane(
                TerminalId::new(7),
                payload,
                WarpPlacement::FocusedTab,
                CLIENT,
                command,
                vec![],
                held.as_ref().map(|held| held.exit_status),
                &mut None,
            )
            .unwrap();
        for (instruction, _) in instructions.try_iter() {
            match instruction {
                ScreenInstruction::NewPane(
                    id,
                    title,
                    hold,
                    run,
                    placement,
                    suppressed,
                    target,
                    completion,
                    first,
                ) => {
                    destination
                        .place_new_pane(
                            id,
                            title,
                            hold,
                            run,
                            placement,
                            suppressed,
                            target,
                            completion,
                            first,
                            &mut pending,
                        )
                        .unwrap();
                },
                ScreenInstruction::RestoreHeldPane(id, held) => {
                    assert!(destination.restore_held_pane(id, &held));
                },
                _ => {},
            }
        }
    } else {
        let companion = path == "companion";
        let cover = TerminalId::new(8);
        let cover_command = RunCommand {
            command: "bash".into(),
            ..Default::default()
        };
        let pane_layout = TiledPaneLayout {
            run: Some(Run::Command(if companion {
                cover_command.clone()
            } else {
                command.clone()
            })),
            ..Default::default()
        };
        let floating = if companion {
            vec![FloatingPaneLayout {
                run: Some(Run::Command(command.clone())),
                ..Default::default()
            }]
        } else {
            vec![]
        };
        let adopted = crate::pty::AdoptedPane {
            terminal_id: TerminalId::new(7),
            cmd: command.clone(),
            buffered: vec![],
            child_exit_status: held.as_ref().map(|held| held.exit_status),
        };
        let (tiled_panes, floating_panes, tiled, floating_adopted) = if companion {
            (
                vec![WarpedPane::default()],
                vec![payload],
                vec![crate::pty::AdoptedPane {
                    terminal_id: cover,
                    cmd: cover_command.clone(),
                    buffered: vec![],
                    child_exit_status: None,
                }],
                vec![adopted],
            )
        } else {
            (vec![payload], vec![], vec![adopted], vec![])
        };
        let tab_id = destination.get_new_tab_id();
        let (receipt_tx, receipt) = crossbeam::channel::bounded(1);
        destination
            .place_adopted_tab(
                WarpedTab {
                    tiled_layout: pane_layout.clone(),
                    floating_layout: floating.clone(),
                    tiled_panes,
                    floating_panes,
                    relationships: Some(WarpRelationships {
                        hidden: if companion {
                            vec![WarpHiddenPane {
                                pane: 1,
                                restored_by: Some(0),
                                is_scrollback_editor: false,
                            }]
                        } else {
                            vec![]
                        },
                        stacks: vec![],
                    }),
                    ..Default::default()
                },
                crate::pty::AdoptedTabPanes {
                    tiled,
                    floating: floating_adopted,
                },
                CLIENT,
                false,
                companion.then_some(WarpPlacement::FocusedTab),
                Some(receipt_tx),
                None,
            )
            .unwrap();
        let layout = if fail_layout {
            TiledPaneLayout {
                children: vec![
                    TiledPaneLayout {
                        split_size: Some(yinz_types::geometry::SplitSize::Fixed(10_000)),
                        ..Default::default()
                    },
                    TiledPaneLayout::default(),
                ],
                ..Default::default()
            }
        } else {
            pane_layout
        };
        let hold = held.as_ref().map(|_| command.clone());
        let (tiled_ids, floating_ids, adopted_commands) = if companion {
            (
                vec![(cover, None)],
                vec![(TerminalId::new(7), hold)],
                vec![
                    (cover, cover_command, None),
                    (TerminalId::new(7), command, None),
                ],
            )
        } else {
            (
                vec![(TerminalId::new(7), hold)],
                vec![],
                vec![(TerminalId::new(7), command, None)],
            )
        };
        let (completion_tx, completion) = tokio::sync::oneshot::channel();
        destination
            .land_layout(
                layout,
                floating,
                tiled_ids,
                floating_ids,
                adopted_commands,
                HashMap::new(),
                tab_id,
                true,
                CLIENT,
                Some(crate::route::NotificationEnd::new(completion_tx)),
                None,
                &mut pending,
            )
            .unwrap();
        assert_eq!(
            completion.blocking_recv().unwrap().exit_status == Some(1),
            fail_layout
        );
        assert!(
            receipt.try_recv().unwrap().is_ok(),
            "adoption must acknowledge retained panes"
        );
        if let Some(held) = held {
            assert!(destination.restore_held_pane(PANE, &held));
        }
    }
    assert!(
        !destination.restore_staged_transfer_state(PANE),
        "the staged identity must be consumed exactly once"
    );
    (destination, plugins)
}

fn arrival_hooks(plugins: &Receiver<(PluginInstruction, ErrorContext)>) -> Vec<HookPoint> {
    plugins
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::RunHook {
                point,
                pane_id: Some(PANE),
                payload,
                ..
            } if matches!(point, HookPoint::AgentStart | HookPoint::AgentWaiting) => {
                let crate::plugins::HookFiring::Headless(yinz_types::data::HookPayload::Agent {
                    agent,
                    ..
                }) = payload
                else {
                    panic!("agent hook payload")
                };
                assert_eq!(
                    agent.state,
                    AgentState::Waiting,
                    "hooks must see the restored identity"
                );
                Some(point)
            },
            _ => None,
        })
        .collect()
}

#[test]
fn warp_tracking_session_end_stays_absent_in_held_panes_tabs_and_companions() {
    for path in ["pane", "tab", "companion"] {
        for fail_layout in [false, true]
            .into_iter()
            .filter(|failed| !failed || path != "pane")
        {
            let (mut source, _, _) = setup("codex");
            report(&mut source, hook("SessionStart"));
            report(&mut source, hook("SessionEnd"));
            let tab = source.tab_containing_pane_mut(PANE).unwrap();
            tab.hold_pane(PANE, Some(0), false, tab.pane_run_command(PANE).unwrap());
            assert!(source.get_pane_info(PANE).unwrap().agent.is_none());
            assert!(source.capture_agent_tracking(PANE).is_none());
            let (destination, plugins) = adopt_tracking(&mut source, path, fail_layout, true);
            let info = destination.get_pane_info(PANE).unwrap();
            assert!(info.is_held);
            assert!(info.agent.is_none(), "{path}, recovery={fail_layout}");
            assert!(arrival_hooks(&plugins).is_empty());
        }
    }
}

#[test]
fn warp_tracking_arrival_hooks_fire_once_after_restoring_waiting_identity() {
    for path in ["pane", "tab", "companion"] {
        for fail_layout in [false, true]
            .into_iter()
            .filter(|failed| !failed || path != "pane")
        {
            for include_tracking in [false, true] {
                let (mut source, _, _) = setup("codex");
                report(&mut source, hook("SessionStart"));
                report(&mut source, permission_wait("codex"));
                let expected = source.capture_agent_tracking(PANE).unwrap();
                let (mut destination, plugins) =
                    adopt_tracking(&mut source, path, fail_layout, include_tracking);
                assert_eq!(
                    destination.get_pane_info(PANE).unwrap().agent.as_ref(),
                    Some(&expected.agent)
                );
                if include_tracking {
                    assert_eq!(
                        destination.capture_agent_tracking(PANE).unwrap().lifecycle,
                        expected.lifecycle
                    );
                }
                assert_eq!(
                    arrival_hooks(&plugins),
                    vec![HookPoint::AgentStart, HookPoint::AgentWaiting],
                    "{path}, recovery={fail_layout}, tracking={include_tracking}"
                );
                report(&mut destination, permission_wait("codex"));
                report(&mut destination, permission_wait("codex"));
                assert!(!destination.restore_staged_transfer_state(PANE));
                assert!(arrival_hooks(&plugins).is_empty());
            }
        }
    }
}

#[test]
fn warp_tracking_explicit_held_agent_records_survive_all_adoption_paths() {
    for path in ["pane", "tab", "companion"] {
        for fail_layout in [false, true]
            .into_iter()
            .filter(|failed| !failed || path != "pane")
        {
            let (mut source, _, _) = setup("codex");
            report(&mut source, hook("SessionStart"));
            let tab = source.tab_containing_pane_mut(PANE).unwrap();
            tab.hold_pane(PANE, Some(3), false, tab.pane_run_command(PANE).unwrap());
            // An explicit record can accompany a held snapshot; restoring it isn't a new exit.
            report(&mut source, permission_wait("codex"));
            let expected = source.get_pane_info(PANE).unwrap().agent;
            let (destination, plugins) = adopt_tracking(&mut source, path, fail_layout, true);
            let info = destination.get_pane_info(PANE).unwrap();
            assert!(info.is_held);
            assert_eq!(info.agent, expected, "{path}, recovery={fail_layout}");
            assert_eq!(
                arrival_hooks(&plugins),
                vec![HookPoint::AgentStart, HookPoint::AgentWaiting]
            );
        }
    }
}

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
    let (mut destination, _, _) = setup_tracking(kind);
    transfer_into(source, &mut destination);
    destination
}

fn transfer_into(source: &mut Screen, destination: &mut Screen) {
    let handoff = source.capture_agent_tracking(PANE).unwrap();
    // Exercise the wire representation, including the deliberate absence of process clocks.
    let json = serde_json::to_string(&handoff).unwrap();
    assert!(!json.contains("last_report"));
    let decoded = serde_json::from_str(&json).unwrap();
    assert_eq!(handoff, decoded);
    destination
        .pending_agent_tracking
        .insert(PANE, (Some(handoff.agent.clone()), Some(decoded)));
    assert!(destination.restore_staged_transfer_state(PANE));
}

#[test]
fn warp_tracking_legacy_worker_wait_cannot_be_released_by_a_newer_tool() {
    let (mut source, _, _) = setup_tracking("claude");
    report_kind(&mut source, "claude", hook("SessionStart"));
    report_kind(
        &mut source,
        "claude",
        claude_worker_input("PreToolUse", "worker", Some("old-question")),
    );
    let handoff = source.capture_agent_tracking(PANE).unwrap();
    let mut legacy = serde_json::to_value(&handoff).unwrap();
    legacy["lifecycle"]["claude"]
        .as_object_mut()
        .unwrap()
        .remove("worker_wait_tools");
    let decoded = serde_json::from_value(legacy).unwrap();
    let (mut destination, _, _) = setup_tracking("claude");
    destination
        .pending_agent_tracking
        .insert(PANE, (Some(handoff.agent), Some(decoded)));
    assert!(destination.restore_staged_transfer_state(PANE));
    for event in [
        claude_worker_input("PreToolUse", "worker", Some("new-question")),
        claude_worker_input("PostToolUse", "worker", Some("new-question")),
        claude_worker_input("PostToolUse", "worker", Some("old-question")),
    ] {
        report_kind(&mut destination, "claude", event);
        assert_eq!(
            destination.agent_status_for_test().agent.state,
            AgentState::Waiting
        );
    }
}

#[test]
fn warp_tracking_keeps_worker_tool_releases() {
    let (mut source, _, _) = setup_tracking("claude");
    report_kind(&mut source, "claude", hook("SessionStart"));
    let task = "worker";
    report_kind(&mut source, "claude", claude_worker("SubagentStart", task));
    for (name, tool) in [
        ("PreToolUse", "question-1"),
        ("PostToolUse", "question-1"),
        ("PreToolUse", "question-2"),
    ] {
        report_kind(
            &mut source,
            "claude",
            claude_worker_input(name, task, Some(tool)),
        );
    }
    let mut destination = transfer(&mut source, "claude");
    for event in [
        claude_worker("SubagentStop", task),
        claude_worker_input("PostToolUse", task, Some("question-1")),
        claude_worker_input("PreToolUse", task, Some("question-1")),
    ] {
        report_kind(&mut destination, "claude", event);
        assert_eq!(
            destination.agent_status_for_test().agent.state,
            AgentState::Waiting
        );
    }
    report_kind(
        &mut destination,
        "claude",
        claude_worker_input("PostToolUse", task, Some("question-2")),
    );
    assert_eq!(
        destination.agent_status_for_test().agent.state,
        AgentState::Idle
    );
}

#[test]
fn warp_tracking_claude_background_and_waits_survive_transfer() {
    let (mut source, messages, writes) = setup_tracking("claude");
    complete_with_background(&mut source, &messages, &writes, true);
    report_kind(
        &mut source,
        "claude",
        HookEvent {
            tool_name: Some("AskUserQuestion".into()),
            ..claude_worker("PreToolUse", "worker")
        },
    );
    let mut destination = transfer(&mut source, "claude");
    assert_eq!(
        destination
            .capture_agent_tracking(PANE)
            .unwrap()
            .lifecycle
            .unwrap()
            .claude_background
            .as_deref(),
        Some("prompt")
    );
    report_kind(
        &mut destination,
        "claude",
        claude_worker("SubagentStop", "worker"),
    );
    assert!(destination.agent_worker_waits[&PANE]
        .waits
        .contains_key("worker"));
    report_kind(
        &mut destination,
        "claude",
        claude_worker("PostToolUse", "worker"),
    );
    assert_eq!(
        destination.agent_status_for_test().agent.state,
        AgentState::Busy
    );
    let mut again = transfer(&mut destination, "claude");
    report_kind(&mut again, "claude", background_idle("prompt"));
    assert_eq!(again.agent_status_for_test().agent.state, AgentState::Idle);
    let mut legacy = serde_json::to_value(again.capture_agent_tracking(PANE).unwrap()).unwrap();
    legacy["lifecycle"]
        .as_object_mut()
        .unwrap()
        .remove("claude_background");
    for field in [
        "worker_wait_tools",
        "uncorrelated_worker_waits",
        "released_worker_tools",
    ] {
        legacy["lifecycle"]["claude"]
            .as_object_mut()
            .unwrap()
            .remove(field);
    }
    let decoded: yinz_types::agent::tracking::AgentTrackingHandoff =
        serde_json::from_value(legacy).unwrap();
    let lifecycle = decoded.lifecycle.unwrap();
    assert!(lifecycle.claude_background.is_none());
    assert!(lifecycle.claude.worker_wait_tools.is_empty());
    assert!(lifecycle.claude.uncorrelated_worker_waits.is_empty());
    assert!(lifecycle.claude.released_worker_tools.is_empty());
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
        rejected.restore_staged_transfer_state(PANE);
        assert!(
            !rejected.has_agent_lifecycle(PANE, "muse", "conversation"),
            "{mismatch}"
        );
    }
}

#[test]
fn warp_tracking_keeps_overlapping_claude_turns_workers_and_background() {
    let (mut source, messages, writes) = setup_tracking("claude");
    complete_with_background(&mut source, &messages, &writes, true);
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
        claude_stop("unrelated", "ignored", false),
        claude_stop("notification", "background done", false),
        claude_stop("new-prompt", "finished", false),
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
    assert_eq!(
        again
            .capture_agent_tracking(PANE)
            .unwrap()
            .permission_wait
            .unwrap()
            .input_revision,
        0
    );
}

fn permission_wait_survives_transfers(notification: bool) {
    for cli in [false, true] {
        for prior_destination_enter in [false, true] {
            for transfers in [1, 2] {
                let (mut source, messages, mut writes) = setup_tracking("claude");
                if notification {
                    complete_with_background(&mut source, &messages, &writes, true);
                    report_kind(
                        &mut source,
                        "claude",
                        task_notification("notification", &["bash"]),
                    );
                } else {
                    report_kind(&mut source, "claude", hook("SessionStart"));
                    report_kind(&mut source, "claude", hook("UserPromptSubmit"));
                }
                report_kind(
                    &mut source,
                    "claude",
                    HookEvent {
                        prompt_id: notification.then(|| "notification".into()),
                        ..claude_permission_prompt()
                    },
                );
                let waiting = source.agent_status_for_test().agent;
                assert_eq!(waiting.state, AgentState::Waiting);
                for _ in 0..transfers {
                    permission_key(&mut source, KeyWithModifier::new(BareKey::Down), cli);
                    assert_eq!(source.agent_status_for_test().agent, waiting);
                    let (mut destination, _, destination_writes) = setup_tracking("claude");
                    if prior_destination_enter {
                        report_kind(&mut destination, "claude", hook("SessionStart"));
                        permission_key(&mut destination, KeyWithModifier::new(BareKey::Enter), cli);
                        assert_eq!(finish_paste(&destination_writes), b"\r");
                    }
                    transfer_into(&mut source, &mut destination);
                    destination.refresh_agent_controls();
                    assert_eq!(
                        destination.agent_status_for_test().agent,
                        waiting,
                        "earlier destination Enter must not answer a transferred prompt"
                    );
                    source = destination;
                    writes = destination_writes;
                }
                permission_key(&mut source, KeyWithModifier::new(BareKey::Down), cli);
                assert_eq!(source.agent_status_for_test().agent, waiting);
                source
                    .tab_containing_pane_mut(PANE)
                    .unwrap()
                    .get_pane_with_id_mut(PANE)
                    .unwrap()
                    .set_suspended(true);
                permission_key(&mut source, KeyWithModifier::new(BareKey::Enter), cli);
                assert_eq!(source.agent_status_for_test().agent, waiting);
                source
                    .tab_containing_pane_mut(PANE)
                    .unwrap()
                    .get_pane_with_id_mut(PANE)
                    .unwrap()
                    .set_suspended(false);
                finish_paste(&writes);
                permission_key(&mut source, KeyWithModifier::new(BareKey::Enter), cli);
                assert_eq!(finish_paste(&writes), b"\r");
                assert_eq!(source.agent_status_for_test().agent.state, AgentState::Busy);
                assert!(source.claude_permission_waits.is_empty());
                assert!(source
                    .capture_agent_tracking(PANE)
                    .unwrap()
                    .lifecycle
                    .unwrap()
                    .claude_notifications
                    .values()
                    .all(|notification| notification.permission_revision.is_none()));
            }
        }
    }
}

#[test]
fn warp_tracking_claude_permission_wait_clears_after_delivered_enter() {
    permission_wait_survives_transfers(false);
}

#[test]
fn warp_tracking_notification_permission_wait_clears_after_delivered_enter() {
    permission_wait_survives_transfers(true);
}

#[test]
fn warp_tracking_failed_notification_observer_keeps_native_permission_ownership() {
    for loss_after_transfer in [false, true] {
        let (mut source, _, _) = setup_tracking("claude");
        report_kind(&mut source, "claude", hook("SessionStart"));
        report_kind(&mut source, "claude", hook("UserPromptSubmit"));
        report_kind(
            &mut source,
            "claude",
            native_background_launch("bash", "tool", "Bash"),
        );
        report_kind(&mut source, "claude", sourced_stop("prompt", "LAUNCH"));
        report_kind(
            &mut source,
            "claude",
            task_notification("notice", &["bash"]),
        );
        report_kind(
            &mut source,
            "claude",
            identified_notice("notice", "bash", "tool", "run"),
        );
        report_kind(
            &mut source,
            "claude",
            HookEvent {
                prompt_id: Some("notice".into()),
                ..claude_permission_prompt()
            },
        );
        if !loss_after_transfer {
            lose_notification_observation(&mut source, "notice");
        }
        for i in 0..2 {
            let (mut destination, _, writes) = setup_tracking("claude");
            report_kind(&mut destination, "claude", hook("SessionStart"));
            permission_key(&mut destination, KeyWithModifier::new(BareKey::Enter), true);
            finish_paste(&writes);
            transfer_into(&mut source, &mut destination);
            if loss_after_transfer && i == 0 {
                lose_notification_observation(&mut destination, "budget");
            }
            destination.refresh_agent_controls();
            let status = destination.agent_status_for_test();
            assert_eq!(status.agent.state, AgentState::Waiting);
            assert!(status.agent.wait_needs_a_person);
            let state = claude_lifecycle(&destination);
            let notification = &state.claude_notifications["notice"];
            assert!(notification.observation_unavailable);
            assert!(notification.permission_revision.is_some());
            assert_eq!(state.last_message.as_deref(), Some("LAUNCH"));
            permission_key(&mut destination, KeyWithModifier::new(BareKey::Down), true);
            finish_paste(&writes);
            assert_eq!(
                destination.agent_status_for_test().agent.state,
                AgentState::Waiting
            );
            if i == 1 {
                permission_key(&mut destination, KeyWithModifier::new(BareKey::Enter), true);
                assert_eq!(finish_paste(&writes), b"\r");
                assert!(claude_lifecycle(&destination)
                    .claude_notifications
                    .is_empty());
                assert_eq!(
                    destination.agent_status_for_test().agent.state,
                    AgentState::Idle
                );
                report_kind(&mut destination, "claude", steering_ack("later"));
                report_kind(&mut destination, "claude", sourced_stop("later", "LATER"));
                assert_eq!(
                    destination.agent_lifecycle[&PANE].last_message_for_test(),
                    Some("LATER")
                );
            }
            source = destination;
        }
    }
}

#[test]
fn warp_tracking_completed_parent_background_permission_restores_idle_after_approval() {
    for cli in [false, true] {
        for caller_id in [false, true] {
            for prior_destination_enter in [false, true] {
                for transfers in [1, 2] {
                    let (mut source, mut messages, mut writes) = setup_tracking("claude");
                    complete_with_background(&mut source, &messages, &writes, true);
                    report_kind(
                        &mut source,
                        "claude",
                        claude_worker("SubagentStart", "worker"),
                    );
                    claude_background_permission_wait(&mut source, caller_id);
                    let waiting = source.agent_status_for_test().agent;
                    for _ in 0..transfers {
                        let (mut destination, destination_messages, destination_writes) =
                            setup_tracking("claude");
                        if prior_destination_enter {
                            report_kind(&mut destination, "claude", hook("SessionStart"));
                            permission_key(
                                &mut destination,
                                KeyWithModifier::new(BareKey::Enter),
                                cli,
                            );
                            assert_eq!(finish_paste(&destination_writes), b"\r");
                        }
                        transfer_into(&mut source, &mut destination);
                        destination.refresh_agent_controls();
                        assert_eq!(destination.agent_status_for_test().agent, waiting);
                        assert!(destination.agent_status_for_test().background);
                        assert_eq!(
                            claude_lifecycle(&destination).claude_confirmation,
                            Some(true)
                        );
                        assert!(claude_lifecycle(&destination)
                            .claude_notifications
                            .is_empty());
                        permission_key(&mut destination, KeyWithModifier::new(BareKey::Down), cli);
                        assert_eq!(destination.agent_status_for_test().agent, waiting);
                        source = destination;
                        messages = destination_messages;
                        writes = destination_writes;
                    }
                    finish_paste(&writes);
                    permission_key(&mut source, KeyWithModifier::new(BareKey::Enter), cli);
                    assert_eq!(finish_paste(&writes), b"\r");
                    assert!(source.claude_permission_waits.is_empty());
                    assert_eq!(
                        claude_lifecycle(&source)
                            .claude
                            .parent
                            .as_ref()
                            .unwrap()
                            .state,
                        AgentState::Idle
                    );
                    assert_eq!(source.agent_status_for_test().agent.state, AgentState::Busy);
                    draft_frame(&mut source, "claude", "", 0, Instant::now());
                    let waiter = ClientId::new(2);
                    source.agent_control(wait(AgentState::Idle), waiter);
                    for name in ["PreToolUse", "PostToolUse", "SubagentStop"] {
                        report_kind(
                            &mut source,
                            "claude",
                            claude_background_permission_request(name),
                        );
                        assert_eq!(source.agent_status_for_test().agent.state, AgentState::Busy);
                        assert!(replies(&messages, waiter).is_empty());
                    }
                    report_kind(
                        &mut source,
                        "claude",
                        task_notification("notification", &["worker"]),
                    );
                    report_kind(&mut source, "claude", background_idle("prompt"));
                    assert_eq!(source.agent_status_for_test().agent.state, AgentState::Busy);
                    assert!(replies(&messages, waiter).is_empty());
                    report_kind(
                        &mut source,
                        "claude",
                        claude_stop("notification", "Transferred worker finished.", false),
                    );
                    let done = replies(&messages, waiter);
                    assert_eq!(done.len(), 1);
                    assert_eq!(done[0].outcome, AgentOutcome::Ok);
                    assert_eq!(
                        done[0].message.as_deref(),
                        Some("Transferred worker finished.")
                    );
                    assert_eq!(source.agent_status_for_test().agent.state, AgentState::Idle);
                    assert!(!source.agent_status_for_test().background);
                }
            }
        }
    }
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
        destination.restore_staged_transfer_state(PANE);
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

#[test]
fn warp_tracking_retains_identified_obligations_and_run_tombstones() {
    for path in ["pane", "tab", "companion"] {
        let (mut source, _, _) = setup("claude");
        report_kind(&mut source, "claude", hook("SessionStart"));
        report_kind(&mut source, "claude", hook("UserPromptSubmit"));
        report_kind(
            &mut source,
            "claude",
            native_background_launch("bash", "tool", "Bash"),
        );
        report_kind(
            &mut source,
            "claude",
            claude_stop("prompt", "LAUNCH", false),
        );
        let (mut destination, _) = adopt_tracking(&mut source, path, false, true);
        assert_eq!(
            destination.agent_status_for_test().agent.state,
            AgentState::Busy
        );
        let pending = destination
            .agent_lifecycle
            .get(&PANE)
            .unwrap()
            .handoff_for_test();
        assert!(pending.claude.obligations.active());
        assert!(
            pending.claude.obligations.launches[0].remaining_ms
                <= yinz_types::agent::control::MAX_TIMEOUT_MS
        );
        report_kind(
            &mut destination,
            "claude",
            task_notification("notice", &["bash"]),
        );
        let accepted = identified_notice("notice", "bash", "tool", "run");
        report_kind(&mut destination, "claude", accepted.clone());
        let mut stop = claude_stop("notice", "FINAL", false);
        stop.claude_notifications = accepted.claude_notifications;
        report_kind(&mut destination, "claude", stop);
        assert_eq!(
            destination.agent_status_for_test().agent.state,
            AgentState::Idle
        );
        assert_eq!(
            destination
                .agent_lifecycle
                .get(&PANE)
                .unwrap()
                .last_message_for_test(),
            Some("FINAL")
        );
        report_kind(
            &mut destination,
            "claude",
            native_background_launch("bash", "tool", "Bash"),
        );
        assert!(!destination.claude_background_pending_for_test());
    }
}

#[test]
fn warp_tracking_preserves_provider_adoption_and_rejects_retired_source_reports() {
    for path in ["pane", "tab", "companion"] {
        let (mut source, _, _) = setup("claude");
        report_kind(
            &mut source,
            "claude",
            provider_resume(notification_source()),
        );
        report_kind(&mut source, "claude", hook("UserPromptSubmit"));
        report_kind(
            &mut source,
            "claude",
            native_background_launch("bash", "tool", "Bash"),
        );
        report_kind(&mut source, "claude", sourced_stop("prompt", "LAUNCH"));
        report_kind(
            &mut source,
            "claude",
            task_notification("notice", &["bash"]),
        );
        report_kind(
            &mut source,
            "claude",
            identified_notice("notice", "bash", "tool", "run"),
        );
        let (mut destination, _) = adopt_tracking(&mut source, path, false, true);
        report_kind(
            &mut destination,
            "claude",
            provider_resume(notification_source()),
        );
        assert!(destination.claude_background_pending_for_test());
        report_kind(
            &mut destination,
            "claude",
            provider_resume(replacement_source()),
        );
        assert!(!destination.claude_background_pending_for_test());
        report_kind(
            &mut destination,
            "claude",
            with_notification_source(steering_ack("new"), replacement_source()),
        );
        report_kind(
            &mut destination,
            "claude",
            with_notification_source(claude_stop("new", "NEW", false), replacement_source()),
        );
        let (mut next, _) = adopt_tracking(&mut destination, path, false, true);
        for old in [
            provider_resume(notification_source()),
            native_background_launch("bash", "tool", "Bash"),
            sourced_stop("new", "OLD"),
        ] {
            report_kind(&mut next, "claude", old);
            assert!(!next.claude_background_pending_for_test());
            assert_eq!(claude_lifecycle(&next).last_message.as_deref(), Some("NEW"));
            assert_eq!(
                claude_lifecycle(&next).claude.obligations.source,
                Some(replacement_source())
            );
        }
    }
}
