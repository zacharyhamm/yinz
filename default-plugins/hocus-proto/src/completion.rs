//! One stateless model turn on somebody else's behalf: what goes out, and what comes back.
//!
//! Split from [`crate::llm`], which is the *endpoint* — where to send, with what
//! credentials, as which model — and from [`crate::agents`], whose bodies are about
//! describing an agent. This module is the conversation half for a caller that has no
//! conversation: everything the model sees arrives in one [`wire::CompletionRequest`] and
//! nothing is kept between requests.
//!
//! That is what lets a plugin drive its own tool loop. It sends messages and tool specs,
//! gets `tool_calls` back, runs them itself, and sends the whole thing again with the
//! results appended — the daemon serving this holds no transcript to get out of step with.
//!
//! Nothing here calls the Yinz host, so all of it runs on the host triple.

use serde_json::{json, Value};

use crate::llm;
use crate::wire::{
    CompletionMessage, CompletionReply, CompletionRequest, CompletionToolCall, CompletionUsage,
};

/// The whole serialized request, in bytes.
///
/// The number `hocus`'s extension protocol uses for the same reason: a plugin should not be
/// able to put a megabyte through the user's key by accident, and a caller that genuinely
/// needs more is asking the wrong thing of a one-shot completion.
pub const MAX_REQUEST_BYTES: usize = 64_000;
/// How many messages one turn may carry. A caller looping on its own tool results grows
/// this transcript itself, and one that has stopped noticing is what the cap is for.
pub const MAX_MESSAGES: usize = 200;
pub const MAX_TOOLS: usize = 64;

/// The roles the endpoint understands. A caller that misspells one gets told so here rather
/// than by a provider error three seconds later.
const ROLES: [&str; 4] = ["system", "user", "assistant", "tool"];

/// Why a payload is too big to even parse, or `Ok(())`.
///
/// Checked on the JSON as it arrived rather than on the parsed request, so a caller that
/// sends a megabyte pays for one length comparison rather than a megabyte of `serde_json`.
/// [`validate`] is the rest of the refusals, on the request itself.
pub fn within_size(payload: &str) -> Result<(), String> {
    if payload.len() > MAX_REQUEST_BYTES {
        return Err(format!(
            "a completion request is at most {MAX_REQUEST_BYTES} bytes, and this one is {}",
            payload.len()
        ));
    }
    Ok(())
}

/// Why this request cannot go out, or `Ok(())`.
///
/// Every reason is one the caller can fix and must be told about, so these are sentences
/// that go straight into [`wire::CompletionReply::error`] rather than a silent refusal.
pub fn validate(request: &CompletionRequest) -> Result<(), String> {
    if request.messages.is_empty() && request.system.is_none() {
        return Err("a completion needs at least one message".to_owned());
    }
    if request.messages.len() > MAX_MESSAGES {
        return Err(format!(
            "a completion carries at most {MAX_MESSAGES} messages, and this one has {}",
            request.messages.len()
        ));
    }
    if request.tools.len() > MAX_TOOLS {
        return Err(format!(
            "a completion offers at most {MAX_TOOLS} tools, and this one has {}",
            request.tools.len()
        ));
    }
    for message in &request.messages {
        if !ROLES.contains(&message.role.as_str()) {
            return Err(format!(
                "\"{}\" is not a role; expected one of {}",
                message.role,
                ROLES.join(", ")
            ));
        }
        // A tool result the endpoint cannot attribute is rejected by every provider, and
        // the message it comes back with names neither the call nor the caller.
        if message.role == "tool" && message.tool_call_id.is_none() {
            return Err("a tool message needs the tool_call_id it answers".to_owned());
        }
    }
    Ok(())
}

/// The body of one non-streaming completion.
///
/// `"stream": false`, and — like [`crate::agents::summary_request_body`] — no `temperature`
/// and no `max_tokens` unless the caller named them: providers disagree about both, newer
/// OpenAI models reject a `temperature` that is not 1 and want `max_completion_tokens`, and
/// a rejected request buys nothing over a slightly long answer.
///
/// `model` is passed in rather than read off the request because who chooses it is not this
/// module's business: the session may have pinned one.
pub fn completion_request_body(request: &CompletionRequest, model: &str) -> Vec<u8> {
    let mut messages: Vec<Value> = Vec::with_capacity(request.messages.len() + 1);
    if let Some(system) = &request.system {
        messages.push(json!({ "role": "system", "content": system }));
    }
    messages.extend(request.messages.iter().map(wire_message));

    let mut body = json!({
        "model": model,
        "stream": false,
        "messages": messages,
    });
    let object = body.as_object_mut().expect("a json! object is an object");
    if !request.tools.is_empty() {
        object.insert("tools".to_owned(), Value::Array(request.tools.clone()));
        if let Some(choice) = &request.tool_choice {
            object.insert("tool_choice".to_owned(), choice.clone());
        }
    }
    if let Some(temperature) = request.temperature {
        object.insert("temperature".to_owned(), json!(temperature));
    }
    if let Some(max_tokens) = request.max_tokens {
        object.insert("max_tokens".to_owned(), json!(max_tokens));
    }
    serde_json::to_vec(&body).expect("serializing a completion request cannot fail")
}

/// One message, in whichever of the three shapes the endpoint expects.
///
/// The shapes disagree about what "absent" means, which is why this is a match rather than
/// one object of optional keys: a tool result omitting `tool_call_id` is malformed and must
/// send `null`, while a plain message must not carry the key at all. Same distinction
/// `hocus::api::WireMessage` draws.
fn wire_message(message: &CompletionMessage) -> Value {
    if message.role == "tool" {
        return json!({
            "role": "tool",
            "tool_call_id": message.tool_call_id,
            "content": message.content,
        });
    }
    if message.tool_calls.is_empty() {
        return json!({ "role": message.role, "content": message.content });
    }
    // Providers require `content` beside tool calls even when it is empty.
    json!({
        "role": message.role,
        "content": message.content,
        "tool_calls": message
            .tool_calls
            .iter()
            .map(|call| json!({
                "id": call.id,
                "type": "function",
                "function": { "name": call.name, "arguments": call.arguments },
            }))
            .collect::<Vec<_>>(),
    })
}

/// What the endpoint answered, always as a reply the caller can read.
///
/// Never an `Option`, unlike [`crate::agents::parse_summary`]: that one answers a background
/// caller which can simply ask again next time, and this one answers somebody blocked on it,
/// for whom "nothing" is the single outcome that cannot be acted on. So every failure is a
/// reply with `error` set — a non-2xx through [`llm::api_error_message`], and a body that is
/// not a completion in its own words.
pub fn parse_completion(status: u16, body: &[u8], request_id: &str) -> CompletionReply {
    if !(200..300).contains(&status) {
        return error_reply(request_id, &llm::api_error_message(status, body));
    }
    let Ok(response) = serde_json::from_slice::<Value>(body) else {
        return error_reply(
            request_id,
            "the endpoint answered something that is not JSON",
        );
    };
    let Some(choice) = response.pointer("/choices/0") else {
        // A 2xx with no choices is what a provider sends for a content filter or an empty
        // completion, and its own `error` object is the only thing that says which.
        let detail = response
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("the endpoint answered no completion");
        return error_reply(request_id, detail);
    };

    let content = choice
        .pointer("/message/content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    CompletionReply {
        request_id: request_id.to_owned(),
        content: content.to_owned(),
        tool_calls: tool_calls(choice),
        finish_reason: choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        usage: usage(&response),
        model: response
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        error: String::new(),
    }
}

/// A reply that never left the daemon: a refusal, a timeout, a missing endpoint.
pub fn error_reply(request_id: &str, detail: &str) -> CompletionReply {
    CompletionReply {
        request_id: request_id.to_owned(),
        error: detail.to_owned(),
        ..Default::default()
    }
}

/// The calls the model asked for, dropping any that names no function.
///
/// A malformed `arguments` string is kept as it stands rather than dropped — see
/// [`CompletionToolCall::arguments`]; a call with no `name` is not a call at all.
fn tool_calls(choice: &Value) -> Vec<CompletionToolCall> {
    let Some(calls) = choice
        .pointer("/message/tool_calls")
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    calls
        .iter()
        .filter_map(|call| {
            let name = call.pointer("/function/name").and_then(Value::as_str)?;
            Some(CompletionToolCall {
                id: call
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                name: name.to_owned(),
                arguments: call
                    .pointer("/function/arguments")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            })
        })
        .collect()
}

/// What the turn cost, when the provider said. `None` rather than zeroes for one that did
/// not, so a caller counting tokens can tell "free" from "not reported".
fn usage(response: &Value) -> Option<CompletionUsage> {
    let usage = response.get("usage")?.as_object()?;
    let field = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or_default();
    Some(CompletionUsage {
        prompt_tokens: field("prompt_tokens"),
        completion_tokens: field("completion_tokens"),
        total_tokens: field("total_tokens"),
    })
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
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

    /// The provider's own words, through the same helper the chat turn shows a user.
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
        let reply = error_reply("hc1", "hocus has no API key configured");
        assert_eq!(reply.request_id, "hc1");
        assert_eq!(reply.error, "hocus has no API key configured");
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
}
