use super::*;
use crate::lab::LOGIN_CONTEXT;

const LOGIN: &str =
    r#"{"serverUrl":"https://lab.example.test","apiKey":"swamp_key","username":"ada"}"#;

fn context(id: &str, login: bool) -> BTreeMap<String, String> {
    let mut context = BTreeMap::from([("tool_call_id".into(), id.into())]);
    if login {
        context.insert(LOGIN_CONTEXT.into(), "1".into());
    }
    context
}

fn server() -> BTreeMap<String, String> {
    BTreeMap::from([("content-type".into(), "application/json".into())])
}

#[test]
fn swamp_reads_the_login_then_calls_the_lab_without_a_project() {
    let mut state = State {
        plan: Plan::Planning,
        ..ready_state()
    };
    assert!(state.active_project.is_none());
    assert!(state.tool_is_read_only("swamp_issue_search"));
    assert!(!run_file_tool(
        &mut state,
        "swamp_issue_search",
        r#"{"query":"vault"}"#
    ));
    assert!(matches!(state.status, Status::AwaitingCommand));
    // Someone else's command is not the read.
    assert!(!state.on_command_result(Some(0), LOGIN.as_bytes(), &[], &context("other", true)));
    assert!(matches!(state.status, Status::AwaitingCommand));
    // Ours defers again, now on the Lab.
    assert!(!state.on_command_result(Some(0), LOGIN.as_bytes(), &[], &context("mine", true)));
    assert!(
        state.status
            == Status::AwaitingSwamp {
                server_url: "https://lab.example.test".into()
            }
    );
    assert!(tool_results(&state).is_empty());
    assert!(state.is_loading());

    let reply = br#"{"issues":[{"number":42,"title":"Fix vault","type":"bug","status":"closed","authorUsername":"Ada"}],"total":1}"#;
    assert!(!state.finish_swamp(200, &server(), reply, &context("other", false)));
    assert!(state.finish_swamp(200, &server(), reply, &context("mine", false)));
    assert!(matches!(state.status, Status::Idle));
    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(
        results[0]
            .content
            .contains("https://lab.example.test/lab/42"),
        "{}",
        results[0].content
    );
    // A late duplicate finds nothing waiting.
    assert!(!state.finish_swamp(200, &server(), reply, &context("mine", false)));
}

#[test]
fn without_a_login_reads_are_anonymous_and_writes_are_refused() {
    let mut state = ready_state();
    assert!(!run_file_tool(
        &mut state,
        "swamp_issue_get",
        r#"{"number":42}"#
    ));
    state.on_command_result(Some(0), b"", &[], &context("mine", true));
    assert!(
        state.status
            == Status::AwaitingSwamp {
                server_url: "https://swamp-club.com".into()
            }
    );
    state.finish_swamp(
        404,
        &server(),
        br#"{"error":"Issue not found"}"#,
        &context("mine", false),
    );
    assert_eq!(tool_results(&state)[0].content, "Issue not found");

    let mut state = ready_state();
    run_file_tool(
        &mut state,
        "swamp_issue_comment",
        r#"{"number":42,"body":"hello"}"#,
    );
    assert!(state.on_command_result(Some(0), b"", &[], &context("mine", true)));
    assert!(!matches!(state.status, Status::AwaitingSwamp { .. }));
    assert!(tool_results(&state)[0].content.contains("swamp auth login"));
}

#[test]
fn an_unreadable_login_is_reported_not_treated_as_logged_out() {
    let mut state = ready_state();
    run_file_tool(&mut state, "swamp_issue_search", "{}");
    assert!(state.on_command_result(
        Some(1),
        b"",
        b"cat: auth.json: Permission denied",
        &context("mine", true)
    ));
    let content = &tool_results(&state)[0].content;
    assert!(
        content.contains("auth.json") && content.contains("Permission denied"),
        "{content}"
    );
}

#[test]
fn invalid_swamp_arguments_never_defer() {
    for (name, args) in [
        ("swamp_issue_get", r#"{"number":0}"#),
        ("swamp_issue_search", r#"{"limit":500}"#),
        ("swamp_issue_create", r#"{"type":"bug"}"#),
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

#[test]
fn filing_an_issue_asks_first_and_shows_the_redacted_text() {
    let mut state = ready_state();
    let frame = tool_call_frame(
        "call_1",
        "swamp_issue_create",
        r#"{"type":"bug","title":"Crash","body":"reported by ada@example.com"}"#,
    );
    state.on_chunk("hocus-1", &frame, false, None);
    state.on_chunk("hocus-1", b"", true, None);
    assert!(matches!(state.status, Status::ConfirmingTool));
    let call = state.pending.front().unwrap().clone();
    let body = state.confirmation_body(&call);
    assert!(
        body[0].1 && body[0].0.contains("redacted 1 email"),
        "{body:?}"
    );
    assert!(
        body.iter()
            .any(|(line, _)| line == "body: reported by [REDACTED-EMAIL]"),
        "{body:?}"
    );
    state.confirm_tool();
    assert!(matches!(state.status, Status::AwaitingCommand));
}
