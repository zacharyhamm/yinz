use super::*;

// -- the tool loop ------------------------------------------------------

#[test]
fn a_pane_closure_runs_without_confirmation() {
    let mut state = ready_state();
    let frame = tool_call_frame("call_1", "close_pane", r#"{"pane_id":1,"is_plugin":false}"#);
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);

    assert!(!matches!(state.status, Status::ConfirmingTool));
    assert_eq!(tool_results(&state).len(), 1);
    assert!(state.pending.is_empty());
}

/// Every tool that moves a pane between sessions stops for a y/n. The pane
/// leaves the screen, and a wrong target session puts it somewhere the user
/// is not looking.
#[test]
fn every_warp_stops_for_confirmation() {
    for (name, arguments) in [
        (
            "warp_pane",
            r#"{"pane_id":1,"is_plugin":false,"to_session":"other"}"#,
        ),
        ("warp_pane_here", r#"{"from_session":"other","pane_id":1}"#),
        ("warp_tab", r#"{"to_session":"other"}"#),
        ("warp_tab_here", r#"{"from_session":"other","tab_id":1}"#),
        ("adopt_orphaned_pane", r#"{"host_id":"abc123"}"#),
    ] {
        let mut state = ready_state();
        let frame = tool_call_frame("call_1", name, arguments);
        state.on_chunk("hocus-1", &frame, false, None);
        state.on_chunk("hocus-1", b"", true, None);

        assert!(
            matches!(state.status, Status::ConfirmingTool),
            "{name} should ask before running"
        );
        assert!(
            tool_results(&state).is_empty(),
            "{name} should not have run yet"
        );
    }
}

#[test]
fn listing_other_sessions_runs_without_asking() {
    for name in ["list_other_sessions", "list_orphaned_panes"] {
        let mut state = ready_state();
        let frame = tool_call_frame("call_1", name, "{}");
        state.on_chunk("hocus-1", &frame, false, None);
        state.on_chunk("hocus-1", b"", true, None);

        assert!(
            !matches!(state.status, Status::ConfirmingTool),
            "{name} only reads; it should not ask"
        );
        assert_eq!(tool_results(&state).len(), 1);
    }
}

#[test]
fn a_read_only_call_runs_without_asking() {
    let mut state = ready_state();
    let frame = tool_call_frame("call_1", "list_session", "{}");
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);

    assert!(!matches!(state.status, Status::ConfirmingTool));
    assert_eq!(tool_results(&state).len(), 1);
    assert_eq!(state.tool_rounds, 1);
}

/// Declining must still answer the call: the API rejects a transcript where
/// a requested call has no matching result.
#[test]
fn declining_records_a_result_rather_than_dropping_the_call() {
    let mut state = ready_state();
    let frame = tool_call_frame("call_1", "clear_pane", r#"{"pane_id":1,"is_plugin":false}"#);
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);
    state.decline_tool();

    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].tool_call_id.as_deref(), Some("call_1"));
    assert!(results[0].content.contains("declined"));
    assert!(state.pending.is_empty());
}

#[test]
fn confirming_runs_the_call_and_moves_on() {
    let mut state = ready_state();
    let frame = tool_call_frame("call_1", "clear_pane", r#"{"pane_id":1,"is_plugin":false}"#);
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);
    state.confirm_tool();

    assert_eq!(tool_results(&state).len(), 1);
    assert!(!state.messages[tool_result_index(&state)]
        .content
        .contains("declined"));
    assert!(state.pending.is_empty());
}

/// Aborting answers every outstanding call, so the transcript stays valid if
/// the conversation continues.
#[test]
fn aborting_answers_every_outstanding_call() {
    let mut state = ready_state();
    state.pending = vec![
        ToolCall {
            id: "a".into(),
            name: "close_pane".into(),
            arguments: "{}".into(),
        },
        ToolCall {
            id: "b".into(),
            name: "close_tab".into(),
            arguments: "{}".into(),
        },
    ]
    .into();
    state.status = Status::ConfirmingTool;
    state.abort_tools();

    let results = tool_results(&state);
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].tool_call_id.as_deref(), Some("a"));
    assert_eq!(results[1].tool_call_id.as_deref(), Some("b"));
    assert!(matches!(state.status, Status::Idle));
}
