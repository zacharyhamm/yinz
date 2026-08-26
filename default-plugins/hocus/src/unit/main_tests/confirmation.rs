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

/// The gate reads the arguments, not just the name: the same tool stops or does
/// not depending on whether the model named a command to run over there.
#[test]
fn a_remote_pane_stops_for_a_command_but_not_for_a_login_shell() {
    let mut state = ready_state();
    let frame = tool_call_frame(
        "call_1",
        "new_remote_pane",
        r#"{"remote_host":"box","command":"htop"}"#,
    );
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);
    assert!(
        matches!(state.status, Status::ConfirmingTool),
        "running a command on another machine should ask first"
    );

    let mut state = ready_state();
    let frame = tool_call_frame("call_1", "new_remote_pane", r#"{"remote_host":"box"}"#);
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);
    assert!(
        !matches!(state.status, Status::ConfirmingTool),
        "a bare login shell should not ask, any more than new_pane does"
    );
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

// -- the spotlight ------------------------------------------------------
//
// The host calls are no-ops off wasm, so what these pin is the bookkeeping:
// which pane was singled out, and that it stops being singled out on every way
// out of the prompt. A pane left wearing `hocus: send_input?` after the answer
// was given is a lie about what the session is waiting for.

/// The prompt says "types into pane 7"; `spotlight` is how the session then goes
/// and shows you pane 7.
#[test]
fn confirming_a_pane_tool_singles_out_that_pane() {
    let mut state = ready_state();
    let frame = tool_call_frame("call_1", "send_input", r#"{"pane_id":7,"is_plugin":false}"#);
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);

    assert!(matches!(state.status, Status::ConfirmingTool));
    assert_eq!(state.spotlight, Some(PaneId::Terminal(7)));
}

/// A confirmation with nothing to point at is still a confirmation. `slack_post`
/// asks because it is public, not because it touches this session.
#[test]
fn a_confirmation_about_no_pane_singles_out_nothing() {
    let mut state = ready_state();
    let frame = tool_call_frame("call_1", "slack_post", r#"{"channel":"general"}"#);
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);

    assert!(matches!(state.status, Status::ConfirmingTool));
    assert_eq!(state.spotlight, None);
}

/// Every way out. Written as a table rather than three tests because the point
/// is that *none* of them leaves the mark up, and a fourth exit added later
/// should be added here.
#[test]
fn answering_takes_the_mark_back_off() {
    for (answer, resolve) in [
        ("y", State::confirm_tool as fn(&mut State) -> bool),
        ("n", State::decline_tool),
        ("Ctrl-c", State::abort_tools),
        ("Ctrl-l", State::new_chat),
    ] {
        let mut state = ready_state();
        let frame = tool_call_frame("call_1", "clear_pane", r#"{"pane_id":3,"is_plugin":false}"#);
        state.on_chunk("hocus-1", &frame, false, None);
        state.on_chunk("hocus-1", b"", true, None);
        assert_eq!(
            state.spotlight,
            Some(PaneId::Terminal(3)),
            "{answer}: the mark goes up to begin with"
        );

        resolve(&mut state);

        assert_eq!(state.spotlight, None, "{answer} should clear the mark");
    }
}

/// A chat parked mid-confirmation is still waiting on that pane, and is the one
/// that will have to clear it. Leaving the mark behind on the foreground chat
/// would have the wrong conversation take it down.
#[test]
fn the_mark_travels_with_a_parked_chat() {
    let mut state = ready_state();
    let frame = tool_call_frame("call_1", "clear_pane", r#"{"pane_id":5,"is_plugin":false}"#);
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);

    let parked = state.take_chat();

    assert_eq!(state.spotlight, None, "the foreground keeps nothing");
    assert_eq!(parked.spotlight, Some(PaneId::Terminal(5)));

    state.put_chat(parked);

    assert_eq!(state.spotlight, Some(PaneId::Terminal(5)));
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
