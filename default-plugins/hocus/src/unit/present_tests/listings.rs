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
    // The ids lead, because they are what the next command needs. Lining the
    // columns up is the renderer's job now that this emits a real table.
    assert!(shown.contains("| id |  | title | status |"), "{shown}");
    assert!(shown.contains("| 3 | \\* | vim | running |"), "{shown}");
    assert!(shown.contains("| 4 |  | build | exited 1 |"), "{shown}");
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
    assert!(shown.contains("| 3 | \\* | vim | running |"), "{shown}");
}

#[test]
fn a_session_with_no_tabs_says_so_rather_than_showing_braces() {
    let shown = present(&call("list_session", "{}"), r#"{"session":"w","tabs":[]}"#);
    assert_eq!(shown, Some("## session w\n\n_no tabs_\n".to_owned()));
}

#[test]
fn authoritative_agent_list_keeps_readiness_separate_from_state() {
    let raw = r#"{"outcome":"ok","agents":[{"pane_id":"terminal_1","agent":{"kind":"codex","state":"Idle"},"ready":false,"status_available":true}]}"#;
    let shown = present(&call("agent_list", "{}"), raw).unwrap();
    for value in ["codex", "idle", "ready", "false"] {
        assert!(shown.contains(value), "{shown}");
    }
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

/// An item with notes names them by id on its own line, and one without says
/// nothing about notes at all.
#[test]
fn todo_items_show_the_ids_of_their_notes() {
    let raw = r#"{"project":"h","open":2,"done":0,
        "items":[{"id":1,"text":"noted","notes":[3,7]},{"id":2,"text":"bare"}]}"#;
    let shown = present(&call("todo_list", "{}"), raw).expect("presented");
    assert!(
        shown.contains("- [ ] `1` · notes 3, 7\n\n  noted\n"),
        "{shown}"
    );
    assert!(shown.contains("- [ ] `2`\n\n  bare\n"), "{shown}");
}

/// The shape `note_list` answers with: no bodies, everything else on one line.
#[test]
fn notes_are_listed_with_ids_tags_links_and_age_but_no_body() {
    let raw = r#"{"scope":"hocus","count":2,"notes":[
        {"id":3,"title":"Why *this*","author":"hocus","archived":false,"tags":["design","rust"],
         "todo_ids":[4,5],"age_seconds":7200},
        {"id":4,"title":"Old","author":"user","archived":true,"tags":[],"todo_ids":[],"age_seconds":10}]}"#;
    let shown = present(&call("note_list", "{}"), raw).expect("presented");
    assert!(
        shown.starts_with("## notes — hocus\n\n2 notes\n\n"),
        "{shown}"
    );
    assert!(
        shown.contains("- `3` **Why \\*this\\*** — 2h · #design #rust · todos 4, 5 · by hocus\n"),
        "{shown}"
    );
    assert!(
        shown.contains("- `4` **Old** _(archived)_ — just now\n"),
        "{shown}"
    );
    assert!(
        !shown.contains("by user"),
        "the user's own notes need no byline: {shown}"
    );
    let empty = present(
        &call("note_list", "{}"),
        r#"{"scope":"global","count":0,"notes":[]}"#,
    );
    assert_eq!(empty, Some("## notes — global\n\n_none_\n".to_owned()));
}

#[test]
fn note_get_shows_the_body_as_markdown_under_a_meta_line() {
    let raw = r##"{"id":3,"title":"Why","project":"hocus","author":"hocus","archived":false,
        "tags":["design"],"todo_ids":[4],"age_seconds":90,"body":"# because\n\n- reasons"}"##;
    let shown = present(&call("note_get", "{}"), raw).expect("presented");
    assert!(shown.starts_with("## note 3 — Why\n\n"), "{shown}");
    assert!(
        shown.contains("_hocus · by hocus · 1m · #design · todos 4_\n"),
        "{shown}"
    );
    assert!(shown.ends_with("\n# because\n\n- reasons\n"), "{shown}");
    let global = r#"{"id":1,"title":"g","global":true,"author":"user","body":"b","age_seconds":5}"#;
    let shown = present(&call("note_get", "{}"), global).expect("presented");
    assert!(shown.contains("_global · by user · just now_\n"), "{shown}");
}

#[test]
fn note_search_shows_a_snippet_under_each_hit() {
    let raw = r#"{"query":"rebase","matches":[
        {"id":2,"title":"Plan","project":"hocus","author":"user","tags":[],"todo_ids":[],
         "age_seconds":30,"snippet":"rebase before merging"}],"truncated":true}"#;
    let shown = present(&call("note_search", "{}"), raw).expect("presented");
    assert!(shown.starts_with("## note search — rebase\n\n"), "{shown}");
    assert!(
        shown.contains("- `2` **Plan** — just now\n  rebase before merging\n"),
        "{shown}"
    );
    assert!(
        shown.ends_with("_more matches were cut; narrow the query_\n"),
        "{shown}"
    );
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

// -- agent run history --------------------------------------------------

#[test]
fn a_run_listing_leads_with_the_id_the_next_call_needs() {
    let raw = r#"{"scope":"all sessions","count":2,"offset":0,"more":true,"next_offset":2,
        "runs":[
          {"run_id":"claude:one","kind":"claude","title":"fix the parser","project":"hocus",
           "age":"3h","outcome":"completed","summary":"rewrote the snapshot helper"},
          {"run_id":"codex:two","kind":"codex","title":"benchmark it","project":null,
           "age":"1d","outcome":"active","summary":null}]}"#;
    let shown = present(&call("agent_runs", "{}"), raw).expect("presented");
    assert!(
        shown.starts_with("## agent runs — all sessions\n\n"),
        "{shown}"
    );
    assert!(shown.contains("`claude:one` **fix the parser**"), "{shown}");
    assert!(shown.contains("claude · hocus · 3h · completed"), "{shown}");
    // A summary earns its own indented line; a run without one takes no space.
    assert!(shown.contains("  rewrote the snapshot helper"), "{shown}");
    assert!(shown.contains("codex · 1d · active"), "{shown}");
    assert!(shown.contains("offset=2"), "{shown}");
}

#[test]
fn an_empty_run_listing_says_so_rather_than_showing_a_bare_heading() {
    let raw = r#"{"scope":"session main","count":0,"offset":0,"more":false,"runs":[]}"#;
    let shown = present(&call("agent_runs", "{}"), raw).expect("presented");
    assert!(shown.contains("_no runs_"), "{shown}");
}

/// An entry is the agent's own words and is already markdown, so it goes through
/// as markdown — the same choice `note_get` makes about a note's body.
#[test]
fn a_run_shows_its_entries_as_markdown_under_their_kind() {
    let raw = r#"{"run_id":"claude:one","kind":"claude","title":"fix the parser",
        "project":"hocus","outcome":"active","entry_count":9,"shown":2,"earlier":true,
        "next_revision":6,"more":"agent_run run_id=claude:one from_revision=6",
        "entries":[
          {"revision":4,"kind":"prompt","age":"3h","text":"fix the **flaky** test"},
          {"revision":5,"kind":"plan","age":"2h","text":"- rewrite it\n- rerun"}]}"#;
    let shown = present(&call("agent_run", "{}"), raw).expect("presented");
    assert!(shown.starts_with("## fix the parser\n\n"), "{shown}");
    assert!(
        shown.contains("claude:one · claude · hocus · active · 2 of 9 entries"),
        "{shown}"
    );
    assert!(shown.contains("_earlier entries not shown_"), "{shown}");
    assert!(shown.contains("**prompt · 3h**"), "{shown}");
    // Passed through, not escaped into `\*\*flaky\*\*`.
    assert!(shown.contains("fix the **flaky** test"), "{shown}");
    assert!(shown.contains("- rewrite it\n- rerun"), "{shown}");
    // The footer names the exact call, so a fragment is never mistaken for the whole.
    assert!(
        shown.contains("`agent_run run_id=claude:one from_revision=6`"),
        "{shown}"
    );
}

#[test]
fn a_run_with_no_entries_says_so() {
    let raw = r#"{"run_id":"claude:one","kind":"claude","title":"t","outcome":"active",
        "entry_count":0,"shown":0,"earlier":false,"next_revision":null,"more":null,
        "entries":[]}"#;
    let shown = present(&call("agent_run", "{}"), raw).expect("presented");
    assert!(shown.contains("_no entries_"), "{shown}");
}

#[test]
fn a_history_search_shows_a_snippet_under_each_hit() {
    let raw = r#"{"query":"parser","scope":"all sessions","count":1,"truncated":true,
        "matches":[{"run_id":"claude:one","revision":3,"kind":"result","agent":"claude",
                    "age":"3h","snippet":"…the parser now handles tabs…",
                    "run_title":"fix the parser"}]}"#;
    let shown = present(&call("agent_history_search", "{}"), raw).expect("presented");
    assert!(
        shown.starts_with("## agent history — parser\n\n"),
        "{shown}"
    );
    assert!(shown.contains("**fix the parser**"), "{shown}");
    assert!(shown.contains("result · claude · 3h"), "{shown}");
    assert!(shown.contains("  …the parser now handles tabs…"), "{shown}");
    assert!(shown.contains("narrow the query"), "{shown}");
}

#[test]
fn a_history_search_with_no_hits_says_so() {
    let raw = r#"{"query":"nothing","scope":"all sessions","count":0,"truncated":false,
        "matches":[]}"#;
    let shown = present(&call("agent_history_search", "{}"), raw).expect("presented");
    assert!(shown.contains("_no matches_"), "{shown}");
}
