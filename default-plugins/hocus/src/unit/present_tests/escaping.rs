use super::*;

// -- escaping -----------------------------------------------------------

/// A pane title is not ours, and markdown in one must not reach the
/// renderer as markup.
#[test]
fn markup_in_a_value_is_escaped() {
    let raw = r##"{"title":"# heading *bold*"}"##;
    let shown = present(&call("some_future_tool", "{}"), raw).expect("presented");
    assert!(shown.contains(r"\# heading \*bold\*"), "{shown}");
}

#[test]
fn a_newline_in_a_value_cannot_break_the_list_item() {
    let raw = r#"{"title":"one\ntwo"}"#;
    let shown = present(&call("some_future_tool", "{}"), raw).expect("presented");
    assert_eq!(shown, "- **title**: one two\n");
}

/// The deliberate half of the escaping rule: an identifier must survive
/// intact, or every listing is full of backslashes.
#[test]
fn an_underscore_is_left_alone() {
    assert_eq!(inline("pane_id"), "pane_id");
}

/// `|` and `~` were on that list until the renderer learned tables and
/// strikethrough. A pipe in a pane title is a cell boundary now.
#[test]
fn a_pipe_and_a_tilde_are_escaped() {
    assert_eq!(inline("a|b~~c~~"), r"a\|b\~\~c\~\~");
}

/// A backtick in a value must not be able to open a code span that swallows
/// the rest of the line.
#[test]
fn a_backtick_in_a_value_is_escaped() {
    assert_eq!(inline("a `b` c"), r"a \`b\` c");
}

#[test]
fn fence_survives_a_body_containing_backticks() {
    let body = "run ``` then ```` more";
    let fenced = fence(body);
    let ticks = fenced.lines().next().expect("a fence line");
    assert!(
        ticks.len() > 4,
        "fence {ticks:?} is not longer than the body's longest run"
    );
    assert!(fenced.contains(body));
}

#[test]
fn fence_of_a_plain_body_is_the_usual_three() {
    assert_eq!(fence("hello"), "```\nhello\n```\n");
}

/// A listing is the likeliest place for a hostile pane title to land. The table
/// it goes in used to be fenced, which is what made the title inert; the table
/// is real markdown now, so the escaping is what has to do it instead - the
/// title must stay one cell, and none of its markup may reach the renderer.
#[test]
fn a_pane_title_full_of_markup_cannot_restyle_the_transcript() {
    let raw = r##"{"session":"s","tabs":[{"tab_number":1,"tab_id":1,"name":"t","active":false,
        "panes":[{"pane_id":1,"title":"# **x** ``` | y","focused":false}]}]}"##;
    let shown = present(&call("list_session", "{}"), raw).expect("presented");
    let row = shown
        .lines()
        .find(|line| line.contains('x'))
        .expect("the pane's row");
    assert!(row.contains(r"\# \*\*x\*\* \`\`\` \| y"), "{row}");
    // Four columns in the header, and the title's pipe has not made a fifth.
    let header = shown
        .lines()
        .find(|line| line.contains("title"))
        .expect("the header");
    assert_eq!(
        row.matches(" | ").count(),
        header.matches(" | ").count(),
        "{row}"
    );
}
