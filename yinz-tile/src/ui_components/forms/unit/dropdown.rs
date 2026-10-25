use super::*;

fn options() -> Vec<String> {
    ["default", "compact", "dev-three-pane"]
        .iter()
        .map(|o| o.to_string())
        .collect()
}

fn a_dropdown() -> Dropdown {
    let mut dropdown = Dropdown::default();
    dropdown.refilter(&options(), "");
    dropdown
}

#[test]
fn a_fresh_dropdown_highlights_nothing() {
    let dropdown = a_dropdown();
    assert_eq!(dropdown.matches().len(), 3);
    assert_eq!(dropdown.highlighted(), None);
    assert_eq!(dropdown.highlighted_index(), None);
}

#[test]
fn moving_from_nothing_lands_at_either_end() {
    let mut forward = a_dropdown();
    forward.move_highlight(true);
    assert_eq!(forward.highlighted_index(), Some(0));

    let mut backward = a_dropdown();
    backward.move_highlight(false);
    assert_eq!(backward.highlighted_index(), Some(2));
}

#[test]
fn the_highlight_wraps_at_both_ends() {
    let mut dropdown = a_dropdown();
    dropdown.set_highlighted(Some(2));
    dropdown.move_highlight(true);
    assert_eq!(dropdown.highlighted(), Some(0));
    dropdown.move_highlight(false);
    assert_eq!(dropdown.highlighted(), Some(2));
}

#[test]
fn moving_in_an_empty_list_highlights_nothing() {
    let mut dropdown = Dropdown::default();
    dropdown.refilter(&options(), "zzzz");
    dropdown.move_highlight(true);
    assert_eq!(dropdown.highlighted(), None);
}

#[test]
fn narrowing_clamps_the_highlight_onto_a_visible_row() {
    let mut dropdown = a_dropdown();
    dropdown.set_highlighted(Some(2));
    dropdown.refilter(&options(), "def");
    assert_eq!(dropdown.matches().len(), 1);
    assert_eq!(dropdown.highlighted(), Some(0));
}

#[test]
fn a_filter_that_matches_nothing_drops_the_highlight() {
    let mut dropdown = a_dropdown();
    dropdown.set_highlighted(Some(1));
    dropdown.refilter(&options(), "zzzz");
    assert_eq!(dropdown.highlighted(), None);
    assert_eq!(dropdown.highlighted_index(), None);
}

/// A select re-anchors after every refilter; a combo never does.
#[test]
fn ensure_highlight_only_fills_in_a_missing_one() {
    let mut dropdown = a_dropdown();
    dropdown.ensure_highlight();
    assert_eq!(dropdown.highlighted(), Some(0));
    dropdown.set_highlighted(Some(2));
    dropdown.ensure_highlight();
    assert_eq!(dropdown.highlighted(), Some(2));

    let mut empty = Dropdown::default();
    empty.refilter(&options(), "zzzz");
    empty.ensure_highlight();
    assert_eq!(empty.highlighted(), None);
}

#[test]
fn a_highlight_past_the_end_is_refused() {
    let mut dropdown = a_dropdown();
    dropdown.set_highlighted(Some(9));
    assert_eq!(dropdown.highlighted(), None);
}

#[test]
fn a_dropdown_that_never_rendered_covers_nothing_and_hits_nothing() {
    let dropdown = a_dropdown();
    assert_eq!(dropdown.overlay_rect(), None);
    assert_eq!(dropdown.option_at(0, 1), None);
}

#[test]
fn clearing_forgets_the_matches_and_the_highlight() {
    let mut dropdown = a_dropdown();
    dropdown.set_highlighted(Some(1));
    dropdown.clear();
    assert!(dropdown.is_empty());
    assert_eq!(dropdown.highlighted(), None);
}

#[test]
fn hover_moves_the_highlight_only_when_it_has_somewhere_to_go() {
    let mut dropdown = a_dropdown();
    assert!(dropdown.highlight_option(1));
    assert_eq!(dropdown.highlighted(), Some(1));
    assert!(
        !dropdown.highlight_option(1),
        "the highlight was already there"
    );
    dropdown.refilter(&options(), "def");
    assert!(
        !dropdown.highlight_option(1),
        "'compact' is not in the matches"
    );
}
