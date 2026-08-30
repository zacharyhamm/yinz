use super::*;

/// The cap is checked only after the last batch's results are recorded, and it
/// does not buy one more large request in the hope that the model stops itself.
#[test]
fn the_last_round_records_its_results_and_stops() {
    let mut state = ready_state();
    state.tool_rounds = state.max_tool_rounds() - 1;

    let frame = tool_call_frame("call_1", "list_session", "{}");
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);

    assert!(!state.is_receiving(), "no follow-up request");
    assert_eq!(state.tool_rounds, state.max_tool_rounds());
    assert_eq!(tool_results(&state).len(), 1, "the last result was kept");
    assert_eq!(state.messages.last().unwrap().role, Role::Error);
    assert!(state
        .messages
        .last()
        .unwrap()
        .content
        .contains("type continue"));
}

/// The whole of the DeepSeek bug, end to end: a thinking model streams its
/// chain-of-thought and then the tool call it decided on. The placeholder that
/// caught the thinking is thrown away when the tool-call message replaces it,
/// and if the thinking goes with it the *next* request omits
/// `reasoning_content` and DeepSeek answers 400.
#[test]
fn thinking_that_preceded_a_tool_call_survives_onto_the_message_and_the_wire() {
    let mut state = ready_state();
    let thinking = br#"data: {"choices":[{"delta":{"reasoning_content":"htop it is","content":"I'll inspect it."}}]}
"#;
    state.on_chunk("hocus-1", thinking, false, None);
    ui::render(&mut state, 24, 80);
    assert!(state.transcript_cache.is_some());
    state.save_now(true);
    let chat_id = state.chat_id.expect("the partial reply was archived");

    let frame = tool_call_frame("call_1", "clear_pane", r#"{"pane_id":1,"is_plugin":false}"#);
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);

    // The swap rewrites the last message in place without moving the count, so
    // the render memo has to have been told about it by hand: the cache now
    // covers less than the whole transcript.
    assert!(
        state
            .transcript_cache
            .as_ref()
            .is_none_or(|cache| cache.len() < state.messages.len()),
        "the prose cache is stale"
    );
    assert_ne!(state.saved, state.fingerprint(), "the archive row is dirty");
    state.status = Status::Idle;
    state.autosave();

    let asked = state
        .messages
        .iter()
        .find(|message| !message.tool_calls.is_empty())
        .expect("the turn asked for a tool");
    assert_eq!(
        asked.reasoning,
        Some(Reasoning::new("htop it is", ReasoningKey::ReasoningContent))
    );
    assert_eq!(asked.content, "I'll inspect it.");
    assert!(
        asked.display.as_deref().is_some_and(|display| {
            display.contains("I'll inspect it.") && display.contains("clear_pane")
        }),
        "the UI keeps both the prose and the call summary"
    );
    let archived = history::load(&state.private_store(), chat_id).expect("the chat was archived");
    assert_eq!(archived[1], *asked, "autosave rewrote the partial row");

    let cfg = state.cfg.as_ref().expect("configured");
    let body = String::from_utf8(api::request_body_with_project(
        cfg,
        &state.messages,
        None,
        false,
    ))
    .unwrap();
    assert!(
        body.contains(r#""reasoning_content":"htop it is""#),
        "the next round has to hand the thinking back or the endpoint refuses \
         it: {body}"
    );
    assert!(
        body.contains(r#""content":"I'll inspect it.""#),
        "DeepSeek requires the response content on the same assistant message: {body}"
    );
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
