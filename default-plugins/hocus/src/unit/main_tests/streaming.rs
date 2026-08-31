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
fn a_hidden_wand_stops_its_timer_chain() {
    let mut state = idle_state();
    state.wand_drawing = true;
    state.hidden = true;
    state.timer_armed = true;

    assert!(!state.on_timer(FRAME_SECONDS), "no hidden redraw");
    assert!(
        !state.timer_armed,
        "the decorative clock stops while hidden"
    );
    assert_eq!(state.wand_frame, 0, "the hidden wand does not advance");
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

// -- rate limits --------------------------------------------------------

/// The body OpenAI turns a request away with once the per-minute token budget is spent.
fn rate_limit_body(hint: &str) -> Vec<u8> {
    format!(
        r#"{{"error":{{"message":"Rate limit reached for gpt-5.6-luna on tokens per min (TPM): Limit 200000, Used 164171, Requested 47713. Please try again in {hint}. Visit https://platform.openai.com/account/rate-limits to learn more."}}}}"#
    )
    .into_bytes()
}

fn stream_context(id: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("stream".to_owned(), id.to_owned())])
}

/// The whole point: a rate limit is a "not yet", and answering it by ending the turn makes the
/// user retype - which sends the very transcript that was too big all over again.
#[test]
fn a_rate_limit_arms_a_retry_instead_of_ending_the_turn() {
    let mut state = ready_state();
    let before = state.messages.len();

    assert!(state.on_response(429, &rate_limit_body("3.565s"), &stream_context("hocus-1")));

    assert!(state.retry.is_some(), "a retry is armed");
    assert!(matches!(state.status, Status::AwaitingRetry));
    assert!(
        !state.messages.iter().any(|m| m.role == Role::Error),
        "no error line: nothing has gone wrong yet"
    );
    assert_eq!(
        state.messages.len(),
        before - 1,
        "only the empty placeholder went; nothing was added to the payload that hit the limit"
    );
    let retry = state.retry.expect("armed");
    assert_eq!(retry.attempt, 0);
    assert_eq!(
        retry.due_ms,
        Some(3565),
        "the wait the provider itself named"
    );
}

/// A status a resend would only repeat is still an error, immediately.
#[test]
fn a_bad_request_is_not_retried() {
    let mut state = ready_state();

    state.on_response(
        400,
        br#"{"error":{"message":"invalid tool schema"}}"#,
        &stream_context("hocus-1"),
    );

    assert!(state.retry.is_none());
    assert!(state.messages.iter().any(|m| m.role == Role::Error));
}

/// The transcript is what makes a resend a resend. It must go out byte-identical, which means
/// nothing may be appended to it while the retry waits.
#[test]
fn the_retry_resends_the_transcript_untouched() {
    let mut state = ready_state();
    state.on_response(429, &rate_limit_body("1s"), &stream_context("hocus-1"));
    let waiting = state.messages.clone();
    let seq = state.stream_seq;

    state.on_timer(2.0);

    assert!(
        state.retry.is_some(),
        "still armed, so a second refusal keeps counting"
    );
    assert!(matches!(state.status, Status::Idle));
    assert_eq!(
        state.messages.len(),
        waiting.len() + 1,
        "only the new streaming placeholder"
    );
    assert_eq!(state.messages[..waiting.len()], waiting[..]);
    assert!(state.stream_seq > seq, "a fresh request went out");
}

/// A job's authority has to survive its own rate limit, or the resend comes back unable to call
/// the tools the run was armed for. Two paths could revoke it: the 429 itself, and the
/// terminator the host still owes for the request that was turned away.
#[test]
fn a_rate_limited_scheduled_run_keeps_its_authority() {
    let mut state = ready_state();
    state.scheduled_run = Some(ScheduledRun {
        job_id: 1,
        auto_confirm: true,
    });

    state.on_response(429, &rate_limit_body("1s"), &stream_context("hocus-1"));
    assert!(
        state.scheduled_run.is_some(),
        "the refusal did not revoke it"
    );

    state.on_chunk("hocus-1", b"", true, None);
    assert!(state.scheduled_run.is_some(), "nor did the owed terminator");
}

/// A provider that keeps saying no is a provider the person should be told about, rather than
/// one hocus spins on forever.
#[test]
fn the_retry_gives_up_after_the_cap() {
    let mut state = ready_state();
    // Each resend goes out under a fresh stream id, and `on_response` only answers to the one
    // in flight - so the refusals have to be addressed to the request they are refusing.
    let refuse = |state: &mut State| {
        let id = state.active_stream_id().expect("a request in flight");
        state.on_response(429, &rate_limit_body("1s"), &stream_context(&id));
    };
    for _ in 0..MAX_RETRIES {
        refuse(&mut state);
        assert!(state.retry.is_some());
        state.on_timer(2.0);
    }

    refuse(&mut state);

    assert!(state.retry.is_none(), "the cap is spent");
    assert!(state.messages.iter().any(|m| m.role == Role::Error));
    assert!(matches!(state.status, Status::Idle));
    assert!(state.scheduled_run.is_none());
}

/// The count is per-outage. A reply that lands resets it, so a rate limit an hour later starts
/// its backoff from the beginning instead of from wherever the last one stopped.
#[test]
fn a_reply_that_lands_clears_the_retry_count() {
    let mut state = ready_state();
    state.on_response(429, &rate_limit_body("1s"), &stream_context("hocus-1"));
    state.on_timer(2.0);
    let resent = state.active_stream_id().expect("resent");

    state.on_chunk(
        &resent,
        b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n",
        false,
        None,
    );
    assert!(state.retry.is_some(), "a partial reply is not success yet");
    state.on_chunk(&resent, b"", true, None);

    assert!(state.retry.is_none());
}

/// `AwaitingRetry` reads as busy to every predicate built on `Status`, so a scheduled job does
/// not fire into the gap between a refusal and the resend answering it.
#[test]
fn a_job_does_not_fire_while_a_retry_is_pending() {
    let mut state = ready_state();
    state.on_response(429, &rate_limit_body("30s"), &stream_context("hocus-1"));

    assert!(state.is_in_flight());
    assert!(state.is_loading(), "the spinner stays up");
    assert!(state.tick_jobs(1.0).is_none());
}

/// Ctrl-c reaches a waiting retry, which has no stream of its own to cancel.
#[test]
fn the_interrupt_cancels_a_waiting_retry() {
    let mut state = ready_state();
    state.scheduled_run = Some(ScheduledRun {
        job_id: 1,
        auto_confirm: true,
    });
    state.on_response(429, &rate_limit_body("30s"), &stream_context("hocus-1"));
    state.on_chunk("hocus-1", b"", true, None);

    assert!(state.cancel_stream());

    assert!(state.retry.is_none());
    assert!(state.scheduled_run.is_none());
    assert!(matches!(state.status, Status::Idle));
    assert_eq!(state.messages.last().expect("notice").role, Role::Notice);
}

/// A rate limit inside the stream, the way a provider that has already answered `200` reports
/// one.
fn streamed_rate_limit(hint: &str) -> Vec<u8> {
    format!(
        "data: {{\"error\":{{\"message\":\"Rate limit reached for gpt-5.6-luna on tokens per \
         min (TPM). Please try again in {hint}.\"}}}}\n"
    )
    .into_bytes()
}

/// `poll_retry` runs on every frame, and a provider can take seconds to answer. The resend has
/// to be one request, not one per frame: each extra POST bumps `stream_seq`, orphans the
/// stream before it, and is itself another unit of the budget that was already spent.
#[test]
fn the_resend_goes_out_once_however_long_the_answer_takes() {
    let mut state = ready_state();
    state.on_response(429, &rate_limit_body("1s"), &stream_context("hocus-1"));

    state.on_timer(2.0);
    let resent = state.stream_seq;
    // Thirty frames of the endpoint thinking about it, with nothing arriving.
    for _ in 0..30 {
        state.on_timer(0.1);
    }

    assert_eq!(state.stream_seq, resent, "one resend, not one per frame");
    assert_eq!(
        state
            .messages
            .iter()
            .filter(|m| m.role == Role::Assistant)
            .count(),
        1,
        "and one placeholder to stream into"
    );
}

/// The countdown is about a wait. Once the resend is out there is nothing left to count down
/// to, and a loader still saying "retrying in 1s" describes a request that is already running.
#[test]
fn the_countdown_stops_once_the_resend_is_out() {
    let mut state = ready_state();
    state.on_response(429, &rate_limit_body("1s"), &stream_context("hocus-1"));
    assert!(state.loader().expect("waiting").contains("rate limited"));

    state.on_timer(2.0);

    let phrase = state.loader().expect("still working");
    assert!(!phrase.contains("rate limited"), "{phrase}");
}

/// The count has to survive the error that increments it. It used to be cleared on the way into
/// every chunk, error or not, so a stream that kept failing armed attempt 0 forever: the cap was
/// unreachable and the countdown said "1/4" for as long as the provider kept saying no.
#[test]
fn a_stream_that_keeps_failing_counts_towards_the_cap() {
    let mut state = ready_state();
    let refuse_after_partial = |state: &mut State| {
        let id = state.active_stream_id().expect("a request in flight");
        state.on_chunk(&id, &delta_frame("partial"), false, None);
        state.on_chunk(&id, &streamed_rate_limit("1s"), false, None);
    };

    for attempt in 0..MAX_RETRIES {
        refuse_after_partial(&mut state);
        assert_eq!(
            state.retry.expect("armed").attempt,
            attempt,
            "partial content did not reset attempt {attempt}"
        );
        state.on_timer(2.0);
    }
    refuse_after_partial(&mut state);

    assert!(state.retry.is_none(), "the cap is reachable");
    assert!(state.messages.iter().any(|m| m.role == Role::Error));
}

/// A refused stream can overlap a command. Once the command lands, its queued follow-up and a
/// user ask must leave the request slot to the retry, including across the refused terminator.
#[test]
fn a_retry_takes_the_next_request_after_a_concurrent_command_finishes() {
    let mut state = ready_state();
    state.pending = vec![ToolCall {
        id: "command".into(),
        name: "slack_read".into(),
        arguments: r##"{"channel":"#general"}"##.into(),
    }]
    .into();
    state.status = Status::AwaitingCommand;
    state.owes_reply = true;
    state.input.set_text("queued ask".to_owned());
    state.submit();

    state.on_response(429, &rate_limit_body("30s"), &stream_context("hocus-1"));
    state.on_command_result(
        Some(0),
        b"done",
        b"",
        &BTreeMap::from([("tool_call_id".to_owned(), "command".to_owned())]),
    );

    assert!(matches!(state.status, Status::AwaitingRetry));
    assert!(state.owes_reply, "the tool follow-up is still owed");
    assert_eq!(state.asks.len(), 1, "the queued ask is still waiting");

    state.on_chunk("hocus-1", b"", true, None);
    assert!(matches!(state.status, Status::AwaitingRetry));
    assert!(
        state.stream.is_none(),
        "the refused stream is fully terminated"
    );

    state.on_timer(31.0);
    assert!(state.is_receiving(), "the retry owns the next request");
    assert!(state.retry.is_some_and(|retry| retry.due_ms.is_none()));
    assert!(state.owes_reply);
    assert_eq!(state.asks.len(), 1);
}

/// Ctrl-c while the resend is in flight. The stream is a stream like any other: cancelling has
/// to reach the host, or the notice is a lie and the reply lands behind it.
#[test]
fn the_interrupt_cancels_a_resend_in_flight() {
    let mut state = ready_state();
    state.on_response(429, &rate_limit_body("1s"), &stream_context("hocus-1"));
    state.on_timer(2.0);
    assert!(state.is_receiving(), "the resend is in flight");

    assert!(state.cancel_stream());

    assert!(state.retry.is_none(), "the retry is spent either way");
    assert!(
        !state.is_receiving(),
        "the stream was cancelled, not left running under the notice"
    );
    // Kept rather than dropped: the host still owes one terminator for the cancelled request,
    // and `finishing` is what stops it being recorded as a second outcome.
    assert!(state.is_streaming());
    assert_eq!(state.messages.last().expect("notice").role, Role::Notice);
}

/// A resend is the same request again, so it has to go out the same. Text that arrived before
/// the provider gave up is a half-finished assistant turn: left in the transcript it goes back
/// as prefill for the model to continue from, and the answer comes back with its first
/// sentence twice.
#[test]
fn a_streamed_retry_takes_back_the_partial_reply() {
    let mut state = ready_state();
    state.on_chunk(
        "hocus-1",
        b"data: {\"choices\":[{\"delta\":{\"content\":\"half a sen\"}}]}\n",
        false,
        None,
    );
    assert!(state.messages.iter().any(|m| m.content == "half a sen"));

    state.on_chunk("hocus-1", &streamed_rate_limit("1s"), false, None);

    assert!(state.retry.is_some(), "armed");
    assert!(
        !state
            .messages
            .iter()
            .any(|m| m.content.contains("half a sen")),
        "the half sentence went back with the request that failed"
    );
    assert!(state.reply_at.is_none());
}

/// A stream and a tool run can both be outstanding. A refusal that arrives while the queue is
/// running a command used to take `Status` for itself, and `on_command_result` answers to
/// `AwaitingCommand` alone - so the result was dropped and the call stayed unanswered forever.
#[test]
fn a_refusal_does_not_take_the_status_a_tool_is_using() {
    let mut state = ready_state();
    state.status = Status::AwaitingCommand;

    state.on_response(429, &rate_limit_body("30s"), &stream_context("hocus-1"));

    assert!(state.retry.is_some(), "the retry is armed all the same");
    assert!(
        matches!(state.status, Status::AwaitingCommand),
        "the tool still owns the status it is going to be answered on"
    );
}

/// The countdown is drawn, never stored: a message mutated once a second would churn the
/// render cache and the autosave for text nobody needs to keep.
#[test]
fn the_countdown_is_in_the_loader() {
    let mut state = ready_state();
    state.on_response(429, &rate_limit_body("4s"), &stream_context("hocus-1"));

    let phrase = state.loader().expect("a spinner while waiting");

    assert!(phrase.contains("rate limited"), "{phrase}");
    assert!(phrase.contains("4s"), "{phrase}");
    assert!(phrase.contains("1/4"), "{phrase}");
}
