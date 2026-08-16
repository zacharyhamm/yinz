use super::*;

#[test]
fn confirmation_policy_lists_tools_correctly() {
    for name in [
        "rerun_pane",
        "clear_pane",
        "send_input",
        "agent_send",
        "agent_control",
        // Forgetting a project is the one project tool that takes
        // something away.
        "project_forget",
        "slack_post",
        // Moving a pane takes it off the screen, and briefly leaves it
        // belonging to no session at all.
        "warp_pane",
        "warp_pane_here",
        "adopt_orphaned_pane",
    ] {
        assert!(needs_confirmation(name), "{name} should be confirmed");
    }
    for name in [
        "list_session",
        // Listing is always free, including the two that look outward.
        "list_other_sessions",
        "list_orphaned_panes",
        "read_pane",
        "new_pane",
        "new_tab",
        "focus",
        "rename",
        "move_pane",
        "swap_panes",
        "resize_pane",
        "toggle_pane",
        "move_panes_to_tab",
        "stack_panes",
        "set_pane_pinned",
        "set_floating_pane_geometry",
        "scroll_pane",
        "web_search",
        "schedule",
        "unschedule",
        "agent_register",
        "agent_list",
        "close_pane",
        "close_tab",
        // A todo tool only writes to a file of the user's own notes.
        "todo_add",
        "todo_list",
        "todo_search",
        "todo_done",
        "todo_edit",
        // Reading Slack is free; only posting stops for a y/n.
        "slack_status",
        "slack_search",
        "slack_read",
        "slack_backfill",
        // Reading the project's own files changes nothing, and the root
        // check is what keeps them the project's own.
        "find_files",
        "search_files",
        "read_file",
    ] {
        assert!(!needs_confirmation(name), "{name} should not be confirmed");
    }
}

/// The confirmation prompt is the only thing standing between the model and
/// a closed pane, so every advertised tool must be in the table above.
#[test]
fn every_advertised_tool_is_covered_by_the_confirmation_table() {
    let specs = specs();
    let names: Vec<&str> = specs
        .as_array()
        .unwrap()
        .iter()
        .map(|spec| spec["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 52);
    let confirmed = names.iter().filter(|name| needs_confirmation(name)).count();
    assert_eq!(confirmed, 13);
}

/// The same guard as the confirmation table above, one layer along: a tool
/// that stops to ask and then cannot say what it will do is a prompt with
/// nothing in it but the model's JSON.
#[test]
fn every_confirmed_tool_says_what_it_will_do() {
    for name in names().iter().filter(|name| needs_confirmation(name)) {
        let sentence = consequence(&call(name, "{}"));
        assert!(sentence.is_some(), "{name} should say what it will do");
        assert!(
            !sentence.expect("checked").trim().is_empty(),
            "{name}'s consequence should not be blank"
        );
    }
}

#[test]
fn a_tool_that_never_asks_has_no_consequence() {
    assert_eq!(consequence(&call("list_session", "{}")), None);
}

/// `schedule` is not in the confirmation table, but asking for standing
/// permission stops `pump_tools` all the same - and only then.
#[test]
fn scheduling_only_says_anything_when_it_asks_for_standing_permission() {
    assert_eq!(
        consequence(&call(
            "schedule",
            r#"{"prompt":"check ci","auto_confirm":true}"#
        )),
        Some("lets every run of this job use tools without stopping to ask again".into())
    );
    assert_eq!(
        consequence(&call("schedule", r#"{"prompt":"check ci"}"#)),
        None
    );
}

#[test]
fn a_consequence_names_the_pane_it_will_act_on() {
    assert_eq!(
        consequence(&call("send_input", r#"{"pane_id":3,"text":"hi"}"#)),
        Some("types into pane 3, as if you had typed it yourself".into())
    );
}

/// `agent_control`'s argument is a verb, and interrupting is not restarting.
#[test]
fn agent_control_says_which_of_its_verbs_is_about_to_run() {
    let restart = consequence(&call(
        "agent_control",
        r#"{"pane_id":7,"action":"restart"}"#,
    ));
    assert_eq!(
        restart,
        Some("restarts the agent in pane 7, losing its conversation".into())
    );
    let close = consequence(&call("agent_control", r#"{"pane_id":7,"action":"close"}"#));
    assert_eq!(
        close,
        Some("closes pane 7 and the agent running in it".into())
    );
}

/// Arguments the model got wrong are exactly when a prompt must not go quiet.
#[test]
fn a_consequence_survives_arguments_that_do_not_parse() {
    let sentence = consequence(&call("send_input", "not json at all"));
    assert_eq!(
        sentence,
        Some("types into a pane, as if you had typed it yourself".into())
    );
}

#[test]
fn arguments_are_one_readable_line_each_rather_than_json() {
    assert_eq!(
        argument_lines(&call("send_input", r#"{"pane_id":3,"text":"git push"}"#)),
        vec!["pane_id: 3", "text: git push"]
    );
}

#[test]
fn an_empty_argument_object_contributes_no_lines() {
    assert!(argument_lines(&call("clear_pane", "{}")).is_empty());
    assert!(argument_lines(&call("clear_pane", "")).is_empty());
}

#[test]
fn arguments_that_do_not_parse_are_shown_verbatim_rather_than_dropped() {
    assert_eq!(
        argument_lines(&call("send_input", "not json at all")),
        vec!["not json at all"]
    );
}

#[test]
fn the_confirmation_block_names_the_call_what_it_does_and_what_it_was_given() {
    assert_eq!(
        confirmation_detail(&call("clear_pane", r#"{"pane_id":4}"#)),
        vec![
            "clear_pane",
            "  throws away pane 4's scrollback; it cannot be scrolled back",
            "  pane_id: 4",
        ]
    );
}

/// A toast is read from a pane that has nothing to do with hocus, so it has
/// to say who is asking before it says what for.
#[test]
fn the_toast_says_who_is_asking_before_the_block() {
    let toast = confirmation_toast(&call("clear_pane", r#"{"pane_id":4}"#));
    let mut lines = toast.lines();
    assert_eq!(lines.next(), Some(TOAST_HEADLINE));
    assert_eq!(lines.next(), Some("clear_pane"));
    assert!(toast.ends_with("  pane_id: 4"), "got {toast:?}");
}
