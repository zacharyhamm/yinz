//! One stateless model turn on somebody else's behalf: what goes out, and what comes back.
//!
//! Split from [`crate::llm`], which is the *endpoint* — where to send, with what
//! credentials, as which model — and from [`crate::agents`], whose bodies are about
//! describing an agent. This module serves stateless callers: everything the model sees arrives in one [`wire::CompletionRequest`] and
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
/// The number `agent`'s extension protocol uses for the same reason: a plugin should not be
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
/// `agent::api::WireMessage` draws.
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
#[path = "unit/completion.rs"]
mod tests;
