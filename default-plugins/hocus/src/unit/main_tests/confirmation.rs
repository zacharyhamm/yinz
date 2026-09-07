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

/// Every tool that pulls something into this session runs without asking: what
/// arrives lands in front of whoever asked for it, so the confirmation would be
/// a prompt about a thing they are about to see anyway. There is no tool for the
/// other direction — a push to a session nobody is watching — at all. The cost
/// is borne by the source, which ends if it gave away its last tab; that is a
/// choice, not an oversight.
#[test]
fn pulling_something_into_this_session_runs_without_asking() {
    for (name, arguments) in [
        ("warp_pane_here", r#"{"from_session":"other","pane_id":1}"#),
        ("warp_tab_here", r#"{"from_session":"other","tab_id":1}"#),
        ("adopt_orphaned_pane", r#"{"host_id":"abc123"}"#),
    ] {
        let mut state = ready_state();
        let frame = tool_call_frame("call_1", name, arguments);
        state.on_chunk("hocus-1", &frame, false, None);
        state.on_chunk("hocus-1", b"", true, None);

        assert!(
            !matches!(state.status, Status::ConfirmingTool),
            "{name} should not ask before running"
        );
        assert_eq!(
            tool_results(&state).len(),
            1,
            "{name} should have run and reported back"
        );
        assert!(
            state.pending.is_empty(),
            "{name} should leave nothing pending"
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
    assert!(!state.is_receiving(), "and no follow-up request went out");
    assert!(matches!(state.status, Status::Idle));
    assert!(
        !state.owes_reply,
        "the model is not owed a reply until the user speaks"
    );
}

/// `n` takes the turn back. The declined result is recorded, but it is not
/// handed to the model to try something else with: the next request is the
/// user's next message, and the result rides along in front of it.
#[test]
fn declining_halts_until_the_user_speaks() {
    let mut state = ready_state();
    let frame = tool_call_frame("call_1", "clear_pane", r#"{"pane_id":1,"is_plugin":false}"#);
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);
    state.decline_tool();

    assert!(!state.is_receiving(), "nothing was sent on the decline");
    assert!(
        state.messages.last().unwrap().role == Role::Notice,
        "the pane says it is waiting"
    );

    state.input.set_text("do something else instead".to_owned());
    state.submit();

    assert!(state.is_receiving(), "the user's message is what goes out");
    let result_at = tool_result_index(&state);
    let user_at = state
        .messages
        .iter()
        .rposition(|message| message.role == Role::User)
        .unwrap();
    assert!(
        result_at < user_at,
        "the declined result precedes the new message"
    );
    assert_eq!(state.messages[user_at].content, "do something else instead");
}

/// A user who said no to one call has not agreed to the ones queued behind it.
/// They are answered — the transcript needs a result per call — but not run.
#[test]
fn declining_one_call_answers_the_rest_of_the_batch_without_running_it() {
    let calls: Vec<ToolCall> = [("call_1", "clear_pane", 1), ("call_2", "close_pane", 2)]
        .into_iter()
        .map(|(id, name, pane)| ToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments: format!(r#"{{"pane_id":{pane},"is_plugin":false}}"#),
        })
        .collect();
    let mut state = State {
        stream: None,
        reply_at: None,
        status: Status::ConfirmingTool,
        owes_reply: true,
        ..ready_state()
    };
    state.messages.push(Message::tool_calls(calls.clone()));
    state.pending.extend(calls);

    state.decline_tool();

    let results = tool_results(&state);
    assert_eq!(results.len(), 2, "every call the model made was answered");
    assert_eq!(results[0].content, "declined by the user");
    assert!(
        results[1].content.starts_with("not run"),
        "the second call was answered rather than run: {}",
        results[1].content
    );
    assert!(state.pending.is_empty());
    assert!(matches!(state.status, Status::Idle));
    assert!(!state.is_receiving(), "no follow-up request");
}

/// Something typed while the prompt was up is the user's next message already,
/// so the decline is what sends it — it does not wait on a timer.
#[test]
fn a_question_queued_behind_the_prompt_goes_out_on_the_decline() {
    let mut state = ready_state();
    let frame = tool_call_frame("call_1", "clear_pane", r#"{"pane_id":1,"is_plugin":false}"#);
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);
    state.input.set_text("actually, leave that pane".to_owned());
    state.submit();
    assert_eq!(state.asks.len(), 1, "typed at the prompt, so queued");

    state.decline_tool();

    assert!(state.asks.is_empty(), "the queued question was taken");
    assert!(state.is_receiving(), "and it is the request in flight");
    let last_user = state
        .messages
        .iter()
        .rfind(|message| message.role == Role::User)
        .unwrap();
    assert_eq!(last_user.content, "actually, leave that pane");
    assert!(
        !state
            .messages
            .iter()
            .any(|message| message.content.starts_with("declined — waiting")),
        "and hocus does not claim to be waiting while that question is in flight",
    );
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
    assert_eq!(
        state.spotlight,
        Some(PaneId::Terminal(zellij_tile::prelude::TerminalId::new(7)))
    );
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
            Some(PaneId::Terminal(zellij_tile::prelude::TerminalId::new(3))),
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
    assert_eq!(
        parked.spotlight,
        Some(PaneId::Terminal(zellij_tile::prelude::TerminalId::new(5)))
    );

    state.put_chat(parked);

    assert_eq!(
        state.spotlight,
        Some(PaneId::Terminal(zellij_tile::prelude::TerminalId::new(5)))
    );
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

/// The toast is drawn over whatever pane the user is in, so the click on it is the only gesture
/// that can say "the thing this was about, now". It carries hocus's own pane - which is what the
/// host focuses - and the chat that asked, which the focus alone cannot express.
#[test]
fn the_confirmation_toast_is_clickable_and_names_the_chat_that_asked() {
    tools::test_seams::reset();
    let mut state = ready_state();
    state.own_plugin_id = zellij_tile::prelude::PluginId::new(7);
    state.chat_id = Some(42);

    let frame = tool_call_frame(
        "call_1",
        "new_remote_pane",
        r#"{"remote_host":"box","command":"htop"}"#,
    );
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);
    assert!(matches!(state.status, Status::ConfirmingTool));

    let toasts = tools::test_seams::TOASTS.with_borrow(Clone::clone);
    let (_, pane, chat_id) = toasts.first().expect("the ask went up as a toast");
    assert_eq!(
        *pane,
        PaneId::Plugin(zellij_tile::prelude::PluginId::new(7)),
        "the click brings hocus itself back"
    );
    assert_eq!(*chat_id, Some(42), "onto the chat that asked");
}
