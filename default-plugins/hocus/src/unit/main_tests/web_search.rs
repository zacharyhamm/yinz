use super::*;

/// The whole point of the deferred path: the queue must not advance until
/// the response lands, or the model is answered before it was asked.
#[test]
fn a_search_defers_until_its_response_arrives() {
    let mut state = searching_state("yinz");
    let call = state.pending.front().cloned().unwrap();

    assert!(!state.run_head(&call), "the queue must wait");
    assert!(matches!(state.status, Status::AwaitingSearch));
    assert!(tool_results(&state).is_empty());

    let rendered = state.on_response(
        200,
        br#"{"results":[{"title":"Yinz","url":"https://example.com","content":"a workspace"}]}"#,
        &BTreeMap::from([("tool_call_id".to_owned(), "mine".to_owned())]),
    );

    assert!(rendered);
    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(results[0].content.contains("https://example.com"));
}

#[test]
fn a_search_response_for_a_call_we_are_not_waiting_on_is_ignored() {
    let mut state = searching_state("yinz");
    state.status = Status::AwaitingSearch;

    let rendered = state.on_response(
        200,
        br#"{"results":[]}"#,
        &BTreeMap::from([("tool_call_id".to_owned(), "someone_elses".to_owned())]),
    );

    assert!(!rendered);
    assert!(tool_results(&state).is_empty());
    assert_eq!(state.pending.len(), 1, "still waiting on ours");
}

/// A dead SearxNG must answer the call rather than hang the tool loop.
#[test]
fn a_failed_search_is_reported_as_the_tools_result() {
    let mut state = searching_state("yinz");
    state.status = Status::AwaitingSearch;

    state.on_response(
        502,
        b"bad gateway",
        &BTreeMap::from([("tool_call_id".to_owned(), "mine".to_owned())]),
    );

    let results = tool_results(&state);
    assert_eq!(results.len(), 1);
    assert!(results[0].content.contains("502"), "{}", results[0].content);
}

#[test]
fn a_search_with_no_query_is_answered_rather_than_sent() {
    let mut state = ready_state();
    let call = ToolCall {
        id: "mine".into(),
        name: "web_search".into(),
        arguments: r#"{"query":"  "}"#.into(),
    };
    state.pending = vec![call.clone()].into();

    assert!(state.run_head(&call), "the queue advances");
    assert!(!matches!(state.status, Status::AwaitingSearch));
    assert_eq!(tool_results(&state).len(), 1);
}

/// The API stream's own lead response is told apart by its `stream` context,
/// so it must still reach the streaming path even while a search is somehow
/// in flight.
#[test]
fn a_stream_response_is_not_mistaken_for_a_search_result() {
    let mut state = state_streaming(vec![Message::user("hi"), Message::assistant("")], "hocus-1");
    state.scheduled_run = Some(ScheduledRun {
        job_id: 1,
        auto_confirm: true,
    });
    state.on_response(
        401,
        br#"{"error":{"message":"bad key"}}"#,
        &stream_context("hocus-1"),
    );

    assert_eq!(state.messages[1].role, Role::Error);
    assert!(state.stream.as_ref().is_some_and(|s| s.finishing));
    assert!(state.scheduled_run.is_none());
}
