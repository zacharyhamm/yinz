use super::*;
use yinz_types::internal_ids::TerminalId;

const MOTHER: PaneId = PaneId::Terminal(TerminalId::new(1));

fn event(sequence: usize, pane: &str, kind: &str) -> Value {
    json!({"cursor":cursor("session", sequence),"sequence":sequence,"pane_id":pane,"event_type":kind,
        "kind":"claude","conversation_id":"conversation","state":"idle","background_tasks":[]})
}

fn cursor(session: &str, sequence: usize) -> String {
    json!({"v":1,"host":"host","session":session,"epoch":"epoch","sequence":sequence,
        "filters":EventSubscription::default()})
    .to_string()
}

#[test]
fn loading_resets_stale_malformed_and_incomplete_cursors_without_losing_memory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.json");
    let mut memory = Memory {
        revision: 12,
        checkpoint: json!({"assignment":"keep"}),
        paused: true,
        filters: EventSubscription {
            kinds: vec!["codex".into()],
            ..Default::default()
        },
        ..Default::default()
    };
    memory.batch("pending".into(), false);
    memory.pending[0].attempted = true;
    memory.receipts.push_back(Receipt {
        batch: memory.pending[0].id.clone(),
        conversation: "conversation".into(),
        submission: "submission".into(),
        response: None,
        settled: false,
    });
    let mut valid: Value = serde_json::from_str(&cursor("session", 10)).unwrap();
    valid["filters"] = json!(memory.filters);
    let mut stale = valid.clone();
    stale["session"] = "old-session".into();
    let mut malformed = valid.clone();
    malformed["sequence"] = "invalid".into();
    for (cursor, snapshotting, keep) in [
        (Some(valid.to_string()), false, true),
        (Some(valid.to_string()), true, false),
        (Some(stale.to_string()), false, false),
        (Some(malformed.to_string()), false, false),
        (Some("not json".into()), false, false),
        (Some("null".into()), false, false),
        (Some(json!({"session":"session"}).to_string()), false, false),
        (None, false, false),
    ] {
        memory.cursor = cursor.clone();
        memory.snapshotting = snapshotting;
        memory.save(&path).unwrap();
        let restored = Memory::load(&path, "session").unwrap();
        let mut expected = json!(memory);
        expected["cursor"] = if keep { json!(cursor) } else { Value::Null };
        assert_eq!(json!(restored), expected);
        assert!(restored.pending[0].attempted);
        assert!(!restored.receipts[0].settled);
    }
}

#[test]
fn snapshot_duplicates_new_agents_and_self_activity() {
    let mut memory = Memory::default();
    memory.event(event(1, "terminal_2", "snapshot"), MOTHER);
    assert!(memory.pending.is_empty());
    assert_eq!(memory.roster.len(), 1);
    let fact = event(2, "terminal_3", "pane_spawned");
    memory.event(fact.clone(), MOTHER);
    assert!(!memory.event(fact, MOTHER));
    assert_eq!(memory.pending[0].events, 1);
    assert_eq!(memory.roster.len(), 2);
    memory.event(event(3, "terminal_1", "snapshot"), MOTHER);
    let count = memory.pending[0].events;
    memory.event(event(4, "terminal_1", "turn_completed"), MOTHER);
    assert_eq!(memory.pending[0].events, count);
    assert!(!memory.roster.contains_key("terminal_1"));
}

#[test]
fn activity_does_not_wake_and_idle_is_not_completion() {
    let mut memory = Memory::default();
    let mut fact = event(1, "terminal_2", "snapshot");
    fact["background_tasks"] = json!(["task-1"]);
    memory.event(fact.clone(), MOTHER);
    fact["event_type"] = "state_changed".into();
    fact["cursor"] = "2".into();
    fact["detail"] = "reading a file".into();
    memory.event(fact, MOTHER);
    assert!(memory.pending.is_empty());
    let row = &memory.roster["terminal_2"];
    assert!(row["last_outcome"].is_null());
    assert_eq!(row["background_tasks"], json!(["task-1"]));
    assert_eq!(row["current_activity"], "reading a file");
    memory.event(event(3, "terminal_2", "turn_failed"), MOTHER);
    assert!(memory.pending[0].urgent);
    assert_eq!(memory.status(true, false).blockers, 1);
}

#[test]
fn worker_departures_clear_the_roster_and_only_wake_when_selected() {
    for kind in [
        "conversation_replaced",
        "state_changed",
        "pane_closed",
        "pane_transferred",
    ] {
        for selected in [true, false] {
            let mut memory = Memory {
                filters: EventSubscription {
                    lifecycle_types: vec![if selected { kind } else { "turn_failed" }.into()],
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut worker = event(1, "terminal_2", "snapshot");
            worker["needs_input"] = true.into();
            memory.event(worker, MOTHER);
            assert_eq!(memory.status(true, false).blockers, 1);
            let mut departure = event(2, "terminal_2", kind);
            if kind == "conversation_replaced" {
                departure["next_kind"] = Value::Null;
                departure["next_conversation_id"] = Value::Null;
            } else {
                departure["kind"] = Value::Null;
                departure["conversation_id"] = Value::Null;
                departure["state"] = Value::Null;
            }
            memory.event(departure.clone(), MOTHER);
            assert!(memory.roster.is_empty(), "{kind}");
            assert_eq!(memory.status(true, false).blockers, 0, "{kind}");
            assert_eq!(memory.pending.len(), usize::from(selected), "{kind}");
            assert_eq!(memory.cursor, Some(cursor("session", 2)));
            departure["journal_cursor"] = 2.into();
            assert_eq!(memory.journal.back().unwrap().as_ref(), &departure);
        }
    }
}

#[test]
fn replacement_waits_for_a_selected_agent_state_to_rejoin_the_roster() {
    for next_kind in ["claude", "codex"] {
        let mut memory = Memory {
            filters: EventSubscription {
                kinds: vec!["claude".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        memory.event(event(1, "terminal_2", "snapshot"), MOTHER);
        memory.roster.get_mut("terminal_2").unwrap()["assignment"] = "old task".into();
        let mut replacement = event(2, "terminal_2", "conversation_replaced");
        replacement["next_kind"] = next_kind.into();
        replacement["next_conversation_id"] = "replacement".into();
        memory.event(replacement, MOTHER);
        assert!(memory.roster.is_empty());
        assert_eq!(memory.pending[0].events, 1);
        // The stream excludes the replacement state when its provider is not selected.
        if next_kind == "claude" {
            let mut state = event(3, "terminal_2", "state_changed");
            state["conversation_id"] = "replacement".into();
            memory.event(state, MOTHER);
            assert_eq!(
                memory.roster["terminal_2"]["conversation_id"],
                "replacement"
            );
            assert!(memory.roster["terminal_2"]["assignment"].is_null());
        } else {
            memory.event(event(3, "", "checkpoint"), MOTHER);
            assert!(memory.roster.is_empty());
        }
        assert_eq!(memory.cursor, Some(cursor("session", 3)));
        assert_eq!(memory.journal[1]["conversation_id"], "conversation");
        assert_eq!(memory.journal[1]["next_kind"], next_kind);
    }
}

#[test]
fn shell_lifecycle_advances_the_cursor_without_queuing_turns() {
    let mut memory = Memory::default();
    for (index, kind) in [
        "pane_spawned",
        "state_changed",
        "conversation_replaced",
        "pane_closed",
    ]
    .into_iter()
    .enumerate()
    {
        let sequence = index + 1;
        let mut shell = event(sequence, "terminal_2", kind);
        for key in ["kind", "conversation_id", "state"] {
            shell[key] = Value::Null;
        }
        assert!(memory.event(shell, MOTHER));
        assert_eq!(memory.cursor, Some(cursor("session", sequence)));
        assert_eq!(memory.journal.len(), sequence);
        assert!(memory.pending.is_empty(), "{kind}");
        assert!(memory.roster.is_empty(), "{kind}");
    }
}

#[test]
fn checkpoint_conflicts_pause_and_recovery_preserve_memory() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("checkpoint.json");
    let mut memory = Memory::default();
    memory
        .command(MotherRequest::Checkpoint {
            expect_rev: Some(0),
            value: Some(json!({"goals":["finish"]})),
        })
        .unwrap();
    assert!(memory
        .command(MotherRequest::Checkpoint {
            expect_rev: Some(0),
            value: Some(json!({}))
        })
        .is_err());
    memory.command(MotherRequest::Pause).unwrap();
    memory.event(event(1, "terminal_2", "needs_input"), MOTHER);
    memory.save(&path).unwrap();
    let recovered = Memory::load(&path, "session").unwrap();
    assert_eq!(recovered.revision, 1);
    assert_eq!(recovered.checkpoint["goals"], json!(["finish"]));
    assert!(recovered.paused);
    assert_eq!(recovered.pending[0].id, memory.pending[0].id);
    assert_eq!(recovered.cursor, memory.cursor);
    assert!(memory.save(directory.path()).is_err());
    assert_eq!(Memory::load(&path, "session").unwrap().revision, 1);
}

#[test]
fn journal_and_pending_batches_are_bounded_with_explicit_gaps() {
    let mut memory = Memory {
        journal: (0..JOURNAL_RECORDS)
            .map(|sequence| Arc::new(event(sequence, "terminal_2", "turn_completed")))
            .collect(),
        ..Default::default()
    };
    let committed = memory.clone();
    assert!(Arc::ptr_eq(&memory.journal[0], &committed.journal[0]));
    memory.event(
        event(JOURNAL_RECORDS, "terminal_2", "turn_completed"),
        MOTHER,
    );
    assert!(memory.journal.len() <= JOURNAL_RECORDS);
    assert_eq!(committed.journal.len(), JOURNAL_RECORDS);
    assert!(!committed.gap);
    let retained = memory.journal.front().unwrap();
    let sequence = retained["sequence"].as_u64().unwrap() as usize;
    assert!(Arc::ptr_eq(retained, &committed.journal[sequence]));
    assert!(memory
        .journal
        .iter()
        .any(|v| v["reason"] == "mother_journal_overflow"));
    assert!(memory.gap);
    let inbox = memory.command(MotherRequest::Inbox).unwrap();
    assert!(inbox["journal"]["records"].as_array().unwrap().len() <= 50);
    assert_eq!(inbox["gap"], true);
    for _ in 0..300 {
        memory.pending.back_mut().unwrap().attempted = true;
        memory.batch("completion".into(), false);
    }
    assert_eq!(memory.pending.len(), 256);
    assert!(memory.pending.back().unwrap().summary.len() <= 32);
}

#[test]
fn filters_are_validated_and_force_a_new_snapshot() {
    let mut memory = Memory {
        cursor: Some("old".into()),
        ..Default::default()
    };
    memory
        .command(MotherRequest::Configure {
            panes: vec!["2".into()],
            projects: vec!["app".into()],
            kinds: vec!["codex".into()],
            lifecycle_types: vec!["turn_completed".into()],
        })
        .unwrap();
    assert_eq!(memory.filters.panes, vec!["terminal_2"]);
    assert!(memory.cursor.is_none());
    assert!(memory
        .command(MotherRequest::Configure {
            panes: vec!["bogus".into()],
            projects: vec![],
            kinds: vec![],
            lifecycle_types: vec![]
        })
        .is_err());
}

#[test]
fn lifecycle_filters_select_wakes_without_staling_the_roster() {
    let mut memory = Memory {
        filters: EventSubscription {
            lifecycle_types: vec!["turn_failed".into()],
            ..Default::default()
        },
        ..Default::default()
    };
    memory.event(event(1, "terminal_2", "snapshot"), MOTHER);
    memory.event(event(2, "terminal_2", "turn_failed"), MOTHER);
    assert_eq!(memory.status(true, false).blockers, 1);
    assert_eq!(memory.pending[0].events, 1);
    assert!(memory.selects_wake("turn_failed"));
    assert!(!memory.selects_wake("needs_input"));

    memory.event(event(3, "terminal_2", "submission_acknowledged"), MOTHER);
    assert_eq!(memory.status(true, false).blockers, 0);
    let mut busy = event(4, "terminal_2", "state_changed");
    busy["state"] = "running".into();
    memory.event(busy, MOTHER);
    assert_eq!(memory.roster["terminal_2"]["state"], "running");
    assert_eq!(memory.pending[0].events, 1);
    memory.event(event(5, "terminal_2", "turn_failed"), MOTHER);
    memory.event(event(6, "terminal_2", "pane_closed"), MOTHER);
    assert!(memory.roster.is_empty());
    assert_eq!(memory.status(true, false).blockers, 0);
    assert_eq!(memory.pending[0].events, 2);
    assert_eq!(memory.journal.len(), 6);
}

#[test]
fn project_departures_remove_blockers_but_preserve_the_journal() {
    for (selected, projects, kind, retained) in [
        (vec!["app"], json!([]), "project_membership_changed", false),
        (
            vec!["app"],
            json!(["other"]),
            "project_membership_changed",
            false,
        ),
        (
            vec!["app", "other"],
            json!(["other"]),
            "project_membership_changed",
            true,
        ),
        (vec![], json!([]), "project_membership_changed", true),
        (vec!["app"], json!([]), "observation_uncertain", true),
        (vec!["app"], Value::Null, "project_membership_changed", true),
    ] {
        for wake_on_departure in [true, false] {
            let mut memory = Memory {
                filters: EventSubscription {
                    projects: selected.iter().map(|project| (*project).into()).collect(),
                    lifecycle_types: if wake_on_departure {
                        vec![]
                    } else {
                        vec!["turn_failed".into()]
                    },
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut failed = event(1, "terminal_2", "turn_failed");
            failed["projects"] = json!(["app"]);
            failed["needs_input"] = true.into();
            memory.event(failed, MOTHER);
            assert_eq!(memory.status(true, false).blockers, 1);

            let mut departure = event(2, "terminal_2", kind);
            departure["previous_projects"] = json!(["app"]);
            departure["projects"] = projects.clone();
            departure["needs_input"] = true.into();
            if kind == "observation_uncertain" {
                departure["reason"] = "project_membership_unresolved".into();
                departure["observed_event_type"] = "project_membership_changed".into();
            }
            assert!(memory.event(departure.clone(), MOTHER));
            assert_eq!(memory.roster.contains_key("terminal_2"), retained);
            assert_eq!(memory.status(true, false).blockers, usize::from(retained));
            assert_eq!(
                memory.journal.back().unwrap()["event_type"],
                departure["event_type"]
            );
            assert_eq!(memory.journal.len(), 2);
            assert_eq!(memory.cursor, Some(cursor("session", 2)));
            assert_eq!(memory.pending[0].events, 1 + usize::from(wake_on_departure));
        }
    }
}

#[test]
fn lifecycle_wake_filters_use_unfiltered_subscription_cursors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.json");
    let mut memory = Memory {
        filters: EventSubscription {
            panes: vec!["terminal_2".into()],
            projects: vec!["app".into()],
            kinds: vec!["codex".into()],
            lifecycle_types: vec!["turn_failed".into()],
            ..Default::default()
        },
        ..Default::default()
    };
    let mut expected = memory.filters.clone();
    expected.lifecycle_types.clear();
    assert_eq!(json!(memory.subscription_filters()), json!(expected));
    let mut cursor: Value = serde_json::from_str(&cursor("session", 10)).unwrap();
    cursor["filters"] = json!(expected);
    memory.cursor = Some(cursor.to_string());
    memory.batch("keep".into(), false);
    memory.save(&path).unwrap();
    let restored = Memory::load(&path, "session").unwrap();
    assert_eq!(restored.cursor, memory.cursor);
    assert_eq!(restored.filters.lifecycle_types, vec!["turn_failed"]);

    cursor["filters"] = json!(memory.filters);
    memory.cursor = Some(cursor.to_string());
    memory.save(&path).unwrap();
    let restored = Memory::load(&path, "session").unwrap();
    assert!(restored.cursor.is_none());
    assert_eq!(restored.pending[0].id, memory.pending[0].id);
}

#[test]
fn reconciled_failures_survive_state_updates_and_clear_on_recovery() {
    let mut memory = Memory::default();
    memory.event(event(1, "terminal_2", "turn_failed"), MOTHER);
    memory.event(event(2, "", "snapshot_begin"), MOTHER);
    let mut fact = event(2, "terminal_2", "snapshot");
    fact["submission_id"] = "submission".into();
    memory.event(fact.clone(), MOTHER);
    memory.event(event(2, "", "ready"), MOTHER);
    let request = AgentRequest::WaitTurn {
        pane_id: "terminal_2".parse().unwrap(),
        expected_session_id: Some("conversation".into()),
        submission_id: "submission".into(),
        timeout_ms: 1,
    };
    let mut result = AgentResponse::new(AgentOutcome::Failed);
    result.submission_id = Some("submission".into());
    assert!(memory.reconcile(&request, result.clone()));
    assert_eq!(memory.status(true, false).blockers, 0);
    result.acknowledged = true;
    assert!(memory.reconcile(&request, result.clone()));
    assert_eq!(memory.status(true, false).blockers, 1);
    assert_eq!(memory.status(true, false).state, "needs_attention");

    fact["event_type"] = "state_changed".into();
    fact["cursor"] = cursor("session", 3).into();
    memory.event(fact.clone(), MOTHER);
    assert_eq!(memory.status(true, false).blockers, 1);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.json");
    memory.save(&path).unwrap();
    let mut memory = Memory::load(&path, "session").unwrap();
    assert_eq!(memory.status(true, false).blockers, 1);
    fact["event_type"] = "submission_acknowledged".into();
    fact["submission_id"] = "new-submission".into();
    fact["cursor"] = cursor("session", 4).into();
    memory.event(fact, MOTHER);
    assert_eq!(memory.status(true, false).blockers, 0);
    assert!(!memory.reconcile(&request, result));
    assert!(memory.roster["terminal_2"]["retained_result"].is_null());
}

#[test]
fn reconciliation_rejects_replaced_conversations_closed_panes_and_mismatched_results() {
    let request = AgentRequest::WaitTurn {
        pane_id: "terminal_2".parse().unwrap(),
        expected_session_id: Some("conversation".into()),
        submission_id: "submission".into(),
        timeout_ms: 1,
    };
    for change in ["conversation", "closed", "result"] {
        let mut memory = Memory::default();
        let mut fact = event(1, "terminal_2", "snapshot");
        fact["submission_id"] = "submission".into();
        if change == "conversation" {
            fact["conversation_id"] = "replacement".into();
        }
        memory.event(fact, MOTHER);
        if change == "closed" {
            memory.event(event(2, "terminal_2", "pane_closed"), MOTHER);
        }
        let mut result = AgentResponse::new(AgentOutcome::Failed);
        result.acknowledged = true;
        result.submission_id = Some(if change == "result" {
            "other".into()
        } else {
            "submission".into()
        });
        assert!(!memory.reconcile(&request, result), "{change}");
        assert_eq!(memory.status(true, false).blockers, 0);
    }
}

#[test]
fn inbox_retains_closed_worker_identities_across_pause_and_reload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.json");
    let mut memory = Memory::default();
    memory.command(MotherRequest::Pause).unwrap();
    let mut completed = event(1, "terminal_2", "turn_completed");
    completed["submission_id"] = "submission".into();
    completed["turn_id"] = "turn".into();
    completed["run_id"] = "run".into();
    memory.event(completed.clone(), MOTHER);
    memory.event(event(2, "terminal_2", "pane_closed"), MOTHER);
    memory.save(&path).unwrap();
    let mut restored = Memory::load(&path, "session").unwrap();
    let inbox = restored.command(MotherRequest::Inbox).unwrap();
    assert!(restored.paused);
    assert!(restored.roster.is_empty());
    assert_eq!(
        inbox["journal"]["records"][0]["event_type"],
        completed["event_type"]
    );
    assert_eq!(inbox["journal"]["records"][1]["event_type"], "pane_closed");
    assert_eq!(inbox["batches"][0]["events"], 2);
    assert_eq!(inbox["gap"], false);
}

#[test]
fn incomplete_snapshot_restarts_from_a_snapshot_not_a_partial_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.json");
    let mut memory = Memory::default();
    memory.event(event(1, "", "snapshot_begin"), MOTHER);
    memory.event(event(1, "terminal_2", "snapshot"), MOTHER);
    memory.save(&path).unwrap();
    let mut restored = Memory::load(&path, "session").unwrap();
    assert!(restored.cursor.is_none());
    assert!(restored.snapshotting);
    restored.event(event(1, "", "ready"), MOTHER);
    assert_eq!(restored.cursor, Some(cursor("session", 1)));
}

#[test]
fn acknowledging_inbox_preserves_later_worker_events() {
    for paused in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memory.json");
        let mut memory = Memory {
            paused,
            ..Default::default()
        };
        memory.event(event(1, "terminal_2", "turn_completed"), MOTHER);
        memory.event(event(2, "terminal_3", "turn_completed"), MOTHER);
        assert_eq!(memory.pending.len(), 1);
        let inbox = memory.command(MotherRequest::Inbox).unwrap();
        let batch = inbox["batches"][0]["id"].as_str().unwrap().to_owned();
        assert_eq!(inbox["batches"][0]["events"], 2);
        assert!(!memory.pending[0].attempted);
        assert_eq!(
            memory.status(true, false).state,
            if paused { "paused" } else { "queued" }
        );
        if paused {
            memory.save(&path).unwrap();
            memory = Memory::load(&path, "session").unwrap();
        }
        memory.event(event(3, "terminal_4", "turn_failed"), MOTHER);
        assert_eq!(memory.pending.len(), 2);
        assert_eq!(memory.pending[0].events, 2);
        memory
            .command(MotherRequest::Ack {
                batch: batch.clone(),
            })
            .unwrap();
        assert_eq!(memory.pending.len(), 1);
        assert_ne!(memory.pending[0].id, batch);
        assert_eq!(memory.pending[0].events, 1);
        assert!(memory.pending[0].urgent);
        assert!(!memory.pending[0].attempted);
        assert!(memory.pending[0].summary[0].contains("terminal_4"));
    }
}

#[test]
fn legacy_batches_load_and_unexposed_batches_still_coalesce() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memory.json");
    let mut memory = Memory::default();
    memory.batch("first".into(), false);
    let mut legacy = json!(memory);
    legacy["pending"][0]
        .as_object_mut()
        .unwrap()
        .remove("frozen");
    std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let mut memory = Memory::load(&path, "session").unwrap();
    assert!(!memory.pending[0].frozen);
    memory.batch("second".into(), true);
    assert_eq!(memory.pending.len(), 1);
    assert_eq!(memory.pending[0].events, 2);
    assert!(memory.pending[0].urgent);
}

#[test]
fn uncertain_delivery_never_requeues_an_unknown_submission() {
    let mut memory = Memory::default();
    memory.batch("work finished".into(), false);
    let batch = memory.pending[0].id.clone();
    memory.pending[0].attempted = true;
    memory.pending[0].frozen = true;
    let receipt = Receipt {
        batch,
        conversation: "conv".into(),
        submission: "submission".into(),
        response: None,
        settled: false,
    };
    memory.receipts.push_back(receipt.clone());
    memory.receipt_response(AgentResponse::new(AgentOutcome::Rejected), false);
    assert!(memory.pending[0].attempted);
    assert!(!memory.receipts[0].settled);
    assert_eq!(memory.status(true, false).state, "needs_attention");
    memory.receipts.clear();
    memory.receipts.push_back(receipt);
    memory.receipt_response(AgentResponse::new(AgentOutcome::Rejected), true);
    assert!(!memory.pending[0].attempted);
    assert!(memory.receipts.is_empty());
    memory.batch("later".into(), false);
    assert_eq!(memory.pending.len(), 2);
    assert_eq!(memory.pending[0].events, 1);
    assert_ne!(memory.pending[0].id, memory.pending[1].id);
}

#[test]
fn failed_turn_needs_attention_until_batch_acknowledgement_without_retrying() {
    for delivery in [false, true] {
        let mut memory = Memory::default();
        memory.batch("original".into(), false);
        memory.pending[0].attempted = true;
        let batch = memory.pending[0].id.clone();
        memory.receipts.push_back(Receipt {
            batch: batch.clone(),
            conversation: "conversation".into(),
            submission: "failed-submission".into(),
            response: None,
            settled: false,
        });
        let mut result = AgentResponse::new(AgentOutcome::Failed);
        result.submission_id = Some("failed-submission".into());
        result.acknowledged = true;
        memory.receipt_response(result, delivery);
        assert!(memory.receipts[0].settled);
        assert!(memory.pending[0].attempted);
        assert_eq!(memory.status(true, false).state, "needs_attention");

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memory.json");
        memory.save(&path).unwrap();
        let mut memory = Memory::load(&path, "session").unwrap();
        assert_eq!(memory.status(true, false).state, "needs_attention");
        assert!(memory.pending.iter().all(|b| b.attempted));

        memory.receipts.push_back(Receipt {
            batch: "later-batch".into(),
            conversation: "conversation".into(),
            submission: "later-submission".into(),
            response: Some(AgentResponse::new(AgentOutcome::Completed)),
            settled: true,
        });
        assert_eq!(memory.status(true, false).state, "needs_attention");
        memory.command(MotherRequest::Ack { batch }).unwrap();
        assert_eq!(memory.status(true, false).state, "watching");
        assert!(memory.pending.is_empty());
        assert!(memory.receipts[0].settled);
    }
}

#[test]
fn confirmed_interrupt_settles_without_retrying_the_batch() {
    for acknowledged in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memory.json");
        let mut memory = Memory::default();
        memory.batch("first".into(), false);
        memory.pending[0].attempted = true;
        let batch = memory.pending[0].id.clone();
        memory.receipts.push_back(Receipt {
            batch: batch.clone(),
            conversation: "conversation".into(),
            submission: "submission".into(),
            response: None,
            settled: false,
        });
        let mut result = AgentResponse::new(AgentOutcome::Interrupted);
        result.submission_id = Some("submission".into());
        result.acknowledged = acknowledged;
        memory.receipt_response(result.clone(), true);
        memory.command(MotherRequest::Ack { batch }).unwrap();
        memory.command(MotherRequest::Pause).unwrap();
        memory.command(MotherRequest::Resume).unwrap();
        memory.batch("later".into(), false);
        memory.save(&path).unwrap();
        let mut memory = Memory::load(&path, "session").unwrap();
        assert!(!memory.receipts[0].settled);
        assert_eq!(memory.status(true, false).state, "needs_attention");
        memory.receipt_response(AgentResponse::new(AgentOutcome::TimedOut), false);
        assert!(!memory.receipts[0].settled);
        memory.receipt_response(result, false);
        assert!(memory.receipts[0].settled);
        assert_eq!(memory.status(true, false).state, "queued");
        assert_eq!(memory.pending.len(), 1);
        assert_eq!(memory.pending[0].summary, vec!["later"]);
    }
}

#[test]
fn rename_preserves_memory_and_rolls_back_failed_session_rename() {
    let cache = tempfile::tempdir().unwrap();
    let source = memory_path(cache.path(), "old").unwrap();
    let destination = memory_path(cache.path(), "new").unwrap();
    let mut memory = Memory {
        revision: 12,
        checkpoint: json!({"assignment":"keep"}),
        cursor: Some(cursor("old", 10)),
        paused: true,
        ..Default::default()
    };
    memory.batch("pending".into(), false);
    memory.pending[0].attempted = true;
    memory.receipts.push_back(Receipt {
        batch: memory.pending[0].id.clone(),
        conversation: "conversation".into(),
        submission: "submission".into(),
        response: None,
        settled: false,
    });
    memory.save(&source).unwrap();
    let bytes = std::fs::read(&source).unwrap();
    assert!(rename_memory(cache.path(), "old", "new", || {
        assert!(!source.exists());
        assert_eq!(std::fs::read(&destination).unwrap(), bytes);
        assert!(writer_lock(&source).is_err());
        assert!(writer_lock(&destination).is_err());
        bail!("session checkpoint rename failed")
    })
    .is_err());
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
    assert!(!destination.exists());
    rename_memory(cache.path(), "old", "new", || Ok(())).unwrap();
    assert!(!source.exists());
    assert_eq!(std::fs::read(&destination).unwrap(), bytes);
    let restored = Memory::load(&destination, "new").unwrap();
    assert!(restored.paused);
    assert!(restored.pending[0].attempted);
    assert!(!restored.receipts[0].settled);
    assert_eq!(restored.revision, 12);
    assert!(restored.cursor.is_none());
}

#[test]
fn rename_refuses_existing_memory_and_active_writers() {
    let cache = tempfile::tempdir().unwrap();
    let source = memory_path(cache.path(), "old").unwrap();
    let destination = memory_path(cache.path(), "new").unwrap();
    Memory::default().save(&source).unwrap();
    let bytes = std::fs::read(&source).unwrap();
    Memory::default().save(&destination).unwrap();
    assert!(rename_memory(cache.path(), "old", "new", || panic!("must not rename")).is_err());
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
    assert_eq!(std::fs::read(&destination).unwrap(), bytes);
    std::fs::remove_file(&destination).unwrap();
    for path in [&source, &destination] {
        let _lock = writer_lock(path).unwrap();
        assert!(rename_memory(cache.path(), "old", "new", || panic!("must not rename")).is_err());
        assert_eq!(std::fs::read(&source).unwrap(), bytes);
        assert!(!destination.exists());
    }
    std::fs::remove_file(&source).unwrap();
    rename_memory(cache.path(), "old", "new", || Ok(())).unwrap();
    assert!(!destination.exists());
}

#[test]
fn private_activity_enriches_only_matching_conversations_without_waking() {
    let mut memory = Memory::default();
    memory.event(event(1, "terminal_2", "snapshot"), MOTHER);
    let mut observation = json!({"conversation_id":"conversation", "assignment":"private task", "current_activity":"read file", "waiting_on":"Allow reading the file?"});
    let row = memory.roster.get_mut("terminal_2").unwrap();
    assert!(enrich_row(row, &observation));
    assert_eq!(row["assignment"], "private task");
    assert_eq!(row["waiting_on"], "Allow reading the file?");
    merge_activity(
        &mut observation,
        &json!({"conversation_id":"conversation", "current_activity":"write file"}),
    );
    assert_eq!(observation["assignment"], "private task");
    assert!(enrich_row(row, &observation));
    assert_eq!(row["current_activity"], "write file");
    assert!(row["waiting_on"].is_null());
    assert!(!enrich_row(
        row,
        &json!({"conversation_id":"replacement", "assignment":"unrelated"})
    ));
    assert!(memory.pending.is_empty());
    memory.event(event(2, "terminal_2", "turn_completed"), MOTHER);
    assert_eq!(memory.roster["terminal_2"]["assignment"], "private task");
    assert!(memory
        .journal
        .iter()
        .all(|fact| fact["assignment"].is_null()));
}

#[test]
fn overview_and_journal_are_read_only_and_pages_survive_restart() {
    let mut memory = Memory::default();
    for sequence in 1..=130 {
        memory.event(event(sequence, "terminal_2", "turn_completed"), MOTHER);
    }
    let before = json!(memory);
    let directory = tempfile::tempdir().unwrap();
    // Saving to a directory fails, so these requests must never attempt persistence.
    for request in [
        MotherRequest::Status,
        MotherRequest::Overview,
        MotherRequest::Checkpoint {
            expect_rev: None,
            value: None,
        },
    ] {
        memory.persist_command(request, directory.path()).unwrap();
    }
    let first = memory
        .persist_command(
            MotherRequest::Journal {
                after: None,
                through: None,
                limit: None,
            },
            directory.path(),
        )
        .unwrap();
    assert!(memory
        .persist_command(
            MotherRequest::Checkpoint {
                expect_rev: Some(0),
                value: None,
            },
            directory.path()
        )
        .is_err());
    assert_eq!(json!(memory), before);
    assert_eq!(first["records"].as_array().unwrap().len(), 50);
    assert_eq!(first["next_cursor"], 50);
    let path = directory.path().join("memory.json");
    memory.save(&path).unwrap();
    let mut restored = Memory::load(&path, "session").unwrap();
    restored.event(event(131, "terminal_2", "turn_completed"), MOTHER);
    let page = restored.journal_page(Some(100), Some(130), None).unwrap();
    assert_eq!(page["records"].as_array().unwrap().len(), 30);
    assert!(page["next_cursor"].is_null());
    restored.journal.drain(..100);
    assert_eq!(
        restored.journal_page(Some(50), Some(130), None).unwrap()["retention_gap"],
        true
    );
    assert!(restored.journal_page(None, None, Some(201)).is_err());
}

#[test]
fn journal_payloads_are_shared_and_legacy_json_remains_compatible() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memory.json");
    let mut memory = Memory::default();
    memory.event(event(1, "terminal_2", "turn_completed"), MOTHER);
    let mut next = memory.clone();
    next.event(event(2, "terminal_2", "turn_completed"), MOTHER);
    assert!(Arc::ptr_eq(&memory.journal[0], &next.journal[0]));
    assert_eq!(memory.journal.len(), 1);
    assert_eq!(memory.journal_sequence, 1);
    next.save(&path).unwrap();
    let serialized: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(serialized["journal"][0], *memory.journal[0]);
    assert_eq!(json!(Memory::load(&path, "session").unwrap()), serialized);

    let mut legacy = serialized;
    legacy.as_object_mut().unwrap().remove("journal_sequence");
    for record in legacy["journal"].as_array_mut().unwrap() {
        record.as_object_mut().unwrap().remove("journal_cursor");
    }
    std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let migrated = Memory::load(&path, "session").unwrap();
    assert_eq!(migrated.journal_sequence, 2);
    assert_eq!(json!(migrated), json!(next));
}

#[test]
fn failed_command_persistence_leaves_committed_memory_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("memory.json");
    let mut memory = Memory::default();
    memory.event(event(1, "terminal_2", "turn_completed"), MOTHER);
    memory.save(&path).unwrap();
    let before = json!(memory);
    let record = memory.journal[0].clone();
    for request in [
        MotherRequest::Pause,
        MotherRequest::Inbox,
        MotherRequest::Ack {
            batch: memory.pending[0].id.clone(),
        },
        MotherRequest::Checkpoint {
            expect_rev: Some(0),
            value: Some(json!({"task":"new"})),
        },
        MotherRequest::Configure {
            panes: vec![],
            projects: vec![],
            kinds: vec!["codex".into()],
            lifecycle_types: vec![],
        },
    ] {
        assert!(memory.persist_command(request, directory.path()).is_err());
        assert_eq!(json!(memory), before);
        assert_eq!(json!(Memory::load(&path, "session").unwrap()), before);
        assert!(Arc::ptr_eq(&memory.journal[0], &record));
    }
    memory
        .persist_command(
            MotherRequest::Checkpoint {
                expect_rev: Some(0),
                value: Some(json!({"task":"saved"})),
            },
            &path,
        )
        .unwrap();
    assert_eq!(memory.revision, 1);
    assert_eq!(memory.checkpoint["task"], "saved");
    assert_eq!(
        json!(Memory::load(&path, "session").unwrap()),
        json!(memory)
    );
    assert!(Arc::ptr_eq(&memory.journal[0], &record));
}

#[test]
fn reviewed_unavailable_result_preserves_unknown_and_never_retries_original_batch() {
    let mut memory = Memory::default();
    memory.batch("original".into(), false);
    memory.pending[0].attempted = true;
    let mut response = AgentResponse::new(AgentOutcome::StatusUnavailable);
    response.submission_id = Some("submission".into());
    memory.receipts.push_back(Receipt {
        batch: memory.pending[0].id.clone(),
        conversation: "conversation".into(),
        submission: "submission".into(),
        response: Some(response),
        settled: false,
    });
    for (id, rev) in [("wrong", 0), ("submission", 1)] {
        assert!(memory
            .command(MotherRequest::Resolve {
                submission_id: id.into(),
                expect_rev: rev
            })
            .is_err());
    }
    memory
        .command(MotherRequest::Resolve {
            submission_id: "submission".into(),
            expect_rev: 0,
        })
        .unwrap();
    assert!(memory.receipts[0].settled);
    assert_eq!(
        memory.receipts[0].response.as_ref().unwrap().outcome,
        AgentOutcome::StatusUnavailable
    );
    assert!(memory.pending[0].attempted);
    assert!(!memory.pending[1].attempted);
    assert_eq!(memory.revision, 1);
    assert_eq!(memory.journal.back().unwrap()["outcome"], "unknown");
    assert!(memory
        .command(MotherRequest::Resolve {
            submission_id: "submission".into(),
            expect_rev: 1
        })
        .is_err());
}
