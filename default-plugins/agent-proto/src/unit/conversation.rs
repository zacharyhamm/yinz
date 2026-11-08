use super::*;
use crate::store::{self, shared_defs};
fn backend() -> store::Local {
    let b = store::Local::shared("conversation");
    b.define_all(&shared_defs()).unwrap();
    b
}
fn entry(
    revision: u64,
    kind: RunEntryKind,
    at: Option<i64>,
    worker: Option<&str>,
    turn: Option<&str>,
    text: &str,
) -> RunEntry {
    RunEntry {
        revision,
        kind,
        created_at_ms: at,
        worker_id: worker.map(str::to_owned),
        conversation_turn_id: turn.map(str::to_owned),
        text: text.into(),
        event_id: None,
        tool: None,
    }
}
fn save(b: &store::Local, entries: &[RunEntry]) {
    b.apply(
        entries
            .iter()
            .map(|e| rows::entry_mutation("run", &"host".into(), e))
            .collect(),
    )
    .unwrap();
}
#[test]
fn conversation_selects_prose_before_tool_budgets_and_attributes_workers() {
    let b = backend();
    let mut entries = vec![entry(
        1,
        RunEntryKind::Message,
        Some(1),
        None,
        Some("one"),
        "parent reply",
    )];
    entries.extend((2..=2500).map(|r| {
        entry(
            r,
            RunEntryKind::ToolCall,
            Some(r as i64),
            Some("worker"),
            None,
            "tool",
        )
    }));
    entries.push(entry(
        2501,
        RunEntryKind::Message,
        Some(3),
        Some("worker"),
        Some("worker-turn"),
        "worker prose",
    ));
    for chunk in entries.chunks(100) {
        save(&b, chunk);
    }
    let parent = page(&b, "run", None, None, None).unwrap();
    assert_eq!(parent.entries.len(), 1);
    assert_eq!(parent.entries[0].text, "parent reply");
    assert!(parent.earlier.is_none());
    assert_eq!(parent.latest_received_revision, 2501);
    let worker = page(&b, "run", Some("worker"), None, None).unwrap();
    assert_eq!(worker.entries[0].text, "worker prose");
    assert_eq!(workers(&b, "run").unwrap(), ["worker"]);
}
#[test]
fn conversation_pages_by_timestamp_and_revision_while_delivery_uses_revision() {
    let b = backend();
    let entries: Vec<_> = (1..=65)
        .map(|r| {
            entry(
                r,
                RunEntryKind::Message,
                Some(r as i64),
                None,
                None,
                &r.to_string(),
            )
        })
        .collect();
    save(&b, &entries);
    let newest = page(&b, "run", None, None, None).unwrap();
    assert_eq!(newest.entries[0].revision, 36);
    let earlier = page(&b, "run", None, newest.earlier, None).unwrap();
    assert_eq!(earlier.entries[0].revision, 6);
    assert_eq!(earlier.entries.last().unwrap().revision, 35);
    save(
        &b,
        &[entry(
            66,
            RunEntryKind::Message,
            Some(2),
            None,
            None,
            "recovered",
        )],
    );
    let delivered = page(&b, "run", None, None, Some(66)).unwrap();
    assert_eq!(delivered.entries[0].text, "recovered");
    assert_eq!(delivered.latest_received_revision, 66);
    let older = page(&b, "run", None, earlier.earlier, None).unwrap();
    assert_eq!(
        older.entries.iter().map(|e| e.revision).collect::<Vec<_>>(),
        [1, 2, 66, 3, 4, 5]
    );
}
#[test]
fn completion_alternatives_are_suppressed_only_within_the_same_turn() {
    let mut entries = vec![
        entry(1, RunEntryKind::Message, Some(1), None, Some("one"), "same"),
        entry(2, RunEntryKind::Result, Some(2), None, Some("one"), "same"),
        entry(3, RunEntryKind::Result, Some(3), None, Some("two"), "same"),
        entry(
            4,
            RunEntryKind::Result,
            Some(4),
            Some("worker"),
            Some("one"),
            "same",
        ),
    ];
    suppress_completions(&mut entries);
    assert_eq!(
        entries.iter().map(|e| e.revision).collect::<Vec<_>>(),
        [1, 3, 4]
    );
}
#[test]
fn legacy_timestamps_and_prefixed_worker_messages_remain_readable() {
    let b = backend();
    save(
        &b,
        &[
            entry(1, RunEntryKind::Message, None, None, None, "legacy"),
            entry(
                2,
                RunEntryKind::Message,
                None,
                None,
                None,
                "Subagent worker:\nworker reply",
            ),
            entry(3, RunEntryKind::Message, Some(5), None, None, "current"),
        ],
    );
    let p = page(&b, "run", None, None, None).unwrap();
    assert_eq!(
        p.entries.iter().map(|e| e.revision).collect::<Vec<_>>(),
        [1, 3]
    );
    assert_eq!(workers(&b, "run").unwrap(), ["worker"]);
}
#[test]
fn expanded_tools_obey_row_and_byte_budgets_and_advance_past_large_records() {
    let b = backend();
    let entries: Vec<_> = (1..=65)
        .map(|r| {
            entry(
                r,
                RunEntryKind::ToolCall,
                Some(r as i64),
                None,
                None,
                &"x".repeat(if r == 65 { 100_000 } else { 1000 }),
            )
        })
        .collect();
    save(&b, &entries);
    let range = Range {
        start: None,
        end: None,
        worker: None,
    };
    let mut before = None;
    let mut seen = Vec::new();
    loop {
        let p = tools(&b, "run", &range, before).unwrap();
        assert!(p.entries.len() <= 30);
        assert!(
            serde_json::to_vec(&p.entries).unwrap().len() <= crate::run_history::PAGE_TEXT_BUDGET
        );
        assert!(!p.entries.is_empty());
        seen.extend(p.entries.iter().map(|e| e.revision));
        before = p.earlier;
        if before.is_none() {
            break;
        }
    }
    seen.sort();
    assert_eq!(seen, (1..=65).collect::<Vec<_>>());
}

#[test]
fn repair_skips_legacy_worker_completion_but_preserves_identical_prose_in_another_turn() {
    use yinz_types::agent::CapturedMessage;
    let mut known = entry(
        5,
        RunEntryKind::Message,
        Some(30),
        None,
        None,
        "Subagent worker:\nanswer",
    );
    known.event_id = Some("subagent:[\"session\",\"\",\"worker\"]".into());
    let messages = vec![
        CapturedMessage {
            source_id: "source-one".into(),
            turn_id: "one".into(),
            text: "answer".into(),
            worker_id: Some("worker".into()),
            created_at_ms: Some(10),
            incomplete: false,
        },
        CapturedMessage {
            source_id: "source-two".into(),
            turn_id: "two".into(),
            text: "answer".into(),
            worker_id: Some("worker".into()),
            created_at_ms: Some(20),
            incomplete: false,
        },
    ];
    let missing = missing_messages(messages, &[known]);
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].turn_id, "one");
}

fn recovered(
    source: &str,
    worker: Option<&str>,
    turn: &str,
    text: &str,
) -> yinz_types::agent::CapturedMessage {
    yinz_types::agent::CapturedMessage {
        source_id: source.into(),
        worker_id: worker.map(str::to_owned),
        turn_id: turn.into(),
        text: text.into(),
        created_at_ms: Some(10),
        incomplete: false,
    }
}

#[test]
fn repair_preserves_distinct_sources_with_identical_text() {
    for worker in [None, Some("worker")] {
        let messages = vec![
            recovered("one", worker, "turn", "answer"),
            recovered("two", worker, "turn", "answer"),
        ];
        assert_eq!(missing_messages(messages.clone(), &[]), messages);
        let mut known = entry(
            1,
            RunEntryKind::Message,
            Some(10),
            worker,
            Some("turn"),
            "answer",
        );
        // Prose without a source identity cannot stand in for either source.
        assert_eq!(
            missing_messages(messages.clone(), &[known.clone()]),
            messages
        );
        known.event_id = Some(format!("message:{}", serde_json::json!(["turn", "one"])));
        assert_eq!(
            missing_messages(messages.clone(), &[known.clone()]),
            messages[1..]
        );
        // Identity remains authoritative even if the source's text changes.
        known.text = "different text".into();
        assert_eq!(
            missing_messages(messages.clone(), &[known.clone()]),
            messages[1..]
        );
        let mut second = known.clone();
        second.event_id = Some(format!("message:{}", serde_json::json!(["turn", "two"])));
        assert!(missing_messages(messages, &[known, second]).is_empty());
    }
}

#[test]
fn repair_matches_stable_sources_after_the_prompt_leaves_the_transcript_tail() {
    for worker in [None, Some("worker")] {
        let mut known = entry(
            1,
            RunEntryKind::Message,
            Some(10),
            worker,
            Some("prompt"),
            "answer",
        );
        known.event_id = Some(format!(
            "message:{}",
            serde_json::json!(["prompt", "reply"])
        ));
        let archived = [known];
        let original = archived.clone();
        let mut message = recovered("reply", worker, "source:reply", "answer");
        message.incomplete = true;
        assert!(missing_messages(vec![message.clone()], &archived).is_empty());
        // Source identity also wins if a later recovery restores the prompt.
        let mut fallback = archived[0].clone();
        fallback.conversation_turn_id = Some("source:reply".into());
        fallback.event_id = Some(format!(
            "message:{}",
            serde_json::json!(["source:reply", "reply"])
        ));
        message.turn_id = "prompt".into();
        assert!(missing_messages(vec![message], &[fallback]).is_empty());
        assert_eq!(archived, original);
    }
}

#[test]
fn repair_source_identity_requires_matching_worker_attribution() {
    let mut known = entry(
        1,
        RunEntryKind::Message,
        Some(10),
        Some("worker"),
        Some("turn"),
        "answer",
    );
    known.event_id = Some("message:[\"turn\",\"source\"]".into());
    for worker in [None, Some("other")] {
        for turn in ["turn", "source:source"] {
            let message = recovered("source", worker, turn, "answer");
            assert_eq!(
                missing_messages(vec![message.clone()], &[known.clone()]),
                [message]
            );
        }
    }
}

#[test]
fn repair_ignores_malformed_or_unrelated_source_identities() {
    let mut known = entry(1, RunEntryKind::Message, Some(10), None, None, "answer");
    let message = recovered("source", None, "source:source", "answer");
    for identity in [
        "message:invalid",
        "message:[\"turn\"]",
        "message:[\"turn\",\"source\",\"extra\"]",
        "message:[null,\"source\"]",
        "message:[\"turn\",42]",
        "message:[\"turn\",\"source\"] trailing",
        "subagent:[\"turn\",\"source\"]",
    ] {
        known.event_id = Some(identity.into());
        assert_eq!(
            missing_messages(vec![message.clone()], &[known.clone()]),
            [message.clone()],
            "{identity}"
        );
    }
}

#[test]
fn repair_matches_result_text_only_within_the_same_worker_and_turn() {
    let message = recovered("source", Some("worker"), "turn", "answer");
    for (worker, turn, expected) in [
        (Some("worker"), Some("turn"), 0),
        (Some("other"), Some("turn"), 1),
        (None, Some("turn"), 1),
        (Some("worker"), Some("other"), 1),
        (Some("worker"), None, 1),
    ] {
        let known = entry(1, RunEntryKind::Result, Some(20), worker, turn, " answer\n");
        assert_eq!(
            missing_messages(vec![message.clone()], &[known]).len(),
            expected
        );
    }
}

#[test]
fn repair_worker_fallback_accounts_only_for_that_workers_final_source() {
    for turn in [None, Some("turn")] {
        for final_text in ["answer", "different"] {
            let mut known = entry(
                1,
                RunEntryKind::Message,
                Some(20),
                Some("worker"),
                turn,
                "answer",
            );
            known.event_id = Some("subagent:[\"session\",\"\",\"worker\"]".into());
            let messages = vec![
                recovered("earlier", Some("worker"), "turn", "answer"),
                recovered("final", Some("worker"), "turn", final_text),
                recovered("other-final", Some("other"), "turn", "answer"),
            ];
            let expected = if final_text == "answer" {
                vec![messages[0].clone(), messages[2].clone()]
            } else {
                messages.clone()
            };
            assert_eq!(missing_messages(messages, &[known]), expected);
        }
    }
}

#[test]
fn repair_legacy_worker_fallback_requires_a_source_timestamp_no_later_than_the_fallback() {
    let mut known = entry(
        1,
        RunEntryKind::Message,
        Some(20),
        Some("worker"),
        None,
        "answer",
    );
    known.event_id = Some("subagent:[\"session\",\"\",\"worker\"]".into());
    for (recorded, source, expected) in [
        (Some(20), Some(10), 0),
        (Some(20), Some(20), 0),
        (Some(20), Some(30), 1),
        (None, Some(10), 1),
        (Some(20), None, 1),
    ] {
        known.created_at_ms = recorded;
        let mut message = recovered("final", Some("worker"), "turn", "answer");
        message.created_at_ms = source;
        assert_eq!(
            missing_messages(vec![message], &[known.clone()]).len(),
            expected
        );
    }
}

#[test]
fn many_recovered_messages_do_not_advance_the_delivery_watermark_past_an_unread_revision() {
    let b = backend();
    let entries: Vec<_> = (1..=65)
        .map(|r| {
            entry(
                r,
                RunEntryKind::Message,
                Some((66 - r) as i64),
                None,
                None,
                &r.to_string(),
            )
        })
        .collect();
    save(&b, &entries);
    let mut next = 1;
    let mut seen = Vec::new();
    while next <= 65 {
        let p = page(&b, "run", None, None, Some(next)).unwrap();
        assert!(p.entries.len() <= 30);
        seen.extend(p.entries.iter().map(|e| e.revision));
        assert!(p.latest_received_revision >= next);
        next = p.latest_received_revision + 1;
    }
    seen.sort();
    assert_eq!(seen, (1..=65).collect::<Vec<_>>());
}

#[test]
fn v4_archive_upgrade_adds_nullable_attribution_without_changing_legacy_rows() {
    let b = store::Local::shared("upgrade");
    let mut old = store::agent_run_entry_def();
    old.version = 4;
    old.attrs
        .retain(|attr| !matches!(attr.name.as_str(), "worker_id" | "conversation_turn_id"));
    b.define(&old).unwrap();
    save(
        &b,
        &[entry(
            1,
            RunEntryKind::Message,
            None,
            None,
            None,
            "legacy text",
        )],
    );
    // Write v4 rows directly: the current entry writer fills scalar attribution.
    for revision in 2..=102 {
        let hook = yinz_tile::agent::HookEvent {
            hook_event_name: "PostToolUse".into(),
            tool_name: Some("shell".into()),
            agent_id: Some(format!("worker-{revision:03}")),
            ..Default::default()
        };
        let capture = yinz_tile::agent::tool_capture::prepare("claude", &hook).unwrap();
        b.apply(vec![yinz_tile::prelude::Mutation::put(rows::ENTRY_ENTITY)
            .uid(format!("legacy-tool-{revision:03}"))
            .set("run_id", "run")
            .set("host_id", "host")
            .set("revision", revision as i64)
            .set("kind", "tool_result")
            .set("text", "tool")
            .set("tool", serde_json::to_string(&capture).unwrap())
            .build()])
            .unwrap();
    }
    b.define(&store::agent_run_entry_def()).unwrap();
    // The same worker can appear in both legacy and current attribution.
    save(
        &b,
        &[entry(
            103,
            RunEntryKind::ToolCall,
            Some(10),
            Some("worker-002"),
            None,
            "current tool",
        )],
    );
    assert_eq!(
        workers(&b, "run").unwrap(),
        (2..=102)
            .map(|revision| format!("worker-{revision:03}"))
            .collect::<Vec<_>>()
    );
    let range = Range {
        start: None,
        end: None,
        worker: Some("worker-102".into()),
    };
    let worker_tools = tools(&b, "run", &range, None).unwrap();
    assert_eq!(worker_tools.entries.len(), 1);
    let legacy_tool = &worker_tools.entries[0];
    assert_eq!(legacy_tool.revision, 102);
    assert!(legacy_tool.worker_id.is_none());
    assert_eq!(
        legacy_tool.tool.as_ref().unwrap().worker_id.as_deref(),
        Some("worker-102")
    );
    assert!(tools(
        &b,
        "run",
        &Range {
            worker: None,
            ..range
        },
        None
    )
    .unwrap()
    .entries
    .is_empty());
    let p = page(&b, "run", None, None, None).unwrap();
    assert_eq!(p.entries.len(), 1);
    assert_eq!(p.entries[0].text, "legacy text");
    assert!(p.entries[0].worker_id.is_none());
    assert!(p.entries[0].conversation_turn_id.is_none());
}
