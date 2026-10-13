use super::*;
use serde_json::{json, Value};
use yinz_ipc::ipc::EventSubscription;

const WATCHER: ClientId = ClientId::new(99);

fn events(screen: &mut Screen, messages: &CapturedClientMessages) -> Vec<Value> {
    screen.refresh_agent_events();
    screen.flush_agent_events();
    messages
        .lock()
        .unwrap()
        .remove(&WATCHER)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|m| {
            if let ServerToClientMsg::EventUpdate {
                event_type,
                event_json,
            } = m
            {
                assert_eq!(event_type, "agent_lifecycle");
                Some(serde_json::from_str(&event_json).unwrap())
            } else {
                None
            }
        })
        .collect()
}

fn start(
    screen: &mut Screen,
    messages: &CapturedClientMessages,
    filters: EventSubscription,
) -> Vec<Value> {
    screen.subscribe_agent_events(WATCHER, filters);
    events(screen, messages)
}

#[test]
fn lifecycle_atomic_snapshot_replay_and_filtered_checkpoints() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    let filter = EventSubscription {
        lifecycle_types: vec!["turn_completed".into()],
        ..Default::default()
    };
    let initial = start(&mut screen, &messages, filter.clone());
    assert_eq!(initial.first().unwrap()["event_type"], "snapshot_begin");
    assert!(initial
        .iter()
        .any(|e| e["event_type"] == "snapshot" && e["pane_id"] == "terminal_7"));
    assert_eq!(initial.last().unwrap()["event_type"], "ready");
    let cursor = initial.last().unwrap()["cursor"]
        .as_str()
        .unwrap()
        .to_owned();
    report(&mut screen, hook("UserPromptSubmit"));
    let live = events(&mut screen, &messages);
    assert!(live.iter().all(|e| e["event_type"] == "checkpoint"));
    assert!(
        live.last().unwrap()["sequence"].as_u64() > initial.last().unwrap()["sequence"].as_u64()
    );
    screen.agent_events.remove(WATCHER);
    report(&mut screen, hook("Stop"));
    let replay = start(
        &mut screen,
        &messages,
        EventSubscription {
            cursor: Some(cursor),
            ..filter
        },
    );
    assert_eq!(
        replay
            .iter()
            .filter(|e| e["event_type"] == "turn_completed")
            .count(),
        1
    );
    assert_eq!(replay.last().unwrap()["event_type"], "ready");
    assert!(!replay.iter().any(|e| e["event_type"] == "snapshot"));
    let outcome = replay
        .iter()
        .find(|e| e["event_type"] == "turn_completed")
        .unwrap();
    assert!(outcome["submission_id"].is_null()); // Human-entered turn.
    assert_eq!(outcome["turn_id"], "turn");
    assert!(!outcome.to_string().contains("review"));
    report(&mut screen, hook("Stop"));
    assert!(!events(&mut screen, &messages)
        .iter()
        .any(|e| e["event_type"] == "turn_completed"));
}

#[test]
fn lifecycle_submission_survives_detach_and_multiple_waiters() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    start(&mut screen, &messages, Default::default());
    let mut request = prompt(false);
    if let AgentRequest::Prompt { submission_id, .. } = &mut request {
        *submission_id = Some("chosen".into());
    }
    screen.agent_control_with_clear(request, CLIENT, &writes);
    screen.detach_agent_controls(CLIENT);
    report(&mut screen, hook("UserPromptSubmit"));
    for client in [2, 3] {
        screen.agent_control(
            AgentRequest::WaitTurn {
                pane_id: PANE,
                expected_session_id: Some("conversation".into()),
                submission_id: "chosen".into(),
                timeout_ms: 30_000,
            },
            ClientId::new(client),
        );
    }
    report(&mut screen, hook("Stop"));
    report(&mut screen, hook("Stop"));
    let all = events(&mut screen, &messages);
    for name in ["submission_acknowledged", "turn_completed"] {
        let facts: Vec<_> = all.iter().filter(|e| e["event_type"] == name).collect();
        assert_eq!(facts.len(), 1, "{name}: {all:?}");
        assert_eq!(facts[0]["submission_id"], "chosen");
    }
}

#[test]
fn lifecycle_idle_is_not_completion_and_provider_confirmation_is_required() {
    for kind in ["claude", "agy", "codex"] {
        let (mut screen, messages, _writes) = setup(kind);
        report_kind(&mut screen, kind, hook("SessionStart"));
        start(&mut screen, &messages, Default::default());
        report_kind(&mut screen, kind, hook("UserPromptSubmit"));
        let mut idle = screen
            .tab_containing_pane(PANE)
            .unwrap()
            .pane_agent_info(PANE)
            .unwrap();
        idle.state = AgentState::Idle;
        crate::screen::apply_pane_agent_report(&mut screen, PANE, Some(idle)).unwrap();
        assert!(!events(&mut screen, &messages)
            .iter()
            .any(|e| e["event_type"] == "turn_completed"));
        if kind != "codex" {
            let mut stop = hook("Stop");
            stop.claude_turn_complete = None;
            stop.agy_turn_complete = None;
            report_kind(&mut screen, kind, stop);
            assert!(!events(&mut screen, &messages)
                .iter()
                .any(|e| e["event_type"] == "turn_completed"));
        }
        report_kind(&mut screen, kind, hook("Stop"));
        assert_eq!(
            events(&mut screen, &messages)
                .iter()
                .filter(|e| e["event_type"] == "turn_completed")
                .count(),
            1,
            "{kind}"
        );
    }
}

#[test]
fn lifecycle_uncertainty_keeps_observing_and_interrupt_is_definite() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    start(&mut screen, &messages, Default::default());
    screen.agent_control_with_clear(prompt(true), CLIENT, &writes);
    report(&mut screen, hook("UserPromptSubmit"));
    screen.refresh_agent_controls_at(Instant::now() + Duration::from_secs(31));
    let uncertain = events(&mut screen, &messages);
    assert!(uncertain
        .iter()
        .any(|e| e["event_type"] == "observation_uncertain"));
    assert!(!uncertain
        .iter()
        .any(|e| e["event_type"] == "turn_completed" || e["event_type"] == "turn_interrupted"));
    report(&mut screen, hook("Interrupt"));
    report(&mut screen, hook("Interrupt"));
    report(&mut screen, hook("Stop"));
    let all = events(&mut screen, &messages);
    assert_eq!(
        all.iter()
            .filter(|e| e["event_type"] == "turn_interrupted")
            .count(),
        1
    );
    assert!(!all.iter().any(|e| e["event_type"] == "turn_completed"));
}

#[test]
fn lifecycle_expiry_restart_and_cursor_rejection() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    let initial = start(&mut screen, &messages, Default::default());
    let cursor = initial.last().unwrap()["cursor"]
        .as_str()
        .unwrap()
        .to_owned();
    screen.agent_events.remove(WATCHER);
    for i in 0..4100 {
        screen.publish_agent_event(PANE, "state_changed", json!({"test":i}));
    }
    let expired = start(
        &mut screen,
        &messages,
        EventSubscription {
            cursor: Some(cursor.clone()),
            ..Default::default()
        },
    );
    assert_eq!(expired[0]["event_type"], "gap");
    assert_eq!(expired[1]["event_type"], "snapshot_begin");
    let (mut restarted, messages2, _writes) = setup("codex");
    let gap = start(
        &mut restarted,
        &messages2,
        EventSubscription {
            cursor: Some(cursor.clone()),
            ..Default::default()
        },
    );
    assert_eq!(gap[0]["event_type"], "gap");
    let (tx, rx) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(tx));
    for (key, value) in [
        ("host", json!("wrong")),
        ("session", json!("wrong")),
        ("sequence", json!(u64::MAX)),
        ("filters", json!({})),
    ] {
        let mut invalid: Value = serde_json::from_str(&cursor).unwrap();
        invalid[key] = value;
        screen.subscribe_agent_events(
            WATCHER,
            EventSubscription {
                cursor: Some(invalid.to_string()),
                ..Default::default()
            },
        );
        assert!(
            rx.try_iter()
                .any(|(m, _)| matches!(m, ServerInstruction::LogError(..))),
            "{key}"
        );
    }
}

#[test]
fn lifecycle_replacement_retains_old_identity_and_uses_new_generation() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    let initial = start(&mut screen, &messages, Default::default());
    let generation = initial
        .iter()
        .find(|e| e["event_type"] == "snapshot" && e["pane_id"] == "terminal_7")
        .unwrap()["pane_generation"]
        .clone();
    let tab = screen.tab_containing_pane_mut(PANE).unwrap();
    tab.hold_pane(PANE, Some(0), false, tab.pane_run_command(PANE).unwrap());
    screen.rerun_command_pane_with_id(TerminalId::new(7), None);
    let all = events(&mut screen, &messages);
    let replaced = all
        .iter()
        .find(|e| e["event_type"] == "pane_replaced")
        .unwrap();
    assert_eq!(replaced["conversation_id"], "conversation");
    assert_eq!(replaced["pane_generation"], generation);
    let new = all
        .iter()
        .find(|e| e["event_type"] == "pane_spawned")
        .unwrap();
    assert_ne!(new["pane_generation"], generation);
}

#[test]
fn lifecycle_project_probe_rejects_stale_result_before_ready() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    let (tx, rx) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    let filters = EventSubscription {
        projects: vec!["yinz".into()],
        ..Default::default()
    };
    assert!(start(&mut screen, &messages, filters.clone()).is_empty());
    let old_version = rx
        .try_iter()
        .find_map(|(m, _)| match m {
            PluginInstruction::ResolveAgentEventProjects { version, .. } => Some(version),
            _ => None,
        })
        .unwrap();
    // A second subscription refreshes external assignments and invalidates the first probe.
    screen.subscribe_agent_events(WATCHER, filters);
    screen.apply_agent_event_projects(old_version, BTreeMap::from([("yinz".into(), Ok(None))]));
    assert!(events(&mut screen, &messages).is_empty());
    let new_version = rx
        .try_iter()
        .find_map(|(m, _)| match m {
            PluginInstruction::ResolveAgentEventProjects { version, .. } => Some(version),
            _ => None,
        })
        .unwrap();
    assert!(new_version > old_version);
    screen.apply_agent_event_projects(
        new_version,
        BTreeMap::from([("yinz".into(), Ok(Some(vec![PANE])))]),
    );
    let all = events(&mut screen, &messages);
    assert!(all
        .iter()
        .any(|e| e["event_type"] == "snapshot" && e["pane_id"] == "terminal_7"));
    assert_eq!(all.last().unwrap()["event_type"], "ready");
}

#[test]
fn lifecycle_slow_subscriber_is_removed_without_affecting_history() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    let (tx, rx) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(tx));
    start(&mut screen, &messages, Default::default());
    // The test deliberately never drains this subscriber while producing facts.
    for i in 0..4100 {
        screen.publish_agent_event(PANE, "state_changed", json!({"test":i}));
    }
    assert!(rx
        .try_iter()
        .any(|(m, _)| matches!(m, ServerInstruction::RemoveClient(id) if id == WATCHER)));
    let fresh = start(&mut screen, &messages, Default::default());
    assert_eq!(fresh.last().unwrap()["event_type"], "ready");
}

#[test]
fn lifecycle_conversation_reset_closure_exit_and_background_notifications() {
    let (mut screen, messages, _writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    start(&mut screen, &messages, Default::default());
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    report_kind(
        &mut screen,
        "claude",
        claude_stop("prompt", "private answer", &["task-1"]),
    );
    let all = events(&mut screen, &messages);
    assert!(all
        .iter()
        .any(|e| e["event_type"] == "background_tasks_changed"
            && e["background_tasks"] == json!(["task-1"])));
    assert!(!all.iter().any(|e| e.to_string().contains("private answer")));
    report_kind(
        &mut screen,
        "claude",
        task_notification("notification", &["task-1"]),
    );
    report_kind(
        &mut screen,
        "claude",
        claude_stop("notification", "private background", &[]),
    );
    let all = events(&mut screen, &messages);
    assert!(all.iter().any(
        |e| e["event_type"] == "background_tasks_changed" && e["background_tasks"] == json!([])
    ));
    assert!(all.iter().any(|e| e["event_type"] == "turn_completed"
        && e["provider_prompt_id"] == "notification"
        && e["submission_id"].is_null()));
    let mut changed = hook("SessionStart");
    changed.session_id = "new-conversation".into();
    report_kind(&mut screen, "claude", changed);
    let all = events(&mut screen, &messages);
    assert!(all
        .iter()
        .any(|e| e["event_type"] == "conversation_replaced"
            && e["conversation_id"] == "conversation"
            && e["next_conversation_id"] == "new-conversation"));
    let tab = screen.tab_containing_pane_mut(PANE).unwrap();
    tab.hold_pane(PANE, Some(0), false, tab.pane_run_command(PANE).unwrap());
    let all = events(&mut screen, &messages);
    assert!(all
        .iter()
        .any(|e| e["event_type"] == "state_changed" && e["state"] == "exited"));
    screen.tabs.clear();
    let all = events(&mut screen, &messages);
    assert!(all.iter().any(|e| e["event_type"] == "pane_closed"
        && e["pane_id"] == "terminal_7"
        && e["conversation_id"] == "new-conversation"));
}

#[test]
fn lifecycle_filters_combine_or_with_and_and_replay_precedes_live() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    let filters = EventSubscription {
        panes: vec!["terminal_7".into(), "terminal_8".into()],
        kinds: vec!["codex".into(), "claude".into()],
        ..Default::default()
    };
    let initial = start(&mut screen, &messages, filters.clone());
    let cursor = initial.last().unwrap()["cursor"]
        .as_str()
        .unwrap()
        .to_owned();
    screen.agent_events.remove(WATCHER);
    for i in 0..70 {
        screen.publish_agent_event(PANE, "state_changed", json!({"test":i}));
    }
    screen.subscribe_agent_events(
        WATCHER,
        EventSubscription {
            cursor: Some(cursor),
            ..filters
        },
    );
    screen.publish_agent_event(PANE, "state_changed", json!({"test":70}));
    let mut all = Vec::new();
    for _ in 0..4 {
        all.extend(events(&mut screen, &messages));
    }
    let values: Vec<_> = all.iter().filter_map(|e| e["test"].as_u64()).collect();
    assert_eq!(values, (0..=70).collect::<Vec<_>>());
    let ready = all.iter().position(|e| e["event_type"] == "ready").unwrap();
    assert_eq!(all[ready - 1]["test"], 69);
    assert_eq!(all[ready + 1]["test"], 70);
    let mismatch = start(
        &mut screen,
        &messages,
        EventSubscription {
            panes: vec!["terminal_8".into()],
            kinds: vec!["codex".into()],
            ..Default::default()
        },
    );
    assert!(!mismatch.iter().any(|e| e["event_type"] == "snapshot"));
}

#[test]
fn lifecycle_identical_human_prompt_does_not_inherit_a_submission_id() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    start(&mut screen, &messages, Default::default());
    let mut request = prompt(false);
    if let AgentRequest::Prompt { submission_id, .. } = &mut request {
        *submission_id = Some("chosen".into());
    }
    screen.agent_control_with_clear(request, CLIENT, &writes);
    report(&mut screen, hook("UserPromptSubmit"));
    events(&mut screen, &messages);
    let human = HookEvent {
        turn_id: Some("human-turn".into()),
        prompt_id: Some("human-prompt".into()),
        ..hook("UserPromptSubmit")
    };
    report(&mut screen, human.clone());
    report(
        &mut screen,
        HookEvent {
            hook_event_name: "Stop".into(),
            ..human
        },
    );
    let all = events(&mut screen, &messages);
    for name in ["submission_acknowledged", "turn_completed"] {
        let fact = all.iter().find(|e| e["event_type"] == name).unwrap();
        assert_eq!(fact["turn_id"], "human-turn");
        assert!(fact["submission_id"].is_null(), "{fact}");
    }
}

#[test]
fn lifecycle_wait_needs_a_person_and_stale_hook_rejection() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    start(&mut screen, &messages, Default::default());
    report(&mut screen, hook("UserPromptSubmit"));
    let mut waiting = screen
        .tab_containing_pane(PANE)
        .unwrap()
        .pane_agent_info(PANE)
        .unwrap();
    waiting.state = AgentState::Waiting;
    waiting.wait_needs_a_person = false;
    crate::screen::apply_pane_agent_report(&mut screen, PANE, Some(waiting.clone())).unwrap();
    assert!(!events(&mut screen, &messages)
        .iter()
        .any(|e| e["event_type"] == "needs_input"));
    waiting.wait_needs_a_person = true;
    crate::screen::apply_pane_agent_report(&mut screen, PANE, Some(waiting)).unwrap();
    assert_eq!(
        events(&mut screen, &messages)
            .iter()
            .filter(|e| e["event_type"] == "needs_input")
            .count(),
        1
    );
    report(&mut screen, hook("Interrupt"));
    events(&mut screen, &messages);
    report(&mut screen, hook("PreToolUse"));
    assert!(events(&mut screen, &messages).is_empty());
}

#[test]
fn lifecycle_invalid_project_does_not_reject_other_subscriptions() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    let (tx, rx) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    screen.subscribe_agent_events(
        WATCHER,
        EventSubscription {
            projects: vec!["yinz".into()],
            ..Default::default()
        },
    );
    let old = rx
        .try_iter()
        .find_map(|(m, _)| match m {
            PluginInstruction::ResolveAgentEventProjects { version, .. } => Some(version),
            _ => None,
        })
        .unwrap();
    screen.subscribe_agent_events(
        ClientId::new(98),
        EventSubscription {
            projects: vec!["typo".into()],
            ..Default::default()
        },
    );
    screen.apply_agent_event_projects(old, BTreeMap::from([("yinz".into(), Ok(Some(vec![PANE])))]));
    let current = rx
        .try_iter()
        .find_map(|(m, _)| match m {
            PluginInstruction::ResolveAgentEventProjects { version, .. } => Some(version),
            _ => None,
        })
        .unwrap();
    screen.apply_agent_event_projects(
        current,
        BTreeMap::from([
            ("yinz".into(), Ok(Some(vec![PANE]))),
            ("typo".into(), Err("unknown project".into())),
        ]),
    );
    assert_eq!(
        events(&mut screen, &messages).last().unwrap()["event_type"],
        "ready"
    );
    screen.invalidate_agent_event_projects();
    let projects = rx
        .try_iter()
        .find_map(|(m, _)| match m {
            PluginInstruction::ResolveAgentEventProjects { projects, .. } => Some(projects),
            _ => None,
        })
        .unwrap();
    assert_eq!(projects, ["yinz"]);
}

#[test]
fn lifecycle_oversized_payload_is_explicit_even_for_filtered_readers() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    start(
        &mut screen,
        &messages,
        EventSubscription {
            kinds: vec!["claude".into()],
            ..Default::default()
        },
    );
    screen.publish_agent_event(
        PANE,
        "state_changed",
        json!({"background_tasks":["x".repeat(64 * 1024)]}),
    );
    let all = events(&mut screen, &messages);
    let fact = all
        .iter()
        .find(|e| e["event_type"] == "observation_uncertain")
        .unwrap();
    assert_eq!(fact["reason"], "payload_limit");
    assert!(fact.to_string().len() <= 64 * 1024);
}

#[test]
fn lifecycle_close_releases_generation_before_pane_id_reuse() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    start(&mut screen, &messages, Default::default());
    // Cleanup is immediate, even if replacement reuses the ID before the next screen tick.
    screen.cleanup_closed_pane(PANE);
    let all = events(&mut screen, &messages);
    let closed = all
        .iter()
        .find(|e| e["event_type"] == "pane_closed")
        .unwrap();
    let reused = all
        .iter()
        .find(|e| e["event_type"] == "pane_spawned")
        .unwrap();
    assert_eq!(closed["pane_id"], reused["pane_id"]);
    assert_ne!(closed["pane_generation"], reused["pane_generation"]);
    assert_eq!(reused["pane_sequence"], 1);
}

#[test]
fn lifecycle_turn_window_eviction_is_explicit() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    start(&mut screen, &messages, Default::default());
    for turn in 0..33 {
        report(
            &mut screen,
            HookEvent {
                turn_id: Some(format!("turn-{turn}")),
                prompt_id: Some(format!("prompt-{turn}")),
                ..hook("UserPromptSubmit")
            },
        );
    }
    let mut all = events(&mut screen, &messages);
    all.extend(events(&mut screen, &messages));
    let evicted = all
        .iter()
        .find(|e| e["reason"] == "turn_tracking_limit")
        .unwrap();
    assert_eq!(evicted["event_type"], "observation_uncertain");
    assert_eq!(evicted["turn_id"], "turn-0");
    assert!(!all.iter().any(|e| e["event_type"] == "turn_interrupted"));
}

#[test]
fn lifecycle_outcomes_use_the_same_provider_identity_as_wait() {
    for kind in ["codex", "claude"] {
        let (mut screen, messages, _writes) = setup(kind);
        report_kind(&mut screen, kind, hook("SessionStart"));
        start(&mut screen, &messages, Default::default());
        report_kind(&mut screen, kind, hook("UserPromptSubmit"));
        let mut complete = hook("Stop");
        if kind == "codex" {
            complete.prompt_id = None;
        } else {
            complete.turn_id = None;
        }
        report_kind(&mut screen, kind, complete);
        assert_eq!(
            events(&mut screen, &messages)
                .iter()
                .filter(|e| e["event_type"] == "turn_completed")
                .count(),
            1,
            "{kind}"
        );
    }
}

#[test]
fn lifecycle_missing_provider_identity_is_uncertain_not_acknowledged() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    start(&mut screen, &messages, Default::default());
    report(
        &mut screen,
        HookEvent {
            turn_id: None,
            ..hook("UserPromptSubmit")
        },
    );
    let all = events(&mut screen, &messages);
    assert!(all
        .iter()
        .any(|e| e["event_type"] == "observation_uncertain"));
    assert!(!all
        .iter()
        .any(|e| e["event_type"] == "submission_acknowledged"));
}

#[test]
fn lifecycle_restart_of_the_same_conversation_reports_uncertain_observation() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    start(&mut screen, &messages, Default::default());
    report(&mut screen, hook("UserPromptSubmit"));
    events(&mut screen, &messages);
    report(
        &mut screen,
        HookEvent {
            source: Some("resume".into()),
            ..hook("SessionStart")
        },
    );
    let reset = events(&mut screen, &messages);
    assert!(reset
        .iter()
        .any(|e| e["event_type"] == "observation_uncertain"));
    report(&mut screen, hook("Stop"));
    assert!(!events(&mut screen, &messages)
        .iter()
        .any(|e| e["event_type"] == "turn_completed"));
}

#[test]
fn lifecycle_busy_edge_uses_the_new_turns_correlation() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    start(&mut screen, &messages, Default::default());
    let mut request = prompt(false);
    if let AgentRequest::Prompt { submission_id, .. } = &mut request {
        *submission_id = Some("old-submission".into());
    }
    screen.agent_control_with_clear(request, CLIENT, &writes);
    report(&mut screen, hook("UserPromptSubmit"));
    report(&mut screen, hook("Stop"));
    events(&mut screen, &messages);
    report(
        &mut screen,
        HookEvent {
            turn_id: Some("human-turn".into()),
            prompt_id: Some("human-prompt".into()),
            ..hook("UserPromptSubmit")
        },
    );
    let all = events(&mut screen, &messages);
    let busy = all
        .iter()
        .find(|e| e["event_type"] == "state_changed" && e["state"] == "busy")
        .unwrap();
    assert!(busy["submission_id"].is_null());
    assert_eq!(busy["turn_id"], "human-turn");
}

#[test]
fn lifecycle_older_turn_uncertainty_preserves_identity_and_outcome() {
    let (mut screen, messages, writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    start(&mut screen, &messages, Default::default());
    let mut request = prompt(false);
    if let AgentRequest::Prompt { submission_id, .. } = &mut request {
        *submission_id = Some("older-submission".into());
    }
    screen.agent_control_with_clear(request, CLIENT, &writes);
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    report_kind(
        &mut screen,
        "claude",
        HookEvent {
            turn_id: Some("background-turn".into()),
            ..task_notification("background", &["task"])
        },
    );
    events(&mut screen, &messages);
    let uncertain = HookEvent {
        claude_turn_complete: Some(false),
        ..hook("Stop")
    };
    report_kind(&mut screen, "claude", uncertain.clone());
    let all = events(&mut screen, &messages);
    let facts: Vec<_> = all
        .iter()
        .filter(|e| e["event_type"] == "observation_uncertain")
        .collect();
    assert!(!facts.is_empty());
    // Submission observation may also report uncertainty; every record must use the old identity.
    for fact in &facts {
        assert_eq!(fact["submission_id"], "older-submission");
        assert_eq!(fact["turn_id"], "turn");
        assert_eq!(fact["provider_prompt_id"], "prompt");
    }
    assert_eq!(
        facts
            .iter()
            .filter(|e| e["outcome"] == json!(AgentOutcome::StatusUnavailable))
            .count(),
        1
    );
    report_kind(&mut screen, "claude", uncertain);
    assert!(!events(&mut screen, &messages)
        .iter()
        .any(|e| e["event_type"] == "observation_uncertain"));

    // Session restart has a different outcome and must retain it, not map it to unavailable.
    report_kind(
        &mut screen,
        "claude",
        HookEvent {
            turn_id: None,
            prompt_id: None,
            source: Some("resume".into()),
            ..hook("SessionStart")
        },
    );
    let all = events(&mut screen, &messages);
    let fact = all
        .iter()
        .find(|e| {
            e["event_type"] == "observation_uncertain" && e["submission_id"] == "older-submission"
        })
        .unwrap();
    assert_eq!(fact["outcome"], json!(AgentOutcome::TargetChanged));
    assert_eq!(fact["provider_prompt_id"], "prompt");
}

#[test]
fn lifecycle_human_and_background_uncertainty_is_deduplicated_per_turn() {
    let (mut screen, messages, _writes) = setup("claude");
    report_kind(&mut screen, "claude", hook("SessionStart"));
    start(&mut screen, &messages, Default::default());
    report_kind(&mut screen, "claude", hook("UserPromptSubmit"));
    report_kind(
        &mut screen,
        "claude",
        task_notification("background", &["task"]),
    );
    events(&mut screen, &messages);
    for prompt in ["prompt", "background", "prompt", "background"] {
        report_kind(
            &mut screen,
            "claude",
            HookEvent {
                prompt_id: Some(prompt.into()),
                claude_turn_complete: Some(false),
                ..hook("Stop")
            },
        );
    }
    let all = events(&mut screen, &messages);
    let facts: Vec<_> = all
        .iter()
        .filter(|e| e["event_type"] == "observation_uncertain")
        .collect();
    assert_eq!(facts.len(), 2, "{all:?}");
    for (fact, prompt) in facts.iter().zip(["prompt", "background"]) {
        assert_eq!(fact["provider_prompt_id"], prompt);
        assert!(fact["submission_id"].is_null());
    }
}

#[test]
fn lifecycle_submission_uncertainty_never_inherits_the_latest_turn() {
    let (mut screen, messages, writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    start(&mut screen, &messages, Default::default());
    let mut request = prompt(false);
    if let AgentRequest::Prompt { submission_id, .. } = &mut request {
        *submission_id = Some("older-submission".into());
    }
    screen.agent_control_with_clear(request, CLIENT, &writes);
    report(&mut screen, hook("UserPromptSubmit"));
    report(
        &mut screen,
        HookEvent {
            turn_id: Some("new-turn".into()),
            prompt_id: Some("new-prompt".into()),
            ..hook("UserPromptSubmit")
        },
    );
    events(&mut screen, &messages);
    for submission in [Some("older-submission"), Some("unacknowledged"), None] {
        screen.agent_observation_uncertain(
            PANE,
            submission.map(str::to_owned),
            AgentOutcome::TimedOut,
        );
        let all = events(&mut screen, &messages);
        let fact = all
            .iter()
            .find(|e| e["event_type"] == "observation_uncertain")
            .unwrap();
        assert_eq!(fact["submission_id"], json!(submission));
        assert_eq!(fact["reason"], json!(AgentOutcome::TimedOut));
        if submission == Some("older-submission") {
            assert_eq!(fact["turn_id"], "turn");
            assert_eq!(fact["provider_prompt_id"], "prompt");
        } else {
            assert!(fact["turn_id"].is_null());
            assert!(fact["provider_prompt_id"].is_null());
        }
    }
}

#[test]
fn lifecycle_project_uncertainty_is_immediate_and_replayed_after_resolution() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    let (tx, rx) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    let probe = || {
        rx.try_iter()
            .find_map(|(message, _)| match message {
                PluginInstruction::ResolveAgentEventProjects { version, .. } => Some(version),
                _ => None,
            })
            .unwrap()
    };
    let pane = PaneId::Terminal(TerminalId::new(8));
    let filters = EventSubscription {
        projects: vec!["yinz".into()],
        panes: vec![pane.to_string()],
        kinds: vec!["codex".into()],
        lifecycle_types: vec!["submission_acknowledged".into(), "turn_completed".into()],
        ..Default::default()
    };
    assert!(start(&mut screen, &messages, filters.clone()).is_empty());
    screen.apply_agent_event_projects(probe(), BTreeMap::from([("yinz".into(), Ok(Some(vec![])))]));
    let initial = events(&mut screen, &messages);
    let cursor = initial.last().unwrap()["cursor"]
        .as_str()
        .unwrap()
        .to_owned();
    screen
        .tabs
        .get_mut(&TabId::new(0))
        .unwrap()
        .new_pane(
            pane,
            None,
            Some(Run::Command(RunCommand {
                command: PathBuf::from("codex"),
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
    let HookOutcome::Reported(agent) = hook_outcome("codex", &hook("SessionStart")) else {
        panic!("session start must report an agent");
    };
    crate::screen::apply_pane_agent_report(&mut screen, pane, Some(agent)).unwrap();
    screen.refresh_agent_events();
    let stale = probe();
    for name in ["UserPromptSubmit", "Stop"] {
        let hook = hook(name);
        let event = AgentEvent::from_hook(
            "codex",
            TerminalId::new(8),
            &hook,
            serde_json::to_string(&hook).unwrap(),
        );
        screen.observe_agent_event(&event, Some(&hook), Some("new-submission".into()));
    }
    screen.publish_agent_event(PANE, "turn_completed", json!({}));
    screen.publish_agent_event(pane, "turn_completed", json!({"kind":"claude"}));
    screen.publish_agent_event(pane, "state_changed", json!({}));
    let live = events(&mut screen, &messages);
    let uncertain: Vec<_> = live
        .iter()
        .filter(|e| e["event_type"] == "observation_uncertain")
        .cloned()
        .collect();
    assert_eq!(uncertain.len(), 2, "{live:?}");
    for (fact, observed) in uncertain
        .iter()
        .zip(["submission_acknowledged", "turn_completed"])
    {
        assert_eq!(fact["reason"], "project_membership_unresolved");
        assert_eq!(fact["observed_event_type"], observed);
        assert_eq!(fact["projects_resolved_for"], json!([]));
        assert_eq!(fact["submission_id"], "new-submission");
        assert_eq!(fact["turn_id"], "turn");
        assert_eq!(fact["provider_prompt_id"], "prompt");
    }
    screen.invalidate_agent_event_projects();
    screen.apply_agent_event_projects(
        stale,
        BTreeMap::from([("yinz".into(), Ok(Some(vec![pane])))]),
    );
    let current = probe();
    assert!(current > stale);
    screen.publish_agent_event(pane, "turn_completed", json!({}));
    assert!(events(&mut screen, &messages)
        .iter()
        .any(|e| e["reason"] == "project_membership_unresolved"));
    screen.apply_agent_event_projects(current, BTreeMap::from([("yinz".into(), Ok(Some(vec![])))]));
    events(&mut screen, &messages);
    screen.publish_agent_event(pane, "turn_completed", json!({}));
    let resolved = events(&mut screen, &messages);
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0]["event_type"], "checkpoint");

    assert!(start(
        &mut screen,
        &messages,
        EventSubscription {
            cursor: Some(cursor),
            ..filters
        }
    )
    .is_empty());
    screen.apply_agent_event_projects(
        probe(),
        BTreeMap::from([("yinz".into(), Ok(Some(vec![pane])))]),
    );
    let replay = events(&mut screen, &messages);
    for fact in &uncertain {
        assert_eq!(
            replay.iter().find(|e| e["sequence"] == fact["sequence"]),
            Some(fact)
        );
    }
    assert!(!replay.iter().any(|e| e["event_type"] == "turn_completed"));
    screen.publish_agent_event(pane, "turn_completed", json!({}));
    assert!(events(&mut screen, &messages)
        .iter()
        .any(|e| e["event_type"] == "turn_completed"
            && e["projects_resolved_for"] == json!(["yinz"])));

    // A reader without project filters still sees the original historical facts.
    let raw = start(&mut screen, &messages, Default::default());
    let mut cursor: Value =
        serde_json::from_str(raw.last().unwrap()["cursor"].as_str().unwrap()).unwrap();
    cursor["sequence"] = 0.into();
    let raw = start(
        &mut screen,
        &messages,
        EventSubscription {
            cursor: Some(cursor.to_string()),
            ..Default::default()
        },
    );
    for fact in uncertain {
        let original = raw
            .iter()
            .find(|e| e["sequence"] == fact["sequence"])
            .unwrap();
        assert_eq!(original["event_type"], fact["observed_event_type"]);
        assert_eq!(original["projects_resolved_for"], json!([]));
    }
}

#[test]
fn lifecycle_queued_replacements_and_rejected_reruns_preserve_the_turn() {
    for queued in [false, true] {
        let (mut screen, messages, writes) = setup("codex");
        report(&mut screen, hook("SessionStart"));
        start(&mut screen, &messages, Default::default());
        let mut request = prompt(false);
        if let AgentRequest::Prompt { submission_id, .. } = &mut request {
            *submission_id = Some("chosen".into());
        }
        screen.agent_control_with_clear(request, CLIENT, &writes);
        report(&mut screen, hook("UserPromptSubmit"));
        let acknowledged = events(&mut screen, &messages)
            .into_iter()
            .find(|e| e["event_type"] == "submission_acknowledged")
            .unwrap();
        let waiter = ClientId::new(2);
        screen.agent_control(
            AgentRequest::WaitTurn {
                pane_id: PANE,
                expected_session_id: Some("conversation".into()),
                submission_id: "chosen".into(),
                timeout_ms: 30_000,
            },
            waiter,
        );
        let replacement = |command: &str| RunCommand {
            command: command.into(),
            ..Default::default()
        };
        if queued {
            for command in ["first", "last"] {
                screen.run_command_in_pane_with_id(TerminalId::new(7), replacement(command), true);
            }
        } else {
            screen.rerun_command_pane_with_id(TerminalId::new(7), None);
        }
        screen.refresh_agent_controls();
        assert!(replies(&messages, waiter).is_empty());
        assert!(events(&mut screen, &messages).is_empty());
        report(&mut screen, hook("Stop"));
        let completed = events(&mut screen, &messages)
            .into_iter()
            .find(|e| e["event_type"] == "turn_completed")
            .unwrap();
        for field in [
            "submission_id",
            "pane_generation",
            "turn_id",
            "conversation_id",
        ] {
            assert_eq!(completed[field], acknowledged[field], "{field}");
        }
        assert_eq!(
            replies(&messages, waiter)[0].outcome,
            AgentOutcome::Completed
        );
        let (tx, rx) = channels::unbounded();
        let tab = screen.tab_containing_pane_mut(PANE).unwrap();
        tab.senders.to_pty = Some(SenderWithContext::new(tx));
        let pending = tab.hold_pane(PANE, Some(0), false, tab.pane_run_command(PANE).unwrap());
        if queued {
            let (id, command, keep_scrollback) = pending.unwrap();
            assert_eq!(command, replacement("last"));
            assert!(keep_scrollback);
            screen.run_command_in_pane_with_id(id, command, keep_scrollback);
        } else {
            assert!(pending.is_none());
            screen.rerun_command_pane_with_id(TerminalId::new(7), None);
        }
        assert_eq!(
            rx.try_iter()
                .filter(|(i, _)| matches!(i, PtyInstruction::ReRunCommandInPane(..)))
                .count(),
            1
        );
        let all = events(&mut screen, &messages);
        let replaced: Vec<_> = all
            .iter()
            .filter(|e| e["event_type"] == "pane_replaced")
            .collect();
        assert_eq!(replaced.len(), 1);
        for field in [
            "submission_id",
            "pane_generation",
            "turn_id",
            "conversation_id",
        ] {
            assert_eq!(replaced[0][field], acknowledged[field], "{field}");
        }
        let spawned: Vec<_> = all
            .iter()
            .filter(|e| e["event_type"] == "pane_spawned")
            .collect();
        assert_eq!(spawned.len(), 1);
        assert_ne!(
            spawned[0]["pane_generation"],
            acknowledged["pane_generation"]
        );
        assert!(spawned[0]["sequence"].as_u64() > replaced[0]["sequence"].as_u64());
    }
}

#[test]
fn lifecycle_pending_projects_retry_without_invalidation() {
    for member in [true, false] {
        let (mut screen, messages, _writes) =
            setup_on_target("codex", PaneTarget::ssh("test-host").unwrap());
        report(&mut screen, hook("SessionStart"));
        let (tx, rx) = channels::unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
        let probe = || {
            rx.try_iter().find_map(|(m, _)| match m {
                PluginInstruction::ResolveAgentEventProjects { version, .. } => Some(version),
                _ => None,
            })
        };
        assert!(start(
            &mut screen,
            &messages,
            EventSubscription {
                projects: vec!["yinz".into()],
                ..Default::default()
            }
        )
        .is_empty());
        let version = probe().unwrap();
        for _ in 0..3 {
            let before = Instant::now();
            screen.apply_agent_event_projects(version, BTreeMap::from([("yinz".into(), Ok(None))]));
            assert!(events(&mut screen, &messages).is_empty());
            let timeout = screen.agent_control_deadline().unwrap();
            assert!(timeout <= Duration::from_millis(100));
            screen.retry_agent_event_projects(before);
            assert!(probe().is_none());
            // Advance the retry clock explicitly: no cwd change, dirty flag, or sleep.
            screen.retry_agent_event_projects(Instant::now() + Duration::from_millis(100));
            assert_eq!(probe(), Some(version));
            screen.retry_agent_event_projects(Instant::now() + Duration::from_secs(1));
            assert!(probe().is_none(), "only one probe may be in flight");
        }
        screen.apply_agent_event_projects(
            version,
            BTreeMap::from([(
                "yinz".into(),
                Ok(Some(if member { vec![PANE] } else { vec![] })),
            )]),
        );
        let initial = events(&mut screen, &messages);
        assert_eq!(
            initial.iter().any(|e| e["event_type"] == "snapshot"),
            member
        );
        assert_eq!(initial.last().unwrap()["event_type"], "ready");
        report(&mut screen, hook("UserPromptSubmit"));
        assert_eq!(
            events(&mut screen, &messages)
                .iter()
                .any(|e| e["event_type"] == "submission_acknowledged"),
            member
        );
        screen.retry_agent_event_projects(Instant::now() + Duration::from_secs(1));
        assert!(probe().is_none());

        // A later unresolved probe preserves membership and can be cancelled.
        screen.invalidate_agent_event_projects();
        let version = probe().unwrap();
        screen.apply_agent_event_projects(version, BTreeMap::from([("yinz".into(), Ok(None))]));
        assert!(!events(&mut screen, &messages)
            .iter()
            .any(|e| e["event_type"] == "project_membership_changed"));
        screen.publish_agent_event(PANE, "state_changed", json!({}));
        let unresolved = events(&mut screen, &messages);
        let fact = unresolved
            .iter()
            .find(|e| e["reason"] == "project_membership_unresolved")
            .unwrap();
        assert_eq!(
            fact["projects"],
            if member { json!(["yinz"]) } else { json!([]) }
        );
        screen.agent_events.remove(WATCHER);
        screen.retry_agent_event_projects(Instant::now() + Duration::from_secs(1));
        assert!(probe().is_none());
        assert!(screen.agent_events.deadline().is_none());
    }
}

#[test]
fn lifecycle_waits_for_all_requested_projects_and_cancels_pending_subscriptions() {
    let (mut screen, messages, _writes) = setup("codex");
    report(&mut screen, hook("SessionStart"));
    let (tx, rx) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(tx));
    let probe = || {
        rx.try_iter().find_map(|(m, _)| match m {
            PluginInstruction::ResolveAgentEventProjects { version, .. } => Some(version),
            _ => None,
        })
    };
    assert!(start(
        &mut screen,
        &messages,
        EventSubscription {
            projects: vec!["ready".into(), "pending".into()],
            ..Default::default()
        }
    )
    .is_empty());
    let version = probe().unwrap();
    screen.apply_agent_event_projects(
        version,
        BTreeMap::from([
            ("ready".into(), Ok(Some(vec![PANE]))),
            ("pending".into(), Ok(None)),
        ]),
    );
    assert!(events(&mut screen, &messages).is_empty());
    screen.retry_agent_event_projects(Instant::now() + Duration::from_millis(100));
    assert_eq!(probe(), Some(version));
    screen.agent_events.remove(WATCHER);
    screen.apply_agent_event_projects(
        version,
        BTreeMap::from([
            ("ready".into(), Ok(Some(vec![PANE]))),
            ("pending".into(), Ok(None)),
        ]),
    );
    screen.retry_agent_event_projects(Instant::now() + Duration::from_secs(1));
    assert!(probe().is_none());
    assert!(events(&mut screen, &messages).is_empty());
    assert!(screen.agent_events.deadline().is_none());
}
