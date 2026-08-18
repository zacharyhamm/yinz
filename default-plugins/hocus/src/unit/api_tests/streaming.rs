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
fn ignores_a_usage_only_chunk_with_no_choices() {
    let mut parser = SseParser::default();
    let frame = b"data: {\"choices\":[],\"usage\":{\"total_tokens\":7}}\n";
    assert!(parser.feed(frame).is_empty());
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
