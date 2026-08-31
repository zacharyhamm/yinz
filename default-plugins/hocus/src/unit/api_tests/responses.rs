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

/// `response.incomplete` carries no error object at all - it says why it stopped
/// under `incomplete_details`, and reading only `error` let a truncated answer
/// through as if the model had simply finished.
#[test]
fn surfaces_an_incomplete_response() {
    let mut parser = SseParser::default();
    let items = parser.feed(
        br#"data: {"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}
"#,
    );
    assert_eq!(items, vec![SseItem::ApiError("max_output_tokens".into())]);
}

/// A `type: "error"` frame spells its message as a bare top-level field, not as
/// the `{"error": {...}}` object a non-2xx body uses.
#[test]
fn surfaces_a_top_level_error_message() {
    let mut parser = SseParser::default();
    let items = parser.feed(
        br#"data: {"type":"error","code":"server_error","message":"upstream reset","param":null}
"#,
    );
    assert_eq!(items, vec![SseItem::ApiError("upstream reset".into())]);
}

/// A terminal failure must never come back empty, whatever shape it arrives in:
/// silence there reads as a clean stop and the user is shown nothing at all.
#[test]
fn a_failure_with_nothing_readable_still_says_something() {
    let mut parser = SseParser::default();
    for line in [
        r#"data: {"type":"response.failed","response":{}}"#,
        r#"data: {"type":"response.incomplete","response":{"incomplete_details":{}}}"#,
        r#"data: {"type":"error","message":"   "}"#,
    ] {
        let items = parser.feed(format!("{line}\n").as_bytes());
        assert!(
            matches!(items.as_slice(), [SseItem::ApiError(message)] if !message.is_empty()),
            "{line} produced {items:?}"
        );
    }
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

// -- reasoning items ----------------------------------------------------

fn sealed(id: &str, encrypted_content: &str, before: usize) -> ReasoningItem {
    ReasoningItem {
        id: id.to_owned(),
        encrypted_content: encrypted_content.to_owned(),
        before,
    }
}

/// The sealed content is asked for only when there is reasoning to seal, and
/// only when the echo is on - it is worth nothing to a model that produces none,
/// and worth nothing at all if it is never sent back.
#[test]
fn include_is_sent_only_with_reasoning_and_only_when_echoed() {
    let mut cfg = responses_config();
    assert!(responses_body(&cfg, &[Message::user("hi")])
        .get("include")
        .is_none());

    cfg.reasoning_effort = Some("medium".into());
    assert_eq!(
        responses_body(&cfg, &[Message::user("hi")])["include"],
        json!(["reasoning.encrypted_content"])
    );

    cfg.reasoning_echo = false;
    assert!(responses_body(&cfg, &[Message::user("hi")])
        .get("include")
        .is_none());
}

/// Reasoning is positioned against every replayable output item, including the
/// assistant message rather than only calls.
#[test]
fn captures_sealed_reasoning_items_in_position() {
    let mut parser = SseParser::default();
    let mut accumulator = ToolCallAccumulator::default();
    for line in [
        r#"data: {"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"rs_1","encrypted_content":"sealed-1"}}"#,
        r#"data: {"type":"response.output_item.added","output_index":1,"item":{"type":"message","role":"assistant"}}"#,
        r#"data: {"type":"response.output_item.done","output_index":2,"item":{"type":"reasoning","id":"rs_2","encrypted_content":"sealed-2"}}"#,
        r#"data: {"type":"response.output_item.added","output_index":3,"item":{"type":"function_call","call_id":"a","name":"close_pane"}}"#,
    ] {
        for item in parser.feed(format!("{line}\n").as_bytes()) {
            accumulator.feed(&item);
        }
    }

    let (reasoning, prose_before) = accumulator.take_replay();
    assert_eq!(
        reasoning,
        vec![sealed("rs_1", "sealed-1", 0), sealed("rs_2", "sealed-2", 1)]
    );
    assert_eq!(prose_before, Some(0));
    // And the items are not also mistaken for calls.
    assert_eq!(accumulator.take().len(), 1);
}

/// An item with no sealed content is not one that can be handed back - echoing a
/// half-formed one would break the pairing rule rather than satisfy it.
#[test]
fn ignores_a_reasoning_item_with_nothing_sealed() {
    let mut parser = SseParser::default();
    let mut accumulator = ToolCallAccumulator::default();
    for line in [
        r#"data: {"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"rs_1"}}"#,
        r#"data: {"type":"response.output_item.done","output_index":1,"item":{"type":"reasoning","id":"rs_2","encrypted_content":""}}"#,
        r#"data: {"type":"response.output_item.done","output_index":2,"item":{"type":"message"}}"#,
    ] {
        for item in parser.feed(format!("{line}\n").as_bytes()) {
            accumulator.feed(&item);
        }
    }
    assert_eq!(accumulator.take_replay(), (Vec::new(), None));
}

/// The correctness requirement: each item back in the position it came in. The
/// API rejects a reasoning item that is not followed by the item it produced.
#[test]
fn request_body_interleaves_reasoning_items_with_the_calls() {
    let mut cfg = responses_config();
    cfg.reasoning_effort = Some("medium".into());
    let mut asked = Message::tool_calls(vec![
        call("call_1", "close_pane", "{}"),
        call("call_2", "close_tab", "{}"),
    ]);
    asked.reasoning_items = vec![sealed("rs_1", "sealed-1", 0), sealed("rs_2", "sealed-2", 1)];
    let body = responses_body(&cfg, &[asked.clone()]);
    assert_eq!(
        body["input"],
        json!([
            {"type": "reasoning", "id": "rs_1", "encrypted_content": "sealed-1", "summary": []},
            {"type": "function_call", "call_id": "call_1", "name": "close_pane", "arguments": "{}"},
            {"type": "reasoning", "id": "rs_2", "encrypted_content": "sealed-2", "summary": []},
            {"type": "function_call", "call_id": "call_2", "name": "close_tab", "arguments": "{}"},
        ])
    );

    // The ordinary shape: one item, then everything it decided on.
    let mut one = asked.clone();
    one.reasoning_items = vec![sealed("rs_1", "sealed-1", 0)];
    let kinds: Vec<_> = responses_body(&cfg, &[one])["input"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["type"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(kinds, ["reasoning", "function_call", "function_call"]);

    // Same switch as the other transport turns the whole thing off.
    cfg.reasoning_echo = false;
    let body = responses_body(&cfg, &[asked]);
    assert_eq!(body["input"].as_array().map(Vec::len), Some(2));
}

/// A Responses turn may put prose between its sealed reasoning and function
/// calls. Stateless replay must keep all three in the emitted order.
#[test]
fn request_body_keeps_reasoning_before_assistant_prose() {
    let mut cfg = responses_config();
    cfg.reasoning_effort = Some("medium".into());
    let mut asked = Message::tool_calls(vec![call("call_1", "close_pane", "{}")]);
    asked.content = "I'll inspect it.".into();
    asked.prose_before = Some(0);
    asked.reasoning_items = vec![sealed("rs_1", "sealed-1", 0), sealed("rs_2", "sealed-2", 1)];

    assert_eq!(
        responses_body(&cfg, &[asked])["input"],
        json!([
            {"type": "reasoning", "id": "rs_1", "encrypted_content": "sealed-1", "summary": []},
            {"type": "message", "role": "assistant", "content": "I'll inspect it."},
            {"type": "reasoning", "id": "rs_2", "encrypted_content": "sealed-2", "summary": []},
            {"type": "function_call", "call_id": "call_1", "name": "close_pane", "arguments": "{}"},
        ])
    );
}

// -- config -------------------------------------------------------------

#[test]
fn api_key_selects_the_endpoint() {
    let env = map(&[("OPENAI_API_KEY", "sk-test")]);
    let cfg = Config::resolve(&map(&[("api", "responses")]), &env).unwrap();
    assert_eq!(cfg.api, Api::Responses);
    assert_eq!(cfg.url, "https://api.openai.com/v1/responses");

    let cfg = Config::resolve(&map(&[("api", "chat")]), &env).unwrap();
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

/// The built-in default `base_url` points at OpenAI's own endpoint, which
/// refuses `reasoning_effort` alongside `tools` on chat/completions - so an
/// unconfigured hocus should reach for the one transport that accepts both,
/// not silently drop the setting.
#[test]
fn openai_base_url_defaults_to_responses() {
    let env = map(&[("OPENAI_API_KEY", "sk-test")]);
    let cfg = Config::resolve(&BTreeMap::new(), &env).unwrap();
    assert_eq!(cfg.api, Api::Responses);
    assert_eq!(cfg.url, "https://api.openai.com/v1/responses");
}

/// The effort default needs all three of OpenAI, Responses and a model that
/// reasons. The built-in `DEFAULT_MODEL` is `gpt-4o-mini`, which rejects a
/// `reasoning` object outright, so an untouched configuration must send none.
#[test]
fn the_effort_default_needs_a_reasoning_model_on_responses() {
    let env = map(&[("OPENAI_API_KEY", "sk-test")]);

    let untouched = Config::resolve(&BTreeMap::new(), &env).unwrap();
    assert_eq!(untouched.model, DEFAULT_MODEL);
    assert_eq!(
        untouched.reasoning_effort, None,
        "gpt-4o-mini does not reason"
    );

    let reasoning = Config::resolve(&map(&[("model", "gpt-5")]), &env).unwrap();
    assert_eq!(reasoning.reasoning_effort.as_deref(), Some("medium"));

    // chat/completions is the pairing OpenAI's reasoning models 400 on, which is
    // the whole reason the Responses transport exists here.
    let chat = Config::resolve(&map(&[("model", "gpt-5"), ("api", "chat")]), &env).unwrap();
    assert_eq!(chat.reasoning_effort, None);

    // An explicit value always wins, in every combination
    // (`api_key_selects_the_endpoint` covers the same for `api`).
    for kdl in [
        vec![("reasoning_effort", "low")],
        vec![("reasoning_effort", "low"), ("model", "gpt-5")],
        vec![
            ("reasoning_effort", "low"),
            ("model", "gpt-5"),
            ("api", "chat"),
        ],
    ] {
        let cfg = Config::resolve(&map(&kdl), &env).unwrap();
        assert_eq!(cfg.reasoning_effort.as_deref(), Some("low"), "{kdl:?}");
    }
}

/// Everyone else - a compatible proxy, a local model, DeepSeek, OpenRouter -
/// keeps today's defaults: `reasoning_effort` and `/v1/responses` are OpenAI
/// specifics that another endpoint may well reject.
#[test]
fn non_openai_base_url_keeps_the_old_defaults() {
    let env = map(&[
        ("OPENAI_API_KEY", "sk-test"),
        ("OPENAI_BASE_URL", "http://localhost:11434"),
    ]);
    let cfg = Config::resolve(&BTreeMap::new(), &env).unwrap();
    assert_eq!(cfg.api, Api::Chat);
    assert_eq!(cfg.url, "http://localhost:11434/v1/chat/completions");
    assert_eq!(cfg.reasoning_effort, None);
}
