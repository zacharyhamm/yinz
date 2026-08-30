use super::*;

// -- listings -----------------------------------------------------------

/// The shape `tools::list_session` builds.
#[test]
fn a_session_listing_becomes_tabs_of_panes() {
    let raw = r#"{"session":"work","tabs":[{"tab_number":1,"tab_id":1,"name":"edit",
        "active":true,"panes":[
          {"pane_id":3,"is_plugin":false,"title":"vim","focused":true,"floating":false,
           "command":"vim","exit_status":null},
          {"pane_id":4,"is_plugin":false,"title":"build","focused":false,"floating":false,
           "command":"cargo","exit_status":1}]}]}"#;
    let shown = present(&call("list_session", "{}"), raw).expect("presented");
    assert!(shown.starts_with("## session work\n\n"), "{shown}");
    // The heading carries the number the tab bar shows, not the position.
    assert!(shown.contains("**1: edit** — active"), "{shown}");
    // The ids lead, because they are what the next command needs, and the
    // columns line up under them.
    assert!(shown.contains("3   *  vim    running"), "{shown}");
    assert!(shown.contains("4      build  exited 1"), "{shown}");
    // The marks column is only readable with a key, and only one pane here
    // is marked at all.
    assert!(shown.contains("_* focused_"), "{shown}");
    assert!(!shown.contains("floating"), "{shown}");
}

/// A listing is a session's tabs and their panes, with a marks key only when
/// something in it actually carries a mark.
#[test]
fn a_listing_renders_its_tabs_and_their_panes() {
    let raw = r#"{"session":"work","tabs":[{"tab_number":1,"tab_id":1,"name":"edit",
        "active":true,"panes":[
          {"pane_id":3,"is_plugin":false,"title":"vim","focused":true,"floating":false,
           "command":"vim","exit_status":null}]}]}"#;
    let shown = present(&call("list_session", "{}"), raw).expect("presented");
    assert!(!shown.contains("hidden"), "{shown}");
    assert!(shown.contains("3   *  vim    running"), "{shown}");
}

#[test]
fn a_session_with_no_tabs_says_so_rather_than_showing_braces() {
    let shown = present(&call("list_session", "{}"), r#"{"session":"w","tabs":[]}"#);
    assert_eq!(shown, Some("## session w\n\n_no tabs_\n".to_owned()));
}

/// `agents::list_json` answers with a bare array, and in pane order — so the
/// grouping has to be built here, not assumed.
#[test]
fn agents_group_under_their_project_and_branch() {
    let raw = r#"[
        {"pane_id":1,"status":"running","title":"one","project":"a","branch":"main",
         "idle_seconds":0},
        {"pane_id":2,"status":"running","title":"two","project":"b","branch":"topic",
         "idle_seconds":180},
        {"pane_id":3,"status":"running","title":"three","project":"a","branch":"main"}]"#;
    let shown = present(&call("agent_list", "{}"), raw).expect("presented");
    assert!(shown.contains("**a** · main"), "{shown}");
    assert!(shown.contains("**b** · topic"), "{shown}");
    // Interleaved input, one header each.
    assert_eq!(shown.matches("**a** · main").count(), 1, "{shown}");
    assert!(shown.contains("`1` busy — one"), "{shown}");
    assert!(shown.contains("`2` idle 3m — two"), "{shown}");
    // No `idle_seconds` yet means the poll has not seen it.
    assert!(shown.contains("`3` running — three"), "{shown}");
}

#[test]
fn an_agent_without_a_project_still_gets_a_header() {
    let raw = r#"[{"pane_id":1,"status":"exited","title":"x"}]"#;
    let shown = present(&call("agent_list", "{}"), raw).expect("presented");
    assert!(shown.contains("**no project** · no branch"), "{shown}");
    assert!(shown.contains("`1` exited — x"), "{shown}");
}

#[test]
fn no_agents_says_none() {
    let shown = present(&call("agent_list", "{}"), "[]");
    assert_eq!(shown, Some("## agents\n\n_none_\n".to_owned()));
}

/// The shape `todos::list_json` builds, including the child nesting.
#[test]
fn todos_become_a_nested_checklist_keeping_their_ids() {
    let raw = r#"{"project":"hocus","root":"/w/h","open":2,"done":1,
        "items":[{"id":1,"text":"title\n---\n\n- detail",
        "children":[{"id":2,"text":"    code"}]}]}"#;
    let shown = present(&call("todo_list", "{}"), raw).expect("presented");
    assert!(shown.starts_with("## todos — hocus\n\n"), "{shown}");
    assert!(shown.contains("2 open, 1 done"), "{shown}");
    assert!(
        shown.contains("- [ ] `1`\n\n  title\n  ---\n  \n  - detail\n"),
        "{shown}"
    );
    assert!(shown.contains("  - [ ] `2`\n\n        code\n"), "{shown}");
}

#[test]
fn a_done_todo_is_ticked() {
    let raw = r#"{"project":"h","open":0,"done":1,
        "items":[{"id":1,"text":"t","done":true}]}"#;
    let shown = present(&call("todo_list", "{}"), raw).expect("presented");
    assert!(shown.contains("- [x] `1`\n\n  t"), "{shown}");
}

#[test]
fn todo_search_keeps_markdown_in_each_result() {
    let raw = r#"{"query":"ship","matches":[
        {"id":2,"text":"2. ship it\n\n    now","project":"hocus"}]}"#;
    let shown = present(&call("todo_search", "{}"), raw).expect("presented");
    assert!(shown.contains("- [ ] `2` — _hocus_\n\n"), "{shown}");
    assert!(shown.contains("  2. ship it\n  \n      now\n"), "{shown}");
}

#[test]
fn a_project_listing_marks_the_active_one() {
    let raw = r#"{"active":"hocus","projects":[
        {"name":"hocus","root":"/w/h","tab_ids":[1,2]},
        {"name":"other","root":"/w/o","tab_ids":[]}]}"#;
    let shown = present(&call("project_list", "{}"), raw).expect("presented");
    assert!(shown.contains("- **hocus** — active"), "{shown}");
    assert!(shown.contains("- **other**\n"), "{shown}");
    assert!(shown.contains("  - tabs 1, 2"), "{shown}");
}

#[test]
fn orphans_report_an_age_a_person_reads() {
    let raw = r#"{"orphans":[{"host_id":"abc","command":"top","session":"old",
        "exited":false,"age_seconds":7200}]}"#;
    let shown = present(&call("list_orphaned_panes", "{}"), raw).expect("presented");
    assert!(shown.contains("abc"), "{shown}");
    assert!(shown.contains("2h"), "{shown}");
}
