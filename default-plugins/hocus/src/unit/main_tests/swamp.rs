use super::*;

#[test]
fn swamp_runs_without_a_project_and_correlates_command_results() {
    let fixtures = [
        (
            "swamp_issue_search",
            "{}",
            0,
            r#"{"issues":[{"number":42,"title":"Fix vault","type":"bug","status":"closed","author":"Ada","url":"https://swamp.club/lab/42"}],"total":1}"#,
            "",
        ),
        (
            "swamp_issue_search",
            "{}",
            0,
            r#"{"issues":[],"total":0}"#,
            "",
        ),
        (
            "swamp_issue_get",
            r#"{"number":42}"#,
            0,
            r#"{"number":42,"title":"Fix vault","type":"bug","status":"closed","author":"Ada","url":"https://swamp.club/lab/42","body":"Full issue body","assignees":["Ada"],"comments":[{"author":"Bob","body":"Fixed now","createdAt":"2026-09-18T00:00:00Z"}]}"#,
            "",
        ),
        (
            "swamp_issue_get",
            r#"{"number":42}"#,
            1,
            r#"{"error":"not found"}"#,
            "Login required",
        ),
        (
            "swamp_issue_search",
            "{}",
            2,
            "",
            "No such file or directory (os error 2)",
        ),
        ("swamp_issue_search", "{}", 127, "", "swamp: not found"),
    ];
    for (name, args, code, stdout, stderr) in fixtures {
        let mut state = State {
            plan: Plan::Planning,
            ..ready_state()
        };
        assert!(state.active_project.is_none());
        assert!(state.tool_is_read_only(name));
        assert!(!run_file_tool(&mut state, name, args));
        assert!(matches!(state.status, Status::AwaitingCommand));
        assert!(!state.on_command_result(
            Some(code),
            stdout.as_bytes(),
            stderr.as_bytes(),
            &BTreeMap::from([("tool_call_id".into(), "other".into())])
        ));
        assert!(tool_results(&state).is_empty());
        assert!(state.on_command_result(
            Some(code),
            stdout.as_bytes(),
            stderr.as_bytes(),
            &BTreeMap::from([("tool_call_id".into(), "mine".into())])
        ));
        let results = tool_results(&state);
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains(stdout));
        assert!(results[0].content.contains(stderr));
        if code == 2 || code == 127 {
            assert!(results[0].content.contains("Swamp could not be run"));
        }
    }
}

#[test]
fn invalid_swamp_arguments_never_defer() {
    for (name, args) in [
        ("swamp_issue_get", r#"{"number":0}"#),
        ("swamp_issue_search", r#"{"query":"--help"}"#),
    ] {
        let mut state = State {
            plan: Plan::Planning,
            ..ready_state()
        };
        assert!(run_file_tool(&mut state, name, args));
        assert!(!matches!(state.status, Status::AwaitingCommand));
        assert_eq!(tool_results(&state).len(), 1);
    }
}
