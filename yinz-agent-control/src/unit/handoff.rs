use super::*;
use yinz_types::agent::tracking::{
    ClaudeNotification, DialogInput, EventTrackingHandoff, LifecycleState, PermissionWait,
};
use yinz_types::agent::{AgentInfo, AgentSource, AgentState};

#[test]
fn adoption_remaps_consumption_and_permission_thresholds_independently() {
    for destination in [0, 19] {
        for consumed_at in [None, Some(6), Some(7)] {
            let conversation = "original".to_owned();
            let mut tracking = AgentTrackingHandoff {
                agent: AgentInfo {
                    kind: "claude".into(),
                    state: AgentState::Waiting,
                    source: AgentSource::Reported,
                    detail: None,
                    waiting_on: None,
                    wait_needs_a_person: true,
                    cwd: None,
                    session_id: Some(conversation.clone()),
                    run_id: None,
                },
                input_revision: 7,
                input_consumed: None,
                events: EventTrackingHandoff::default(),
                lifecycle: Some(LifecycleState {
                    kind: "claude".into(),
                    conversation: conversation.clone(),
                    claude: Default::default(),
                    input_revision: consumed_at,
                    ended_turn: None,
                    closeout_turn: None,
                    claude_prompt_id: None,
                    claude_confirmation: None,
                    claude_previous_prompt_id: None,
                    agy_confirmation: None,
                    claude_background: None,
                    claude_notifications: [(
                        String::from("notice"),
                        ClaudeNotification {
                            observation_unavailable: true,
                            waiting: None,
                            permission_revision: Some(2),
                        },
                    )]
                    .into(),
                    last_message: Some("LAUNCH".into()),
                }),
                permission_wait: Some(PermissionWait {
                    conversation: conversation.clone(),
                    input_revision: 1,
                }),
                dialog_input: Some(DialogInput {
                    kind: "claude".into(),
                    conversation: Some(conversation),
                    input_revision: 6,
                    startup: false,
                    fresh_waiting: true,
                }),
                launch: None,
                submissions: Default::default(),
                worker_waits: None,
            };
            remap_input(&mut tracking, destination);
            let state = tracking.lifecycle.unwrap();
            assert_eq!(tracking.input_revision, destination);
            assert_eq!(
                state.input_revision,
                consumed_at.map(|r| if r == 7 {
                    destination
                } else {
                    destination.wrapping_sub(1)
                })
            );
            assert_eq!(
                tracking.dialog_input.unwrap().input_revision,
                destination.wrapping_sub(1)
            );
            assert_eq!(
                tracking.permission_wait.unwrap().input_revision,
                destination
            );
            assert_eq!(
                state.claude_notifications["notice"].permission_revision,
                Some(destination)
            );
            assert!(state.claude_notifications["notice"].observation_unavailable);
            assert_eq!(state.last_message.as_deref(), Some("LAUNCH"));
        }
    }
}
