use super::*;

// -- verbs --------------------------------------------------------------

#[test]
fn a_verb_becomes_a_sentence_naming_the_pane() {
    assert_eq!(
        present(&call("clear_pane", r#"{"pane_id":3}"#), "cleared"),
        Some("Cleared pane 3's scrollback.".to_owned())
    );
}

#[test]
fn a_verb_without_a_pane_id_still_reads() {
    assert_eq!(
        present(&call("close_pane", "{}"), "closed"),
        Some("Closed the pane.".to_owned())
    );
}

/// The two `toggle_pane` results are different sentences, so the table has
/// to be keyed on the result and not only on the tool.
#[test]
fn one_tool_with_two_results_says_two_different_things() {
    let call = call("toggle_pane", r#"{"pane_id":1}"#);
    assert_eq!(
        present(&call, "toggled floating"),
        Some("Toggled floating on pane 1.".to_owned())
    );
    assert_eq!(
        present(&call, "toggled fullscreen"),
        Some("Toggled fullscreen on pane 1.".to_owned())
    );
}

// -- falling back -------------------------------------------------------

/// The whole safety property: anything we have nothing better to say about
/// is shown exactly as it arrived.
#[test]
fn an_error_is_never_dressed_up_as_a_success() {
    for (name, result) in [
        ("clear_pane", "pane_id and is_plugin are both required"),
        ("close_pane", "no such tool: close_pane"),
        (
            "rerun_pane",
            "rerun_pane only supports terminal command panes",
        ),
        ("set_pane_pinned", "pinned is required"),
        ("agent_list", "unknown project x; call project_list"),
    ] {
        assert_eq!(present(&call(name, "{}"), result), None, "{name}: {result}");
    }
}

/// A verb tool whose result is *nearly* its success string is still a
/// failure, and must not match.
#[test]
fn a_near_miss_of_a_success_string_does_not_match() {
    assert_eq!(present(&call("clear_pane", "{}"), "not cleared"), None);
    assert_eq!(present(&call("clear_pane", "{}"), "cleared."), None);
}

#[test]
fn a_tool_that_already_speaks_prose_is_left_alone() {
    assert_eq!(
        present(
            &call("schedule", "{}"),
            "scheduled job 1 — runs every 60s until unscheduled"
        ),
        None
    );
}
