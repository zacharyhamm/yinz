use super::*;
use hegel::generators;

fn a_select() -> Select {
    Select::new(["default", "compact", "dev-three-pane"])
}

fn key(bare_key: BareKey) -> KeyWithModifier {
    KeyWithModifier::new(bare_key)
}

fn char_key(c: char) -> KeyWithModifier {
    KeyWithModifier::new(BareKey::Char(c))
}

#[test]
fn a_new_select_has_the_first_option_and_is_closed() {
    let select = a_select();
    assert_eq!(select.selected_option(), Some("default"));
    assert!(!select.is_open());
}

#[test]
fn a_select_with_no_options_has_no_value() {
    let select = Select::new(Vec::<String>::new());
    assert_eq!(select.selected_option(), None);
    assert_eq!(select.selected_index(), None);
}

/// The containing form handles Enter on a closed select to activate it.
#[test]
fn enter_on_a_closed_select_is_not_consumed() {
    let mut select = a_select();
    assert_eq!(
        select.handle_key(key(BareKey::Enter)),
        SelectAction::NoAction
    );
    assert!(!select.is_open());
    assert_eq!(select.handle_key(key(BareKey::Esc)), SelectAction::NoAction);
}

#[test]
fn space_and_the_arrows_open_the_list() {
    for opener in [BareKey::Char(' '), BareKey::Down, BareKey::Up] {
        let mut select = a_select();
        assert_eq!(select.handle_key(key(opener)), SelectAction::Continue);
        assert!(select.is_open(), "{:?} did not open the list", opener);
        assert_eq!(select.filter_text(), "");
    }
}

#[test]
fn opening_puts_the_highlight_on_the_current_choice() {
    let mut select = a_select().with_selected(2);
    select.open();
    assert_eq!(select.highlighted_option(), Some("dev-three-pane"));
}

#[test]
fn typing_opens_the_list_and_filters_it() {
    let mut select = a_select();
    select.handle_key(char_key('c'));
    assert!(select.is_open());
    assert_eq!(select.filter_text(), "c");
    assert_eq!(
        select.dropdown.matches().len(),
        1,
        "only 'compact' contains a 'c': {:?}",
        select.dropdown.matches()
    );
    assert_eq!(select.highlighted_option(), Some("compact"));
}

#[test]
fn enter_commits_the_highlighted_option_and_closes() {
    let mut select = a_select();
    select.handle_key(char_key('c'));
    assert_eq!(
        select.handle_key(key(BareKey::Enter)),
        SelectAction::Committed
    );
    assert_eq!(select.selected_option(), Some("compact"));
    assert!(!select.is_open());
    assert_eq!(select.filter_text(), "", "the filter outlived the dropdown");
}

#[test]
fn esc_closes_without_changing_the_value() {
    let mut select = a_select();
    select.handle_key(char_key('c'));
    assert_eq!(
        select.handle_key(key(BareKey::Esc)),
        SelectAction::Cancelled
    );
    assert!(!select.is_open());
    assert_eq!(select.selected_option(), Some("default"));
}

#[test]
fn the_arrows_move_the_highlight_and_wrap() {
    let mut select = a_select();
    select.open();
    select.handle_key(key(BareKey::Down));
    assert_eq!(select.highlighted_option(), Some("compact"));
    select.handle_key(key(BareKey::Up));
    assert_eq!(select.highlighted_option(), Some("default"));
    select.handle_key(key(BareKey::Up));
    assert_eq!(
        select.highlighted_option(),
        Some("dev-three-pane"),
        "up from the first option should wrap to the last"
    );
}

/// The highlight indexing into the old, longer match list is how a select
/// commits an option the user cannot see.
#[test]
fn narrowing_the_filter_keeps_the_highlight_on_a_visible_row() {
    let mut select = a_select();
    select.open();
    select.handle_key(key(BareKey::Down));
    select.handle_key(key(BareKey::Down));
    assert_eq!(select.highlighted_option(), Some("dev-three-pane"));
    select.handle_key(char_key('d'));
    select.handle_key(char_key('e'));
    select.handle_key(char_key('f'));
    assert_eq!(select.highlighted_option(), Some("default"));
}

#[test]
fn a_filter_that_matches_nothing_commits_nothing() {
    let mut select = a_select();
    select.open();
    for c in "zzzz".chars() {
        select.handle_key(char_key(c));
    }
    assert_eq!(select.highlighted_option(), None);
    assert_eq!(
        select.handle_key(key(BareKey::Enter)),
        SelectAction::Continue,
        "committing an empty list should not close the dropdown"
    );
    assert!(select.is_open());
    assert_eq!(select.selected_option(), Some("default"));
}

#[test]
fn backspacing_the_filter_widens_the_list_again() {
    let mut select = a_select();
    select.open();
    select.handle_key(char_key('z'));
    assert!(select.dropdown.is_empty());
    select.handle_key(key(BareKey::Backspace));
    assert_eq!(select.dropdown.matches().len(), 3);
    assert_eq!(
        select.highlighted_option(),
        Some("default"),
        "a list with rows in it always has one of them highlighted"
    );
}

#[test]
fn refreshed_options_keep_the_selection_on_the_same_label() {
    let mut select = a_select().with_selected(2);
    select.set_options(
        ["new-one", "default", "dev-three-pane"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
    );
    assert_eq!(select.selected_option(), Some("dev-three-pane"));
    assert_eq!(select.selected_index(), Some(2));
}

#[test]
fn a_selection_whose_option_is_gone_falls_back_to_the_first() {
    let mut select = a_select().with_selected(2);
    select.set_options(vec!["only-one".to_string()]);
    assert_eq!(select.selected_option(), Some("only-one"));
    select.set_options(vec![]);
    assert_eq!(select.selected_option(), None);
}

#[test]
fn a_select_that_never_rendered_has_no_overlay_and_hits_nothing() {
    let mut select = a_select();
    select.open();
    assert_eq!(select.overlay_rect(), None);
    assert!(!select.row_contains(0, 0));
    assert_eq!(select.option_at(0, 1), None);
}

#[test]
fn a_closed_select_shows_its_value_and_a_marker() {
    let select = a_select();
    assert_eq!(select.closed_line("Layout: ", 40), "Layout: default ▾");
}

#[test]
fn a_closed_select_with_no_value_shows_its_placeholder() {
    let select = Select::new(Vec::<String>::new()).placeholder("none available");
    assert_eq!(
        select.closed_line("Layout: ", 40),
        "Layout: none available ▾"
    );
}

#[test]
fn a_collapsed_row_never_exceeds_its_width() {
    let select = a_select().with_selected(2);
    let line = select.closed_line("Layout: ", 14);
    assert!(
        line.as_str().width() <= 14,
        "{:?} is wider than 14 columns",
        line
    );
}

#[test]
fn a_closed_select_places_no_cursor() {
    let mut select = a_select();
    assert_eq!(select.cursor_column("Layout: ", 40), None);
    select.open();
    assert_eq!(select.cursor_column("Layout: ", 40), Some(8));
}

/// Whatever the user types, the value stays one of the options — that is
/// what makes this a select rather than a text field with suggestions.
#[hegel::test]
fn the_value_is_always_one_of_the_options(tc: hegel::TestCase) {
    let options: Vec<String> = tc.draw(
        generators::vecs(generators::text())
            .min_size(1)
            .max_size(10),
    );
    let mut select = Select::new(options.clone());
    let keys = tc.draw(generators::vecs(generators::integers::<u8>()).max_size(40));

    for k in keys {
        let key = match k % 6 {
            0 => key(BareKey::Enter),
            1 => key(BareKey::Esc),
            2 => key(BareKey::Down),
            3 => key(BareKey::Up),
            4 => key(BareKey::Backspace),
            _ => char_key(char::from(b'a' + (k % 26))),
        };
        select.handle_key(key);
        if let Some(value) = select.selected_option() {
            assert!(
                options.iter().any(|o| o == value),
                "{:?} is not one of {:?}",
                value,
                options
            );
        }
    }
}

/// An out-of-range highlight is an Enter away from committing the wrong
/// option or nothing at all.
#[hegel::test]
fn the_highlight_stays_inside_the_matches(tc: hegel::TestCase) {
    let options: Vec<String> = tc.draw(generators::vecs(generators::text()).max_size(10));
    let query = tc.draw(generators::text());
    let mut select = Select::new(options);
    select.open();
    for c in query.chars() {
        select.handle_key(char_key(c));
    }
    if select.dropdown.is_empty() {
        assert_eq!(select.highlighted_option(), None);
    } else {
        assert!(
            select.highlighted_option().is_some(),
            "an open select with {} matches highlighted nothing",
            select.dropdown.matches().len()
        );
    }
}
