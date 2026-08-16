use super::*;

// -- the command list ---------------------------------------------------

/// Grouping is only useful if it is complete: every command has to appear,
/// and none twice.
#[test]
fn the_command_list_holds_every_command_exactly_once() {
    let listed: Vec<String> = commands()
        .split_whitespace()
        .filter_map(|word| word.strip_prefix('/'))
        .map(str::to_owned)
        .collect();
    let mut expected: Vec<String> = crate::LOCAL_COMMANDS
        .iter()
        .map(|name| (*name).to_owned())
        .chain(tools::names())
        .collect();
    let mut sorted = listed.clone();
    sorted.sort();
    expected.sort();
    assert_eq!(sorted, expected);
    assert_eq!(
        listed.len(),
        expected.len(),
        "a command was listed under two families"
    );
}

#[test]
fn the_command_list_is_grouped_and_plain() {
    let listed = commands();
    for family in ["hocus", "panes", "agents", "todos", "slack"] {
        assert!(listed.contains(family), "{family} missing from {listed}");
    }
    // A notice is not rendered as markdown, so any syntax would be shown
    // literally.
    assert!(!listed.contains('`') && !listed.contains('#'), "{listed}");
}

/// `agent_send` must land under agents, not be dragged into panes by a
/// substring match on `send`.
#[test]
fn a_family_matches_whole_words_not_substrings() {
    assert!(in_family("agent_send", "agent"));
    assert!(!in_family("agent_send", "pane"));
    assert!(in_family("close_pane", "pane"));
    assert!(in_family("rename", "rename"));
}

// -- coverage -----------------------------------------------------------

/// Every advertised tool reaches an arm that answers, one way or the other.
///
/// A `match` on `&str` cannot be exhaustive, so this is what stops a tool
/// added to `tools::specs` from quietly going back to showing raw JSON. It
/// runs each tool's plausible answers through and asserts none of them
/// panics and that a structured one is never left unpresented.
#[test]
fn every_tool_is_covered() {
    for name in tools::names() {
        let call = call(&name, r#"{"pane_id":1,"path":"x"}"#);
        // Whatever it answers, an error is passed through. Not asserted for
        // the file tools: their output is arbitrary text from `rg`/`sed`,
        // so no string is distinguishable from a legitimate answer. Their
        // real failure shapes are the ones `file_tool` checks for, and
        // `an_empty_search_keeps_its_own_wording` covers those.
        if !tools::FILE_TOOLS.contains(&name.as_str()) {
            assert_eq!(
                present(&call, "something went wrong"),
                None,
                "{name} dressed up an error"
            );
        }
        // And anything structured is always turned into something, whether
        // by its own arm or by the generic renderer.
        // A shape no arm recognises still reaches the generic renderer,
        // except for the tools that answer in prose on purpose.
        let verbatim = PROSE.contains(&name.as_str())
            || tools::SLACK_TOOLS.contains(&name.as_str())
            || tools::FILE_TOOLS.contains(&name.as_str());
        assert!(
            present(&call, r#"{"a":1}"#).is_some() || verbatim,
            "{name} left structured output unpresented"
        );
    }
}
