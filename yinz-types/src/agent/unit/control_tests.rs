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
fn literal_notification_text_acknowledges_the_callers_prompt() {
    for text in [
        "<task-notification>Explain this XML tag</task-notification>",
        "<task-notification><task-id>example</task-id></task-notification>",
    ] {
        let mut observation = PromptObservation::new("claude", "conversation", text);
        assert_eq!(
            observe(
                &mut observation,
                &event("claude", "UserPromptSubmit", Some("caller"), Some(text))
            ),
            Some(AgentOutcome::Submitted)
        );
        assert_eq!(
            observe(
                &mut observation,
                &event("claude", "Stop", Some("caller"), None)
            ),
            Some(AgentOutcome::Completed)
        );
    }
}

#[test]
fn submission_ids_are_bounded_and_safe_to_pass_as_one_argument() {
    for value in ["request-7", "caller.task_1", &"x".repeat(128)] {
        assert!(validate_submission_id(value).is_ok(), "{value}");
    }
    for value in [
        "",
        "two words",
        "x/y",
        "é",
        "x\ny",
        "$(touch-file)",
        &"x".repeat(129),
    ] {
        assert!(validate_submission_id(value).is_err(), "{value}");
    }
}

/// Muse correlates like Codex: the submitted prompt's turn id accepts the turn, and
/// only that turn's end completes it. A submit without a turn id cannot be tied to
/// this prompt, so the observation waits rather than guessing.
#[test]
fn a_muse_prompt_requires_acceptance_and_the_matching_provider_turn() {
    let mut observation = PromptObservation::new("muse", "conversation", "review");
    assert_eq!(
        observe(
            &mut observation,
            &event("muse", "UserPromptSubmit", None, Some("review"))
        ),
        Some(AgentOutcome::StatusUnavailable)
    );
    assert!(!observation.acknowledged());
    let acceptance = event("muse", "UserPromptSubmit", Some("new"), Some("review"));
    assert_eq!(
        observe(&mut observation, &acceptance),
        Some(AgentOutcome::Submitted)
    );
    assert_eq!(
        observe(&mut observation, &event("muse", "Stop", Some("old"), None)),
        None
    );
    let completion = event("muse", "Stop", Some("new"), None);
    assert_eq!(
        observe(&mut observation, &completion),
        Some(AgentOutcome::Completed)
    );
    assert_eq!(observe(&mut observation, &completion), None);
}

/// A Muse pane verified at startup observes its first prompt unpinned: the bootstrap
/// carries no session id, so the deferred SessionStart is not a replacement and the
/// first submit binds the conversation the rest of the turn is correlated against.
#[test]
fn an_unpinned_muse_prompt_binds_on_its_first_submit() {
    let mut observation = PromptObservation::new("muse", "", "review");
    let start = event("muse", "SessionStart", None, None);
    assert_eq!(observe(&mut observation, &start), None);
    assert!(!observation.acknowledged());
    let acceptance = event("muse", "UserPromptSubmit", Some("new"), Some("review"));
    assert_eq!(
        observe(&mut observation, &acceptance),
        Some(AgentOutcome::Submitted)
    );
    assert!(observation.acknowledged());
    assert_eq!(observation.turn_id(), Some("new"));
    let completion = event("muse", "Stop", Some("new"), None);
    assert_eq!(
        observe(&mut observation, &completion),
        Some(AgentOutcome::Completed)
    );
    // Another conversation still cannot complete the bound turn.
    let mut observation = PromptObservation::new("muse", "", "review");
    let acceptance = event("muse", "UserPromptSubmit", Some("new"), Some("review"));
    assert_eq!(
        observe(&mut observation, &acceptance),
        Some(AgentOutcome::Submitted)
    );
    let hook = HookEvent {
        session_id: "elsewhere".into(),
        ..serde_json::from_str(&acceptance.raw).unwrap()
    };
    let foreign = AgentEvent::from_hook(
        "muse",
        TerminalId::new(7),
        &hook,
        serde_json::to_string(&hook).unwrap(),
    );
    assert_eq!(
        observe(&mut observation, &foreign),
        Some(AgentOutcome::TargetChanged)
    );
}

#[test]
fn an_unpinned_muse_prompt_rejects_conflicts_before_it_binds() {
    // A submit for another prompt interrupts rather than binds.
    let mut observation = PromptObservation::new("muse", "", "review");
    assert_eq!(
        observe(
            &mut observation,
            &event("muse", "UserPromptSubmit", Some("new"), Some("other"))
        ),
        Some(AgentOutcome::Interrupted)
    );
    // A submit without a turn id cannot be tied to this prompt.
    let mut observation = PromptObservation::new("muse", "", "review");
    assert_eq!(
        observe(
            &mut observation,
            &event("muse", "UserPromptSubmit", None, Some("review"))
        ),
        Some(AgentOutcome::StatusUnavailable)
    );
    assert!(!observation.acknowledged());
    // The session ending before any submit retargets the wait.
    let mut observation = PromptObservation::new("muse", "", "review");
    assert_eq!(
        observe(&mut observation, &event("muse", "SessionEnd", None, None)),
        Some(AgentOutcome::TargetChanged)
    );
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

/// A human wait in the observed conversation ends the wait without turn identity:
/// Muse's approval notification is session-scoped, and the pane is blocked on a
/// person whatever turn the provider names. Completions still require the
/// acknowledged turn, even after the wait.
#[test]
fn a_session_scoped_human_wait_ends_the_operation_without_a_turn_id() {
    for kind in ["muse", "codex"] {
        let mut observation = PromptObservation::new(kind, "conversation", "review");
        assert_eq!(
            observe(
                &mut observation,
                &event(kind, "UserPromptSubmit", Some("new"), Some("review"))
            ),
            Some(AgentOutcome::Submitted),
            "{kind}"
        );
        // The reported shape: a Muse approval with no turn id, translated to
        // the unconditional wait on the way in.
        let name = if kind == "muse" {
            "Elicitation"
        } else {
            "PermissionRequest"
        };
        for turn in [None, Some("new"), Some("other")] {
            assert_eq!(
                observe(&mut observation, &event(kind, name, turn, None)),
                Some(AgentOutcome::NeedsInput),
                "{kind}/{turn:?}"
            );
        }
        // An automatically approved permission is work, not a wait.
        let hook = HookEvent {
            hook_event_name: "PermissionRequest".into(),
            session_id: "conversation".into(),
            approvals_reviewer: Some("auto_review".into()),
            ..Default::default()
        };
        let broadcast = AgentEvent::from_hook(
            kind,
            TerminalId::new(7),
            &hook,
            serde_json::to_string(&hook).unwrap(),
        );
        assert_eq!(observe(&mut observation, &broadcast), None, "{kind}");
        for turn in [None, Some("other")] {
            assert_eq!(
                observe(&mut observation, &event(kind, "Stop", turn, None)),
                None,
                "{kind}/{turn:?}"
            );
        }
        assert_eq!(
            observe(&mut observation, &event(kind, "Stop", Some("new"), None)),
            Some(AgentOutcome::Completed),
            "{kind}"
        );
    }
}

#[test]
fn repeated_startup_does_not_retarget_a_prompt_to_the_same_conversation() {
    for kind in ["codex", "claude", "muse"] {
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
fn codex_and_muse_require_turn_identity_while_claude_requires_prompt_identity() {
    for (kind, expected) in [
        ("codex", AgentOutcome::StatusUnavailable),
        ("muse", AgentOutcome::StatusUnavailable),
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
fn claude_permission_evaluation_does_not_end_prompt_observation() {
    let mut observation = PromptObservation::new("claude", "conversation", "review");
    assert_eq!(
        observe(
            &mut observation,
            &event("claude", "UserPromptSubmit", Some("new"), Some("review"))
        ),
        Some(AgentOutcome::Submitted)
    );
    let broadcast = |hook: &HookEvent| {
        AgentEvent::from_hook(
            "claude",
            TerminalId::new(7),
            hook,
            serde_json::to_string(hook).unwrap(),
        )
    };
    for worker in [None, Some("worker")] {
        for mode in [
            "plan",
            "default",
            "acceptEdits",
            "auto",
            "dontAsk",
            "bypassPermissions",
        ] {
            let hook = HookEvent {
                hook_event_name: "PermissionRequest".into(),
                session_id: "conversation".into(),
                permission_mode: Some(mode.into()),
                tool_name: Some("Bash".into()),
                agent_id: worker.map(str::to_owned),
                ..Default::default()
            };
            assert_eq!(observe(&mut observation, &broadcast(&hook)), None);
            for kind in ["permission_prompt", "worker_permission_prompt"] {
                let notification = HookEvent {
                    hook_event_name: "Notification".into(),
                    notification_type: Some(kind.into()),
                    ..hook.clone()
                };
                assert_eq!(
                    observe(&mut observation, &broadcast(&notification)),
                    (kind == "permission_prompt").then_some(AgentOutcome::NeedsInput)
                );
            }
            let finished = HookEvent {
                hook_event_name: "PostToolUse".into(),
                ..hook
            };
            assert_eq!(observe(&mut observation, &broadcast(&finished)), None);
        }
        for tool in ["AskUserQuestion", "ExitPlanMode"] {
            let hook = HookEvent {
                hook_event_name: "PreToolUse".into(),
                session_id: "conversation".into(),
                tool_name: Some(tool.into()),
                agent_id: worker.map(str::to_owned),
                ..Default::default()
            };
            assert_eq!(
                observe(&mut observation, &broadcast(&hook)),
                Some(AgentOutcome::NeedsInput)
            );
            let evaluation = HookEvent {
                hook_event_name: "PermissionRequest".into(),
                ..hook.clone()
            };
            assert_eq!(observe(&mut observation, &broadcast(&evaluation)), None);
            let finished = HookEvent {
                hook_event_name: "PostToolUse".into(),
                ..hook
            };
            assert_eq!(observe(&mut observation, &broadcast(&finished)), None);
        }
    }
    assert_eq!(
        observe(
            &mut observation,
            &event("claude", "Stop", Some("new"), None)
        ),
        Some(AgentOutcome::Completed)
    );
}

#[test]
fn worker_human_waits_match_the_parent_without_acknowledging_it() {
    for kind in ["claude", "codex", "muse"] {
        for name in [
            "Notification",
            "PermissionRequest",
            "PreToolUse",
            "Elicitation",
        ] {
            let mut observation = PromptObservation::new(kind, "conversation", "review");
            let mut hook = HookEvent {
                hook_event_name: name.into(),
                notification_type: (name == "Notification").then(|| "elicitation_dialog".into()),
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
                (kind != "claude" || name != "PermissionRequest")
                    .then_some(AgentOutcome::NeedsInput)
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
    assert_eq!(status.input_revision, None);
    assert_eq!(status.readiness, None);
    assert!(!status.clear_draft_supported);
    assert!(!status.is_suppressed);
    for (reason, name) in [
        (NotReadyReason::ClearingDraft, "clearing_draft"),
        (NotReadyReason::PromptUnsupported, "prompt_unsupported"),
        (NotReadyReason::NotIdle, "not_idle"),
        (NotReadyReason::PendingSubmission, "pending_submission"),
        (NotReadyReason::UnresolvedTurn, "unresolved_turn"),
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

#[test]
fn agy_continuation_invocation_is_treated_as_progress_not_prompt_acceptance() {
    let mut observation = PromptObservation::new("agy", "conv-1", "fix bug");
    assert!(!observation.acknowledged());

    // PreInvocation before acceptance without prompt fingerprint does not accept or interrupt
    let raw_pre_inv = HookEvent {
        hook_event_name: "PreInvocation".into(),
        session_id: "conv-1".into(),
        ..Default::default()
    };
    let event_pre_inv = AgentEvent::from_hook(
        "agy",
        TerminalId::new(7),
        &raw_pre_inv,
        serde_json::to_string(&raw_pre_inv).unwrap(),
    );
    assert_eq!(observe(&mut observation, &event_pre_inv), None);
    assert!(!observation.acknowledged());

    // UserPromptSubmit accepts the prompt
    let raw_submit = HookEvent {
        hook_event_name: "UserPromptSubmit".into(),
        prompt_id: Some("prompt".into()),
        session_id: "conv-1".into(),
        prompt: Some("fix bug".into()),
        prompt_sha256: Some(prompt_sha256("fix bug")),
        ..Default::default()
    };
    let event_submit = AgentEvent::from_hook(
        "agy",
        TerminalId::new(7),
        &raw_submit,
        serde_json::to_string(&raw_submit).unwrap(),
    );
    assert_eq!(
        observe(&mut observation, &event_submit),
        Some(AgentOutcome::Submitted)
    );
    assert!(observation.acknowledged());

    // Tool execution is progress
    let raw_tool = HookEvent {
        hook_event_name: "PreToolUse".into(),
        session_id: "conv-1".into(),
        tool_name: Some("run_command".into()),
        ..Default::default()
    };
    let event_tool = AgentEvent::from_hook(
        "agy",
        TerminalId::new(7),
        &raw_tool,
        serde_json::to_string(&raw_tool).unwrap(),
    );
    assert_eq!(observe(&mut observation, &event_tool), None);

    // Continuation invocation (invocationNum > 0) preserves PreInvocation without fingerprint;
    // it must be treated as progress rather than prompt acceptance or interruption.
    assert_eq!(observe(&mut observation, &event_pre_inv), None);

    // Pre-decision Stop keeps the wait pending
    let raw_stop = HookEvent {
        hook_event_name: "Stop".into(),
        session_id: "conv-1".into(),
        ..Default::default()
    };
    let event_stop = AgentEvent::from_hook(
        "agy",
        TerminalId::new(7),
        &raw_stop,
        serde_json::to_string(&raw_stop).unwrap(),
    );
    assert_eq!(observe(&mut observation, &event_stop), None);

    // Confirmed Stop completes the turn
    let confirmed_stop = HookEvent {
        hook_event_name: "Stop".into(),
        session_id: "conv-1".into(),
        agy_turn_complete: Some(true),
        prompt_id: Some("prompt".into()),
        ..Default::default()
    };
    let event_confirmed = AgentEvent::from_hook(
        "agy",
        TerminalId::new(7),
        &confirmed_stop,
        serde_json::to_string(&confirmed_stop).unwrap(),
    );
    assert_eq!(
        observe(&mut observation, &event_confirmed),
        Some(AgentOutcome::Completed)
    );
}

#[test]
fn agy_complete_translated_hook_sequence_from_raw_payloads() {
    let mut observation = PromptObservation::new("agy", "conv-1234", "review code");

    let broadcast = |hook: &HookEvent| {
        AgentEvent::from_hook(
            "agy",
            TerminalId::new(7),
            hook,
            serde_json::to_string(hook).unwrap(),
        )
    };

    // 1. Initial PreInvocation (invocationNum: 0) translated to UserPromptSubmit and enriched with fingerprint
    let payload_turn_start = serde_json::json!({
        "hook_event_name": "PreInvocation",
        "conversationId": "conv-1234",
        "workspacePaths": ["/workspace"],
        "invocationNum": 0,
        "stepIdx": 5,
    });
    let mut hook_start = super::super::agy::to_hook_event(&payload_turn_start);
    assert_eq!(hook_start.hook_event_name, "UserPromptSubmit");
    hook_start.prompt = Some("review code".into());
    hook_start.prompt_sha256 = Some(prompt_sha256("review code"));

    assert_eq!(
        observe(&mut observation, &broadcast(&hook_start)),
        Some(AgentOutcome::Submitted)
    );

    // 2. PreToolUse for run_command
    let payload_pre_tool = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "conversationId": "conv-1234",
        "toolCall": {
            "name": "run_command",
            "args": { "CommandLine": "cargo test" }
        },
        "stepIdx": 6,
    });
    let hook_pre_tool = super::super::agy::to_hook_event(&payload_pre_tool);
    assert_eq!(observe(&mut observation, &broadcast(&hook_pre_tool)), None);

    // 3. PostToolUse
    let payload_post_tool = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "conversationId": "conv-1234",
        "toolCall": {
            "name": "run_command",
            "args": { "CommandLine": "cargo test" }
        },
        "stepIdx": 6,
    });
    let hook_post_tool = super::super::agy::to_hook_event(&payload_post_tool);
    assert_eq!(observe(&mut observation, &broadcast(&hook_post_tool)), None);

    // 4. Continuation PreInvocation (invocationNum: 1, stepIdx: 7) preserves PreInvocation
    let payload_continuation = serde_json::json!({
        "hook_event_name": "PreInvocation",
        "conversationId": "conv-1234",
        "invocationNum": 1,
        "stepIdx": 7,
    });
    let hook_continuation = super::super::agy::to_hook_event(&payload_continuation);
    assert_eq!(hook_continuation.hook_event_name, "PreInvocation");
    assert!(hook_continuation.prompt_sha256.is_none());
    assert_eq!(
        observe(&mut observation, &broadcast(&hook_continuation)),
        None
    );

    // 5. PreToolUse with ask_question needs input
    let payload_question = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "conversationId": "conv-1234",
        "toolCall": {
            "name": "ask_question",
            "args": {
                "questions": [{ "question": "Proceed?" }]
            }
        },
        "stepIdx": 8,
    });
    let hook_question = super::super::agy::to_hook_event(&payload_question);
    assert_eq!(
        observe(&mut observation, &broadcast(&hook_question)),
        Some(AgentOutcome::NeedsInput)
    );

    // 6. Pre-decision Stop keeps wait pending
    let payload_stop = serde_json::json!({
        "hook_event_name": "Stop",
        "conversationId": "conv-1234",
        "terminationReason": "model_stop",
    });
    let hook_stop = super::super::agy::to_hook_event(&payload_stop);
    assert_eq!(observe(&mut observation, &broadcast(&hook_stop)), None);

    // 7. Confirmed Stop completes the turn
    let payload_confirmed = serde_json::json!({
        "hook_event_name": "Stop",
        "conversationId": "conv-1234",
        "terminationReason": "model_stop",
        "_yinz_agy_turn_complete": true,
    });
    let mut hook_confirmed = super::super::agy::to_hook_event(&payload_confirmed);
    hook_confirmed.prompt_id = hook_start.prompt_id.clone();
    assert_eq!(
        observe(&mut observation, &broadcast(&hook_confirmed)),
        Some(AgentOutcome::Completed)
    );
}

#[test]
fn agy_blocking_stop_and_continuation_resumes_loop() {
    let mut observation = PromptObservation::new("agy", "conv-loop", "run tasks");

    let broadcast = |hook: &HookEvent| {
        AgentEvent::from_hook(
            "agy",
            TerminalId::new(7),
            hook,
            serde_json::to_string(hook).unwrap(),
        )
    };

    // 1. Prompt submit
    let start = HookEvent {
        hook_event_name: "UserPromptSubmit".into(),
        prompt_id: Some("prompt".into()),
        session_id: "conv-loop".into(),
        prompt: Some("run tasks".into()),
        prompt_sha256: Some(prompt_sha256("run tasks")),
        turn_id: Some("1".into()),
        ..Default::default()
    };
    assert_eq!(
        observe(&mut observation, &broadcast(&start)),
        Some(AgentOutcome::Submitted)
    );

    // 2. Initial tool execution
    let tool1 = HookEvent {
        hook_event_name: "PreToolUse".into(),
        session_id: "conv-loop".into(),
        tool_name: Some("run_command".into()),
        ..Default::default()
    };
    assert_eq!(observe(&mut observation, &broadcast(&tool1)), None);

    // 3. Pre-decision Stop notification arrives while Stop hooks execute
    let pre_decision_stop = HookEvent {
        hook_event_name: "Stop".into(),
        session_id: "conv-loop".into(),
        ..Default::default()
    };
    assert_eq!(
        crate::agent::hooks::hook_state_for("agy", &pre_decision_stop),
        Some(crate::agent::AgentState::Busy)
    );
    assert_eq!(
        observe(&mut observation, &broadcast(&pre_decision_stop)),
        None
    );

    // 4. A later Stop handler returns {"decision":"continue"}.
    // Antigravity resumes the loop, emitting PreInvocation (continuation).
    let continuation = HookEvent {
        hook_event_name: "PreInvocation".into(),
        session_id: "conv-loop".into(),
        ..Default::default()
    };
    assert_eq!(observe(&mut observation, &broadcast(&continuation)), None);

    // 5. Model calls another tool to address the Stop hook decision
    let tool2 = HookEvent {
        hook_event_name: "PreToolUse".into(),
        session_id: "conv-loop".into(),
        tool_name: Some("replace_file_content".into()),
        ..Default::default()
    };
    assert_eq!(observe(&mut observation, &broadcast(&tool2)), None);

    // 6. Stop hooks finish with confirmation
    let confirmed_stop = HookEvent {
        hook_event_name: "Stop".into(),
        session_id: "conv-loop".into(),
        agy_turn_complete: Some(true),
        prompt_id: Some("prompt".into()),
        ..Default::default()
    };
    assert_eq!(
        crate::agent::hooks::hook_state_for("agy", &confirmed_stop),
        Some(crate::agent::AgentState::Idle)
    );
    assert_eq!(
        observe(&mut observation, &broadcast(&confirmed_stop)),
        Some(AgentOutcome::Completed)
    );
}

#[test]
fn agy_stop_confirmation_failure_returns_status_unavailable() {
    let mut observation = PromptObservation::new("agy", "conv-fail", "run tasks");

    let broadcast = |hook: &HookEvent| {
        AgentEvent::from_hook(
            "agy",
            TerminalId::new(7),
            hook,
            serde_json::to_string(hook).unwrap(),
        )
    };

    let start = HookEvent {
        hook_event_name: "UserPromptSubmit".into(),
        prompt_id: Some("prompt".into()),
        session_id: "conv-fail".into(),
        prompt: Some("run tasks".into()),
        prompt_sha256: Some(prompt_sha256("run tasks")),
        turn_id: Some("1".into()),
        ..Default::default()
    };
    assert_eq!(
        observe(&mut observation, &broadcast(&start)),
        Some(AgentOutcome::Submitted)
    );

    let failed_confirmation = HookEvent {
        hook_event_name: "Stop".into(),
        session_id: "conv-fail".into(),
        agy_turn_complete: Some(false),
        prompt_id: Some("prompt".into()),
        ..Default::default()
    };
    assert_eq!(
        observe(&mut observation, &broadcast(&failed_confirmation)),
        Some(AgentOutcome::StatusUnavailable)
    );
}

#[test]
fn agy_competing_prompt_and_stop_failure() {
    let mut observation = PromptObservation::new("agy", "conv-1", "original prompt");

    let broadcast = |hook: &HookEvent| {
        AgentEvent::from_hook(
            "agy",
            TerminalId::new(7),
            hook,
            serde_json::to_string(hook).unwrap(),
        )
    };

    let submit = HookEvent {
        hook_event_name: "UserPromptSubmit".into(),
        prompt_id: Some("prompt".into()),
        session_id: "conv-1".into(),
        prompt: Some("original prompt".into()),
        prompt_sha256: Some(prompt_sha256("original prompt")),
        ..Default::default()
    };
    assert_eq!(
        observe(&mut observation, &broadcast(&submit)),
        Some(AgentOutcome::Submitted)
    );

    // A competing prompt with different fingerprint interrupts
    let competing = HookEvent {
        hook_event_name: "UserPromptSubmit".into(),
        prompt_id: Some("prompt".into()),
        session_id: "conv-1".into(),
        prompt: Some("different prompt".into()),
        prompt_sha256: Some(prompt_sha256("different prompt")),
        ..Default::default()
    };
    assert_eq!(
        observe(&mut observation, &broadcast(&competing)),
        Some(AgentOutcome::Interrupted)
    );

    // StopFailure outcome
    let mut observation_fail = PromptObservation::new("agy", "conv-2", "failing prompt");
    let submit_fail = HookEvent {
        hook_event_name: "UserPromptSubmit".into(),
        prompt_id: Some("prompt".into()),
        session_id: "conv-2".into(),
        prompt: Some("failing prompt".into()),
        prompt_sha256: Some(prompt_sha256("failing prompt")),
        ..Default::default()
    };
    assert_eq!(
        observe(&mut observation_fail, &broadcast(&submit_fail)),
        Some(AgentOutcome::Submitted)
    );
    let stop_failure = HookEvent {
        hook_event_name: "StopFailure".into(),
        session_id: "conv-2".into(),
        ..Default::default()
    };
    assert_eq!(
        observe(&mut observation_fail, &broadcast(&stop_failure)),
        Some(AgentOutcome::Failed)
    );
}

#[test]
fn only_pi_normalizes_editor_input_before_correlating_a_prompt() {
    for (prompt, normalized) in [
        (
            "review\tthese\t\tfiles\nthen\treport",
            "review    these        files\nthen    report",
        ),
        ("\u{feff}review", "review"),
        (" \u{feff}\treview\u{feff}\n", "review"),
        (
            "\u{feff}review\tthese\u{feff}files\u{feff}",
            "review    these\u{feff}files",
        ),
    ] {
        for kind in ["pi", "claude", "codex", "muse", "agy"] {
            for received in [prompt, normalized] {
                let mut observation = PromptObservation::new(kind, "conversation", prompt);
                let hook = HookEvent {
                    hook_event_name: "UserPromptSubmit".into(),
                    session_id: "conversation".into(),
                    turn_id: Some("turn".into()),
                    prompt_id: Some("prompt".into()),
                    prompt: Some(received.into()),
                    ..Default::default()
                };
                let event = AgentEvent::from_hook(kind, TerminalId::new(7), &hook, String::new());
                let matches = (kind == "pi") == (received == normalized);
                assert_eq!(
                    observation.observe(&event, Some(&hook)),
                    Some(if matches {
                        AgentOutcome::Submitted
                    } else {
                        AgentOutcome::Interrupted
                    }),
                    "{kind}: {received:?}"
                );
                assert_eq!(observation.acknowledged(), matches);
            }
        }
    }
}

#[test]
fn pi_dialog_before_submit_needs_input_without_acknowledging() {
    let mut observation = PromptObservation::new("pi", "conversation", "review");
    for turn in [Some("stale"), Some("")] {
        assert_eq!(
            observe(&mut observation, &event("pi", "Elicitation", turn, None)),
            None
        );
    }
    assert_eq!(
        observe(&mut observation, &event("pi", "Elicitation", None, None)),
        Some(AgentOutcome::NeedsInput)
    );
    assert!(!observation.acknowledged());
    assert_eq!(observation.turn_id(), None);
    assert_eq!(
        observe(&mut observation, &event("pi", "Stop", Some("old"), None)),
        None
    );
    assert_eq!(
        observe(
            &mut observation,
            &event("pi", "UserPromptSubmit", Some("new"), Some("review"))
        ),
        Some(AgentOutcome::Submitted)
    );
    for turn in [None, Some("old")] {
        assert_eq!(
            observe(&mut observation, &event("pi", "Elicitation", turn, None)),
            None,
            "acknowledgement requires subsequent dialogs to match the turn"
        );
    }
    assert_eq!(
        observe(&mut observation, &event("pi", "Stop", Some("new"), None)),
        Some(AgentOutcome::Completed)
    );
}

#[test]
fn pi_dialog_before_submit_requires_matching_conversation_and_agent() {
    for mismatch in [
        "hook-conversation",
        "event-conversation",
        "kind",
        "nonhuman",
    ] {
        let mut observation = PromptObservation::new("pi", "conversation", "review");
        let mut hook = HookEvent {
            hook_event_name: "Elicitation".into(),
            session_id: "conversation".into(),
            ..Default::default()
        };
        if mismatch == "nonhuman" {
            hook.hook_event_name = "PermissionRequest".into();
            hook.approvals_reviewer = Some("auto_review".into());
        }
        let mut event = AgentEvent::from_hook("pi", TerminalId::new(7), &hook, String::new());
        match mismatch {
            "hook-conversation" => hook.session_id = "other".into(),
            "event-conversation" => event.session_id = Some("other".into()),
            "kind" => event.kind = "codex".into(),
            _ => {},
        }
        assert_eq!(
            observation.observe(&event, Some(&hook)),
            if mismatch == "nonhuman" {
                None
            } else {
                Some(AgentOutcome::TargetChanged)
            },
            "{mismatch}"
        );
        assert!(!observation.acknowledged());
    }
}

#[test]
fn pi_requires_matching_prompt_conversation_and_turn() {
    for (stop, outcome) in [
        ("Stop", AgentOutcome::Completed),
        ("StopFailure", AgentOutcome::Failed),
        ("Interrupt", AgentOutcome::Interrupted),
    ] {
        let mut observation = PromptObservation::new("pi", "conversation", "review");
        assert_eq!(
            observe(&mut observation, &event("pi", "Stop", Some("old"), None)),
            None
        );
        assert_eq!(
            observe(
                &mut observation,
                &event("pi", "UserPromptSubmit", None, Some("review"))
            ),
            Some(AgentOutcome::StatusUnavailable)
        );
        assert_eq!(
            observe(
                &mut observation,
                &event("pi", "UserPromptSubmit", Some("new"), Some("review"))
            ),
            Some(AgentOutcome::Submitted)
        );
        for turn in [None, Some("old")] {
            assert_eq!(
                observe(&mut observation, &event("pi", stop, turn, None)),
                None
            );
        }
        assert_eq!(
            observe(&mut observation, &event("pi", stop, Some("new"), None)),
            Some(outcome)
        );
    }
    let mut observation = PromptObservation::new("pi", "conversation", "review");
    assert_eq!(
        observe(
            &mut observation,
            &event("pi", "UserPromptSubmit", Some("new"), Some("manual"))
        ),
        Some(AgentOutcome::Interrupted)
    );
    let mut observation = PromptObservation::new("pi", "other", "review");
    assert_eq!(
        observe(
            &mut observation,
            &event("pi", "UserPromptSubmit", Some("new"), Some("review"))
        ),
        Some(AgentOutcome::TargetChanged)
    );
    assert!(is_conversation_reset("pi", "/new"));
    assert!(!is_command_prompt("pi", "/new"));
    for prompt in [
        "/reload",
        "/new extra",
        "!ls",
        "  !pwd",
        "\u{feff}!command",
        " \u{feff}\t/reload",
        "\u{feff}/new",
        "/new\u{feff}",
    ] {
        assert!(is_command_prompt("pi", prompt));
    }
    assert!(!is_command_prompt("pi", "\u{feff}ordinary prompt"));
    assert_eq!(
        pi_prompt_text("\u{0085}review\u{0085}"),
        "\u{0085}review\u{0085}"
    );
}

#[test]
fn plugin_prompt_observation_requires_the_acknowledged_turn_and_matches_input_quirks() {
    use crate::agent::provider::{register, unregister_owner, AgentInput, AgentProvider};
    let kind = "control-observation-test";
    let owner = 70_001;
    register(
        owner,
        AgentProvider {
            kind: kind.into(),
            executable: kind.into(),
            input: Some(AgentInput {
                paste_prefix: "Instructions: ".into(),
                newline: "\r".into(),
                command_prefixes: vec![":".into()],
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(supports_prompt(kind));
    assert!(is_command_prompt(kind, " :reset"));
    assert!(!is_command_prompt(kind, "/a/path"));
    for prompt in ["review\ncode", "Instructions: review\rcode"] {
        let mut observation = PromptObservation::new(kind, "conversation", "review\ncode");
        assert_eq!(
            observe(&mut observation, &event(kind, "Stop", Some("old"), None)),
            None
        );
        assert_eq!(
            observe(
                &mut observation,
                &event(kind, "UserPromptSubmit", None, Some(prompt))
            ),
            Some(AgentOutcome::StatusUnavailable)
        );
        assert!(!observation.acknowledged());
        assert_eq!(
            observe(
                &mut observation,
                &event(kind, "UserPromptSubmit", Some("new"), Some(prompt))
            ),
            Some(AgentOutcome::Submitted)
        );
        for stale in [None, Some("old")] {
            assert_eq!(
                observe(&mut observation, &event(kind, "Stop", stale, None)),
                None
            );
            assert_eq!(
                observe(&mut observation, &event(kind, "StopFailure", stale, None)),
                None
            );
        }
        assert_eq!(
            observe(&mut observation, &event(kind, "Stop", Some("new"), None)),
            Some(AgentOutcome::Completed)
        );
        assert_eq!(
            observe(&mut observation, &event(kind, "Stop", Some("new"), None)),
            None
        );
    }
    unregister_owner(owner);
    assert!(!supports_prompt(kind));
    let mut unsupported = PromptObservation::new(kind, "conversation", "review");
    assert_eq!(
        observe(
            &mut unsupported,
            &event(kind, "UserPromptSubmit", Some("new"), Some("review"))
        ),
        Some(AgentOutcome::StatusUnavailable)
    );
    assert!(!unsupported.acknowledged());
}

#[test]
fn spawn_placement_validates_flat_fields_and_direct_requests() {
    use super::super::control::AgentPlacement;
    use serde_json::json;
    for (tab, new_tab) in [
        (json!(-1), json!(null)),
        (json!(1.5), json!(null)),
        (json!("7"), json!(null)),
        (json!(true), json!(null)),
        (json!(null), json!(7)),
        (json!(0), json!(true)),
        (json!(null), json!("")),
        (json!(null), json!("  ")),
        (json!(null), json!("review\n")),
        (json!(null), json!("review\u{7f}")),
    ] {
        assert!(
            AgentPlacement::from_launch_fields(&tab, &new_tab, &json!(null)).is_err(),
            "{tab} {new_tab}"
        );
    }
    let direct: AgentPlacement = serde_json::from_value(json!({"new_tab":"\t"})).unwrap();
    assert!(direct.validate().is_err());
    assert!(serde_json::from_value::<AgentPlacement>(json!({"tab":-1})).is_err());
    assert!(serde_json::from_value::<AgentPlacement>(json!({"tab":1,"new_tab":null})).is_err());
    let old: AgentRequest = serde_json::from_value(json!({"Spawn": {
        "spec": {}, "cwd":null,"project":null,"remote_host":null
    }}))
    .unwrap();
    assert!(matches!(
        old,
        AgentRequest::Spawn {
            placement: AgentPlacement::Automatic,
            ..
        }
    ));
}
