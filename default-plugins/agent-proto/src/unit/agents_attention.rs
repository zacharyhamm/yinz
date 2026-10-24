use super::presentation::awaiting_confirmation;
use super::*;
use serde_json::json;

#[test]
fn attention_uses_current_evidence_and_never_marks_an_exited_pane() {
    let summaries = Summaries::from([(
        7,
        Summary {
            text: "Please approve".into(),
            title: String::new(),
            awaiting: true,
            hash: 1,
            created_at_ms: None,
        },
    )]);
    let activity = Activity::from([(7, (1, 2))]);
    let waiting = HookFeeds::from([(
        7,
        HookFeed {
            phase: HookPhase::Waiting,
            detail: None,
            waiting_on: Some("Which approach?".into()),
            session_id: "session".into(),
            cwd: None,
        },
    )]);
    let mut busy = waiting.clone();
    busy.get_mut(&7).unwrap().phase = HookPhase::Busy;
    for status in [AgentStatus::Running, AgentStatus::Held, AgentStatus::Exited] {
        for hooks in [&HookFeeds::new(), &waiting, &busy] {
            let expected = status == AgentStatus::Running && hooks != &busy;
            assert_eq!(
                awaiting_confirmation(&summaries, &activity, hooks, status, 7),
                expected,
                "{status:?} {hooks:?}"
            );
        }
    }
    for stale_or_busy in [(2, 2), (1, 0)] {
        assert!(!awaiting_confirmation(
            &summaries,
            &Activity::from([(7, stale_or_busy)]),
            &HookFeeds::new(),
            AgentStatus::Running,
            7,
        ));
    }
}

#[test]
fn older_strips_without_waiting_metadata_do_not_guess_from_the_title() {
    let row: StripRow = serde_json::from_value(json!({
        "Agent": {"pane_id": 7, "kind": "codex", "text": "! waiting in a title"}
    }))
    .unwrap();
    assert!(matches!(row, StripRow::Agent { waiting: false, .. }));
}
