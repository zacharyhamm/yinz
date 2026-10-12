use super::*;
use yinz_types::internal_ids::TerminalId;

const MOTHER: PaneId = PaneId::Terminal(TerminalId::new(1));

fn event(sequence: usize, pane: &str, kind: &str) -> Value {
    json!({"cursor":sequence.to_string(),"sequence":sequence,"pane_id":pane,"event_type":kind,
        "kind":"claude","conversation_id":"conversation","state":"idle","background_tasks":[]})
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
    let recovered = Memory::load(&path).unwrap();
    assert_eq!(recovered.revision, 1);
    assert_eq!(recovered.checkpoint["goals"], json!(["finish"]));
    assert!(recovered.paused);
    assert_eq!(recovered.pending[0].id, memory.pending[0].id);
    assert_eq!(recovered.cursor, memory.cursor);
    assert!(memory.save(directory.path()).is_err());
    assert_eq!(Memory::load(&path).unwrap().revision, 1);
}

#[test]
fn journal_and_pending_batches_are_bounded_with_explicit_gaps() {
    let mut memory = Memory {
        journal: (0..JOURNAL_RECORDS)
            .map(|sequence| event(sequence, "terminal_2", "turn_completed"))
            .collect(),
        ..Default::default()
    };
    memory.event(
        event(JOURNAL_RECORDS, "terminal_2", "turn_completed"),
        MOTHER,
    );
    assert!(memory.journal.len() <= JOURNAL_RECORDS);
    assert!(memory
        .journal
        .iter()
        .any(|v| v["reason"] == "mother_journal_overflow"));
    assert!(memory.gap);
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
fn incomplete_snapshot_restarts_from_a_snapshot_not_a_partial_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.json");
    let mut memory = Memory::default();
    memory.event(event(1, "", "snapshot_begin"), MOTHER);
    memory.event(event(1, "terminal_2", "snapshot"), MOTHER);
    memory.save(&path).unwrap();
    let mut restored = Memory::load(&path).unwrap();
    assert!(restored.cursor.is_none());
    assert!(restored.snapshotting);
    restored.event(event(1, "", "ready"), MOTHER);
    assert_eq!(restored.cursor.as_deref(), Some("1"));
}

#[test]
fn uncertain_delivery_never_requeues_an_unknown_submission() {
    let mut memory = Memory::default();
    memory.batch("work finished".into(), false);
    let batch = memory.pending[0].id.clone();
    memory.pending[0].attempted = true;
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
        let mut memory = Memory::load(&path).unwrap();
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
    let restored = Memory::load(&destination).unwrap();
    assert!(restored.paused);
    assert!(restored.pending[0].attempted);
    assert!(!restored.receipts[0].settled);
    assert_eq!(restored.revision, 12);
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
