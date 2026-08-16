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
        "You are Hocus in Zellij. Use native tools for Zellij operations. Before an ID-based action, call `list_session` unless the immediately previous result supplied the IDs; use `list_other_sessions` for cross-session IDs. Address tabs only by `tab_id` and panes by `pane_id`; `tab_number` is the tab bar's label, not an ID, so never pass it as one. Do not verify successful mutations or retry failed pane moves with variants. Use `send_input` for an existing program. You cannot run arbitrary commands. Inspect code only with `find_files`, `search_files`, and `read_file` inside the active project. Call `agent_list` before acting on agents. Make independent calls together."
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
    assert_eq!(body["tools"].as_array().unwrap().len(), 52);
}

#[test]
fn hocus_prompt_contract_keeps_the_tool_payload_under_20_kb() {
    let mut cfg = test_config();
    cfg.tools = true;
    let body = request_body(&cfg, &[Message::user("hi")], true);
    assert!(
        body.len() < 20_000,
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
    let transcript = [
        Message::user("open htop"),
        Message::tool_calls(vec![call("call_1", "new_pane", r#"{"command":"htop"}"#)]),
        Message::tool_result("call_1", r#"{"opened":true,"pane_id":7}"#),
    ];
    let body = body_value(&test_config(), &transcript);
    assert_eq!(
        body["messages"],
        json!([
            {"role": "user", "content": "open htop"},
            {
                "role": "assistant",
                "content": null,
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
                "content": null,
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
