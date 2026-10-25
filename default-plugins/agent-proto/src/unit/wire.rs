use super::*;

/// What someone types at a shell: no `request_id`, because there is nothing
/// to correlate when the answer comes back down the same pipe.
#[test]
fn a_cli_mutation_parses_without_a_request_id() {
    let mutation: Mutation =
        serde_json::from_str(r#"{"op":"project_remember","args":{"name":"work","root":"/work"}}"#)
            .expect("parsed");
    assert_eq!(mutation.op, "project_remember");
    assert!(mutation.request_id.is_empty());
    assert_eq!(mutation.args["root"], "/work");
}

/// Same for a query — and a plugin's query keeps the id it sent, which is
/// the only thing that lets the reply find its way back to the right call.
#[test]
fn a_query_carries_a_request_id_only_when_one_was_sent() {
    let typed: Query = serde_json::from_str(r#"{"op":"project_list","args":{}}"#).expect("parsed");
    assert!(typed.request_id.is_empty());

    let from_plugin: Query =
        serde_json::from_str(r#"{"request_id":"call_7","op":"project_list","args":{}}"#)
            .expect("parsed");
    assert_eq!(from_plugin.request_id, "call_7");
}

#[test]
fn a_completion_request_parses_with_nothing_but_its_messages() {
    let request: CompletionRequest =
        serde_json::from_str(r#"{"messages":[{"role":"user","content":"why?"}]}"#).expect("parsed");
    assert!(request.request_id.is_empty());
    assert_eq!(request.messages.len(), 1);
    assert_eq!(request.messages[0].role, "user");
    assert!(request.messages[0].tool_calls.is_empty());
    assert_eq!(request.messages[0].tool_call_id, None);
    assert_eq!(request.system, None);
    assert_eq!(request.model, None);
    assert!(request.tools.is_empty());
    assert_eq!(request.tool_choice, None);
    assert_eq!(request.temperature, None);
    assert_eq!(request.max_tokens, None);
}

/// The round a caller's own tool loop sends back: the assistant's calls, then a result
/// naming the call it answers.
#[test]
fn a_completion_request_round_trips_a_tool_round() {
    let request = CompletionRequest {
        request_id: "ts:hc1".to_owned(),
        messages: vec![
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
        ],
        tools: vec![serde_json::json!({ "type": "function" })],
        tool_choice: Some(serde_json::json!("auto")),
        temperature: Some(0.5),
        max_tokens: Some(256),
        ..Default::default()
    };
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("../completion-fixtures.json")).unwrap();
    assert_eq!(serde_json::to_value(&request).unwrap(), fixtures["request"]);
    let json = serde_json::to_string(&request).expect("encoded");
    let back: CompletionRequest = serde_json::from_str(&json).expect("parsed");
    assert_eq!(back.request_id, "ts:hc1");
    assert_eq!(
        back.messages[0].tool_calls[0].arguments,
        r#"{"city":"Oslo"}"#
    );
    assert_eq!(back.messages[1].tool_call_id.as_deref(), Some("call_1"));
    assert_eq!(back.tool_choice, Some(serde_json::json!("auto")));
    assert_eq!(back.temperature, Some(0.5));
    assert_eq!(back.max_tokens, Some(256));
}

#[test]
fn a_completion_reply_round_trips_with_tool_calls_and_usage() {
    let reply = CompletionReply {
        request_id: "ts:hc1".to_owned(),
        content: "here".to_owned(),
        tool_calls: vec![CompletionToolCall {
            id: "call_1".to_owned(),
            name: "lookup".to_owned(),
            arguments: "{}".to_owned(),
        }],
        finish_reason: "tool_calls".to_owned(),
        usage: Some(CompletionUsage {
            prompt_tokens: 38,
            completion_tokens: 24,
            total_tokens: 62,
        }),
        model: "gpt-5".to_owned(),
        error: String::new(),
    };
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("../completion-fixtures.json")).unwrap();
    assert_eq!(serde_json::to_value(&reply).unwrap(), fixtures["reply"]);
    let json = serde_json::to_string(&reply).expect("encoded");
    let back: CompletionReply = serde_json::from_str(&json).expect("parsed");
    assert_eq!(back.request_id, "ts:hc1");
    assert_eq!(back.tool_calls, reply.tool_calls);
    assert_eq!(back.finish_reason, "tool_calls");
    assert_eq!(back.usage.unwrap().total_tokens, 62);
    assert_eq!(back.model, "gpt-5");
    // Success is the empty error, which is what a caller branches on.
    assert!(back.error.is_empty());
}

/// A provider that echoes no usage, and a caller that must not need one on the wire to
/// read the answer.
#[test]
fn a_completion_reply_parses_without_its_optional_halves() {
    let reply: CompletionReply =
        serde_json::from_str(r#"{"request_id":"x","content":"hi"}"#).expect("parsed");
    assert_eq!(reply.content, "hi");
    assert!(reply.tool_calls.is_empty());
    assert!(reply.finish_reason.is_empty());
    assert_eq!(reply.usage, None);
    assert!(reply.error.is_empty());
}
