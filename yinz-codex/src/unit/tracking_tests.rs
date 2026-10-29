use super::*;

fn completed(thread: &str, turn: &str, status: &str) -> Value {
    json!({"method":"turn/completed", "params":{
        "threadId":thread, "turn":{"id":turn, "status":status}
    }})
}

fn start_review(
    conversations: &mut Conversations,
    registration: &Registration,
    id: usize,
) -> Value {
    conversations
        .request(
            &json!({"id":id,"method":"review/start","params":{"threadId":"selected"}}),
            registration,
        )
        .unwrap();
    json!({"id":id,"result":{"reviewThreadId":"selected","turn":{"id":format!("review-{id}")}}})
}

#[test]
fn tracking_helpers_need_no_tombstones_and_unknown_threads_cannot_claim_the_pane() {
    let root = tempfile::tempdir().unwrap();
    let registration = registration(root.path(), "aaaaaaaaaaaa");
    let mut conversations = loaded_conversations(&registration, &["selected"]);
    for id in 0..1000 {
        let helper = format!("helper-{id}");
        conversations
            .request(
                &json!({"id":id,"method":"thread/start","params":{"ephemeral":id % 2 == 0}}),
                &registration,
            )
            .unwrap();
        assert!(conversations
            .response(
                &json!({"id":id,"result":{"thread":{"id":helper,"ephemeral":id % 2 == 1}}}),
                &registration
            )
            .unwrap()
            .is_empty());
        conversations
            .response(
                &json!({"method":"thread/closed","params":{"threadId":helper}}),
                &registration,
            )
            .unwrap();
        for method in ["turn/start", "turn/steer", "review/start"] {
            conversations
                .request(
                    &json!({"id":"late","method":method,"params":{"threadId":helper}}),
                    &registration,
                )
                .unwrap();
        }
        assert_eq!(conversations.selected.as_deref(), Some("selected"));
        assert_eq!(conversations.visible.len(), 1);
        assert!(conversations.pending.is_empty());
    }
    conversations.reconnect();
    conversations
        .request(
            &json!({"id":"unknown","method":"turn/start","params":{"threadId":"unseen"}}),
            &registration,
        )
        .unwrap();
    assert_eq!(conversations.selected.as_deref(), Some("selected"));
    assert!(binding_for(root.path(), "selected").unwrap().is_some());
}

#[test]
fn tracking_visible_threads_prune_only_on_authoritative_closure_and_can_be_resumed() {
    let root = tempfile::tempdir().unwrap();
    let registration = registration(root.path(), "aaaaaaaaaaaa");
    let mut conversations = loaded_conversations(&registration, &["other", "selected"]);
    for value in [
        json!({"id":"server-request","method":"thread/closed","params":{"threadId":"other"}}),
        json!({"method":"thread/closed","params":{"threadId":17}}),
        json!({"method":"thread/archived","params":{"threadId":"other"}}),
    ] {
        conversations.response(&value, &registration).unwrap();
        assert!(conversations.visible.contains("other"));
    }
    conversations
        .request(
            &json!({"id":1,"method":"turn/start","params":{"threadId":"other"}}),
            &registration,
        )
        .unwrap();
    assert_eq!(conversations.selected.as_deref(), Some("other"));
    conversations
        .request(
            &json!({"id":2,"method":"turn/start","params":{"threadId":"selected"}}),
            &registration,
        )
        .unwrap();
    conversations
        .response(
            &json!({"method":"thread/closed","params":{"threadId":"other"}}),
            &registration,
        )
        .unwrap();
    assert_eq!(conversations.visible, HashSet::from(["selected".into()]));
    conversations
        .request(
            &json!({"id":3,"method":"turn/start","params":{"threadId":"other"}}),
            &registration,
        )
        .unwrap();
    assert_eq!(conversations.selected.as_deref(), Some("selected"));
    conversations
        .request(
            &json!({"id":4,"method":"thread/resume","params":{"threadId":"other"}}),
            &registration,
        )
        .unwrap();
    conversations
        .response(
            &json!({"id":4,"error":{"message":"not found"}}),
            &registration,
        )
        .unwrap();
    assert!(!conversations.visible.contains("other"));
    conversations
        .request(
            &json!({"id":5,"method":"thread/resume","params":{"threadId":"other"}}),
            &registration,
        )
        .unwrap();
    conversations
        .response(
            &json!({"id":5,"result":{"thread":{"id":"other","ephemeral":false}}}),
            &registration,
        )
        .unwrap();
    assert!(conversations.visible.contains("other"));
    assert_eq!(conversations.selected.as_deref(), Some("other"));
}

#[test]
fn tracking_completed_reviews_do_not_accumulate_or_authorize_replayed_outcomes() {
    let root = tempfile::tempdir().unwrap();
    let registration = registration(root.path(), "aaaaaaaaaaaa");
    let mut conversations = loaded_conversations(&registration, &["selected"]);
    for id in 0..1000 {
        let reply = start_review(&mut conversations, &registration, id);
        for stale in [
            json!({"method":"item/started","params":{"threadId":"selected","turnId":"review-0","item":{"type":"enteredReviewMode"}}}),
            completed("selected", "review-0", "failed"),
        ] {
            if id > 0 {
                assert!(conversations
                    .response(&stale, &registration)
                    .unwrap()
                    .is_empty());
            }
        }
        assert!(conversations
            .response(&reply, &registration)
            .unwrap()
            .is_empty());
        assert_eq!(conversations.review_turns.len(), 1);
        let hooks = conversations
            .response(
                &completed("selected", &format!("review-{id}"), "completed"),
                &registration,
            )
            .unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["hook_event_name"], "Stop");
        assert!(conversations.review_turns.is_empty());
    }
}

#[test]
fn tracking_unfinished_reviews_prune_on_terminal_events_even_after_selection_changes() {
    let root = tempfile::tempdir().unwrap();
    let registration = registration(root.path(), "aaaaaaaaaaaa");
    let mut conversations = loaded_conversations(&registration, &["other", "selected"]);
    for id in 0..2 {
        let reply = start_review(&mut conversations, &registration, id);
        conversations.response(&reply, &registration).unwrap();
    }
    assert_eq!(conversations.review_turns.len(), 2);
    conversations
        .request(
            &json!({"id":3,"method":"turn/start","params":{"threadId":"other"}}),
            &registration,
        )
        .unwrap();
    assert!(conversations
        .response(
            &completed("selected", "review-0", "inProgress"),
            &registration
        )
        .unwrap()
        .is_empty());
    assert_eq!(conversations.review_turns.len(), 2);
    assert!(conversations
        .response(
            &completed("selected", "review-0", "completed"),
            &registration
        )
        .unwrap()
        .is_empty());
    assert_eq!(conversations.review_turns.len(), 1);
    conversations
        .response(
            &json!({"method":"thread/closed","params":{"threadId":"selected"}}),
            &registration,
        )
        .unwrap();
    assert!(conversations.review_turns.is_empty());
    assert_eq!(conversations.selected.as_deref(), Some("other"));
}

#[test]
fn tracking_early_review_outcomes_are_bounded_and_require_the_matching_reply() {
    let root = tempfile::tempdir().unwrap();
    let registration = registration(root.path(), "aaaaaaaaaaaa");
    let mut conversations = loaded_conversations(&registration, &["selected"]);
    let reply = start_review(&mut conversations, &registration, 7);
    assert!(conversations
        .response(
            &completed("selected", "review-7", "completed"),
            &registration
        )
        .unwrap()
        .is_empty());
    for id in 0..1000 {
        assert!(conversations
            .response(
                &completed("selected", &format!("unrelated-{id}"), "failed"),
                &registration
            )
            .unwrap()
            .is_empty());
        assert!(conversations.review_turns.is_empty());
    }
    assert!(conversations
        .response(&reply, &registration)
        .unwrap()
        .is_empty());
    assert_eq!(
        conversations.outstanding_turn(),
        Some(("selected", "review-7"))
    );
    assert!(conversations
        .review
        .as_ref()
        .unwrap()
        .early_terminal
        .is_none());
    assert_eq!(
        conversations
            .response(
                &completed("selected", "review-7", "completed"),
                &registration
            )
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn tracking_closure_preserves_known_outcome_recovery_and_cancelled_review_identity() {
    let root = tempfile::tempdir().unwrap();
    let registration = registration(root.path(), "aaaaaaaaaaaa");
    let mut conversations = loaded_conversations(&registration, &["selected"]);
    let reply = start_review(&mut conversations, &registration, 7);
    conversations.response(&reply, &registration).unwrap();
    conversations
        .response(
            &json!({"method":"thread/closed","params":{"threadId":"selected"}}),
            &registration,
        )
        .unwrap();
    conversations.reconnect();
    assert!(conversations.visible.is_empty());
    assert!(conversations.review_turns.is_empty());
    assert_eq!(
        conversations.outstanding_turn(),
        Some(("selected", "review-7"))
    );
    assert_eq!(
        conversations
            .response(
                &completed("selected", "review-7", "completed"),
                &registration
            )
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        conversations.reported_review,
        Some(("selected".into(), "review-7".into()))
    );
}

#[test]
fn tracking_resume_history_prunes_only_proven_terminal_review_parents() {
    let root = tempfile::tempdir().unwrap();
    let registration = registration(root.path(), "aaaaaaaaaaaa");
    let mut conversations = loaded_conversations(&registration, &["selected"]);
    for id in 0..3 {
        let reply = start_review(&mut conversations, &registration, id);
        conversations.response(&reply, &registration).unwrap();
    }
    conversations.reconnect();
    conversations
        .request(
            &json!({"id":"resume","method":"thread/resume","params":{"threadId":"selected"}}),
            &registration,
        )
        .unwrap();
    assert!(conversations
        .response(
            &json!({"id":"resume","result":{"thread":{"id":"selected","turns":[
                {"id":"review-0","status":"completed"},
                {"id":"review-1","status":"unknown"},
                {"id":"review-2","status":"inProgress"}
            ]}}}),
            &registration
        )
        .unwrap()
        .is_empty());
    assert_eq!(
        conversations.review_turns,
        HashSet::from([
            ("selected".into(), "review-1".into()),
            ("selected".into(), "review-2".into()),
        ])
    );
    assert_eq!(
        conversations.outstanding_turn(),
        Some(("selected", "review-2"))
    );
}
