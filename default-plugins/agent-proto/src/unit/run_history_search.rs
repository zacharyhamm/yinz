use super::tests::{backend, entry, run, write};
use super::*;

#[test]
fn one_entry_matching_text_and_tool_payload_produces_one_hit() {
    let backend = backend();
    let mut run = run("both", "codex", "main", 1);
    let mut entry = entry(7, RunEntryKind::Message, "matching needle", 1);
    entry.tool = yinz_tile::agent::tool_capture::prepare(
        "codex",
        &yinz_tile::agent::HookEvent {
            hook_event_name: "PostToolUse".into(),
            tool_name: Some("exec".into()),
            tool_response: Some(serde_json::json!("tool needle")),
            ..Default::default()
        },
    );
    assert!(entry.tool.is_some());
    run.entries = vec![entry];
    write(&backend, &run);

    for (filter, id) in [
        (Filter::default(), None),
        (Filter::default(), Some(run.id.as_str())),
        (
            Filter {
                kind: Some("codex".into()),
                ..Filter::default()
            },
            None,
        ),
    ] {
        let found = search(&backend, &filter, id, "needle", false, 1).unwrap();
        assert_eq!(found.hits.len(), 1);
        assert_eq!(found.hits[0].run_id, run.id);
        assert_eq!(found.hits[0].revision, 7);
        assert!(!found.truncated);
    }
}
