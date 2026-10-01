use super::*;
use crate::internal_ids::TerminalId;

fn observe(observation: &mut PromptObservation, event: &AgentEvent) -> Option<AgentOutcome> {
    observation.observe(
        event,
        serde_json::from_str::<HookEvent>(&event.raw).ok().as_ref(),
    )
}

fn event(kind: &str, name: &str, turn: Option<&str>, prompt: Option<&str>) -> AgentEvent {
    let hook = HookEvent {
        hook_event_name: name.into(),
        session_id: "conversation".into(),
        turn_id: turn.map(str::to_owned),
        prompt_id: (kind == "claude").then(|| turn.unwrap_or("prompt").to_owned()),
        claude_turn_complete: (kind == "claude" && name == "Stop").then_some(true),
        prompt: prompt.map(str::to_owned),
        ..Default::default()
    };
    AgentEvent::from_hook(
        kind,
        TerminalId::new(7),
        &hook,
        serde_json::to_string(&hook).unwrap(),
    )
}

#[test]
fn a_prompt_requires_acceptance_and_the_matching_provider_turn() {
    let mut observation = PromptObservation::new("codex", "conversation", "review");
    assert_eq!(
        observe(&mut observation, &event("codex", "Stop", Some("old"), None)),
        None
    );
    assert!(!observation.acknowledged());
    let acceptance = event("codex", "UserPromptSubmit", Some("new"), Some("review"));
    assert_eq!(
        observe(&mut observation, &acceptance),
        Some(AgentOutcome::Submitted)
    );
    assert_eq!(
        observe(&mut observation, &acceptance),
        None,
        "duplicate hook"
    );
    assert!(observation.acknowledged());
    assert_eq!(observation.turn_id(), Some("new"));
    for turn in [Some("old"), None] {
        assert_eq!(
            observe(&mut observation, &event("codex", "Stop", turn, None)),
            None
        );
    }
    let completion = event("codex", "Stop", Some("new"), None);
    assert_eq!(
        observe(&mut observation, &completion),
        Some(AgentOutcome::Completed)
    );
    assert_eq!(observe(&mut observation, &completion), None);
}

#[test]
fn fingerprints_survive_pruning_and_preserve_failure_and_interrupt_outcomes() {
    let prompt = "a".repeat(super::super::HOOK_BROADCAST_MAX_BYTES);
    for (name, outcome) in [
        ("Stop", AgentOutcome::Completed),
        ("StopFailure", AgentOutcome::Failed),
        ("Interrupt", AgentOutcome::Interrupted),
    ] {
        let mut observation = PromptObservation::new("codex", "conversation", &prompt);
        let acceptance = event("codex", "UserPromptSubmit", Some("new"), Some(&prompt));
        assert_ne!(acceptance.prompt.as_deref(), Some(prompt.as_str()));
        assert_eq!(
            observe(&mut observation, &acceptance),
            Some(AgentOutcome::Submitted)
        );
        assert_eq!(
            observe(&mut observation, &event("codex", name, Some("new"), None)),
            Some(outcome)
        );
    }
}

#[test]
fn conflicting_prompts_and_conversations_cannot_complete_the_observed_work() {
    for accept_first in [false, true] {
        let mut observation = PromptObservation::new("codex", "conversation", "review");
        if accept_first {
            observe(
                &mut observation,
                &event("codex", "UserPromptSubmit", Some("new"), Some("review")),
            );
        }
        assert_eq!(
            observe(
                &mut observation,
                &event("codex", "UserPromptSubmit", Some("other"), Some("manual"))
            ),
            Some(AgentOutcome::Interrupted)
        );
        assert_eq!(
            observe(&mut observation, &event("codex", "Stop", Some("new"), None)),
            accept_first.then_some(AgentOutcome::Completed)
        );
    }
    let mut observation = PromptObservation::new("codex", "different conversation", "review");
    assert_eq!(
        observe(
            &mut observation,
            &event("codex", "UserPromptSubmit", Some("new"), Some("review"))
        ),
        Some(AgentOutcome::TargetChanged)
    );
}

#[test]
fn only_person_waits_end_the_operation_and_worker_hooks_do_not() {
    let mut observation = PromptObservation::new("codex", "conversation", "review");
    observe(
        &mut observation,
        &event("codex", "UserPromptSubmit", Some("new"), Some("review")),
    );
    let mut hook = HookEvent {
        hook_event_name: "PermissionRequest".into(),
        session_id: "conversation".into(),
        turn_id: Some("new".into()),
        approvals_reviewer: Some("auto_review".into()),
        ..Default::default()
    };
    let broadcast = |hook: &HookEvent| {
        AgentEvent::from_hook(
            "codex",
            TerminalId::new(7),
            hook,
            serde_json::to_string(hook).unwrap(),
        )
    };
    assert_eq!(observe(&mut observation, &broadcast(&hook)), None);
    hook.hook_event_name = "SessionStart".into();
    hook.source = Some("compact".into());
    assert_eq!(observe(&mut observation, &broadcast(&hook)), None);
    hook.hook_event_name = "Stop".into();
    hook.agent_id = Some("worker".into());
    assert_eq!(observe(&mut observation, &broadcast(&hook)), None);
    hook.agent_id = None;
    hook.hook_event_name = "PermissionRequest".into();
    hook.approvals_reviewer = None;
    assert_eq!(
        observe(&mut observation, &broadcast(&hook)),
        Some(AgentOutcome::NeedsInput)
    );
}

#[test]
fn repeated_startup_does_not_retarget_a_prompt_to_the_same_conversation() {
    for kind in ["codex", "claude"] {
        let mut observation = PromptObservation::new(kind, "conversation", "review");
        assert_eq!(
            observe(&mut observation, &event(kind, "SessionStart", None, None)),
            None,
            "{kind}"
        );
        assert_eq!(
            observe(
                &mut observation,
                &event(kind, "UserPromptSubmit", Some("new"), Some("review"))
            ),
            Some(AgentOutcome::Submitted),
            "{kind}"
        );
        assert_eq!(
            observe(&mut observation, &event(kind, "SessionStart", None, None)),
            Some(AgentOutcome::TargetChanged),
            "{kind}: an accepted turn cannot restart its conversation"
        );
    }
}

#[test]
fn codex_requires_turn_identity_while_claude_requires_prompt_identity() {
    for (kind, expected) in [
        ("codex", AgentOutcome::StatusUnavailable),
        ("claude", AgentOutcome::Submitted),
    ] {
        let mut observation = PromptObservation::new(kind, "conversation", "review");
        assert_eq!(
            observe(
                &mut observation,
                &event(kind, "UserPromptSubmit", None, Some("review"))
            ),
            Some(expected)
        );
        if expected == AgentOutcome::Submitted {
            assert_eq!(
                observe(&mut observation, &event(kind, "Stop", None, None)),
                Some(AgentOutcome::Completed)
            );
        }
    }
}

#[test]
fn status_pane_ids_are_copyable_cli_targets() {
    #[derive(Serialize, Deserialize)]
    struct Target(#[serde(with = "pane_string")] PaneId);
    let encoded = serde_json::to_string(&Target(PaneId::Terminal(TerminalId::new(7)))).unwrap();
    assert_eq!(encoded, "\"terminal_7\"");
    assert_eq!(
        serde_json::from_str::<Target>(&encoded).unwrap().0,
        PaneId::Terminal(TerminalId::new(7))
    );
}

#[test]
fn a_missing_conversation_is_unavailable_evidence_not_a_known_replacement() {
    let mut observation = PromptObservation::new("codex", "conversation", "review");
    let mut acceptance = event("codex", "UserPromptSubmit", Some("new"), Some("review"));
    let mut raw: HookEvent = serde_json::from_str(&acceptance.raw).unwrap();
    raw.session_id.clear();
    acceptance.raw = serde_json::to_string(&raw).unwrap();
    acceptance.session_id = None;
    assert_eq!(
        observe(&mut observation, &acceptance),
        Some(AgentOutcome::StatusUnavailable)
    );
    assert!(!observation.acknowledged());
    assert_eq!(
        observe(
            &mut observation,
            &event("codex", "UserPromptSubmit", Some("new"), Some("review"))
        ),
        Some(AgentOutcome::Submitted)
    );
    assert_eq!(
        observe(&mut observation, &event("codex", "Stop", Some("new"), None)),
        Some(AgentOutcome::Completed)
    );
}

#[test]
fn canonical_fingerprints_survive_pruning_without_losing_internal_whitespace() {
    for size in [1, super::super::HOOK_BROADCAST_MAX_BYTES] {
        let prompt = format!("  {}\r\nsecond\tline\n ", "a".repeat(size));
        let canonical = prompt.replace("\r\n", "\n").trim().to_owned();
        let mut observation = PromptObservation::new("codex", "conversation", &canonical);
        assert_eq!(
            observe(
                &mut observation,
                &event("codex", "UserPromptSubmit", Some("new"), Some(&prompt))
            ),
            Some(AgentOutcome::Submitted)
        );
        assert_ne!(
            prompt_sha256(&prompt),
            prompt_sha256(&prompt.replace('\t', " "))
        );
        assert_eq!(prompt_sha256(&prompt), prompt_sha256(&canonical));
    }
}

#[test]
fn worker_human_waits_match_the_parent_without_acknowledging_it() {
    for kind in ["claude", "codex"] {
        for name in ["PermissionRequest", "PreToolUse", "Elicitation"] {
            let mut observation = PromptObservation::new(kind, "conversation", "review");
            let mut hook = HookEvent {
                hook_event_name: name.into(),
                session_id: "other".into(),
                agent_id: Some("worker".into()),
                tool_name: Some("AskUserQuestion".into()),
                ..Default::default()
            };
            let broadcast = |hook: &HookEvent| {
                AgentEvent::from_hook(
                    kind,
                    TerminalId::new(7),
                    hook,
                    serde_json::to_string(hook).unwrap(),
                )
            };
            assert_eq!(observation.observe(&broadcast(&hook), Some(&hook)), None);
            hook.session_id = "conversation".into();
            assert_eq!(
                observation.observe(&broadcast(&hook), Some(&hook)),
                Some(AgentOutcome::NeedsInput)
            );
            assert!(!observation.acknowledged());
            assert_eq!(
                observe(&mut observation, &event(kind, "Stop", Some("new"), None)),
                None
            );
            assert_eq!(
                observe(
                    &mut observation,
                    &event(kind, "UserPromptSubmit", Some("new"), Some("review"))
                ),
                Some(AgentOutcome::Submitted)
            );
            assert_eq!(
                observe(&mut observation, &event(kind, "Stop", Some("new"), None)),
                Some(AgentOutcome::Completed)
            );
        }
    }
}

#[test]
fn older_spawn_requests_preserve_explicit_vm_names() {
    let request: AgentRequest = serde_json::from_value(serde_json::json!({"Spawn": {
        "spec": {"kind":"codex", "firecracker":"review", "branch":"topic"},
        "cwd":"/repo", "project":null, "remote_host":null
    }}))
    .unwrap();
    assert!(
        matches!(&request, AgentRequest::Spawn { automatic_firecracker_vm: false, spec, .. } if spec.firecracker.as_deref() == Some("review"))
    );
    assert_eq!(
        serde_json::from_str::<AgentRequest>(&serde_json::to_string(&request).unwrap()).unwrap(),
        request
    );
}

#[test]
fn collapsed_pastes_unwrap_to_what_was_pasted() {
    let body = "line 1\nline 2\n\nline 4";
    let wrapped =
        format!("\n\n<pasted_content id=\"8c9e\">\n{body}\n</pasted_content id=\"8c9e\">\n");
    assert_eq!(unwrap_pasted_content(&wrapped), format!("\n\n{body}\n"));
    assert_eq!(
        unwrap_pasted_content(&format!("fix this: {}", wrapped.trim())),
        format!("fix this: {body}")
    );
    for untouched in [
        "no paste here",
        "<pasted_content id=\"a\">\nbody\n</pasted_content id=\"b\">",
        "<pasted_content id=\"a\">\nnever closed",
        "<pasted_content id=\"\">\nempty\n</pasted_content id=\"\">",
    ] {
        assert_eq!(unwrap_pasted_content(untouched), untouched);
    }
    for size in [1, super::super::HOOK_BROADCAST_MAX_BYTES] {
        let prompt = format!("{}\n{body}", "a".repeat(size));
        let hooked = format!(
            "\n\n<pasted_content id=\"8c9e\">\n{prompt}\r\n</pasted_content id=\"8c9e\">\n"
        );
        assert_eq!(prompt_sha256(&hooked), prompt_sha256(&prompt));
        let mut observation = PromptObservation::new("claude", "conversation", &prompt);
        assert_eq!(
            observe(
                &mut observation,
                &event("claude", "UserPromptSubmit", None, Some(&hooked))
            ),
            Some(AgentOutcome::Submitted)
        );
    }
}

fn claude_wrap(id: &str, body: &str) -> String {
    format!("<pasted_content id=\"{id}\">\n{body}\n</pasted_content id=\"{id}\">")
}

#[test]
fn a_typed_paste_element_fingerprints_like_the_collapsed_copy() {
    let typed = format!("see {} and reply", claude_wrap("u", "inner"));
    let hooked = format!("{PASTE_LEAD_IN} \n\n{}\n", claude_wrap("8c9e", &typed));
    assert_eq!(unwrap_pasted_content(&typed), "see inner and reply");
    assert_eq!(submitted_prompt(&hooked), "see inner and reply");
    assert_eq!(prompt_sha256(&hooked), prompt_sha256(&typed));
    let mut observation = PromptObservation::new("claude", "conversation", &typed);
    assert_eq!(
        observe(
            &mut observation,
            &event("claude", "UserPromptSubmit", None, Some(&hooked))
        ),
        Some(AgentOutcome::Submitted)
    );
}

#[test]
fn crossing_and_reused_paste_ids_pair_with_their_first_closer() {
    let crossing = "<pasted_content id=\"a\">\n<pasted_content id=\"b\">\nx\n</pasted_content id=\"a\">\n</pasted_content id=\"b\">";
    assert_eq!(
        unwrap_pasted_content(crossing),
        "<pasted_content id=\"b\">\nx\n</pasted_content id=\"b\">"
    );
    let reused = claude_wrap("a", &claude_wrap("a", "x"));
    assert_eq!(
        unwrap_pasted_content(&reused),
        "<pasted_content id=\"a\">\nx\n</pasted_content id=\"a\">"
    );
    assert_eq!(unwrap_pasted_content(&claude_wrap("a", "")), "");
}

/// Openers with no closer used to cost a scan of the rest of the text each.
#[test]
fn unmatched_paste_openers_unwrap_in_linear_time() {
    let count = super::super::HOOK_BROADCAST_MAX_BYTES / 25;
    let same = "<pasted_content id=\"x\">\n".repeat(count);
    let distinct: String = (0..count)
        .map(|n| format!("<pasted_content id=\"{n}\">\n"))
        .collect();
    let started = std::time::Instant::now();
    for text in [&same, &distinct] {
        assert!(matches!(unwrap_pasted_content(text), Cow::Borrowed(t) if t == text));
    }
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
}

#[test]
fn the_lead_in_typed_ahead_of_a_paste_is_not_part_of_the_prompt() {
    let body = "line 1\nline 2";
    for hooked in [
        format!("{PASTE_LEAD_IN} \n\n<pasted_content id=\"9a9c\">\n{body}\n</pasted_content id=\"9a9c\">\n"),
        format!("{PASTE_LEAD_IN} {body}"),
        body.to_owned(),
    ] {
        assert_eq!(submitted_prompt(&hooked), body);
        assert_eq!(prompt_sha256(&hooked), prompt_sha256(body));
        let mut observation = PromptObservation::new("claude", "conversation", body);
        assert_eq!(
            observe(
                &mut observation,
                &event("claude", "UserPromptSubmit", None, Some(&hooked))
            ),
            Some(AgentOutcome::Submitted)
        );
    }
    assert!(needs_paste_lead_in("claude", body));
    assert!(needs_paste_lead_in("claude", &"a".repeat(500)));
    assert!(!needs_paste_lead_in("claude", "review\n"));
    assert!(!needs_paste_lead_in("codex", body));
}

#[test]
fn command_prefixes_do_not_get_an_instruction_lead_in() {
    for prefix in ['/', '!', '#'] {
        for body in ["a".repeat(500), "first\nsecond".to_owned()] {
            let prompt = format!("{prefix}{body}");
            assert!(!needs_paste_lead_in("claude", &prompt));
            assert!(needs_paste_lead_in("claude", &format!(" {prompt}")));
            assert!(!needs_paste_lead_in("codex", &prompt));
        }
    }
}

#[test]
fn claude_unicode_acknowledgement_accepts_preserved_or_stripped_controls_only() {
    let prompt = "review \u{200b}\u{202e}👩\u{200d}💻";
    for kind in ["claude", "codex"] {
        for hooked in [
            prompt.to_owned(),
            claude_prompt_text(prompt).into_owned(),
            prompt.replace('\u{200d}', ""),
        ] {
            let mut observation = PromptObservation::new(kind, "conversation", prompt);
            let expected =
                if hooked == prompt || (kind == "claude" && hooked == claude_prompt_text(prompt)) {
                    AgentOutcome::Submitted
                } else {
                    AgentOutcome::Interrupted
                };
            assert_eq!(
                observe(
                    &mut observation,
                    &event(kind, "UserPromptSubmit", Some("new"), Some(&hooked))
                ),
                Some(expected)
            );
        }
    }
}

#[test]
fn claude_stop_needs_prompt_identity_and_internal_confirmation() {
    let mut observation = PromptObservation::new("claude", "conversation", "review");
    let mut submit: HookEvent =
        serde_json::from_str(&event("claude", "UserPromptSubmit", None, Some("review")).raw)
            .unwrap();
    submit.prompt_id = None;
    let broadcast = |hook: &HookEvent| {
        AgentEvent::from_hook(
            "claude",
            TerminalId::new(7),
            hook,
            serde_json::to_string(hook).unwrap(),
        )
    };
    assert_eq!(
        observe(&mut observation, &broadcast(&submit)),
        Some(AgentOutcome::StatusUnavailable)
    );
    submit.prompt_id = Some("prompt".into());
    assert_eq!(
        observe(&mut observation, &broadcast(&submit)),
        Some(AgentOutcome::Submitted)
    );
    let mut stop = HookEvent {
        hook_event_name: "Stop".into(),
        session_id: "conversation".into(),
        ..Default::default()
    };
    assert_eq!(observe(&mut observation, &broadcast(&stop)), None);
    stop.claude_turn_complete = Some(true);
    stop.prompt_id = Some("old".into());
    assert_eq!(observe(&mut observation, &broadcast(&stop)), None);
    stop.prompt_id = Some("prompt".into());
    stop.claude_turn_complete = Some(false);
    assert_eq!(
        observe(&mut observation, &broadcast(&stop)),
        Some(AgentOutcome::StatusUnavailable)
    );
    stop.claude_turn_complete = Some(true);
    assert_eq!(
        observe(&mut observation, &broadcast(&stop)),
        Some(AgentOutcome::Completed)
    );
    assert_eq!(observe(&mut observation, &broadcast(&stop)), None);
}

#[test]
fn readiness_reason_is_additive_and_uses_snake_case() {
    let old = serde_json::json!({
        "pane_id":"terminal_7", "session":"dev", "host_id":"host", "tab_id":null,
        "target":"Local", "agent":{
            "kind":"codex", "state":"Idle", "source":"Reported", "session_id":"conversation"
        }, "cwd":null, "branch":null, "ready":false, "status_available":true, "prompt_supported":true
    });
    let status: AgentStatus = serde_json::from_value(old).unwrap();
    assert_eq!(status.not_ready_reason, None);
    for (reason, name) in [
        (NotReadyReason::PromptUnsupported, "prompt_unsupported"),
        (NotReadyReason::NotIdle, "not_idle"),
        (NotReadyReason::PendingSubmission, "pending_submission"),
        (NotReadyReason::UnconsumedInput, "unconsumed_input"),
    ] {
        let status = AgentStatus {
            not_ready_reason: Some(reason),
            ..status.clone()
        };
        let encoded = serde_json::to_value(&status).unwrap();
        assert_eq!(encoded["not_ready_reason"], name);
        assert_eq!(reason.to_string(), name);
        assert_eq!(
            serde_json::from_value::<AgentStatus>(encoded).unwrap(),
            status
        );
    }
}

#[test]
fn task_notification_ids_come_only_from_notification_prompts() {
    let one = "\n<task-notification>\n<task-id>bj30txqod</task-id>\n<tool-use-id>toolu_1</tool-use-id>\n<status>completed</status>\n<summary>done &amp; dusted</summary>\n</task-notification>";
    assert_eq!(
        claude_task_notification_ids(one),
        Some(vec!["bj30txqod".to_owned()])
    );
    let two = format!("{one}\n<task-notification><task-id> a91f </task-id></task-notification>");
    assert_eq!(
        claude_task_notification_ids(&two),
        Some(vec!["bj30txqod".to_owned(), "a91f".to_owned()])
    );
    assert_eq!(
        claude_task_notification_ids("<task-notification><task-id></task-id>"),
        Some(Vec::new())
    );
    for other in [
        "review",
        "Explain <task-notification><task-id>x</task-id></task-notification>",
        "/clear",
    ] {
        assert_eq!(claude_task_notification_ids(other), None, "{other}");
    }
}

#[test]
fn a_claude_notification_turn_never_interrupts_or_completes_a_prompt() {
    let notification = "<task-notification>\n<task-id>bj30txqod</task-id>\n</task-notification>";
    for acknowledged in [false, true] {
        let mut observation = PromptObservation::new("claude", "conversation", "review");
        if acknowledged {
            assert_eq!(
                observe(
                    &mut observation,
                    &event("claude", "UserPromptSubmit", Some("prompt"), Some("review"))
                ),
                Some(AgentOutcome::Submitted)
            );
        }
        for name in ["UserPromptSubmit", "Stop"] {
            let prompt = (name == "UserPromptSubmit").then_some(notification);
            assert_eq!(
                observe(
                    &mut observation,
                    &event("claude", name, Some("notification"), prompt)
                ),
                None,
                "{name}, {acknowledged}"
            );
        }
        assert_eq!(observation.acknowledged(), acknowledged);
    }
    // Codex has no such turns; the same text is an ordinary conflicting prompt there.
    let mut observation = PromptObservation::new("codex", "conversation", "review");
    assert_eq!(
        observe(
            &mut observation,
            &event(
                "codex",
                "UserPromptSubmit",
                Some("turn"),
                Some(notification)
            )
        ),
        Some(AgentOutcome::Interrupted)
    );
}
