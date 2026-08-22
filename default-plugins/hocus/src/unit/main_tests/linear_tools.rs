use super::*;

/// A state with one Linear call queued and `env` carrying whatever the test
/// wants the session environment to have been.
fn linear_state(name: &str, arguments: &str, env: &[(&str, &str)]) -> State {
    let env: BTreeMap<String, String> = env
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .chain([("OPENAI_API_KEY".to_owned(), "sk-test".to_owned())])
        .collect();
    let mut state = ready_state();
    state.cfg = Config::resolve(&BTreeMap::new(), &env);
    state.pending = vec![ToolCall {
        id: "mine".into(),
        name: name.into(),
        arguments: arguments.into(),
    }]
    .into();
    state
}

const KEY: (&str, &str) = ("LINEAR_API_KEY", "lin_api_test");

/// The deferred contract, the same one `web_search` has: the queue must not
/// advance until the response lands, or the model is answered before it asked.
#[test]
fn a_linear_call_defers_until_its_response_arrives() {
    let mut state = linear_state("linear_issues", "{}", &[KEY]);
    let call = state.pending.front().cloned().unwrap();

    assert!(!state.run_head(&call), "the queue must wait");
    assert!(matches!(state.status, Status::AwaitingLinear));
    assert!(tool_results(&state).is_empty());

    let rendered = state.on_response(
        200,
        br#"{"data":{"viewer":{"assignedIssues":{"nodes":[
            {"identifier":"ENG-1","title":"Fix it","state":{"name":"Todo","type":"unstarted"}}
        ]}}}}"#,
        &BTreeMap::from([("tool_call_id".to_owned(), "mine".to_owned())]),
    );

    assert!(rendered);
    assert!(matches!(state.status, Status::Idle));
    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(
        results[0].content.contains("ENG-1"),
        "{}",
        results[0].content
    );
}

/// The status guard doing its job: a Linear reply must not be eaten by the
/// search finisher, and vice versa.
#[test]
fn a_reply_for_a_call_we_are_not_waiting_on_is_ignored() {
    let mut state = linear_state("linear_issues", "{}", &[KEY]);
    state.status = Status::AwaitingLinear;

    let rendered = state.on_response(
        200,
        br#"{"data":{"issues":{"nodes":[]}}}"#,
        &BTreeMap::from([("tool_call_id".to_owned(), "someone_elses".to_owned())]),
    );

    assert!(!rendered);
    assert!(tool_results(&state).is_empty());
    assert_eq!(state.pending.len(), 1, "still waiting on ours");
}

/// The path every user without a Linear key takes. It must answer the call
/// rather than hang the tool loop, and it must say which variable to set.
#[test]
fn no_configured_key_answers_the_call_instead_of_waiting() {
    let mut state = linear_state("linear_issues", "{}", &[]);
    let call = state.pending.front().cloned().unwrap();

    assert!(state.run_head(&call), "the queue must advance");
    assert!(matches!(state.status, Status::Idle));

    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(
        results[0].content.contains("LINEAR_API_KEY"),
        "{}",
        results[0].content
    );
}

/// And it names the variable the *configuration* chose, which is the whole
/// point of the name being configurable.
#[test]
fn the_not_configured_sentence_names_the_configured_variable() {
    let mut state = linear_state("linear_issues", "{}", &[]);
    state.cfg = Config::resolve(
        &BTreeMap::from([("linear_api_key_env".to_owned(), "WORK_LINEAR".to_owned())]),
        &BTreeMap::from([("OPENAI_API_KEY".to_owned(), "sk-test".to_owned())]),
    );
    let call = state.pending.front().cloned().unwrap();

    state.run_head(&call);
    assert!(tool_results(&state)[0].content.contains("WORK_LINEAR"));
}

/// Bad arguments are answered, not deferred — otherwise the loop waits on a
/// request that was never sent.
#[test]
fn an_unusable_argument_answers_the_call_rather_than_deferring() {
    let mut state = linear_state("linear_issue", r#"{"identifier":"nonsense"}"#, &[KEY]);
    let call = state.pending.front().cloned().unwrap();

    assert!(state.run_head(&call), "the queue must advance");
    assert!(matches!(state.status, Status::Idle));
    assert!(tool_results(&state)[0].content.contains("ENG-123"));
}

/// A Linear failure arrives with a 200, so the tool result has to carry the
/// error rather than an empty listing.
#[test]
fn an_error_inside_a_200_reaches_the_model_as_the_error() {
    let mut state = linear_state("linear_search", r#"{"query":"login"}"#, &[KEY]);
    let call = state.pending.front().cloned().unwrap();
    state.run_head(&call);

    state.on_response(
        200,
        br#"{"data":null,"errors":[{"message":"Argument 'nope' on Field 'issues' has an invalid value"}]}"#,
        &BTreeMap::from([("tool_call_id".to_owned(), "mine".to_owned())]),
    );

    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(
        results[0].content.starts_with("Linear error:"),
        "{}",
        results[0].content
    );
}

/// Esc during a Linear call goes through the same generic abort as every other
/// wait, and must leave the transcript valid.
#[test]
fn aborting_answers_the_outstanding_linear_call() {
    let mut state = linear_state("linear_issues", "{}", &[KEY]);
    let call = state.pending.front().cloned().unwrap();
    state.run_head(&call);
    assert!(matches!(state.status, Status::AwaitingLinear));

    state.abort_tools();

    assert!(matches!(state.status, Status::Idle));
    assert!(state.pending.is_empty());
    assert_eq!(
        tool_results(&state).len(),
        1,
        "the call still gets an answer"
    );
}
