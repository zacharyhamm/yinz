use super::*;

fn usage_frame(input: u64, output: u64) -> Vec<u8> {
    format!(
        "data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":{input},\"completion_tokens\":{output}}}}}\n"
    )
    .into_bytes()
}

fn begin_reply(state: &mut State, id: &str) {
    state.messages.push(Message::assistant(""));
    state.reply_at = Some(state.messages.len() - 1);
    state.sse = SseParser::default();
    state.stream = Some(StreamState {
        id: id.to_owned(),
        finishing: false,
        usage: None,
    });
}

fn settle_usage(state: &mut State, id: &str, input: u64, output: u64) {
    state.on_chunk(
        id,
        b"data: {\"choices\":[{\"delta\":{\"content\":\"done\"}}]}\n",
        false,
        None,
    );
    state.on_chunk(id, &usage_frame(input, output), false, None);
    state.on_chunk(id, b"", true, None);
}

#[test]
fn every_request_accumulates_separate_input_and_output_totals() {
    let mut state = state_with(vec![Message::user("one")], Status::Idle);
    begin_reply(&mut state, "hocus-1");
    settle_usage(&mut state, "hocus-1", 400, 25);

    state.messages.push(Message::user("two"));
    begin_reply(&mut state, "hocus-2");
    settle_usage(&mut state, "hocus-2", 700, 30);

    assert_eq!(
        state.usage,
        Some(TokenUsage {
            input_tokens: 1_100,
            output_tokens: 55,
        })
    );
}

#[test]
fn reported_usage_survives_cancelling_the_stream() {
    let mut state = state_with(vec![Message::user("one")], Status::Idle);
    begin_reply(&mut state, "hocus-1");

    assert!(state.cancel_stream());
    state.on_chunk("hocus-1", &usage_frame(400, 25), false, None);
    state.on_chunk("hocus-1", b"", true, None);

    assert_eq!(
        state.usage,
        Some(TokenUsage {
            input_tokens: 400,
            output_tokens: 25,
        })
    );
}

#[test]
fn repeated_usage_chunks_contribute_once_per_stream() {
    let mut state = state_with(vec![Message::user("one")], Status::Idle);
    begin_reply(&mut state, "hocus-1");
    state.on_chunk("hocus-1", &usage_frame(400, 25), false, None);
    state.on_chunk("hocus-1", &usage_frame(400, 25), false, None);
    state.on_chunk("hocus-1", b"", true, None);

    assert_eq!(
        state.usage,
        Some(TokenUsage {
            input_tokens: 400,
            output_tokens: 25,
        })
    );
}

/// A provider that reports its rate limit inside the stream can report usage on the
/// same chunk, and the backoff can elapse before the host delivers that attempt's
/// `done`. The resend replaces the stream, and the terminator that arrives afterwards
/// is dropped by the id check in `on_chunk` — so the attempt's cost has to be banked
/// where it is superseded, or it is never counted at all. It was really spent.
#[test]
fn a_resend_banks_what_the_attempt_it_supersedes_cost() {
    let mut state = ready_state();
    // `ready_state` hands out "hocus-1" already; the resend takes the next id, which is
    // what makes the late terminator below belong to a stream nothing is listening for.
    state.stream_seq = 1;

    state.on_chunk("hocus-1", &usage_frame(400, 25), false, None);
    state.on_chunk(
        "hocus-1",
        b"data: {\"error\":{\"message\":\"Rate limit reached. Please try again in 1s.\"}}\n",
        false,
        None,
    );
    assert!(state.retry.is_some(), "the refusal armed a retry");

    // The backoff elapses and `poll_retry` resends, with the terminator still owed.
    state.on_timer(2.0);
    assert_eq!(state.stream_seq, 2, "the resend went out");

    let spent = Some(TokenUsage {
        input_tokens: 400,
        output_tokens: 25,
    });
    assert_eq!(state.usage, spent, "the failed attempt's cost was banked");

    // And the terminator, when it finally lands, is still nobody's: it adds nothing.
    state.on_chunk("hocus-1", b"", true, None);
    assert_eq!(state.usage, spent);
}

#[test]
fn a_usage_only_change_is_archived() {
    let mut state = state_with(vec![Message::user("one")], Status::Idle);
    state.autosave();
    let id = state.chat_id.expect("the transcript was saved");

    state.usage = Some(TokenUsage {
        input_tokens: 400,
        output_tokens: 25,
    });
    state.autosave();

    assert_eq!(history::load_usage(&state.private_store(), id), state.usage);
}

#[test]
fn usage_travels_with_its_chat_and_a_new_chat_starts_empty() {
    let mut state = ready_state();
    settle(&mut state);
    state.usage = Some(TokenUsage {
        input_tokens: 900,
        output_tokens: 40,
    });

    state.new_chat();
    assert_eq!(state.usage, None);

    state.switch_to_tab(0);
    assert_eq!(
        state.usage,
        Some(TokenUsage {
            input_tokens: 900,
            output_tokens: 40,
        })
    );

    state.reset_chat();
    assert_eq!(state.usage, None);
}

#[test]
fn counts_are_shown_at_title_width() {
    assert_eq!(ui::format_tokens(0), "0");
    assert_eq!(ui::format_tokens(842), "842");
    assert_eq!(ui::format_tokens(1_000), "1.0k");
    assert_eq!(ui::format_tokens(12_345), "12.3k");
    assert_eq!(ui::format_tokens(999_949), "999.9k");
    assert_eq!(ui::format_tokens(999_950), "1.0M");
    assert_eq!(ui::format_tokens(1_234_567), "1.2M");
}
