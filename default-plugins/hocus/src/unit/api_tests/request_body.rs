use super::*;

#[test]
fn request_body_always_streams() {
    let body = body_value(&test_config(), &[]);
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["model"], json!("gpt-4o-mini"));
}

#[test]
fn request_body_prepends_the_system_prompt_when_set() {
    let mut cfg = test_config();
    cfg.system_prompt = Some("be terse".into());
    let body = body_value(&cfg, &[Message::user("hi")]);
    assert_eq!(
        body["messages"],
        json!([
            {"role": "system", "content": "be terse"},
            {"role": "user", "content": "hi"},
        ])
    );
}

#[test]
fn request_body_adds_the_tool_first_policy_when_tools_are_enabled() {
    let mut cfg = test_config();
    cfg.tools = true;
    let body = body_value(&cfg, &[Message::user("move pane 3 left")]);
    assert_eq!(
        body["messages"][0],
        json!({"role": "system", "content": TOOL_FIRST_PROMPT})
    );
}

#[test]
fn hocus_prompt_contract_keeps_global_routing_rules() {
    assert_eq!(
        TOOL_FIRST_PROMPT,
        "You are Hocus. Use native Zellij tools. Before ID actions, call `list_session` unless the prior result gave IDs; use `list_other_sessions` for cross-session IDs. `warp_tab`/`warp_pane` move current items out; `warp_tab_here`/`warp_pane_here` move listed items in. Use only `tab_id`/`pane_id`, never `tab_number`. Do not verify mutations or retry failed pane moves. `send_input` controls running programs. No arbitrary commands. Active-project code: `find_files`/`search_files`/`read_file`; history: `git_log`/`git_show`; GitHub metadata: GET-only `github_read`. Call `agent_list` before agent actions. Make independent calls together."
    );
}

#[test]
fn request_body_appends_the_configured_prompt_to_the_tool_policy() {
    let mut cfg = test_config();
    cfg.tools = true;
    cfg.system_prompt = Some("be terse".into());
    let body = body_value(&cfg, &[Message::user("hi")]);
    assert_eq!(
        body["messages"][0]["content"],
        json!(format!("{TOOL_FIRST_PROMPT}\n\nbe terse"))
    );
    assert_eq!(
        body["messages"].as_array().unwrap().len(),
        2,
        "policy and custom instructions share one system message"
    );
}

#[test]
fn request_body_omits_the_system_prompt_when_unset() {
    let body = body_value(&test_config(), &[Message::user("hi")]);
    assert_eq!(body["messages"], json!([{"role": "user", "content": "hi"}]));
}

#[test]
fn request_body_excludes_our_own_error_and_notice_lines() {
    let transcript = [
        Message::user("hi"),
        Message::error("HTTP 401: nope"),
        Message::notice("cancelled"),
    ];
    let body = body_value(&test_config(), &transcript);
    assert_eq!(body["messages"], json!([{"role": "user", "content": "hi"}]));
}

#[test]
fn request_body_excludes_the_empty_streaming_placeholder() {
    let transcript = [
        Message::user("hi"),
        Message::assistant("hello"),
        Message::user("again"),
        Message::assistant(""), // the placeholder for the reply we are asking for
    ];
    let body = body_value(&test_config(), &transcript);
    assert_eq!(
        body["messages"],
        json!([
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": "hello"},
            {"role": "user", "content": "again"},
        ])
    );
}

#[test]
fn request_body_advertises_the_tools_when_they_are_enabled() {
    let mut cfg = test_config();
    cfg.tools = true;
    let body = body_value(&cfg, &[Message::user("hi")]);
    assert_eq!(body["tools"].as_array().unwrap().len(), 57);
}

/// The tool payload is paid for on every turn, so it is capped rather than left to
/// drift. The cap was 20kb and the payload had grown to sit just under it, which made
/// *any* new tool fail this test rather than only an extravagant one. Raised to 21kb
/// when `set_global_pane_placement` was added — a ceiling with no headroom stops
/// measuring growth and starts forbidding it. Trim descriptions before raising it again.
#[test]
fn hocus_prompt_contract_keeps_the_tool_payload_under_21_kb() {
    let mut cfg = test_config();
    cfg.tools = true;
    let body = request_body(&cfg, &[Message::user("hi")], true);
    assert!(
        body.len() < 21_000,
        "tool payload grew to {} bytes",
        body.len()
    );
}

#[test]
fn request_body_adds_the_active_project_to_the_system_message() {
    let body: serde_json::Value = serde_json::from_slice(&request_body_with_project(
        &test_config(),
        &[Message::user("hi")],
        true,
        Some(("hocus", "/work/hocus")),
    ))
    .unwrap();
    assert_eq!(
        body["messages"][0],
        json!({"role": "system", "content": "Active project: hocus. Project root: /work/hocus."})
    );
}

#[test]
fn request_body_omits_the_tools_when_they_are_disabled() {
    let body = body_value(&test_config(), &[Message::user("hi")]);
    assert!(body.get("tools").is_none());
}

/// The last round of the tool budget: the tools are withheld even though the
/// config enables them, and the prompt says why so the model answers rather
/// than claiming it cannot proceed.
#[test]
fn request_body_omits_the_tools_on_the_final_round() {
    let mut cfg = test_config();
    cfg.tools = true;
    let body: serde_json::Value =
        serde_json::from_slice(&request_body(&cfg, &[Message::user("hi")], false)).unwrap();
    assert!(body.get("tools").is_none());
    let system = body["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("budget for this turn is spent"), "{system}");
}

#[test]
fn request_body_sends_tool_calls_rather_than_their_transcript_summary() {
    // This is the archived shape from before call summaries moved to `display`.
    let mut asked = Message::tool_calls(vec![call("call_1", "new_pane", r#"{"command":"htop"}"#)]);
    asked.content = asked.display.take().unwrap();
    let transcript = [
        Message::user("open htop"),
        asked,
        Message::tool_result("call_1", r#"{"opened":true,"pane_id":7}"#),
    ];
    let body = body_value(&test_config(), &transcript);
    assert_eq!(
        body["messages"],
        json!([
            {"role": "user", "content": "open htop"},
            {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "new_pane", "arguments": r#"{"command":"htop"}"#},
                }],
            },
            {"role": "tool", "tool_call_id": "call_1", "content": r#"{"opened":true,"pane_id":7}"#},
        ])
    );
}

#[test]
fn request_body_keeps_tool_call_prose_on_the_same_assistant_message() {
    let mut asked = Message::tool_calls(vec![call("call_1", "list_session", "{}")]);
    asked.content = "I'll inspect the session.".into();
    let body = body_value(&test_config(), &[asked]);

    assert_eq!(
        body["messages"][0],
        json!({
            "role": "assistant",
            "content": "I'll inspect the session.",
            "tool_calls": [{
                "id": "call_1",
                "type": "function",
                "function": {"name": "list_session", "arguments": "{}"},
            }],
        })
    );
}

/// The whole point of `Message::display` being display-only: a tool the
/// user typed is *shown* as prose and still *sent* as the raw result the
/// model's ids and follow-up calls depend on.
#[test]
fn request_body_sends_the_raw_result_not_its_presentation() {
    let transcript = [
        Message::tool_calls(vec![call("call_1", "new_pane", "{}")]),
        Message::tool_result_shown(
            "call_1",
            r#"{"opened":true,"pane_id":7}"#,
            Some("Opened pane 7.".to_owned()),
        ),
    ];
    let body = body_value(&test_config(), &transcript);
    assert_eq!(
        body["messages"][1],
        json!({
            "role": "tool",
            "tool_call_id": "call_1",
            "content": r#"{"opened":true,"pane_id":7}"#,
        })
    );
    assert!(
        !String::from_utf8_lossy(&request_body(&test_config(), &transcript, true))
            .contains("Opened pane 7"),
        "the presentation must not reach the model"
    );
}

/// A no-argument call streams as `""` from some providers, and echoing that
/// back is a 400: "arguments must be valid json".
#[test]
fn unusable_tool_call_arguments_are_sent_as_an_empty_object() {
    for arguments in ["", "   ", "{not json", "[1,2]"] {
        let transcript = [
            Message::tool_calls(vec![call("call_1", "list_session", arguments)]),
            Message::tool_result("call_1", "{}"),
        ];
        let body = body_value(&test_config(), &transcript);
        assert_eq!(
            body["messages"][0]["tool_calls"][0]["function"]["arguments"],
            json!("{}"),
            "arguments {arguments:?}"
        );
    }
}

/// The API rejects a transcript where a requested call has no reply, so a
/// declined tool must still produce one.
#[test]
fn every_requested_call_is_answered_even_when_declined() {
    let transcript = [
        Message::user("close everything"),
        Message::tool_calls(vec![
            call("call_1", "close_pane", r#"{"pane_id":1,"is_plugin":false}"#),
            call("call_2", "close_pane", r#"{"pane_id":2,"is_plugin":false}"#),
        ]),
        Message::tool_result("call_1", "declined by the user"),
        Message::tool_result("call_2", "declined by the user"),
    ];
    let body = body_value(&test_config(), &transcript);
    let messages = body["messages"].as_array().unwrap();

    let requested = messages
        .iter()
        .filter_map(|m| m["tool_calls"].as_array())
        .flatten()
        .count();
    let answered = messages.iter().filter(|m| m["role"] == "tool").count();
    assert_eq!(requested, 2);
    assert_eq!(answered, requested);
}

/// An empty result is still an answer; skipping it would desynchronise the
/// call/result pairing that the previous test guards.
#[test]
fn an_empty_tool_result_is_still_sent() {
    let transcript = [Message::tool_result("call_1", "")];
    let body = body_value(&test_config(), &transcript);
    assert_eq!(
        body["messages"],
        json!([{"role": "tool", "tool_call_id": "call_1", "content": ""}])
    );
}

/// Every message shape in one transcript, pinned against the literal wire
/// form. Each is built by its own arm, and they disagree about what an
/// absent field looks like: the tool-call message must send `content: null`,
/// while the plain ones must not carry `tool_calls` at all.
#[test]
fn request_body_writes_each_message_shape_whole() {
    let transcript = [
        Message::user("open htop"),
        Message::assistant("On it."),
        Message::tool_calls(vec![call("call_1", "new_pane", r#"{"command":"htop"}"#)]),
        Message::tool_result("call_1", "opened"),
    ];
    assert_eq!(
        body_value(&test_config(), &transcript)["messages"],
        json!([
            {"role": "user", "content": "open htop"},
            {"role": "assistant", "content": "On it."},
            {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "new_pane", "arguments": r#"{"command":"htop"}"#},
                }],
            },
            {"role": "tool", "tool_call_id": "call_1", "content": "opened"},
        ])
    );
}

/// A tool result carries `tool_call_id: Option<String>`, and a `None` has to
/// go out as `null` rather than as an empty string or a missing key — the
/// endpoint rejects a tool message with no id field at all, and `""` would
/// claim to answer a call named `""`.
#[test]
fn a_tool_result_with_no_call_id_sends_null_rather_than_dropping_the_field() {
    let transcript = [Message {
        role: Role::Tool,
        content: "orphaned".to_owned(),
        ..Default::default()
    }];
    assert_eq!(
        body_value(&test_config(), &transcript)["messages"],
        json!([{"role": "tool", "tool_call_id": null, "content": "orphaned"}])
    );
}

// -- reasoning ----------------------------------------------------------
//
// DeepSeek's thinking mode refuses a request whose tool-calling assistant turns
// have lost their `reasoning_content`, so it has to go back out. Everything
// here is about keeping that from leaking anywhere it was not asked for: the
// field is emitted only on a tool-calling turn, only when the endpoint itself
// produced one, and only ever under the key it arrived under.

/// The assistant turn that asked for tools, with the thinking that led to it.
fn thinking_calls(text: &str, key: ReasoningKey) -> Message {
    Message {
        reasoning: Some(Reasoning::new(text, key)),
        ..Message::tool_calls(vec![call("call_1", "new_pane", "{}")])
    }
}

#[test]
fn a_transcript_with_no_thinking_sends_neither_reasoning_field() {
    let transcript = [
        Message::user("open htop"),
        Message::assistant("On it."),
        Message::tool_calls(vec![call("call_1", "new_pane", "{}")]),
        Message::tool_result("call_1", "opened"),
    ];
    let body = String::from_utf8(request_body(&test_config(), &transcript, true)).unwrap();
    assert!(
        !body.contains("reasoning"),
        "an endpoint that never sent thinking must get the request it always \
         got — this is what keeps OpenAI unaffected: {body}"
    );
}

#[test]
fn a_thinking_tool_call_turn_echoes_the_key_it_arrived_under() {
    let transcript = [thinking_calls(
        "weighed it up",
        ReasoningKey::ReasoningContent,
    )];
    assert_eq!(
        body_value(&test_config(), &transcript)["messages"][0],
        json!({
            "role": "assistant",
            "content": "",
            "tool_calls": [{
                "id": "call_1",
                "type": "function",
                "function": {"name": "new_pane", "arguments": "{}"},
            }],
            "reasoning_content": "weighed it up",
        })
    );
}

#[test]
fn thinking_that_arrived_as_reasoning_goes_back_as_reasoning() {
    let transcript = [thinking_calls("weighed it up", ReasoningKey::Reasoning)];
    let message = &body_value(&test_config(), &transcript)["messages"][0];
    assert_eq!(message["reasoning"], json!("weighed it up"));
    assert!(
        message.get("reasoning_content").is_none(),
        "the two families do not accept each other's key, so nothing is \
         translated between them: {message}"
    );
}

#[test]
fn a_plain_assistant_message_never_carries_its_thinking() {
    // DeepSeek's own rule is scoped to tool calls: without them the
    // intermediate reasoning "does not need to participate in the context
    // concatenation" and is ignored, so sending it would only pay for tokens to
    // be thrown away — and would show the field to endpoints with no use for it.
    let transcript = [Message {
        reasoning: Some(Reasoning::new("out loud", ReasoningKey::ReasoningContent)),
        ..Message::assistant("Done.")
    }];
    assert_eq!(
        body_value(&test_config(), &transcript)["messages"],
        json!([{"role": "assistant", "content": "Done."}])
    );
}

#[test]
fn reasoning_echo_off_drops_the_field_from_a_turn_that_has_one() {
    let mut cfg = test_config();
    cfg.reasoning_echo = false;
    let transcript = [thinking_calls(
        "weighed it up",
        ReasoningKey::ReasoningContent,
    )];
    let message = &body_value(&cfg, &transcript)["messages"][0];
    assert!(
        message.get("reasoning_content").is_none() && message.get("reasoning").is_none(),
        "the escape hatch for an endpoint that streams thinking but rejects it \
         on input: {message}"
    );
}

#[test]
fn empty_thinking_is_the_same_as_none() {
    let transcript = [thinking_calls("", ReasoningKey::ReasoningContent)];
    let message = &body_value(&test_config(), &transcript)["messages"][0];
    assert!(
        message.get("reasoning_content").is_none(),
        "a stream that opened a reasoning field and put nothing in it has \
         nothing to hand back: {message}"
    );
}
