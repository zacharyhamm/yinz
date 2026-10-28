use super::*;

fn active_work(registration: &Registration, review: bool) -> Conversations {
    let mut conversations = Conversations::default();
    conversations
        .request(
            &json!({"id":7,"method":if review {"review/start"} else {"turn/start"},"params":{"threadId":"selected"}}),
            registration,
        )
        .unwrap();
    let started = if review {
        json!({"id":7,"result":{"reviewThreadId":"selected","turn":{"id":"parent"}}})
    } else {
        json!({"method":"turn/started","params":{"threadId":"selected","turn":{"id":"parent"}}})
    };
    assert!(conversations
        .response(&started, registration)
        .unwrap()
        .is_empty());
    conversations
}

#[test]
fn reconnect_keeps_failures_and_review_completions_without_reselection() {
    for (review, status, event) in [
        (false, "failed", "StopFailure"),
        (true, "completed", "Stop"),
        (true, "interrupted", "Interrupt"),
        (true, "failed", "StopFailure"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let registration = registration(root.path(), "aaaaaaaaaaaa");
        let mut conversations = active_work(&registration, review);
        conversations.reconnect();
        assert_eq!(
            conversations.outstanding_turn(),
            Some(("selected", "parent"))
        );
        let completed = json!({"method":"turn/completed","params":{"threadId":"selected","turn":{"id":"parent","status":status,"error":{"message":"failure details"}}}});
        let hooks = conversations.response(&completed, &registration).unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["hook_event_name"], event);
        assert_eq!(hooks[0]["session_id"], "selected");
        assert_eq!(hooks[0]["turn_id"], "parent");
        if status == "failed" {
            assert_eq!(hooks[0]["last_assistant_message"], "failure details");
        }
        assert!(conversations.outstanding_turn().is_none());
        assert!(conversations
            .response(&completed, &registration)
            .unwrap()
            .is_empty());
    }
}

#[test]
fn reconnect_new_thread_clears_the_previous_selection() {
    let root = tempfile::tempdir().unwrap();
    let registration = registration(root.path(), "aaaaaaaaaaaa");
    let mut conversations = active_work(&registration, false);
    conversations.reconnect();
    conversations
        .request(&json!({"id":7,"method":"thread/start"}), &registration)
        .unwrap();
    assert_eq!(
        conversations
            .response(
                &json!({"id":7,"result":{"thread":{"id":"new"}}}),
                &registration,
            )
            .unwrap(),
        vec![
            json!({"hook_event_name":"SessionEnd","session_id":"selected","reason":"clear"}),
            json!({"hook_event_name":"SessionStart","session_id":"new","source":"clear","_yinz_codex_hooks_ready":false}),
        ]
    );
    assert_eq!(conversations.selected.as_deref(), Some("new"));
    assert!(conversations.outstanding_turn().is_none());
}

#[test]
fn reconnect_same_thread_resume_keeps_work_but_another_selection_invalidates_it() {
    for review in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let registration = registration(root.path(), "aaaaaaaaaaaa");
        let mut conversations = active_work(&registration, review);
        conversations.reconnect();
        for (thread, expected) in [("selected", Some(("selected", "parent"))), ("other", None)] {
            conversations
                .request(
                    &json!({"id":8,"method":"thread/resume","params":{"threadId":thread}}),
                    &registration,
                )
                .unwrap();
            let hooks = conversations
                .response(
                    &json!({"id":8,"result":{"thread":{"id":thread,"turns":[]}}}),
                    &registration,
                )
                .unwrap();
            if thread == "selected" {
                assert!(
                    hooks.is_empty(),
                    "resuming active work must not reset its lifecycle"
                );
            } else {
                assert_eq!(hooks[0]["source"], "resume");
            }
            assert_eq!(conversations.outstanding_turn(), expected);
        }
    }
}

#[test]
fn reconnect_discards_transport_correlations_before_request_ids_are_reused() {
    let root = tempfile::tempdir().unwrap();
    let registration = registration(root.path(), "aaaaaaaaaaaa");
    let mut conversations = active_work(&registration, true);
    conversations
        .request(
            &json!({"id":8,"method":"thread/start","params":{"ephemeral":true}}),
            &registration,
        )
        .unwrap();
    conversations.reconnect();
    assert!(conversations.pending.is_empty());
    assert!(conversations.review.as_ref().unwrap().request_id.is_none());
    assert!(conversations
        .response(
            &json!({"id":8,"result":{"thread":{"id":"unrelated"}}}),
            &registration,
        )
        .unwrap()
        .is_empty());
    assert_eq!(conversations.selected.as_deref(), Some("selected"));
    conversations
        .request(
            &json!({"id":7,"method":"thread/resume","params":{"threadId":"selected"}}),
            &registration,
        )
        .unwrap();
    let hooks = conversations
        .response(
            &json!({"id":7,"result":{"thread":{"id":"selected"}}}),
            &registration,
        )
        .unwrap();
    assert!(hooks.is_empty());
    assert!(conversations.pending.is_empty());
    assert_eq!(
        conversations.outstanding_turn(),
        Some(("selected", "parent"))
    );
}

#[tokio::test]
async fn reconnect_replays_a_report_cancelled_before_acknowledgement_in_order() {
    let root = tempfile::tempdir().unwrap();
    let entered = root.path().join("entered");
    assert!(Command::new("mkfifo")
        .arg(&entered)
        .status()
        .unwrap()
        .success());
    let pid_file = root.path().join("pid");
    let received = root.path().join("received");
    let stalled = root.path().join("stalled");
    let reporter = root.path().join("reporter");
    yinz_test_support::write_warm_script(
        &stalled,
        "printf '%s' \"$$\" > \"$YINZ_TEST_REPORT_PID\"\ncat >/dev/null\nprintf x > \"$YINZ_TEST_REPORT_ENTERED\"\nexec sleep 60",
    );
    yinz_test_support::write_warm_script(
        &reporter,
        "cat >> \"$YINZ_TEST_REPORT_OUTPUT\"\nprintf '\\n' >> \"$YINZ_TEST_REPORT_OUTPUT\"",
    );
    let mut registration = Registration::new(
        root.path(),
        HashMap::from([
            (
                "YINZ_TEST_REPORT_PID".into(),
                pid_file.to_str().unwrap().into(),
            ),
            (
                "YINZ_TEST_REPORT_ENTERED".into(),
                entered.to_str().unwrap().into(),
            ),
            (
                "YINZ_TEST_REPORT_OUTPUT".into(),
                received.to_str().unwrap().into(),
            ),
        ]),
    )
    .unwrap();
    registration.reporter = Some(stalled);
    let hooks = vec![
        json!({"hook_event_name":"StopFailure","session_id":"selected","turn_id":"parent"}),
        json!({"hook_event_name":"SessionEnd","session_id":"selected","reason":"clear"}),
    ];
    registration.reports.borrow_mut().extend(hooks.clone());
    let mut flush = Box::pin(registration.flush_reports(Path::new("/usr/bin/true")));
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        tokio::select! {
            _ = &mut flush => panic!("stalled reporter completed before cancellation"),
            entered = tokio::fs::read(&entered) => assert_eq!(entered.unwrap(), b"x"),
        }
    })
    .await
    .unwrap();
    drop(flush);
    let pid = std::fs::read_to_string(pid_file).unwrap().parse().unwrap();
    assert_bridge_child_stopped(pid).await;
    assert_eq!(
        registration
            .reports
            .borrow()
            .iter()
            .cloned()
            .collect::<Vec<_>>(),
        hooks
    );
    registration.reporter = Some(reporter);
    registration.flush_reports(Path::new("/usr/bin/true")).await;
    assert!(registration.reports.borrow().is_empty());
    let reports: Vec<Value> = std::fs::read_to_string(received)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(reports, hooks);
}

#[test]
fn reconnect_recovers_an_unacknowledged_review_from_an_active_resume_marker() {
    let root = tempfile::tempdir().unwrap();
    let registration = registration(root.path(), "aaaaaaaaaaaa");
    let mut conversations = active_work(&registration, true);
    conversations
        .request(
            &json!({"id":9,"method":"review/start","params":{"threadId":"selected"}}),
            &registration,
        )
        .unwrap();
    conversations.reconnect();
    conversations
        .request(
            &json!({"id":10,"method":"thread/resume","params":{"threadId":"selected"}}),
            &registration,
        )
        .unwrap();
    let hooks = conversations
        .response(
            &json!({"id":10,"result":{"thread":{"id":"selected","turns":[
                {"id":"parent","status":"inProgress","items":[{"type":"enteredReviewMode"}]},
                {"id":"review-parent","status":"inProgress","items":[{"type":"enteredReviewMode"}]},
                {"id":"review-child","rootTurnId":"review-parent","status":"inProgress","items":[]}
            ]}}}),
            &registration,
        )
        .unwrap();
    assert!(
        hooks.is_empty(),
        "resume must not reset the active lifecycle"
    );
    assert_eq!(
        conversations.outstanding_turn(),
        Some(("selected", "review-parent"))
    );
    let hooks = conversations
        .response(
            &json!({"method":"turn/completed","params":{"threadId":"selected","turn":{"id":"review-parent","status":"completed"}}}),
            &registration,
        )
        .unwrap();
    assert_eq!(
        hooks,
        vec![json!({"hook_event_name":"Stop","session_id":"selected","turn_id":"review-parent"})]
    );
}

#[test]
fn reconnect_does_not_guess_review_identity_from_ambiguous_or_historical_resume_items() {
    for turns in [
        json!([{ "id":"historical","status":"completed","items":[{"type":"enteredReviewMode"}] }]),
        json!([{ "id":"arbitrary-active","status":"inProgress","items":[] }]),
        json!([{ "id":"","status":"inProgress","items":[{"type":"enteredReviewMode"}] }]),
        json!([{ "id":17,"status":"inProgress","items":[{"type":"enteredReviewMode"}] }]),
        json!([{ "id":"malformed","status":"inProgress","items":{"type":"enteredReviewMode"} }]),
        json!([
            {"id":"one","status":"inProgress","items":[{"type":"enteredReviewMode"}]},
            {"id":"two","status":"inProgress","items":[{"type":"enteredReviewMode"}]}
        ]),
        json!([
            {"id":"parent","status":"inProgress","items":[{"type":"enteredReviewMode"}]},
            {"id":"newer","status":"inProgress","items":[]}
        ]),
    ] {
        let root = tempfile::tempdir().unwrap();
        let registration = registration(root.path(), "aaaaaaaaaaaa");
        let mut conversations = Conversations::default();
        conversations
            .request(
                &json!({"id":9,"method":"review/start","params":{"threadId":"selected"}}),
                &registration,
            )
            .unwrap();
        conversations.reconnect();
        conversations
            .request(
                &json!({"id":10,"method":"thread/resume","params":{"threadId":"selected"}}),
                &registration,
            )
            .unwrap();
        assert!(conversations
            .response(
                &json!({"id":10,"result":{"thread":{"id":"selected","turns":turns}}}),
                &registration,
            )
            .unwrap()
            .is_empty());
        assert!(conversations.outstanding_turn().is_none(), "{turns}");
        assert!(conversations.review.as_ref().unwrap().turn.is_none());
    }
}
