use super::*;
use crate::internal_ids::TerminalId;

fn event(kind: &str, name: &str, turn: Option<&str>, prompt: Option<&str>) -> AgentEvent {
    let hook = HookEvent {
        hook_event_name: name.into(),
        session_id: "conversation".into(),
        turn_id: turn.map(str::to_owned),
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
        observation.observe(&event("codex", "Stop", Some("old"), None)),
        None
    );
    assert!(!observation.acknowledged());
    let acceptance = event("codex", "UserPromptSubmit", Some("new"), Some("review"));
    assert_eq!(
        observation.observe(&acceptance),
        Some(AgentOutcome::Submitted)
    );
    assert_eq!(observation.observe(&acceptance), None, "duplicate hook");
    assert!(observation.acknowledged());
    assert_eq!(observation.turn_id(), Some("new"));
    for turn in [Some("old"), None] {
        assert_eq!(
            observation.observe(&event("codex", "Stop", turn, None)),
            None
        );
    }
    let completion = event("codex", "Stop", Some("new"), None);
    assert_eq!(
        observation.observe(&completion),
        Some(AgentOutcome::Completed)
    );
    assert_eq!(observation.observe(&completion), None);
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
            observation.observe(&acceptance),
            Some(AgentOutcome::Submitted)
        );
        assert_eq!(
            observation.observe(&event("codex", name, Some("new"), None)),
            Some(outcome)
        );
    }
}

#[test]
fn conflicting_prompts_and_conversations_cannot_complete_the_observed_work() {
    for accept_first in [false, true] {
        let mut observation = PromptObservation::new("codex", "conversation", "review");
        if accept_first {
            observation.observe(&event(
                "codex",
                "UserPromptSubmit",
                Some("new"),
                Some("review"),
            ));
        }
        assert_eq!(
            observation.observe(&event(
                "codex",
                "UserPromptSubmit",
                Some("other"),
                Some("manual")
            )),
            Some(AgentOutcome::Interrupted)
        );
        assert_eq!(
            observation.observe(&event("codex", "Stop", Some("new"), None)),
            None
        );
    }
    let mut observation = PromptObservation::new("codex", "different conversation", "review");
    assert_eq!(
        observation.observe(&event(
            "codex",
            "UserPromptSubmit",
            Some("new"),
            Some("review")
        )),
        Some(AgentOutcome::TargetChanged)
    );
}

#[test]
fn only_person_waits_end_the_operation_and_worker_hooks_do_not() {
    let mut observation = PromptObservation::new("codex", "conversation", "review");
    observation.observe(&event(
        "codex",
        "UserPromptSubmit",
        Some("new"),
        Some("review"),
    ));
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
    assert_eq!(observation.observe(&broadcast(&hook)), None);
    hook.hook_event_name = "SessionStart".into();
    hook.source = Some("compact".into());
    assert_eq!(observation.observe(&broadcast(&hook)), None);
    hook.hook_event_name = "Stop".into();
    hook.agent_id = Some("worker".into());
    assert_eq!(observation.observe(&broadcast(&hook)), None);
    hook.agent_id = None;
    hook.hook_event_name = "PermissionRequest".into();
    hook.approvals_reviewer = None;
    assert_eq!(
        observation.observe(&broadcast(&hook)),
        Some(AgentOutcome::NeedsInput)
    );
}

#[test]
fn codex_requires_turn_identity_while_claude_uses_serialized_conversation_hooks() {
    for (kind, expected) in [
        ("codex", AgentOutcome::StatusUnavailable),
        ("claude", AgentOutcome::Submitted),
    ] {
        let mut observation = PromptObservation::new(kind, "conversation", "review");
        assert_eq!(
            observation.observe(&event(kind, "UserPromptSubmit", None, Some("review"))),
            Some(expected)
        );
        if expected == AgentOutcome::Submitted {
            assert_eq!(
                observation.observe(&event(kind, "Stop", None, None)),
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
        observation.observe(&acceptance),
        Some(AgentOutcome::StatusUnavailable)
    );
    assert!(!observation.acknowledged());
}
