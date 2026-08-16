use super::*;

// -- alongside a live reply ---------------------------------------------

/// The whole point of the split: a tool the user typed runs while the reply
/// is still arriving, and neither loses anything to the other. The arriving
/// text must keep landing in its own placeholder rather than in whatever
/// message happens to be last.
#[test]
fn a_typed_tool_runs_beside_an_arriving_reply() {
    let mut state = ready_state();
    state.on_chunk("hocus-1", &delta_frame("half a "), false, None);

    state.input.set_text("/list_session".to_owned());
    assert!(state.submit(), "the command is not refused mid-reply");
    assert_eq!(tool_results(&state).len(), 1, "and it ran");

    state.on_chunk("hocus-1", &delta_frame("sentence"), false, None);
    assert_eq!(
        state.messages[1].content, "half a sentence",
        "the reply is still whole"
    );
    assert!(state.is_receiving(), "and still arriving");
}

/// One tool at a time is still the rule — the queue's head owns the result
/// the host is about to deliver. Only the *stream* runs alongside.
#[test]
fn a_typed_tool_waits_for_the_one_already_running() {
    let mut state = ready_state();
    state.status = Status::AwaitingCommand;
    state.input.set_text("/list_session".to_owned());
    state.submit();

    assert!(tool_results(&state).is_empty(), "nothing ran");
    assert_eq!(state.messages.last().map(|m| m.role), Some(Role::Notice));
}

/// A typed tool was run for its output; the model's was run for context.
/// That is the difference the renderer keys on.
#[test]
fn a_typed_tools_result_is_marked_for_full_rendering() {
    let mut state = ready_state();
    state.input.set_text("/list_session".to_owned());
    state.submit();

    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(results[0].full_output);
    assert!(!state.manual_head, "consumed with the head it belonged to");

    let frame = tool_call_frame("call_1", "list_session", "{}");
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);

    let results = tool_results(&state);
    assert_eq!(results.len(), 2);
    assert!(!results[1].full_output, "the model's is still elided");
}

/// The model's calls land when its stream ends, which can be while a typed
/// one is still running. They queue behind it rather than replacing it, and
/// the answer still goes back once the queue drains.
#[test]
fn model_calls_queue_behind_a_typed_one_that_is_still_running() {
    let mut state = ready_state();
    state.pending.push_back(ToolCall {
        id: "manual-0".into(),
        name: "slack_read".into(),
        arguments: r##"{"channel":"#general"}"##.into(),
    });
    state.status = Status::AwaitingCommand;

    let frame = tool_call_frame("call_1", "list_session", "{}");
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);

    assert_eq!(state.pending.len(), 2, "queued, not swapped in");
    assert_eq!(
        state.pending[0].id, "manual-0",
        "the running one is still head"
    );
    assert!(state.owes_reply, "and the model is owed its answer");
}
