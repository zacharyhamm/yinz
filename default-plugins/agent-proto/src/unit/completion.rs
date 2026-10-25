use super::*;

fn user(content: &str) -> CompletionMessage {
    CompletionMessage {
        role: "user".to_owned(),
        content: content.to_owned(),
        ..Default::default()
    }
}

fn request(messages: Vec<CompletionMessage>) -> CompletionRequest {
    CompletionRequest {
        messages,
        ..Default::default()
    }
}

fn body_of(request: &CompletionRequest) -> Value {
    serde_json::from_slice(&completion_request_body(request, "gpt-5")).expect("parsed")
}

#[test]
fn a_body_names_the_model_it_was_given_and_never_streams() {
    let body = body_of(&request(vec![user("why?")]));
    assert_eq!(body["model"], "gpt-5");
    assert_eq!(body["stream"], false);
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][0]["content"], "why?");
}

/// The convenience the shell one-liner wants: a `system` that is not a message.
#[test]
fn a_system_prompt_leads_the_messages() {
    let mut request = request(vec![user("why?")]);
    request.system = Some("You are terse.".to_owned());
    let body = body_of(&request);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][0]["content"], "You are terse.");
    assert_eq!(body["messages"][1]["role"], "user");
}

/// Providers disagree about both, so neither appears unless it was asked for by name.
#[test]
fn temperature_and_max_tokens_are_absent_until_the_caller_names_them() {
    let body = body_of(&request(vec![user("why?")]));
    assert!(body.get("temperature").is_none());
    assert!(body.get("max_tokens").is_none());

    let mut request = request(vec![user("why?")]);
    request.temperature = Some(0.2);
    request.max_tokens = Some(64);
    let body = body_of(&request);
    assert_eq!(body["temperature"], 0.2);
    assert_eq!(body["max_tokens"], 64);
}

/// The specs are the caller's, and travel untouched.
#[test]
fn tools_are_passed_through_verbatim_with_their_choice() {
    let spec = json!({
        "type": "function",
        "function": { "name": "lookup", "parameters": { "type": "object" } },
    });
    let mut request = request(vec![user("where?")]);
    request.tools = vec![spec.clone()];
    request.tool_choice = Some(json!("required"));
    let body = body_of(&request);
    assert_eq!(body["tools"][0], spec);
    assert_eq!(body["tool_choice"], "required");
}

/// No tools means no `tools` key at all — and then no `tool_choice` either, which
/// providers reject on a request that offers nothing to choose from.
#[test]
fn a_tool_choice_without_tools_is_left_off() {
    let mut request = request(vec![user("why?")]);
    request.tool_choice = Some(json!("required"));
    let body = body_of(&request);
    assert!(body.get("tools").is_none());
    assert!(body.get("tool_choice").is_none());
}

/// The round a caller's own tool loop sends back, in the two shapes that disagree about
/// what an absent key means.
#[test]
fn a_tool_round_is_nested_the_way_the_endpoint_expects() {
    let body = body_of(&request(vec![
        CompletionMessage {
            role: "assistant".to_owned(),
            tool_calls: vec![CompletionToolCall {
                id: "call_1".to_owned(),
                name: "lookup".to_owned(),
                arguments: r#"{"city":"Oslo"}"#.to_owned(),
            }],
            ..Default::default()
        },
        CompletionMessage {
            role: "tool".to_owned(),
            content: "clear".to_owned(),
            tool_call_id: Some("call_1".to_owned()),
            ..Default::default()
        },
    ]));
    let call = &body["messages"][0];
    assert_eq!(call["role"], "assistant");
    // Required beside tool calls even when empty.
    assert_eq!(call["content"], "");
    assert_eq!(call["tool_calls"][0]["id"], "call_1");
    assert_eq!(call["tool_calls"][0]["type"], "function");
    assert_eq!(call["tool_calls"][0]["function"]["name"], "lookup");
    // The raw string the model wrote, not a re-encoded object.
    assert_eq!(
        call["tool_calls"][0]["function"]["arguments"],
        r#"{"city":"Oslo"}"#
    );

    let result = &body["messages"][1];
    assert_eq!(result["tool_call_id"], "call_1");
    assert!(result.get("tool_calls").is_none());
}

/// A plain message must not carry the key at all, where a tool result must send `null`.
#[test]
fn a_plain_message_carries_no_tool_keys() {
    let body = body_of(&request(vec![user("why?")]));
    assert!(body["messages"][0].get("tool_call_id").is_none());
    assert!(body["messages"][0].get("tool_calls").is_none());
}

#[test]
fn a_completion_is_parsed_out_of_an_ordinary_answer() {
    let body = br#"{
            "model": "gpt-5",
            "choices": [{ "finish_reason": "stop", "message": { "content": "because." } }],
            "usage": { "prompt_tokens": 38, "completion_tokens": 24, "total_tokens": 62 }
        }"#;
    let reply = parse_completion(200, body, "hc1");
    assert_eq!(reply.request_id, "hc1");
    assert_eq!(reply.content, "because.");
    assert_eq!(reply.finish_reason, "stop");
    assert_eq!(reply.model, "gpt-5");
    assert_eq!(reply.usage.unwrap().total_tokens, 62);
    assert!(reply.error.is_empty());
    assert!(reply.tool_calls.is_empty());
}

#[test]
fn tool_calls_are_flattened_and_their_arguments_left_as_written() {
    let body = br#"{
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "content": null,
                    "tool_calls": [
                        { "id": "call_1", "type": "function",
                          "function": { "name": "lookup", "arguments": "{\"city\":\"Oslo\"}" } },
                        { "id": "call_2", "type": "function",
                          "function": { "name": "broken", "arguments": "{not json" } }
                    ]
                }
            }]
        }"#;
    let reply = parse_completion(200, body, "hc1");
    assert_eq!(reply.finish_reason, "tool_calls");
    // A null content is the ordinary shape of a tool-calling turn, not a failure.
    assert!(reply.content.is_empty());
    assert!(reply.error.is_empty());
    assert_eq!(reply.tool_calls.len(), 2);
    assert_eq!(reply.tool_calls[0].name, "lookup");
    assert_eq!(reply.tool_calls[0].arguments, r#"{"city":"Oslo"}"#);
    // Kept, not dropped: the caller has to be told the model wrote nonsense.
    assert_eq!(reply.tool_calls[1].arguments, "{not json");
}

#[test]
fn a_call_naming_no_function_is_not_a_call() {
    let body = br#"{"choices":[{"message":{"tool_calls":[{"id":"call_1"}]}}]}"#;
    assert!(parse_completion(200, body, "hc1").tool_calls.is_empty());
}

#[test]
fn a_truncated_answer_keeps_the_reason_it_was_truncated() {
    let body = br#"{"choices":[{"finish_reason":"length","message":{"content":"half a"}}]}"#;
    let reply = parse_completion(200, body, "hc1");
    assert_eq!(reply.finish_reason, "length");
    assert_eq!(reply.content, "half a");
}

/// `None` rather than zeroes, so a caller can tell "free" from "not reported".
#[test]
fn a_provider_that_reports_no_usage_reports_none() {
    let body = br#"{"choices":[{"message":{"content":"hi"}}]}"#;
    assert_eq!(parse_completion(200, body, "hc1").usage, None);
    let body = br#"{"choices":[{"message":{"content":"hi"}}],"usage":null}"#;
    assert_eq!(parse_completion(200, body, "hc1").usage, None);
}

/// The provider's own words, through the shared error helper.
#[test]
fn a_rate_limit_comes_back_as_an_error_in_the_providers_words() {
    let body = br#"{"error":{"message":"Rate limit reached. Try again in 2s"}}"#;
    let reply = parse_completion(429, body, "hc1");
    assert_eq!(reply.request_id, "hc1");
    assert!(reply.error.contains("429"));
    assert!(reply.error.contains("Rate limit reached"));
    assert!(reply.content.is_empty());
}

#[test]
fn a_body_that_is_not_json_is_an_error_rather_than_a_panic() {
    let reply = parse_completion(200, b"<html>gateway</html>", "hc1");
    assert!(reply.error.contains("not JSON"));
}

/// A 2xx with no choices: the provider's own message when there is one, and a sentence
/// of ours when there is not.
#[test]
fn an_answer_with_no_choices_says_so() {
    let reply = parse_completion(200, br#"{"choices":[]}"#, "hc1");
    assert_eq!(reply.error, "the endpoint answered no completion");

    let reply = parse_completion(200, br#"{"error":{"message":"content filtered"}}"#, "hc1");
    assert_eq!(reply.error, "content filtered");
}

#[test]
fn long_answers_are_preserved_exactly() {
    for long in ["a".repeat(64_001), "é".repeat(64_001)] {
        let body = serde_json::to_vec(&json!({
            "choices": [{ "message": { "content": long } }],
        }))
        .unwrap();
        let reply = parse_completion(200, &body, "hc1");
        assert_eq!(reply.content, long);
        assert_eq!(error_reply("hc1", &long).error, long);
    }
}

#[test]
fn an_error_reply_carries_nothing_but_the_reason() {
    let reply = error_reply("hc1", "agent has no API key configured");
    assert_eq!(reply.request_id, "hc1");
    assert_eq!(reply.error, "agent has no API key configured");
    assert!(reply.content.is_empty());
    assert!(reply.tool_calls.is_empty());
    assert_eq!(reply.usage, None);
}

#[test]
fn a_payload_too_big_to_parse_is_refused_by_its_length_alone() {
    assert!(within_size("{}").is_ok());
    let huge = "x".repeat(MAX_REQUEST_BYTES + 1);
    assert!(within_size(&huge)
        .expect_err("refused")
        .contains("at most 64000 bytes"));
}

#[test]
fn an_empty_request_is_refused() {
    let error = validate(&request(vec![])).expect_err("refused");
    assert!(error.contains("at least one message"));
    // A system prompt alone is a question, though an odd one.
    let mut only_system = request(vec![]);
    only_system.system = Some("say hello".to_owned());
    assert!(validate(&only_system).is_ok());
}

#[test]
fn too_many_messages_or_tools_are_refused() {
    let long = request(vec![user("why?"); MAX_MESSAGES + 1]);
    assert!(validate(&long)
        .expect_err("refused")
        .contains("at most 200 messages"));

    let mut many = request(vec![user("why?")]);
    many.tools = vec![json!({}); MAX_TOOLS + 1];
    assert!(validate(&many)
        .expect_err("refused")
        .contains("at most 64 tools"));
}

#[test]
fn a_misspelled_role_is_refused_here_rather_than_by_the_provider() {
    let error = validate(&request(vec![CompletionMessage {
        role: "used".to_owned(),
        content: "why?".to_owned(),
        ..Default::default()
    }]))
    .expect_err("refused");
    assert!(error.contains("\"used\" is not a role"));
}

#[test]
fn a_tool_result_with_no_call_to_answer_is_refused() {
    let error = validate(&request(vec![CompletionMessage {
        role: "tool".to_owned(),
        content: "clear".to_owned(),
        ..Default::default()
    }]))
    .expect_err("refused");
    assert!(error.contains("tool_call_id"));
}

#[test]
fn an_ordinary_request_is_accepted() {
    let mut request = request(vec![user("why?")]);
    request.tools = vec![json!({ "type": "function" })];
    assert!(validate(&request).is_ok());
}
