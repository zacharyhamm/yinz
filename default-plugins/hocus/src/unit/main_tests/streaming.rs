use super::*;

// -- the waiting indicator ----------------------------------------------

#[test]
fn waiting_on_a_reply_shows_the_loader() {
    let state = ready_state();
    assert!(state.is_loading());
    assert!(state.loader().is_some());
}

#[test]
fn the_loader_clears_the_moment_a_token_arrives() {
    let mut state = ready_state();
    state.on_chunk(
        "hocus-1",
        b"data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n",
        false,
        None,
    );

    assert!(!state.is_loading(), "text is its own proof of life");
    assert!(state.loader().is_none());
}

/// The prompt is waiting on the user, not on us — a spinner there would be
/// claiming we're busy while we sit doing nothing.
#[test]
fn a_confirmation_prompt_shows_no_loader() {
    let mut state = ready_state();
    let frame = tool_call_frame("call_1", "clear_pane", r#"{"pane_id":1,"is_plugin":false}"#);
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);

    assert!(matches!(state.status, Status::ConfirmingTool));
    assert!(!state.is_loading());
}

#[test]
fn an_idle_pane_shows_no_loader() {
    assert!(!State::default().is_loading());
    assert!(State::default().loader().is_none());
}

#[test]
fn a_running_command_is_named_rather_than_given_a_phrase() {
    let mut state = ready_state();
    state.status = Status::AwaitingCommand;
    state.pending = vec![ToolCall {
        id: "a".into(),
        name: "slack_backfill".into(),
        arguments: "{}".into(),
    }]
    .into();

    let loader = state.loader().expect("a loader while awaiting a command");
    assert!(loader.contains("running: slack_backfill"), "{loader}");
}

/// Unparseable arguments must not lose the indicator entirely.
#[test]
fn a_search_we_cannot_parse_falls_back_to_a_phrase() {
    let mut state = ready_state();
    state.status = Status::AwaitingSearch;
    state.pending = vec![ToolCall {
        id: "a".into(),
        name: "web_search".into(),
        arguments: "{not json".into(),
    }]
    .into();

    let loader = state.loader().expect("still a loader");
    assert!(!loader.contains("searching:"), "{loader}");
}

/// With no job, store write, or animation, there is no timer to keep alive.
#[test]
fn an_idle_timer_tick_stops_without_repainting() {
    let mut state = ready_state();
    settle(&mut state);
    state.timer_armed = true;

    assert!(!state.on_timer(FRAME_SECONDS), "no redraw");
    assert!(!state.timer_armed, "no agent poll keeps the chain alive");
    assert_eq!(state.tick, 0, "the spinner does not advance either");
}

#[test]
fn the_timer_chain_continues_while_waiting() {
    let mut state = ready_state();
    state.timer_armed = true;

    assert!(state.on_timer(FRAME_SECONDS));
    assert!(state.timer_armed, "rearmed for the next frame");
    assert_eq!(state.tick, 1);
}

/// A cancelled stream must not also execute whatever tools it had half
/// emitted before the user hit Esc.
#[test]
fn tool_calls_from_a_cancelled_stream_are_discarded() {
    let mut state = ready_state();
    let frame = tool_call_frame("call_1", "close_pane", r#"{"pane_id":1,"is_plugin":false}"#);
    state.on_chunk("hocus-1", &frame, false, None);
    state.cancel_stream();
    state.on_chunk("hocus-1", b"", true, Some("cancelled".into()));

    assert!(state.pending.is_empty());
    assert!(matches!(state.status, Status::Idle));
}

#[test]
fn a_partial_reply_survives_a_mid_stream_error() {
    let mut state = state_streaming(
        vec![Message::user("hi"), Message::assistant("partial")],
        "hocus-1",
    );
    state.on_chunk("hocus-1", b"", true, Some("connection reset".into()));

    assert_eq!(state.messages[1].content, "partial");
    assert_eq!(state.messages[2].role, Role::Error);
    assert!(matches!(state.status, Status::Idle));
}

#[test]
fn an_api_error_revokes_job_authority_and_discards_tool_calls() {
    let mut state = ready_state();
    state.scheduled_run = Some(ScheduledRun {
        job_id: 1,
        auto_confirm: true,
    });
    let frame = tool_call_frame("call_1", "clear_pane", r#"{"pane_id":1,"is_plugin":false}"#);
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk(
        "hocus-1",
        b"data: {\"error\":{\"message\":\"bad response\"}}\n",
        false,
        None,
    );

    assert!(state.stream.as_ref().is_some_and(|s| s.finishing));
    assert!(state.scheduled_run.is_none());
    state.on_chunk("hocus-1", b"", true, None);
    assert!(tool_results(&state).is_empty());
}

#[test]
fn cancelling_a_stream_revokes_job_authority_immediately() {
    let mut state = ready_state();
    state.scheduled_run = Some(ScheduledRun {
        job_id: 1,
        auto_confirm: true,
    });

    state.cancel_stream();

    assert!(state.scheduled_run.is_none());
}

#[test]
fn an_empty_placeholder_is_replaced_by_the_error() {
    let mut state = state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");
    state.on_chunk("hocus-1", b"", true, Some("connection reset".into()));

    assert_eq!(state.messages.len(), 2);
    assert_eq!(state.messages[1].role, Role::Error);
}

#[test]
fn a_silent_stream_reports_that_nothing_came_back() {
    let mut state = state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");
    state.on_chunk("hocus-1", b"", true, None);

    assert_eq!(state.messages[1].role, Role::Error);
    assert!(state.messages[1].content.contains("empty response"));
}

#[test]
fn a_dsml_tool_call_in_text_is_hidden_and_never_run() {
    let mut state = ready_state();
    let content = "Useful preamble.\n\n<｜｜DSML｜｜tool_calls>\n<｜｜DSML｜｜invoke name=\"search_files\">bad</｜｜DSML｜｜invoke>\n</｜｜DSML｜｜tool_calls>";
    let frame = format!(
        "data: {}\n",
        serde_json::json!({"choices": [{"delta": {"content": content}}]})
    );
    state.on_chunk("hocus-1", frame.as_bytes(), false, None);
    ui::render(&mut state, 24, 80);

    state.on_chunk("hocus-1", b"", true, None);

    assert!(state.pending.is_empty(), "text is never an executable call");
    assert_eq!(state.messages[1].content, "Useful preamble.");
    assert_eq!(state.messages.last().unwrap().role, Role::Error);
    assert!(state
        .messages
        .last()
        .unwrap()
        .content
        .contains("no tool was run"));
    assert!(state
        .messages
        .iter()
        .all(|message| !message.content.contains("DSML")));
}

#[test]
fn finishing_swallows_the_terminator_without_a_second_outcome() {
    // What a cancel leaves behind: notice already recorded, terminator due.
    let mut state = State {
        messages: vec![Message::user("hi"), Message::notice("cancelled")],
        stream: finishing("hocus-1"),
        ..State::default()
    };
    state.on_chunk("hocus-1", b"", true, Some("cancelled".into()));

    assert_eq!(state.messages.len(), 2, "no extra error line");
    assert!(matches!(state.status, Status::Idle));
}

#[test]
fn chunks_from_a_superseded_stream_are_ignored() {
    let mut state = state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-2");
    let rendered = state.on_chunk(
        "hocus-1",
        b"data: {\"choices\":[{\"delta\":{\"content\":\"stale\"}}]}\n",
        false,
        None,
    );

    assert!(!rendered);
    assert_eq!(state.messages[1].content, "");
}

/// Every plugin render costs the host a full screen composite, on the same
/// thread that services every other Zellij action — one per socket read
/// froze the whole session for the length of a reply. Only the terminator
/// asks for a render now; the frame timer repaints what arrives in between.
#[test]
fn a_mid_stream_chunk_accumulates_text_without_asking_for_a_render() {
    let mut state = state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");
    let rendered = state.on_chunk(
        "hocus-1",
        b"data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n",
        false,
        None,
    );

    assert!(!rendered, "no render per chunk");
    assert_eq!(state.messages[1].content, "Hel", "but the text still lands");
    assert!(
        state.on_chunk("hocus-1", b"", true, None),
        "the terminator renders"
    );
}

/// The counterpart to the above: with chunks no longer rendering, the timer
/// is the only thing repainting a reply. It must outlive the spinner, which
/// stops at the first token.
#[test]
fn the_frame_timer_outlives_the_spinner() {
    let mut state = state_streaming(
        vec![Message::user("hi"), Message::assistant("some text")],
        "hocus-1",
    );

    assert!(!state.is_loading(), "text is arriving, so no spinner");
    assert!(state.on_timer(FRAME_SECONDS), "but frames keep coming");
    assert!(state.timer_armed, "and the chain re-armed itself");
}

#[test]
fn the_frame_timer_stops_once_the_stream_ends() {
    let mut state = state_with(vec![Message::user("hi")], Status::Idle);

    assert!(!state.on_timer(FRAME_SECONDS));
    assert_eq!(state.tick, 0, "no more frames for an idle plugin");
}

#[test]
fn streamed_deltas_accumulate_into_the_placeholder() {
    let mut state = state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");
    state.on_chunk(
        "hocus-1",
        b"data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n",
        false,
        None,
    );
    state.on_chunk(
        "hocus-1",
        b"data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n",
        false,
        None,
    );

    assert_eq!(state.messages[1].content, "Hello");
}

#[test]
fn a_non_2xx_response_reports_the_api_error_and_expects_a_terminator() {
    let mut state = state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");
    state.on_response(
        401,
        br#"{"error":{"message":"bad key"}}"#,
        &stream_context("hocus-1"),
    );

    assert_eq!(state.messages.len(), 2);
    assert_eq!(state.messages[1].role, Role::Error);
    assert!(state.messages[1].content.contains("bad key"));
    assert!(state.stream.as_ref().is_some_and(|s| s.finishing));
}

#[test]
fn a_2xx_response_changes_nothing_because_the_body_arrives_as_chunks() {
    let mut state = state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");
    let rendered = state.on_response(200, b"", &BTreeMap::new());

    assert!(!rendered);
    assert_eq!(state.messages.len(), 2);
    assert!(state.is_receiving());
}
