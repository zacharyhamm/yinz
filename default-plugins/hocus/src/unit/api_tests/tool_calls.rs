use super::*;

/// The realistic shape: `id` and `name` arrive once, then `arguments` comes
/// in as many fragments as the provider feels like.
#[test]
fn reassembles_arguments_split_across_many_fragments() {
    let calls = accumulate(&[
        br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"new_pane","arguments":""}}]}}]}
"#,
        br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"com"}}]}}]}
"#,
        br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"mand\":\"h"}}]}}]}
"#,
        br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"top\"}"}}]}}]}
"#,
    ]);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "call_abc");
    assert_eq!(calls[0].name, "new_pane");
    assert_eq!(calls[0].arguments, r#"{"command":"htop"}"#);
}

/// Two calls in one reply are interleaved by index, not sent in sequence.
#[test]
fn keeps_interleaved_parallel_calls_separate() {
    let calls = accumulate(&[
        br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"close_pane","arguments":"{\"pane_id\":"}}]}}]}
"#,
        br#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"b","function":{"name":"close_tab","arguments":"{\"tab_id\":"}}]}}]}
"#,
        br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}}]}}]}
"#,
        br#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"function":{"arguments":"2}"}}]}}]}
"#,
    ]);

    assert_eq!(calls.len(), 2);
    assert_eq!(
        (calls[0].id.as_str(), calls[0].name.as_str()),
        ("a", "close_pane")
    );
    assert_eq!(calls[0].arguments, r#"{"pane_id":1}"#);
    assert_eq!(
        (calls[1].id.as_str(), calls[1].name.as_str()),
        ("b", "close_tab")
    );
    assert_eq!(calls[1].arguments, r#"{"tab_id":2}"#);
}

/// The SSE framing already survives arbitrary chunk boundaries; this checks
/// that accumulation on top of it does too.
#[test]
fn reassembles_a_call_delivered_one_byte_at_a_time() {
    let frame = br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","function":{"name":"list_session","arguments":"{}"}}]}}]}
"#;
    let chunks: Vec<&[u8]> = frame.iter().map(std::slice::from_ref).collect();
    let calls = accumulate(&chunks);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "list_session");
    assert_eq!(calls[0].arguments, "{}");
}

/// Some providers omit `index` when only one call is in flight.
#[test]
fn treats_a_missing_index_as_a_single_call() {
    let calls = accumulate(&[
        br#"data: {"choices":[{"delta":{"tool_calls":[{"id":"x","function":{"name":"list_session","arguments":"{"}}]}}]}
"#,
        br#"data: {"choices":[{"delta":{"tool_calls":[{"function":{"arguments":"}"}}]}}]}
"#,
    ]);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments, "{}");
}

#[test]
fn a_fragment_that_never_named_a_tool_is_dropped() {
    let calls = accumulate(&[
        br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{}"}}]}}]}
"#,
    ]);
    assert!(calls.is_empty());
}

#[test]
fn a_plain_text_reply_accumulates_no_calls() {
    let calls = accumulate(&[FRAME]);
    assert!(calls.is_empty());
}

#[test]
fn taking_the_calls_leaves_the_accumulator_empty() {
    let mut accumulator = ToolCallAccumulator::default();
    accumulator.feed(&SseItem::ToolCallFragment {
        index: 0,
        id: Some("x".into()),
        name: Some("list_session".into()),
        arguments: Some("{}".into()),
    });
    assert_eq!(accumulator.take().len(), 1);
    assert!(
        accumulator.take().is_empty(),
        "a second take yields nothing"
    );
}

#[test]
fn bails_out_and_resets_when_a_line_grows_without_bound() {
    let mut parser = SseParser::default();
    let flood = vec![b'x'; MAX_PENDING + 1];
    assert!(matches!(
        parser.feed(&flood).as_slice(),
        [SseItem::ApiError(_)]
    ));
    // Buffer was reset, so a well-formed frame still parses afterwards.
    assert_eq!(deltas(parser.feed(FRAME)), vec!["Hello"]);
}
