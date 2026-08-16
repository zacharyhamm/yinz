use super::*;

/// The whole point of the tools: no project means no reading, rather than
/// reading whatever folder the session happens to have been started in.
#[test]
fn a_file_tool_without_a_project_refuses_instead_of_running_anything() {
    for name in tools::FILE_TOOLS {
        let mut state = ready_state();
        assert!(
            run_file_tool(
                &mut state,
                name,
                r#"{"pattern": "x", "query": "x", "path": "x"}"#
            ),
            "{name} must answer rather than defer"
        );
        let results = tool_results(&state);
        assert_eq!(results.len(), 1);
        assert!(
            results[0].content.starts_with("no active project"),
            "{name} said {:?}",
            results[0].content
        );
        assert!(!matches!(state.status, Status::AwaitingCommand));
    }
}

#[test]
fn a_file_tool_refuses_a_path_outside_the_project() {
    let mut state = project_state();
    assert!(run_file_tool(
        &mut state,
        "read_file",
        r#"{"path": "../../etc/passwd"}"#
    ));
    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(
        results[0].content.contains(".."),
        "{:?}",
        results[0].content
    );
    assert!(!matches!(state.status, Status::AwaitingCommand));
}

#[test]
fn a_bad_file_call_is_answered_without_reaching_a_command() {
    let mut state = project_state();
    assert!(run_file_tool(&mut state, "search_files", "{}"));
    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(results[0].content.starts_with("query is required"));
}

/// `rg` exits 1 when it ran fine and matched nothing. Reported as a bare
/// exit code that reads as a broken tool, and the model retries it.
#[test]
fn finding_nothing_is_reported_as_no_matches_rather_than_a_failure() {
    let mut state = project_state();
    state.status = Status::AwaitingCommand;
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: "search_files".into(),
        arguments: r#"{"query": "nowhere"}"#.into(),
    }]
    .into();

    state.on_command_result(
        Some(1),
        b"",
        b"",
        &BTreeMap::from([("tool_call_id".to_owned(), "mine".to_owned())]),
    );

    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].content, "no matches");
}

#[test]
fn a_search_result_reaches_the_model_as_the_lines_themselves() {
    let mut state = project_state();
    state.status = Status::AwaitingCommand;
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: "search_files".into(),
        arguments: r#"{"query": "fn main"}"#.into(),
    }]
    .into();

    state.on_command_result(
        Some(0),
        b"src/main.rs:12:fn main() {\n",
        b"",
        &BTreeMap::from([("tool_call_id".to_owned(), "mine".to_owned())]),
    );

    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].content, "src/main.rs:12:fn main() {");
}

/// A listing is cut to the call's cap, well before MAX_TOOL_OUTPUT would cut
/// it mid-line, and says so rather than pretending that was all of it.
#[test]
fn a_listing_longer_than_the_cap_is_cut_and_counted() {
    let mut state = project_state();
    state.status = Status::AwaitingCommand;
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: "find_files".into(),
        arguments: r#"{"pattern": "*.rs", "max_results": 2}"#.into(),
    }]
    .into();

    state.on_command_result(
        Some(0),
        b"a.rs\nb.rs\nc.rs\nd.rs\n",
        b"",
        &BTreeMap::from([("tool_call_id".to_owned(), "mine".to_owned())]),
    );

    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(results[0].content.starts_with("a.rs\nb.rs"));
    assert!(!results[0].content.contains("c.rs"));
    assert!(results[0].content.contains("2 of 4 lines shown"));
}

/// `read_file` returns the window that was asked for, so the listing cap
/// must not quietly shorten it.
#[test]
fn a_read_is_not_cut_to_the_listing_cap() {
    let mut state = project_state();
    state.status = Status::AwaitingCommand;
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: "read_file".into(),
        arguments: r#"{"path": "x", "line_count": 400}"#.into(),
    }]
    .into();

    let lines = (0..400)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    state.on_command_result(
        Some(0),
        lines.as_bytes(),
        b"",
        &BTreeMap::from([("tool_call_id".to_owned(), "mine".to_owned())]),
    );

    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].content.lines().count(), 400);
}

#[test]
fn a_file_tool_that_cannot_find_ripgrep_says_so() {
    let mut state = project_state();
    state.status = Status::AwaitingCommand;
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: "find_files".into(),
        arguments: r#"{"pattern": "x"}"#.into(),
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
    assert!(results[0].content.starts_with("rg could not be run"));
}

/// A file tool's output is still capped: it is re-sent on every later turn.
#[test]
fn a_file_result_is_capped_before_it_reaches_the_transcript() {
    let mut state = project_state();
    state.status = Status::AwaitingCommand;
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: "read_file".into(),
        arguments: r#"{"path": "x", "line_count": 1}"#.into(),
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
}
