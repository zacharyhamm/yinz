use super::*;

// -- single objects -----------------------------------------------------

#[test]
fn opening_a_pane_reports_the_id_the_next_command_needs() {
    let raw = r#"{"opened":true,"pane_id":7,"is_plugin":false}"#;
    let shown = present(&call("new_pane", "{}"), raw).expect("presented");
    assert!(shown.contains('7'), "{shown}");
}

/// `register_spawned` glues its note onto `new_pane`'s JSON before the
/// result ever reaches us, so the suffix has to be split back off.
#[test]
fn a_spawned_agent_reports_the_kind_it_was_registered_as() {
    let raw = r#"{"opened":true,"pane_id":9} (registered as claude)"#;
    assert_eq!(
        present(&call("agent_spawn", "{}"), raw),
        Some("Spawned an agent in pane 9.\nRegistered as claude.\n".to_owned())
    );
}

#[test]
fn a_spawn_without_a_registration_still_reads() {
    let raw = r#"{"opened":true,"pane_id":9}"#;
    assert_eq!(
        present(&call("agent_spawn", "{}"), raw),
        Some("Spawned an agent in pane 9.\n".to_owned())
    );
}

/// The git route reports where it put the agent. That is the part the user cannot
/// see from the pane id alone, and the part they will need to `cd` to later.
#[test]
fn a_spawn_that_cut_a_worktree_says_which_branch_and_where() {
    let raw = r#"{"opened":true,"pane_id":9,"is_plugin":false,
        "branch":"fix-parser","cwd":"/work/hocus/.worktrees/fix-parser"}"#;
    let shown = present(&call("agent_spawn", "{}"), raw).expect("presented");
    assert!(shown.contains("fix-parser"), "{shown}");
    assert!(
        shown.contains("/work/hocus/.worktrees/fix-parser"),
        "{shown}"
    );
}

/// A branch switched in place has no new folder to name, and saying nothing is
/// better than naming the one it was already in.
#[test]
fn a_spawn_that_only_switched_branch_names_the_branch_alone() {
    let raw = r#"{"opened":true,"pane_id":9,"branch":"topic"}"#;
    assert_eq!(
        present(&call("agent_spawn", "{}"), raw),
        Some("Spawned an agent in pane 9.\nOn a new branch topic.\n".to_owned())
    );
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
