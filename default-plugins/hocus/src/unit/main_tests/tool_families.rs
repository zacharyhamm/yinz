use super::*;

/// The tools the next request would advertise, given everything this chat has done.
fn advertised(state: &State) -> Vec<String> {
    let cfg = state.cfg.as_ref().unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&api::request_body(cfg, &state.messages)).unwrap();
    body["tools"]
        .as_array()
        .unwrap()
        .iter()
        // Either wire shape: this state resolves to the Responses endpoint, which
        // flattens the spec, and the chat endpoint nests it under `function`.
        .map(|spec| {
            spec["function"]["name"]
                .as_str()
                .or_else(|| spec["name"].as_str())
                .unwrap()
                .to_owned()
        })
        .collect()
}

/// A chat that has asked for nothing carries the always-on families and no more.
#[test]
fn a_new_chat_advertises_only_the_always_on_families() {
    let state = ready_state();
    let shown = advertised(&state);
    assert!(shown.iter().any(|name| name == "list_session"));
    assert!(shown.iter().any(|name| name == "read_file"));
    assert!(shown.iter().any(|name| name == tools::LOAD_TOOL));
    assert!(!shown.iter().any(|name| name == "plugin_write"));
    assert!(!shown.iter().any(|name| name == "agent_spawn"));
}

/// The path that costs nothing: `TOOL_FIRST_PROMPT` names `plugin_read`, so a model
/// that wants it calls it. `run_head` dispatches by name and never consults the
/// advertised set, so the call runs — and because it is now in the transcript, the
/// rest of the family rides along on the next request. No round trip was spent
/// asking for permission to ask.
#[test]
fn calling_a_withheld_tool_runs_it_and_widens_the_next_request() {
    let mut state = ready_state();
    assert!(!advertised(&state).iter().any(|n| n == "plugin_edit"));

    let frame = tool_call_frame("call_1", "plugin_read", r#"{"name":"clock"}"#);
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);

    let shown = advertised(&state);
    assert!(shown.iter().any(|n| n == "plugin_edit"), "family loaded");
    assert!(shown.iter().any(|n| n == "plugin_write"));
    // Only that family: loading one must not load the rest.
    assert!(!shown.iter().any(|n| n == "agent_spawn"));
    assert!(!shown.iter().any(|n| n == "slack_post"));
}

/// The explicit route, for a model that does not know the name it wants.
#[test]
fn loading_a_family_widens_the_next_request() {
    let mut state = ready_state();
    let frame = tool_call_frame("call_1", tools::LOAD_TOOL, r#"{"family":"slack"}"#);
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);

    let answer = &tool_results(&state)[0].content;
    assert!(answer.contains("slack_post"), "{answer}");

    let shown = advertised(&state);
    for tool in tools::SLACK_TOOLS {
        assert!(shown.iter().any(|name| name == tool), "{tool}");
    }
}

/// A load that was refused must not widen anything: the model was told it failed,
/// and getting the tools anyway would make the answer a lie.
#[test]
fn a_refused_load_widens_nothing() {
    let mut state = ready_state();
    let frame = tool_call_frame("call_1", tools::LOAD_TOOL, r#"{"family":"nonsense"}"#);
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);

    assert!(tool_results(&state)[0]
        .content
        .contains("no such tool family"));
    assert_eq!(advertised(&state).len(), 49);
}

/// What a reopened chat gets. The loaded set is derived from the transcript rather
/// than stored, so it comes back with the messages and nothing has to be archived.
#[test]
fn a_reloaded_transcript_keeps_the_families_it_had_loaded() {
    let mut state = ready_state();
    state.messages = vec![
        Message::user("write me a plugin"),
        Message::tool_calls(vec![ToolCall {
            id: "call_1".into(),
            name: "plugin_write".into(),
            arguments: r#"{"name":"clock","source":"return {}"}"#.into(),
        }]),
        Message::tool_result("call_1", "wrote clock.ts"),
    ];
    assert!(advertised(&state).iter().any(|n| n == "plugin_reload"));
}
