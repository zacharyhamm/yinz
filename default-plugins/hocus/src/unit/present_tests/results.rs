use super::*;

// -- single objects -----------------------------------------------------

#[test]
fn opening_a_pane_reports_the_id_the_next_command_needs() {
    let raw = r#"{"opened":true,"pane_id":7,"is_plugin":false}"#;
    let shown = present(&call("new_pane", "{}"), raw).expect("presented");
    assert!(shown.contains('7'), "{shown}");
}

#[test]
fn an_agent_response_keeps_outcome_and_actual_checkout_visible() {
    let raw = r#"{"outcome":"ok","spawned":{"pane_id":"terminal_9","branch":"topic","cwd":"/work/tree"}}"#;
    let shown = present(&call("agent_spawn", "{}"), raw).unwrap();
    for value in ["ok", "terminal", "9", "topic", "/work/tree"] {
        assert!(shown.contains(value), "{shown}");
    }
}

#[test]
fn crossing_off_a_todo_names_what_else_it_moved() {
    let raw = r#"{"project":"h","id":2,"text":"**child**\n\n- detail","done":true,
        "cascaded":[3,4],"reopened":[]}"#;
    let shown = present(&call("todo_done", "{}"), raw).expect("presented");
    assert!(shown.starts_with("Crossed off `2` in h.\n"), "{shown}");
    assert!(shown.contains("also crossed off: 3, 4"), "{shown}");
    assert!(!shown.contains("also reopened"), "{shown}");
    assert!(shown.ends_with("\n**child**\n\n- detail\n"), "{shown}");
}

#[test]
fn undoing_a_todo_says_reopened_rather_than_crossed_off() {
    let raw = r#"{"project":"h","id":2,"text":"t","done":false}"#;
    let shown = present(&call("todo_done", "{}"), raw).expect("presented");
    assert_eq!(shown, "Reopened `2` in h.\n\nt\n");
}

#[test]
fn adding_a_note_names_it_its_project_tags_and_links() {
    let raw = r#"{"id":3,"project":"h","title":"Why","tags":["design"],"todo_ids":[4,5]}"#;
    let shown = present(&call("note_add", "{}"), raw).expect("presented");
    assert_eq!(
        shown,
        "Added note `3` in h.\n\n**Why**\ntags: #design\ntodos: 4, 5\n"
    );
}

#[test]
fn a_global_note_and_a_warning_are_both_said() {
    let raw = r#"{"id":3,"global":true,"title":"t","tags":[],"todo_ids":[],
        "warning":"the note was written but its tags and links were not: boom"}"#;
    let shown = present(&call("note_add", "{}"), raw).expect("presented");
    assert!(shown.starts_with("Added note `3` (global).\n"), "{shown}");
    assert!(
        shown.ends_with("_the note was written but its tags and links were not: boom_\n"),
        "{shown}"
    );
}

#[test]
fn archiving_deleting_and_linking_a_note_each_say_what_happened() {
    let archived = r#"{"id":3,"project":"h","title":"t","archived":true}"#;
    assert_eq!(
        present(&call("note_archive", "{}"), archived),
        Some("Archived note `3` in h.\n\n**t**\n".to_owned())
    );
    let restored = r#"{"id":3,"project":"h","title":"t","archived":false}"#;
    assert!(present(&call("note_archive", "{}"), restored)
        .unwrap()
        .starts_with("Unarchived note `3` in h.\n"));
    let deleted = r#"{"id":3,"project":"h","title":"t","deleted":true}"#;
    assert!(present(&call("note_delete", "{}"), deleted)
        .unwrap()
        .starts_with("Deleted note `3` in h.\n"));
    let linked = r#"{"note_id":3,"todo_id":4,"linked":true}"#;
    assert_eq!(
        present(&call("note_link", "{}"), linked),
        Some("Linked note `3` and todo `4`.\n".to_owned())
    );
    let unlinked = r#"{"note_id":3,"todo_id":4,"linked":false}"#;
    assert_eq!(
        present(&call("note_link", "{}"), unlinked),
        Some("Unlinked note `3` and todo `4`.\n".to_owned())
    );
}

// -- slack and files ----------------------------------------------------

/// A non-zero exit is a diagnostic; wrapping it would only bury it.
#[test]
fn a_failed_command_is_shown_exactly_as_it_arrived() {
    let raw = "exit code: 2\n\nstderr:\ndobbs: no such channel";
    assert_eq!(present(&call("slack_read", "{}"), raw), None);
}

#[test]
fn a_slack_payload_is_unwrapped_from_its_exit_envelope() {
    let raw = "exit code: 0\n\nstdout:\n{\"ok\":true}";
    let shown = present(&call("slack_status", "{}"), raw).expect("presented");
    assert!(shown.starts_with("## slack_status\n\n"), "{shown}");
    assert!(shown.contains("- **ok**: true"), "{shown}");
    assert!(!shown.contains("exit code"), "{shown}");
}

#[test]
fn a_silent_success_says_it_finished() {
    assert_eq!(
        present(&call("slack_post", "{}"), "exit code: 0"),
        Some("slack post finished.\n".to_owned())
    );
}

#[test]
fn file_output_is_fenced_so_its_indentation_survives() {
    let raw = "src/main.rs:1:    fn main() {";
    let shown = present(&call("read_file", r#"{"path":"src/main.rs"}"#), raw).expect("presented");
    assert!(
        shown.starts_with("## read_file — src/main.rs\n\n"),
        "{shown}"
    );
    assert!(shown.contains("    fn main() {"), "{shown}");
}

/// `events::file_tool_result` already words these, and better.
#[test]
fn an_empty_search_keeps_its_own_wording() {
    assert_eq!(present(&call("find_files", "{}"), "no matches"), None);
    assert_eq!(present(&call("read_file", "{}"), "no lines"), None);
}

#[test]
fn the_truncation_footer_becomes_prose_rather_than_another_line() {
    let raw = "a.rs\nb.rs\n\n… 2 of 90 lines shown; narrow the search.";
    let shown = present(&call("find_files", "{}"), raw).expect("presented");
    assert!(
        shown.contains("_2 of 90 lines shown; narrow the search._"),
        "{shown}"
    );
    // The footer is out of the fenced block, not inside it.
    assert!(!shown.contains("```\n\n… "), "{shown}");
}
