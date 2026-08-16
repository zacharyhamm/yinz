use super::*;

#[test]
fn a_command_result_for_a_call_we_are_not_waiting_on_is_ignored() {
    let mut state = ready_state();
    state.status = Status::AwaitingCommand;
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: "slack_read".into(),
        arguments: "{}".into(),
    }]
    .into();

    let rendered = state.on_command_result(
        Some(0),
        b"out",
        b"",
        &BTreeMap::from([("tool_call_id".to_owned(), "someone_elses".to_owned())]),
    );

    assert!(!rendered);
    assert!(tool_results(&state).is_empty());
    assert_eq!(state.pending.len(), 1, "still waiting on ours");
}

#[test]
fn a_command_result_reports_the_exit_code_and_both_streams() {
    let mut state = ready_state();
    state.status = Status::AwaitingCommand;
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: "slack_read".into(),
        arguments: "{}".into(),
    }]
    .into();

    state.on_command_result(
        Some(2),
        b"stdout text",
        b"stderr text",
        &BTreeMap::from([("tool_call_id".to_owned(), "mine".to_owned())]),
    );

    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(results[0].content.contains("exit code: 2"));
    assert!(results[0].content.contains("stdout text"));
    assert!(results[0].content.contains("stderr text"));
}

/// A tool result is re-sent on every later turn, so a command that dumps
/// megabytes must not be paid for forever.
#[test]
fn a_command_result_is_capped_before_it_reaches_the_transcript() {
    let mut state = ready_state();
    state.status = Status::AwaitingCommand;
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: "slack_read".into(),
        arguments: "{}".into(),
    }]
    .into();

    let flood = "x".repeat(MAX_TOOL_OUTPUT * 2);
    state.on_command_result(
        Some(0),
        flood.as_bytes(),
        b"",
        &BTreeMap::from([("tool_call_id".to_owned(), "mine".to_owned())]),
    );

    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(results[0].content.len() < flood.len());
    assert!(results[0].content.contains("exit code: 0"));
}

/// Zellij reports "could not spawn" as exit code 2 with a bare io::Error,
/// naming neither the binary nor the setting that chose it.
#[test]
fn a_slack_tool_that_cannot_find_dobbs_says_which_binary_and_setting() {
    let mut state = ready_state();
    state.status = Status::AwaitingCommand;
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: "slack_search".into(),
        arguments: "{}".into(),
    }]
    .into();

    state.on_command_result(
        Some(2),
        b"",
        b"No such file or directory (os error 2)",
        &BTreeMap::from([("tool_call_id".to_owned(), "mine".to_owned())]),
    );

    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(results[0].content.contains(api::DEFAULT_DOBBS));
    assert!(results[0].content.contains("dobbs_path"));
}

/// The hint must not swallow a real exit-2 from a dobbs that ran.
#[test]
fn the_dobbs_hint_is_limited_to_a_failure_to_spawn_a_slack_tool() {
    let mut state = ready_state();
    state.status = Status::AwaitingCommand;
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: "slack_search".into(),
        arguments: "{}".into(),
    }]
    .into();

    state.on_command_result(
        Some(2),
        b"",
        b"grep: no matches",
        &BTreeMap::from([("tool_call_id".to_owned(), "mine".to_owned())]),
    );

    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(
        results[0].content.starts_with("exit code: 2"),
        "got the hint: {}",
        results[0].content
    );
}

/// …nor fire for a head that is not a Slack tool. No non-Slack tool can reach
/// `AwaitingCommand` any more, so the guard is exercised where it lives.
#[test]
fn the_dobbs_hint_does_not_fire_for_a_head_that_is_not_a_slack_tool() {
    let mut state = ready_state();
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: "list_session".into(),
        arguments: "{}".into(),
    }]
    .into();

    assert!(state
        .slack_exec_hint(Some(2), b"No such file or directory (os error 2)")
        .is_none());
}

/// A changed KDL block is the only way a running plugin's config can move:
/// Zellij freezes it at server start and replays it even on a reload.
#[test]
fn a_configuration_change_event_re_resolves_the_config() {
    let mut state = ready_state();
    // What the permission grant would have left behind.
    state.granted = true;
    state.env = BTreeMap::from([("OPENAI_API_KEY".to_owned(), "sk-test".to_owned())]);
    assert_eq!(state.cfg.as_ref().unwrap().dobbs, api::DEFAULT_DOBBS);

    state.handle(Event::PluginConfigurationChanged(BTreeMap::from([(
        "dobbs_path".to_owned(),
        "/opt/dobbs".to_owned(),
    )])));

    assert_eq!(state.cfg.as_ref().unwrap().dobbs, "/opt/dobbs");

    // Before the permission grant there is no environment to resolve
    // against, so the config must be left as it is rather than becoming a
    // spurious "no API key" banner.
    let mut state = State::default();
    state.handle(Event::PluginConfigurationChanged(BTreeMap::from([(
        "model".to_owned(),
        "gpt-5".to_owned(),
    )])));
    assert!(state.cfg.is_err());
}

/// The Slack tools are intercepted before `tools::dispatch`, which on the
/// host triple would answer "tools are unavailable outside zellij".
#[test]
fn a_bad_slack_call_is_answered_without_reaching_dispatch() {
    let mut state = ready_state();
    let call = ToolCall {
        id: "mine".into(),
        name: "slack_read".into(),
        arguments: "{}".into(),
    };
    state.pending = vec![call.clone()].into();

    assert!(state.run_head(&call), "a rejected call must not defer");
    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(results[0].content.starts_with("channel is required"));
}
