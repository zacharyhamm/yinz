use super::*;

#[test]
fn a_note_tool_says_when_there_is_no_active_project() {
    let mut state = idle_state();
    let error = note_result(&mut state, "note_add", r#"{"title":"t","body":"b"}"#).unwrap();
    assert!(error.starts_with("no active project"), "{error}");
    assert!(error.contains("global"), "{error}");
}

#[test]
fn a_note_lands_under_the_active_project_root_with_its_tags_and_links() {
    let mut state = state_with_project();
    let item = todo(&mut state, "todo_add", r#"{"text":"ship the parser"}"#)["id"]
        .as_u64()
        .unwrap();
    let added = note(
        &mut state,
        "note_add",
        &format!(
            r##"{{"title":" Why the parser ","body":"# because\n\n- reasons","tags":["Design"],"todo_ids":[{item}]}}"##
        ),
    );

    assert_eq!(added["project"], "hocus");
    assert_eq!(added["root"], "/work/hocus");
    assert_eq!(added["title"], "Why the parser");
    assert_eq!(added["body"], "# because\n\n- reasons");
    assert_eq!(added["tags"], serde_json::json!(["design"]));
    assert_eq!(added["todo_ids"], serde_json::json!([item]));
    assert!(added["created_at"].as_i64().is_some_and(|ms| ms > 0));
    assert!(added.get("warning").is_none());
    // And it is really in the store, not only in the answer.
    let stored = stored_note(&state, added["id"].as_u64().unwrap()).expect("stored");
    assert_eq!(stored.root.as_deref(), Some("/work/hocus"));
    assert_eq!(stored.todo_ids, vec![item]);
}

#[test]
fn a_global_note_has_no_root_and_no_project() {
    let mut state = state_with_project();
    let added = note(
        &mut state,
        "note_add",
        r#"{"title":"t","body":"b","global":true}"#,
    );
    assert_eq!(added["global"], true);
    assert_eq!(added["project"], serde_json::Value::Null);
    assert_eq!(
        stored_note(&state, added["id"].as_u64().unwrap())
            .unwrap()
            .root,
        None
    );
    // A global note is written without a project at all.
    let mut bare = idle_state();
    assert!(note_result(
        &mut bare,
        "note_add",
        r#"{"title":"t","body":"b","global":true}"#
    )
    .unwrap()
    .starts_with('{'));
}

/// A note the user typed is theirs; one the model wrote in a tool round is
/// hocus's. The flag the tool loop sets for a typed command is what tells
/// them apart, and it is still set while the write runs.
#[test]
fn a_typed_note_is_by_the_user_and_a_model_note_is_by_hocus() {
    let mut state = state_with_project();
    let by_model = note(&mut state, "note_add", r#"{"title":"m","body":"b"}"#);
    assert_eq!(by_model["author"], "hocus");
    state.manual_head = true;
    let by_user = note(&mut state, "note_add", r#"{"title":"u","body":"b"}"#);
    assert_eq!(by_user["author"], "user");
}

#[test]
fn note_list_hides_archived_notes_unless_asked_and_carries_no_bodies() {
    let mut state = state_with_project();
    let kept = note(&mut state, "note_add", r#"{"title":"kept","body":"b"}"#);
    let gone = note(&mut state, "note_add", r#"{"title":"gone","body":"b"}"#);
    let archived = note(
        &mut state,
        "note_archive",
        &format!(r#"{{"id":{}}}"#, gone["id"]),
    );
    assert_eq!(archived["archived"], true);

    let listed = note(&mut state, "note_list", "{}");
    assert_eq!(listed["scope"], "hocus");
    assert_eq!(listed["count"], 1);
    assert_eq!(listed["notes"][0]["id"], kept["id"]);
    assert!(listed["notes"][0].get("body").is_none());

    let all = note(&mut state, "note_list", r#"{"include_archived":true}"#);
    assert_eq!(all["count"], 2);

    note(
        &mut state,
        "note_archive",
        &format!(r#"{{"id":{},"undo":true}}"#, gone["id"]),
    );
    assert_eq!(note(&mut state, "note_list", "{}")["count"], 2);
}

#[test]
fn note_list_is_scoped_by_project_global_or_everything() {
    let mut state = state_with_project();
    note(&mut state, "note_add", r#"{"title":"in hocus","body":"b"}"#);
    note(
        &mut state,
        "note_add",
        r#"{"title":"global","body":"b","global":true}"#,
    );
    assert_eq!(note(&mut state, "note_list", "{}")["count"], 1);
    let global = note(&mut state, "note_list", r#"{"global":true}"#);
    assert_eq!(global["scope"], "global");
    assert_eq!(global["notes"][0]["title"], "global");
    let all = note(&mut state, "note_list", r#"{"all_projects":true}"#);
    assert_eq!(all["scope"], "all");
    assert_eq!(all["count"], 2);
}

#[test]
fn note_get_reads_the_body_and_an_unknown_id_says_what_to_call() {
    let mut state = state_with_project();
    let added = note(&mut state, "note_add", r#"{"title":"t","body":"the body"}"#);
    let got = note(
        &mut state,
        "note_get",
        &format!(r#"{{"id":{}}}"#, added["id"]),
    );
    assert_eq!(got["body"], "the body");
    assert_eq!(got["project"], "hocus");
    assert_eq!(
        note_result(&mut state, "note_get", r#"{"id":999}"#),
        Some("no note 999; call note_list".to_owned())
    );
}

#[test]
fn note_edit_changes_what_was_given_and_keeps_the_rest() {
    let mut state = state_with_project();
    let added = note(
        &mut state,
        "note_add",
        r#"{"title":"t","body":"b","tags":["a"]}"#,
    );
    let id = added["id"].as_u64().unwrap();
    let edited = note(
        &mut state,
        "note_edit",
        &format!(r#"{{"id":{id},"body":"new"}}"#),
    );
    assert_eq!(edited["title"], "t");
    assert_eq!(edited["body"], "new");
    assert_eq!(edited["tags"], serde_json::json!(["a"]));
    let retagged = note(
        &mut state,
        "note_edit",
        &format!(r#"{{"id":{id},"tags":[]}}"#),
    );
    assert_eq!(retagged["tags"], serde_json::json!([]));
    assert_eq!(
        note_result(&mut state, "note_edit", &format!(r#"{{"id":{id}}}"#)),
        Some("give a title, a body, tags, or some of each".to_owned())
    );
}

/// A tags-only edit carries `expect_rev` all the way to the store.
///
/// It used not to: the revision only reached `notes::plan_edit`, which only runs when there is a
/// title or a body to change, so replacing a whole tag set landed unguarded and reported
/// success over somebody else's retag. The reply carries the revision so a retry can name what
/// it read, which is the other half of the loop being tested here.
#[test]
fn a_tags_only_edit_from_a_stale_read_is_refused_and_can_be_retried() {
    let mut state = state_with_project();
    let added = note(
        &mut state,
        "note_add",
        r#"{"title":"t","body":"b","tags":["mine"]}"#,
    );
    let id = added["id"].as_u64().unwrap();
    let saw = added["rev"].as_i64().expect("the reply carries a revision");

    // Somebody else retags it in the meantime.
    note(
        &mut state,
        "note_edit",
        &format!(r#"{{"id":{id},"tags":["theirs"]}}"#),
    );

    let refused = note_result(
        &mut state,
        "note_edit",
        &format!(r#"{{"id":{id},"tags":["mine"],"expect_rev":{saw}}}"#),
    )
    .expect("answered");
    assert!(refused.contains("edited by somebody else"), "{refused}");
    assert_eq!(
        stored_note(&state, id).expect("still there").tags,
        vec!["theirs".to_owned()],
        "the refused write changed nothing"
    );

    // Reading it again and naming that revision gets the change in.
    let now = note(&mut state, "note_get", &format!(r#"{{"id":{id}}}"#));
    let rev = now["rev"].as_i64().unwrap();
    note(
        &mut state,
        "note_edit",
        &format!(r#"{{"id":{id},"tags":["mine"],"expect_rev":{rev}}}"#),
    );
    assert_eq!(
        stored_note(&state, id).expect("still there").tags,
        vec!["mine".to_owned()]
    );
}

#[test]
fn note_delete_removes_the_note_with_its_links_and_tags() {
    let mut state = state_with_project();
    let item = todo(&mut state, "todo_add", r#"{"text":"x"}"#)["id"]
        .as_u64()
        .unwrap();
    let added = note(
        &mut state,
        "note_add",
        &format!(r#"{{"title":"t","body":"b","tags":["a"],"todo_ids":[{item}]}}"#),
    );
    let id = added["id"].as_u64().unwrap();
    let deleted = note(&mut state, "note_delete", &format!(r#"{{"id":{id}}}"#));
    assert_eq!(deleted["deleted"], true);
    assert_eq!(deleted["title"], "t");
    assert_eq!(stored_note(&state, id), None);
    assert!(notes::notes_for_todo(&state.store(), item)
        .unwrap()
        .is_empty());
}

#[test]
fn note_link_rejects_an_unknown_todo_and_unlink_takes_a_link_away() {
    let mut state = state_with_project();
    let item = todo(&mut state, "todo_add", r#"{"text":"x"}"#)["id"]
        .as_u64()
        .unwrap();
    let id = note(&mut state, "note_add", r#"{"title":"t","body":"b"}"#)["id"]
        .as_u64()
        .unwrap();
    assert_eq!(
        note_result(
            &mut state,
            "note_link",
            &format!(r#"{{"note_id":{id},"todo_id":999}}"#)
        ),
        Some("no todo item 999; call todo_list".to_owned())
    );
    let linked = note(
        &mut state,
        "note_link",
        &format!(r#"{{"note_id":{id},"todo_id":{item}}}"#),
    );
    assert_eq!(linked["linked"], true);
    assert_eq!(stored_note(&state, id).unwrap().todo_ids, vec![item]);
    let by_item = note(&mut state, "note_list", &format!(r#"{{"todo_id":{item}}}"#));
    assert_eq!(by_item["count"], 1);
    note(
        &mut state,
        "note_link",
        &format!(r#"{{"note_id":{id},"todo_id":{item},"unlink":true}}"#),
    );
    assert!(stored_note(&state, id).unwrap().todo_ids.is_empty());
}

/// The other direction of the reference: a todo listing says which notes are
/// about each item, and only for the items that have any.
#[test]
fn todo_list_reports_the_notes_linked_to_each_item() {
    let mut state = state_with_project();
    let noted = todo(&mut state, "todo_add", r#"{"text":"noted"}"#)["id"]
        .as_u64()
        .unwrap();
    todo(&mut state, "todo_add", r#"{"text":"bare"}"#);
    let id = note(
        &mut state,
        "note_add",
        &format!(r#"{{"title":"t","body":"b","todo_ids":[{noted}]}}"#),
    )["id"]
        .clone();
    let listed = todo(&mut state, "todo_list", "{}");
    assert_eq!(listed["items"][0]["notes"], serde_json::json!([id]));
    assert!(listed["items"][1].get("notes").is_none());
}

#[test]
fn note_search_finds_a_body_word_and_can_span_every_project() {
    let mut state = state_with_project();
    note(
        &mut state,
        "note_add",
        r#"{"title":"Plan","body":"rebase before merging"}"#,
    );
    note(
        &mut state,
        "note_add",
        r#"{"title":"Rebase","body":"elsewhere","global":true}"#,
    );
    let scoped = note(&mut state, "note_search", r#"{"query":"rebase"}"#);
    assert_eq!(scoped["scope"], "hocus");
    assert_eq!(scoped["matches"].as_array().unwrap().len(), 1);
    assert_eq!(scoped["matches"][0]["snippet"], "rebase before merging");
    assert!(scoped["matches"][0].get("body").is_none());
    let everywhere = note(
        &mut state,
        "note_search",
        r#"{"query":"rebase","all_projects":true}"#,
    );
    assert_eq!(everywhere["matches"].as_array().unwrap().len(), 2);
    assert_eq!(everywhere["truncated"], false);
}

#[test]
fn a_note_tool_needs_its_key_argument() {
    let mut state = state_with_project();
    assert_eq!(
        note_result(&mut state, "note_add", r#"{"body":"b"}"#),
        Some("title is required".to_owned())
    );
    assert_eq!(
        note_result(&mut state, "note_add", r#"{"title":"t"}"#),
        Some("body is required".to_owned())
    );
    assert_eq!(
        note_result(&mut state, "note_link", r#"{"todo_id":1}"#),
        Some("note_id is required".to_owned())
    );
    assert_eq!(
        note_result(&mut state, "note_search", "{}"),
        Some("query is required".to_owned())
    );
}

/// A note write is a host call that has happened or failed by the time it
/// returns, so the queue never parks on it — the same guarantee the todo
/// writes give.
#[test]
fn a_note_write_answers_without_parking_the_queue() {
    let mut state = state_with_project();
    let call = agent_call("note_add", r#"{"title":"why","body":"because"}"#);
    state.pending.push_back(call.clone());

    assert!(state.start_note_write(&call), "the queue advances");
    assert!(matches!(state.status, Status::Idle));
    assert!(state.pending.is_empty());
    let answer = tool_results(&state)
        .last()
        .expect("answered")
        .content
        .clone();
    assert!(answer.contains("because"), "got {answer}");
    assert_eq!(stored_note(&state, 1).unwrap().title, "why");
}
