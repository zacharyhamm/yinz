use super::*;
use yinz_types::agent::claude_notifications::{Launch, TaskKind};

#[test]
fn idle_drain_settles_at_the_provider_grace_deadline() {
    let mut pending = Pending {
        launches: vec![Obligation {
            launch: Launch {
                task: "one".into(),
                tool: "tool".into(),
                prompt: "turn".into(),
                kind: TaskKind::Bash,
            },
            remaining_ms: MAX_TIMEOUT_MS,
            answer_prompt: None,
            drained: None,
            unavailable: false,
            settled: false,
        }],
        ..Default::default()
    };
    drain(
        &mut pending,
        &Evidence {
            snapshot: true,
            ..Default::default()
        },
        &HookEvent {
            hook_event_name: "Stop".into(),
            prompt_id: Some("turn".into()),
            ..Default::default()
        },
        TurnFacts {
            newest: Some("turn"),
            idle: true,
        },
    );
    assert_eq!(drain_wait(&pending), Some(DRAIN_GRACE_MS));
    pending.elapse(DRAIN_GRACE_MS - 1);
    settle_drained(&mut pending, Some("turn"));
    assert!(pending.active());
    assert_eq!(drain_wait(&pending), Some(1));
    pending.elapse(1);
    settle_drained(&mut pending, Some("turn"));
    assert!(!pending.active());
    assert_eq!(drain_wait(&pending), None);
}
