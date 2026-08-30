use super::*;

#[test]
fn a_todo_tool_says_when_there_is_no_active_project() {
    let mut state = idle_state();
    assert_eq!(
        todo_result(&mut state, "todo_add", r#"{"text":"ship it"}"#),
        Some("no active project; give project, or call project_list".to_owned()),
    );
}

#[test]
fn a_todo_item_lands_under_the_active_project_root() {
    let mut state = state_with_project();
    let added = todo(&mut state, "todo_add", r#"{"text":"ship the parser"}"#);

    assert_eq!(added["id"], 1);
    assert_eq!(added["project"], "hocus");
    assert_eq!(added["text"], "ship the parser");
    // And it is really in the store, not only in the answer.
    assert_eq!(
        stored_list(&state, "/work/hocus").items[0].text,
        "ship the parser"
    );
}

#[test]
fn a_todo_add_returns_and_stores_clean_markdown() {
    let mut state = state_with_project();
    let added = todo(
        &mut state,
        "todo_add",
        r#"{"text":" **ship it**\n\t- 日本語\u001b "}"#,
    );

    let expected = "**ship it**\n    - 日本語";
    assert_eq!(added["text"], expected);
    assert_eq!(stored_list(&state, "/work/hocus").items[0].text, expected);
}

#[test]
fn a_todo_tool_rejects_a_project_name_it_does_not_know() {
    let mut state = state_with_project();
    assert_eq!(
        todo_result(
            &mut state,
            "todo_add",
            r#"{"text":"ship it","project":"nowhere"}"#
        ),
        Some("unknown project nowhere; call project_list".to_owned()),
    );
}

#[test]
fn a_todo_tool_needs_its_key_argument() {
    let mut state = state_with_project();
    assert_eq!(
        todo_result(&mut state, "todo_add", "{}"),
        Some("text is required".to_owned()),
    );
    assert_eq!(
        state.run_local(&agent_call("todo_search", "{}")),
        Some("query is required".to_owned()),
    );
    assert_eq!(
        todo_result(&mut state, "todo_done", "{}"),
        Some("id is required".to_owned()),
    );
    assert_eq!(
        todo_result(&mut state, "todo_edit", "{}"),
        Some("id is required".to_owned()),
    );
    assert_eq!(
        todo_result(&mut state, "todo_edit", r#"{"id":1}"#),
        Some("text is required".to_owned()),
    );
}

#[test]
fn a_todo_edit_changes_its_text() {
    let mut state = state_with_project();
    todo(&mut state, "todo_add", r#"{"text":"ship the parser"}"#);

    let edited = todo(
        &mut state,
        "todo_edit",
        r#"{"id":1,"text":"**ship it**\n\n- faster\u001b"}"#,
    );
    assert_eq!(edited["id"], 1);
    assert_eq!(edited["text"], "**ship it**\n\n- faster");
    assert_eq!(
        stored_list(&state, "/work/hocus").items[0].text,
        "**ship it**\n\n- faster"
    );
}

#[test]
fn a_todo_edit_rejects_an_unknown_id() {
    let mut state = state_with_project();
    assert_eq!(
        todo_result(&mut state, "todo_edit", r#"{"id":1,"text":"x"}"#),
        Some("no todo item 1 in this project; call todo_list".to_owned()),
    );
}

/// The reason the file is keyed on the folder and not on the name.
#[test]
fn a_todo_list_follows_a_project_the_user_renames() {
    let mut state = state_with_project();
    todo(&mut state, "todo_add", r#"{"text":"ship the parser"}"#);
    state.run_local(&agent_call(
        "project_assign",
        r#"{"name":"pocus","root":"/work/hocus"}"#,
    ));

    let list = todo(&mut state, "todo_list", r#"{"project":"pocus"}"#);
    assert_eq!(list["items"][0]["text"], "ship the parser");
}

/// The other way a name moves: a poll finds a shallower folder and takes
/// the deeper project's name away.
#[test]
fn a_todo_list_survives_a_root_that_absorbs_its_project() {
    let mut state = state_with_tab(9);
    poll(&mut state, vec![agent_in(3, "/work/hocus/src", 9)]);
    todo(&mut state, "todo_add", r#"{"text":"ship the parser"}"#);

    poll(
        &mut state,
        vec![
            agent_in(3, "/work/hocus/src", 9),
            agent_in(4, "/work/hocus", 9),
        ],
    );
    assert_eq!(state.projects.keys().collect::<Vec<_>>(), vec!["hocus"]);

    // The list is still there, under the folder it was made in.
    let list = todo(&mut state, "todo_list", r#"{"project":"/work/hocus/src"}"#);
    assert_eq!(list["project"], serde_json::Value::Null);
    assert_eq!(list["items"][0]["text"], "ship the parser");
}

#[test]
fn a_todo_list_comes_back_as_a_tree_with_ids() {
    let mut state = state_with_project();
    todo(&mut state, "todo_add", r#"{"text":"ship the parser"}"#);
    todo(
        &mut state,
        "todo_add",
        r#"{"text":"write the test","parent_id":1}"#,
    );

    let list = todo(&mut state, "todo_list", "{}");
    assert_eq!(list["open"], 2);
    assert_eq!(list["items"][0]["id"], 1);
    assert_eq!(list["items"][0]["children"][0]["id"], 2);
    assert_eq!(list["items"][0]["children"][0]["text"], "write the test");
}

#[test]
fn crossing_an_item_off_crosses_off_the_items_below_it() {
    let mut state = state_with_project();
    todo(&mut state, "todo_add", r#"{"text":"ship the parser"}"#);
    todo(
        &mut state,
        "todo_add",
        r#"{"text":"write the test","parent_id":1}"#,
    );

    let change = todo(&mut state, "todo_done", r#"{"id":1}"#);
    assert_eq!(change["done"], true);
    // `cascaded`, which is what the store has always answered. This asserted
    // `also_done` until the simulated harness that invented that name went
    // away — it was never the key the model actually saw.
    assert_eq!(change["cascaded"], serde_json::json!([2]));

    let open = todo(&mut state, "todo_list", "{}");
    assert!(open["items"].as_array().unwrap().is_empty());
    let all = todo(&mut state, "todo_list", r#"{"include_done":true}"#);
    assert_eq!(all["done"], 2);
}

#[test]
fn an_undo_opens_the_item_and_the_item_above_it() {
    let mut state = state_with_project();
    todo(&mut state, "todo_add", r#"{"text":"ship the parser"}"#);
    todo(
        &mut state,
        "todo_add",
        r#"{"text":"write the test","parent_id":1}"#,
    );
    todo(&mut state, "todo_done", r#"{"id":1}"#);

    let change = todo(&mut state, "todo_done", r#"{"id":2,"undo":true}"#);
    assert_eq!(change["done"], false);
    assert_eq!(change["reopened"], serde_json::json!([1]));
    assert_eq!(todo(&mut state, "todo_list", "{}")["open"], 2);
}

#[test]
fn a_search_names_the_project_and_the_items_above_the_match() {
    let mut state = state_with_project();
    todo(&mut state, "todo_add", r#"{"text":"release 2"}"#);
    todo(
        &mut state,
        "todo_add",
        r#"{"text":"ship the parser","parent_id":1}"#,
    );

    let found = todo(&mut state, "todo_search", r#"{"query":"PARSER"}"#);
    assert_eq!(found["scope"], "hocus");
    assert_eq!(found["matches"][0]["id"], 2);
    assert_eq!(found["matches"][0]["project"], "hocus");
    assert_eq!(
        found["matches"][0]["trail"],
        serde_json::json!(["release 2", "ship the parser"]),
    );
}

#[test]
fn a_search_over_every_project_finds_a_list_whose_project_is_gone() {
    let mut state = state_with_project();
    todo(
        &mut state,
        "todo_add",
        r#"{"text":"ship the parser","project":"/work/gone"}"#,
    );
    state.projects.clear();
    state.active_project = None;

    let found = todo(
        &mut state,
        "todo_search",
        r#"{"query":"parser","all_projects":true}"#,
    );
    assert_eq!(found["scope"], "all");
    assert_eq!(found["matches"][0]["root"], "/work/gone");
    assert_eq!(found["matches"][0]["project"], serde_json::Value::Null);
}

/// A list is keyed on its folder, so it outlives the project that named it
/// and a write addressed by root still lands. The answer names the project
/// where there is one and `null` where there is not.
#[test]
fn a_write_to_a_list_whose_project_is_gone_still_lands() {
    let mut state = state_with_project();
    let named = todo(&mut state, "todo_add", r#"{"text":"ship it"}"#);
    assert_eq!(named["project"], "hocus");

    state.projects.clear();
    state.active_project = None;
    let orphaned = todo(
        &mut state,
        "todo_add",
        r#"{"text":"still mine","project":"/work/gone"}"#,
    );
    assert_eq!(orphaned["project"], serde_json::Value::Null);
    assert_eq!(
        stored_list(&state, "/work/gone").items[0].text,
        "still mine"
    );
}

/// A todo change is a write and an answer, in that order, with nothing in
/// between.
///
/// This replaces a test for the failure the pipe used to introduce: a change
/// went out, nothing came back, and the queue had to be rescued by a
/// five-second timeout. An apply has either happened or returned an error by
/// the time it returns, so there is no waiting state left to get wrong.
#[test]
fn a_todo_write_answers_without_parking_the_queue() {
    let mut state = state_with_project();
    let call = agent_call("todo_add", r#"{"text":"ship it"}"#);
    state.pending.push_back(call.clone());

    assert!(state.start_todo_write(&call), "the queue advances");
    assert!(matches!(state.status, Status::Idle));
    assert!(state.pending.is_empty());
    let answer = tool_results(&state)
        .last()
        .expect("answered")
        .content
        .clone();
    assert!(answer.contains("ship it"), "got {answer}");
    assert_eq!(stored_list(&state, "/work/hocus").items[0].text, "ship it");
}
