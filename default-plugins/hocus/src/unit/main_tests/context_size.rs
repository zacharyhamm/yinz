use super::*;

// The context size in the title: the provider's count when there is one,
// a `chars / 4` guess for everything else.

fn usage_frame(prompt: u64, completion: u64) -> Vec<u8> {
    format!(
        "data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":{prompt},\"completion_tokens\":{completion}}}}}\n"
    )
    .into_bytes()
}

/// Before any reply there is nothing to go on but the transcript itself.
#[test]
fn an_unanswered_chat_is_estimated_from_its_characters() {
    let mut state = state_with(vec![Message::user("twelve chars")], Status::Idle);
    assert_eq!(state.context_tokens(), (3, true));
}

/// Our own UI lines never go on the wire, so they never count.
#[test]
fn errors_and_notices_are_not_context() {
    let messages = vec![
        Message::user("1234"),
        Message::notice("a long notice that is not sent to anyone"),
        Message::error("an error that is not sent either"),
        Message::assistant("5678"),
    ];
    assert_eq!(api::wire_chars(&messages), 8);
    let mut state = state_with(messages, Status::Idle);
    assert_eq!(state.context_tokens(), (2, true));
}

/// A tool round is context too: what the model asked and what it got back.
#[test]
fn tool_calls_and_results_count() {
    let call = ToolCall {
        id: "c1".to_owned(),
        name: "ab".to_owned(),
        arguments: "{}".to_owned(),
    };
    let messages = vec![
        Message::tool_calls(vec![call]),
        Message::tool_result("c1", "12345678"),
    ];
    // The call's id, name and arguments all go out; the result whole.
    assert_eq!(api::wire_chars(&messages), 2 + 2 + 2 + 8);
}

/// `wire_chars` follows the request body's rules, not the message's fields:
/// arguments that are not a JSON object go out as `{}`, and a call's prose
/// only goes out when the call carries a `display` (old archives kept the
/// summary in `content`, which is never sent).
#[test]
fn wire_chars_counts_what_the_body_sends() {
    let bad_args = ToolCall {
        id: "c1".to_owned(),
        name: "ab".to_owned(),
        arguments: "".to_owned(),
    };
    assert_eq!(
        api::wire_chars(&[Message::tool_calls(vec![bad_args])]),
        2 + 2 + 2
    );

    let mut archived = Message::tool_calls(vec![ToolCall {
        id: "c1".to_owned(),
        name: "ab".to_owned(),
        arguments: "{}".to_owned(),
    }]);
    archived.content = "a summary from an old archive".to_owned();
    archived.display = None;
    assert_eq!(api::wire_chars(&[archived.clone()]), 2 + 2 + 2);
    archived.display = Some(String::new());
    assert_eq!(
        api::wire_chars(&[archived]),
        2 + 2 + 2 + "a summary from an old archive".len()
    );
}

/// The provider's number replaces the guess, and stays exact until something
/// is added behind it.
#[test]
fn a_usage_chunk_makes_the_count_exact_until_the_transcript_grows() {
    let mut state = state_streaming(
        vec![Message::user("hello"), Message::assistant("")],
        "hocus-1",
    );
    state.on_chunk(
        "hocus-1",
        b"data: {\"choices\":[{\"delta\":{\"content\":\"Hi there\"}}]}\n",
        false,
        None,
    );
    assert!(state.context_tokens().1, "still a guess mid-stream");

    state.on_chunk("hocus-1", &usage_frame(400, 25), false, None);
    state.on_chunk("hocus-1", b"", true, None);
    assert_eq!(state.context_tokens(), (425, false));

    // Eight more characters: two more tokens, and a guess again.
    state.messages.push(Message::user("12345678"));
    assert_eq!(state.context_tokens(), (427, true));
}

/// Ollama hangs the count on the last content chunk, ahead of the text it
/// covers. The baseline is taken at `done`, so that text is not "added".
#[test]
fn a_count_riding_on_the_last_content_chunk_is_still_exact() {
    let mut state = state_streaming(
        vec![Message::user("hello"), Message::assistant("")],
        "hocus-1",
    );
    state.on_chunk(
        "hocus-1",
        b"data: {\"choices\":[{\"delta\":{\"content\":\"end\"}}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":1}}\n",
        false,
        None,
    );
    assert!(state.context_tokens().1, "still a guess until done");
    state.on_chunk("hocus-1", b"", true, None);
    assert_eq!(state.context_tokens(), (11, false));
}

/// The provider counted the tool calls it made, so they are in the baseline;
/// only the result that comes back is added on top.
#[test]
fn a_tool_calling_reply_is_exact_until_its_result_lands() {
    let mut state = ready_state();
    state.on_chunk(
        "hocus-1",
        &tool_call_frame("c1", "list_panes", "{}"),
        false,
        None,
    );
    state.on_chunk("hocus-1", &usage_frame(200, 30), false, None);
    state.on_chunk("hocus-1", b"", true, None);
    assert_eq!(
        state.context_tokens(),
        (230, false),
        "the calls are what was counted, not an addition to it"
    );

    state
        .messages
        .push(Message::tool_result("c1", "1234567890ab"));
    assert_eq!(state.context_tokens(), (233, true));
}

/// A stream that was cancelled or had already failed leaves the last good
/// count alone, whatever the host sends after.
#[test]
fn a_finishing_stream_does_not_record_a_count() {
    let mut state = state_streaming(vec![Message::user("hello")], "hocus-1");
    state.stream = finishing("hocus-1");
    state.on_chunk("hocus-1", &usage_frame(400, 25), true, None);
    assert_eq!(state.usage, None);
}

/// A count from one reply must not be shown against the next chat's
/// transcript, and must still be there when its own chat comes back.
#[test]
fn the_count_travels_with_its_chat() {
    let mut state = ready_state();
    settle(&mut state);
    state.messages = vec![Message::user("one"), Message::assistant("two")];
    state.usage = Some(ContextUsage {
        tokens: 900,
        wire_chars: api::wire_chars(&state.messages),
    });
    assert_eq!(state.context_tokens(), (900, false));

    state.new_chat();
    assert_eq!(state.usage, None);
    assert_eq!(
        state.context_tokens(),
        (0, true),
        "a fresh chat starts from nothing"
    );

    state.switch_to_tab(0);
    assert_eq!(state.context_tokens(), (900, false));
}

#[test]
fn counts_are_shown_at_title_width() {
    assert_eq!(ui::format_tokens(0), "0");
    assert_eq!(ui::format_tokens(842), "842");
    assert_eq!(ui::format_tokens(1_000), "1.0k");
    assert_eq!(ui::format_tokens(12_345), "12.3k");
    assert_eq!(ui::format_tokens(999_949), "999.9k");
    assert_eq!(ui::format_tokens(999_950), "1.0M");
    assert_eq!(ui::format_tokens(999_999), "1.0M");
    assert_eq!(ui::format_tokens(1_234_567), "1.2M");
}
