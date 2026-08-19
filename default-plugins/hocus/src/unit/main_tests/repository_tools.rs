use super::*;

#[test]
fn repository_tools_refuse_without_an_active_project() {
    for (name, arguments) in [
        ("git_log", "{}"),
        ("git_show", r#"{"commit":"abcdef12"}"#),
        ("github_read", "{}"),
    ] {
        let mut state = ready_state();
        assert!(run_file_tool(&mut state, name, arguments));
        assert!(tool_results(&state)[0]
            .content
            .starts_with("no active project"));
    }
}

#[test]
fn git_paths_cannot_leave_the_active_project() {
    let mut state = project_state();
    assert!(run_file_tool(
        &mut state,
        "git_log",
        r#"{"path":"../other"}"#,
    ));
    assert!(tool_results(&state)[0].content.contains(".."));
}

#[test]
fn github_read_uses_the_project_remote_and_defers_to_gh() {
    let mut state = project_state();
    state.projects.get_mut("hocus").unwrap().remote =
        Some("git@github.com:openai/codex.git".to_owned());
    assert!(!run_file_tool(
        &mut state,
        "github_read",
        r#"{"endpoint":"pulls"}"#,
    ));
    assert!(matches!(state.status, Status::AwaitingCommand));
}

#[test]
fn a_missing_gh_binary_is_named_in_the_tool_result() {
    let mut state = project_state();
    state.status = Status::AwaitingCommand;
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: "github_read".into(),
        arguments: "{}".into(),
    }]
    .into();
    state.on_command_result(
        Some(2),
        b"",
        b"No such file or directory (os error 2)",
        &BTreeMap::from([("tool_call_id".to_owned(), "mine".to_owned())]),
    );
    assert!(tool_results(&state)[0]
        .content
        .starts_with("gh could not be run"));
}

#[test]
fn successful_git_output_reaches_the_model_without_an_exit_code_wrapper() {
    let mut state = project_state();
    state.status = Status::AwaitingCommand;
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: "git_log".into(),
        arguments: "{}".into(),
    }]
    .into();
    state.on_command_result(
        Some(0),
        b"abcdef12\t2026-08-19T00:00:00Z\tAda\tAdd tools\n",
        b"",
        &BTreeMap::from([("tool_call_id".to_owned(), "mine".to_owned())]),
    );
    assert_eq!(
        tool_results(&state)[0].content,
        "abcdef12\t2026-08-19T00:00:00Z\tAda\tAdd tools"
    );
}
