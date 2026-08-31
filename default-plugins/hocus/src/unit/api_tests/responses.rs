//! The `/v1/responses` transport: the request shape it sends and the event
//! stream it reads. Everything between the two is shared with chat/completions
//! and covered by the modules beside this one.

use super::*;

fn responses_config() -> Config {
    Config {
        url: responses_url(DEFAULT_BASE_URL),
        api: Api::Responses,
        ..test_config()
    }
}

fn responses_body(cfg: &Config, transcript: &[Message]) -> serde_json::Value {
    serde_json::from_slice(&request_body(cfg, transcript)).unwrap()
}

// -- request body -------------------------------------------------------

#[test]
fn request_body_uses_the_responses_shape() {
    let body = responses_body(&responses_config(), &[Message::user("hi")]);
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["model"], json!("gpt-4o-mini"));
    // Nothing is retained server-side: the whole transcript goes every turn.
    assert_eq!(body["store"], json!(false));
    assert_eq!(
        body["input"],
        json!([{"type": "message", "role": "user", "content": "hi"}])
    );
    // The chat/completions spellings must not leak onto this wire.
    assert!(body.get("messages").is_none());
    assert!(body.get("reasoning_effort").is_none());
    assert!(body.get("stream_options").is_none());
    assert!(body.get("thinking").is_none());
}

#[test]
fn request_body_sends_the_system_prompt_as_instructions() {
    let mut cfg = responses_config();
    cfg.system_prompt = Some("Be terse.".into());
    let body = responses_body(&cfg, &[Message::user("hi")]);
    assert_eq!(body["instructions"], json!("Be terse."));
    // And never as a message, which is where the other transport puts it.
    assert_eq!(body["input"][0]["role"], json!("user"));
}

/// The whole reason this transport exists: an effort and a tool set in one
/// request, which chat/completions refuses on a reasoning model.
#[test]
fn request_body_sends_effort_as_an_object_beside_the_tools() {
    let mut cfg = responses_config();
    cfg.reasoning_effort = Some("medium".into());
    cfg.tools = true;
    let body = responses_body(&cfg, &[Message::user("hi")]);
    assert_eq!(
        body["reasoning"],
        json!({"effort": "medium", "summary": "auto"})
    );
    assert!(body["tools"]
        .as_array()
        .is_some_and(|tools| !tools.is_empty()));
}

#[test]
fn request_body_omits_reasoning_when_no_effort_is_configured() {
    let body = responses_body(&responses_config(), &[Message::user("hi")]);
    assert!(body.get("reasoning").is_none());
}

/// Flat, not chat/completions' `{function: {...}}` nesting - and `strict` off,
/// since our schemas have optional parameters.
#[test]
fn request_body_flattens_the_tool_specs() {
    let mut cfg = responses_config();
    cfg.tools = true;
    let body = responses_body(&cfg, &[Message::user("hi")]);
    let spec = &body["tools"][0];
    assert_eq!(spec["type"], json!("function"));
    assert!(spec["name"].is_string());
    assert!(spec["description"].is_string());
    assert!(spec["parameters"].is_object());
    assert_eq!(spec["strict"], json!(false));
    assert!(spec.get("function").is_none());
}

#[test]
fn tools_off_drops_the_key_entirely() {
    let body = responses_body(&responses_config(), &[Message::user("hi")]);
    assert!(body.get("tools").is_none());
}

/// A call and its result are separate items here, tied by `call_id` - and that
/// id is the one the stream reported, so the pair round-trips.
#[test]
fn request_body_sends_a_call_and_its_result_as_separate_items() {
    let mut asked = Message::tool_calls(vec![call("call_1", "new_pane", r#"{"command":"htop"}"#)]);
    asked.content = asked.display.take().unwrap();
    let transcript = [
        Message::user("open htop"),
        asked,
        Message::tool_result("call_1", r#"{"opened":true,"pane_id":7}"#),
    ];
    let body = responses_body(&responses_config(), &transcript);
    assert_eq!(
        body["input"],
        json!([
            {"type": "message", "role": "user", "content": "open htop"},
            {
                "type": "function_call",
                "call_id": "call_1",
                "name": "new_pane",
                "arguments": r#"{"command":"htop"}"#,
            },
            {
                "type": "function_call_output",
                "call_id": "call_1",
                "output": r#"{"opened":true,"pane_id":7}"#,
            },
        ])
    );
}

/// Prose alongside a call becomes its own message item, ahead of the call.
#[test]
fn request_body_puts_call_prose_in_its_own_item() {
    let mut asked = Message::tool_calls(vec![call("call_1", "list_session", "{}")]);
    asked.content = "I'll inspect the session.".into();
    let body = responses_body(&responses_config(), &[asked]);
    assert_eq!(
        body["input"],
        json!([
            {"type": "message", "role": "assistant", "content": "I'll inspect the session."},
            {"type": "function_call", "call_id": "call_1", "name": "list_session", "arguments": "{}"},
        ])
    );
}

/// The same guard as the other transport: arguments that are not a JSON object
/// go out as `{}` rather than getting the next request rejected.
#[test]
fn request_body_substitutes_an_object_for_unparseable_arguments() {
    let asked = Message::tool_calls(vec![call("call_1", "list_session", "")]);
    let body = responses_body(&responses_config(), &[asked]);
    assert_eq!(body["input"][0]["arguments"], json!("{}"));
}

#[test]
fn request_body_skips_our_own_ui_lines() {
    let transcript = [
        Message::user("hi"),
        Message::error("HTTP 401: nope"),
        Message::notice("cancelled"),
        // The streaming placeholder for the reply being asked for.
        Message::assistant(""),
    ];
    let body = responses_body(&responses_config(), &transcript);
    assert_eq!(body["input"].as_array().map(Vec::len), Some(1));
}

// -- stream -------------------------------------------------------------

/// A whole turn as OpenAI streams it: reasoning summary, prose, one tool call
/// whose arguments arrive in pieces, then the terminal event with the usage.
#[test]
fn parses_a_whole_responses_turn() {
    let mut parser = SseParser::default();
    let mut accumulator = ToolCallAccumulator::default();
    let mut items = Vec::new();
    for line in [
        r#"event: response.created"#,
        r#"data: {"type":"response.created","response":{"id":"resp_1"}}"#,
        r#"data: {"type":"response.reasoning_summary_text.delta","output_index":0,"delta":"Listing "}"#,
        r#"data: {"type":"response.reasoning_summary_text.delta","output_index":0,"delta":"panes."}"#,
        r#"data: {"type":"response.output_text.delta","output_index":1,"delta":"On it."}"#,
        r#"data: {"type":"response.output_item.added","output_index":2,"item":{"type":"function_call","id":"fc_1","call_id":"call_abc","name":"new_pane"}}"#,
        r#"data: {"type":"response.function_call_arguments.delta","output_index":2,"delta":"{\"com"}"#,
        r#"data: {"type":"response.function_call_arguments.delta","output_index":2,"delta":"mand\":\"htop\"}"}"#,
        r#"data: {"type":"response.output_item.done","output_index":2,"item":{"type":"function_call"}}"#,
        r#"data: {"type":"response.completed","response":{"usage":{"input_tokens":1200,"output_tokens":34}}}"#,
    ] {
        for item in parser.feed(format!("{line}\n").as_bytes()) {
            accumulator.feed(&item);
            items.push(item);
        }
    }

    assert_eq!(deltas(items.clone()), vec!["On it."]);
    let reasoning: Vec<_> = items
        .iter()
        .filter_map(|item| match item {
            SseItem::Reasoning { text, key } => Some((text.as_str(), *key)),
            _ => None,
        })
        .collect();
    assert_eq!(
        reasoning,
        vec![
            ("Listing ", ReasoningKey::Reasoning),
            ("panes.", ReasoningKey::Reasoning),
        ]
    );
    assert!(items.contains(&SseItem::Usage {
        prompt: 1200,
        completion: 34
    }));
    assert!(items.contains(&SseItem::Done));

    let calls = accumulator.take();
    assert_eq!(calls.len(), 1);
    // The `call_id`, not the item's own `id`: it is what a function_call_output
    // has to quote.
    assert_eq!(calls[0].id, "call_abc");
    assert_eq!(calls[0].name, "new_pane");
    assert_eq!(calls[0].arguments, r#"{"command":"htop"}"#);
}

/// Two calls in one turn are told apart by `output_index`, the way parallel
/// chat/completions calls are told apart by `index`.
#[test]
fn keeps_parallel_calls_separate_by_output_index() {
    let calls = accumulate(&[
        br#"data: {"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"a","name":"close_pane"}}
"#,
        br#"data: {"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","call_id":"b","name":"close_tab"}}
"#,
        br#"data: {"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"pane_id\":1}"}
"#,
        br#"data: {"type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"tab_id\":2}"}
"#,
    ]);

    assert_eq!(calls.len(), 2);
    assert_eq!(
        (calls[0].id.as_str(), calls[0].arguments.as_str()),
        ("a", r#"{"pane_id":1}"#)
    );
    assert_eq!(
        (calls[1].id.as_str(), calls[1].arguments.as_str()),
        ("b", r#"{"tab_id":2}"#)
    );
}

/// An output item that is not a call - a message, a reasoning item - must not
/// become an empty tool call.
#[test]
fn ignores_output_items_that_are_not_calls() {
    let calls = accumulate(&[
        br#"data: {"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs_1"}}
"#,
    ]);
    assert!(calls.is_empty());
}

#[test]
fn surfaces_a_failed_response() {
    let mut parser = SseParser::default();
    let items = parser.feed(
        br#"data: {"type":"response.failed","response":{"error":{"message":"context too long"}}}
"#,
    );
    assert_eq!(items, vec![SseItem::ApiError("context too long".into())]);
}

#[test]
fn surfaces_a_transport_error_event() {
    let mut parser = SseParser::default();
    let items = parser.feed(
        br#"data: {"type":"error","error":{"message":"upstream reset"}}
"#,
    );
    assert_eq!(items, vec![SseItem::ApiError("upstream reset".into())]);
}

/// The lifecycle bookends carry nothing hocus can show. They must produce
/// nothing at all - and, crucially, must not fall through to the
/// chat/completions arm, whose fields all default and so match any object.
#[test]
fn ignores_the_events_that_carry_nothing() {
    let mut parser = SseParser::default();
    for line in [
        r#"data: {"type":"response.created","response":{"id":"resp_1"}}"#,
        r#"data: {"type":"response.in_progress","response":{"id":"resp_1"}}"#,
        r#"data: {"type":"response.output_text.done","output_index":0,"text":"On it."}"#,
        r#"data: {"type":"response.content_part.added","output_index":0}"#,
    ] {
        assert!(
            parser.feed(format!("{line}\n").as_bytes()).is_empty(),
            "{line} produced items"
        );
    }
}

/// Both shapes share one parser, and the dispatch must not have cost the older
/// one anything.
#[test]
fn still_parses_a_chat_frame_alongside_a_responses_one() {
    let mut parser = SseParser::default();
    let mut items = parser.feed(FRAME);
    items.extend(parser.feed(
        br#"data: {"type":"response.output_text.delta","output_index":0,"delta":" there"}
"#,
    ));
    assert_eq!(deltas(items), vec!["Hello", " there"]);
}

// -- config -------------------------------------------------------------

#[test]
fn api_key_selects_the_endpoint() {
    let env = map(&[("OPENAI_API_KEY", "sk-test")]);
    let cfg = Config::resolve(&map(&[("api", "responses")]), &env).unwrap();
    assert_eq!(cfg.api, Api::Responses);
    assert_eq!(cfg.url, "https://api.openai.com/v1/responses");

    let cfg = Config::resolve(&BTreeMap::new(), &env).unwrap();
    assert_eq!(cfg.api, Api::Chat);
    assert_eq!(cfg.url, "https://api.openai.com/v1/chat/completions");
}

#[test]
fn an_unknown_api_is_a_config_error() {
    let env = map(&[("OPENAI_API_KEY", "sk-test")]);
    let error = Config::resolve(&map(&[("api", "grpc")]), &env).unwrap_err();
    assert!(
        error.message.contains("api must be one of"),
        "{}",
        error.message
    );
}

/// `none` is what OpenAI's chat/completions wants when tools are in play, so it
/// has to be settable.
#[test]
fn reasoning_effort_accepts_the_openai_values() {
    let env = map(&[("OPENAI_API_KEY", "sk-test")]);
    for effort in ["none", "minimal", "low", "high", "max"] {
        let cfg = Config::resolve(&map(&[("reasoning_effort", effort)]), &env).unwrap();
        assert_eq!(cfg.reasoning_effort.as_deref(), Some(effort));
    }
}
