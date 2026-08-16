use super::*;

/// One round short of the cap the model still gets a request — but a
/// tool-free one, so the answer comes from the results already in hand
/// instead of the turn dead-ending on an error.
#[test]
fn the_last_round_asks_without_tools() {
    let mut state = ready_state();
    state.tool_rounds = state.max_tool_rounds() - 1;

    let frame = tool_call_frame("call_1", "list_session", "{}");
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);

    assert!(state.is_receiving(), "asked again");
    assert_eq!(state.tool_rounds, state.max_tool_rounds());
    // Last is the streaming placeholder; the notice sits just before it.
    let notice = &state.messages[state.messages.len() - 2];
    assert_eq!(notice.role, Role::Notice);
    assert!(notice.content.contains("tool budget spent"));
}

/// Past the cap the model called a tool it was no longer offered. Nothing
/// left to try, so the turn stops.
#[test]
fn a_model_that_calls_tools_it_was_not_offered_is_stopped() {
    let mut state = ready_state();
    state.tool_rounds = state.max_tool_rounds();

    let frame = tool_call_frame("call_1", "list_session", "{}");
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);

    assert!(matches!(state.status, Status::Idle), "no follow-up request");
    assert_eq!(state.messages.last().unwrap().role, Role::Error);
    assert!(state.messages.last().unwrap().content.contains("rounds"));
}

#[test]
fn a_new_question_resets_the_tool_budget() {
    let mut state = ready_state();
    settle(&mut state);
    state.tool_rounds = state.max_tool_rounds();
    state.input.insert_char('?');
    state.submit();

    assert_eq!(state.tool_rounds, 0);
}

#[test]
fn a_new_chat_clears_pending_tool_calls() {
    let mut state = ready_state();
    state.status = Status::ConfirmingTool;
    state.pending = vec![ToolCall {
        id: "a".into(),
        name: "close_pane".into(),
        arguments: "{}".into(),
    }]
    .into();
    state.tool_rounds = 3;
    state.new_chat();

    assert!(state.pending.is_empty());
    assert_eq!(state.tool_rounds, 0);
    assert!(matches!(state.status, Status::Idle));
}

#[test]
fn only_the_stateful_tools_are_handled_locally() {
    let mut state = idle_state();
    let call = ToolCall {
        id: "1".into(),
        name: "read_pane".into(),
        arguments: "{}".into(),
    };
    assert_eq!(state.run_local(&call), None, "the rest go to the host");
}
