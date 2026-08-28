use super::*;

#[test]
fn reassembles_a_frame_split_across_many_chunks() {
    let mut parser = SseParser::default();
    let mut collected = Vec::new();
    // One byte at a time - the worst case the host can hand us.
    for byte in FRAME {
        collected.extend(parser.feed(&[*byte]));
    }
    assert_eq!(deltas(collected), vec!["Hello"]);
}

#[test]
fn splits_a_multibyte_character_across_chunks_without_corruption() {
    let frame = "data: {\"choices\":[{\"delta\":{\"content\":\"héllo→\"}}]}\n".as_bytes();
    let mut parser = SseParser::default();
    let mut collected = Vec::new();
    for byte in frame {
        collected.extend(parser.feed(&[*byte]));
    }
    assert_eq!(deltas(collected), vec!["héllo→"]);
}

#[test]
fn handles_crlf_line_endings() {
    let mut parser = SseParser::default();
    let frame = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\r\n\r\n";
    assert_eq!(deltas(parser.feed(frame)), vec!["Hi"]);
}

#[test]
fn ignores_keep_alive_comments_and_blank_lines() {
    let mut parser = SseParser::default();
    assert!(parser.feed(b": ping\n\n\n").is_empty());
}

#[test]
fn ignores_non_data_sse_fields() {
    let mut parser = SseParser::default();
    assert!(parser
        .feed(b"event: message\nid: 42\nretry: 1000\n")
        .is_empty());
}

#[test]
fn recognises_the_done_sentinel() {
    let mut parser = SseParser::default();
    assert_eq!(parser.feed(b"data: [DONE]\n"), vec![SseItem::Done]);
}

#[test]
fn ignores_the_finish_chunk_with_an_empty_delta() {
    let mut parser = SseParser::default();
    let frame = b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n";
    assert!(parser.feed(frame).is_empty());
}

#[test]
fn surfaces_an_error_reported_inside_the_stream() {
    let mut parser = SseParser::default();
    let frame = b"data: {\"error\":{\"message\":\"rate limited\"}}\n";
    assert_eq!(
        parser.feed(frame),
        vec![SseItem::ApiError("rate limited".into())]
    );
}

#[test]
fn ignores_a_data_line_that_is_not_json() {
    let mut parser = SseParser::default();
    assert!(parser.feed(b"data: not json at all\n").is_empty());
}

#[test]
fn preserves_order_across_two_frames_in_one_chunk() {
    let mut parser = SseParser::default();
    let chunk = b"data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\n\
                  data: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n\n";
    assert_eq!(deltas(parser.feed(chunk)), vec!["a", "b"]);
}

#[test]
fn retains_an_incomplete_trailing_line_for_the_next_feed() {
    let mut parser = SseParser::default();
    assert!(parser
        .feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"par")
        .is_empty());
    assert_eq!(deltas(parser.feed(b"tial\"}}]}\n")), vec!["partial"]);
}

// -- reasoning ----------------------------------------------------------
//
// A thinking endpoint streams its chain-of-thought under its own field name and
// wants it handed back on the next request. These pin the two spellings in use,
// and that a delta from an endpoint that does not think yields none of it.

#[test]
fn a_reasoning_content_delta_is_read_as_deepseek_style_thinking() {
    let mut parser = SseParser::default();
    let frame = b"data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"let me think\"}}]}\n\n";
    assert_eq!(
        parser.feed(frame),
        vec![SseItem::Reasoning {
            text: "let me think".to_owned(),
            key: ReasoningKey::ReasoningContent,
        }]
    );
}

#[test]
fn a_bare_reasoning_delta_keeps_its_own_spelling() {
    let mut parser = SseParser::default();
    let frame = b"data: {\"choices\":[{\"delta\":{\"reasoning\":\"hmm\"}}]}\n\n";
    assert_eq!(
        parser.feed(frame),
        vec![SseItem::Reasoning {
            text: "hmm".to_owned(),
            key: ReasoningKey::Reasoning,
        }]
    );
}

#[test]
fn a_chunk_carrying_thinking_and_text_yields_both_thinking_first() {
    let mut parser = SseParser::default();
    let frame =
        b"data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"..done\",\"content\":\"Hi\"}}]}\n\n";
    assert_eq!(
        parser.feed(frame),
        vec![
            SseItem::Reasoning {
                text: "..done".to_owned(),
                key: ReasoningKey::ReasoningContent,
            },
            SseItem::Delta("Hi".to_owned()),
        ],
        "dropping the tail of the reasoning would echo a truncated one back, \
         which is refused just as hard as a missing one"
    );
}

#[test]
fn a_chunk_keeps_reasoning_and_every_parallel_tool_call() {
    let mut parser = SseParser::default();
    let frame = br#"data: {"choices":[{"delta":{"reasoning_content":"done","tool_calls":[{"index":0,"id":"a","function":{"name":"list_session","arguments":"{}"}},{"index":1,"id":"b","function":{"name":"list_other_sessions","arguments":"{}"}}]}}]}
"#;
    assert_eq!(
        parser.feed(frame),
        vec![
            SseItem::Reasoning {
                text: "done".to_owned(),
                key: ReasoningKey::ReasoningContent,
            },
            SseItem::ToolCallFragment {
                index: 0,
                id: Some("a".to_owned()),
                name: Some("list_session".to_owned()),
                arguments: Some("{}".to_owned()),
            },
            SseItem::ToolCallFragment {
                index: 1,
                id: Some("b".to_owned()),
                name: Some("list_other_sessions".to_owned()),
                arguments: Some("{}".to_owned()),
            },
        ]
    );
}

#[test]
fn reassembles_reasoning_split_across_chunks() {
    let mut parser = SseParser::default();
    let frame = b"data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"one two\"}}]}\n\n";
    let mut collected = Vec::new();
    for byte in frame {
        collected.extend(parser.feed(&[*byte]));
    }
    assert_eq!(
        collected,
        vec![SseItem::Reasoning {
            text: "one two".to_owned(),
            key: ReasoningKey::ReasoningContent,
        }]
    );
}

#[test]
fn an_ordinary_delta_yields_no_thinking_at_all() {
    let mut parser = SseParser::default();
    assert_eq!(parser.feed(FRAME), vec![SseItem::Delta("Hello".to_owned())]);
}

// -- usage ----------------------------------------------------------------

/// The chunk `stream_options.include_usage` asks for has no choices at all,
/// which used to make it indistinguishable from noise.
#[test]
fn a_usage_only_chunk_yields_the_count_and_nothing_else() {
    let mut parser = SseParser::default();
    let frame = b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":120,\"completion_tokens\":7,\"total_tokens\":127}}\n";
    let items = parser.feed(frame);
    assert_eq!(items.len(), 1);
    assert!(matches!(
        items[0],
        SseItem::Usage {
            prompt: 120,
            completion: 7
        }
    ));
}

/// A `usage` with no counts in it — `total_tokens` alone, or the `{}` some
/// proxies send instead of leaving the field out — is not a number to show as
/// exact, so it is not a `Usage` at all.
#[test]
fn a_usage_without_prompt_or_completion_counts_is_ignored() {
    let mut parser = SseParser::default();
    let frame = b"data: {\"choices\":[],\"usage\":{\"total_tokens\":7}}\n";
    assert!(parser.feed(frame).is_empty());
    let frame = b"data: {\"choices\":[],\"usage\":{}}\n";
    assert!(parser.feed(frame).is_empty());
    // One count is enough; the other reads as zero.
    let frame = b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":9}}\n";
    let items = parser.feed(frame);
    assert!(matches!(
        items[..],
        [SseItem::Usage {
            prompt: 9,
            completion: 0
        }]
    ));
}

/// Ollama hangs the count on the last content chunk instead. Both survive,
/// the count first; `on_chunk` holds it until `done` regardless.
#[test]
fn a_usage_on_a_content_chunk_keeps_both() {
    let mut parser = SseParser::default();
    let frame = b"data: {\"choices\":[{\"delta\":{\"content\":\"end\"}}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1}}\n";
    let items = parser.feed(frame);
    assert_eq!(items.len(), 2);
    assert!(matches!(
        items[0],
        SseItem::Usage {
            prompt: 3,
            completion: 1
        }
    ));
    assert_eq!(deltas(items), vec!["end"]);
}
