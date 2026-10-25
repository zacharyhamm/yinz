use super::*;
use hegel::generators;

const SUGGESTIONS: [&str; 3] = ["main", "feature/login", "feature/logout"];

fn a_combo() -> Combo {
    Combo::new(SUGGESTIONS)
}

fn key(bare_key: BareKey) -> KeyWithModifier {
    KeyWithModifier::new(bare_key)
}

fn char_key(c: char) -> KeyWithModifier {
    KeyWithModifier::new(BareKey::Char(c))
}

fn type_text(combo: &mut Combo, text: &str) {
    for c in text.chars() {
        combo.handle_key(char_key(c));
    }
}

#[test]
fn a_new_combo_is_empty_and_closed() {
    let combo = a_combo();
    assert_eq!(combo.text(), "");
    assert!(!combo.is_open());
    assert_eq!(combo.highlighted_suggestion(), None);
}

#[test]
fn typing_goes_into_the_value_and_opens_the_list() {
    let mut combo = a_combo();
    type_text(&mut combo, "fea");
    assert_eq!(combo.text(), "fea");
    assert!(combo.is_open());
    assert_eq!(combo.dropdown.matches().len(), 2);
    assert_eq!(
        combo.highlighted_suggestion(),
        None,
        "typing must not put the highlight on a row"
    );
}

#[test]
fn typing_something_nothing_matches_leaves_no_list_in_the_way() {
    let mut combo = a_combo();
    type_text(&mut combo, "zzz");
    assert_eq!(combo.text(), "zzz");
    assert!(!combo.is_open());
}

#[test]
fn a_list_with_nothing_in_it_comes_back_when_the_text_matches_again() {
    let mut combo = a_combo();
    type_text(&mut combo, "mz");
    assert!(!combo.is_open());
    combo.handle_key(key(BareKey::Backspace));
    assert!(combo.is_open());
    assert_eq!(combo.text(), "m");
}

/// The whole point of the widget: a value the user typed is the value, and
/// Enter is what commits it.
#[test]
fn enter_with_nothing_highlighted_keeps_what_was_typed() {
    let mut combo = a_combo();
    type_text(&mut combo, "fea");
    assert_eq!(
        combo.handle_key(key(BareKey::Enter)),
        ComboAction::Input(InputAction::Submit),
        "the Enter belongs to the form when no suggestion is highlighted"
    );
    assert_eq!(combo.text(), "fea");
    assert!(!combo.is_open());
}

#[test]
fn down_then_enter_takes_the_suggestion() {
    let mut combo = a_combo();
    type_text(&mut combo, "fea");
    assert_eq!(combo.handle_key(key(BareKey::Down)), ComboAction::Continue);
    assert_eq!(combo.highlighted_suggestion(), Some("feature/login"));
    assert_eq!(
        combo.handle_key(key(BareKey::Enter)),
        ComboAction::Committed
    );
    assert_eq!(combo.text(), "feature/login");
    assert!(!combo.is_open());
    assert_eq!(
        combo.input.get_cursor_position(),
        "feature/login".chars().count(),
        "the caret should be at the end of the suggestion, ready to be edited"
    );
}

#[test]
fn down_on_a_closed_combo_opens_the_list_at_the_first_row() {
    let mut combo = a_combo();
    combo.handle_key(key(BareKey::Down));
    assert!(combo.is_open());
    assert_eq!(combo.highlighted_suggestion(), Some("main"));
}

#[test]
fn up_reaches_the_last_row_first() {
    let mut combo = a_combo();
    combo.handle_key(key(BareKey::Up));
    assert!(combo.is_open());
    assert_eq!(combo.highlighted_suggestion(), Some("feature/logout"));
}

#[test]
fn typing_after_arrowing_drops_the_highlight_again() {
    let mut combo = a_combo();
    type_text(&mut combo, "fea");
    combo.handle_key(key(BareKey::Down));
    assert!(combo.highlighted_suggestion().is_some());
    combo.handle_key(char_key('t'));
    assert_eq!(
        combo.highlighted_suggestion(),
        None,
        "a highlight chosen against text that no longer exists would undo the keystroke"
    );
    assert_eq!(combo.text(), "feat");
}

#[test]
fn esc_closes_the_list_and_keeps_the_text() {
    let mut combo = a_combo();
    type_text(&mut combo, "fea");
    assert_eq!(combo.handle_key(key(BareKey::Esc)), ComboAction::Cancelled);
    assert!(!combo.is_open());
    assert_eq!(combo.text(), "fea");
    assert_eq!(
        combo.handle_key(key(BareKey::Esc)),
        ComboAction::Input(InputAction::Cancel),
        "with the list gone, Esc is the form's again"
    );
}

#[test]
fn editing_keys_reach_the_input() {
    let mut combo = a_combo();
    type_text(&mut combo, "main");
    combo.handle_key(KeyWithModifier::new(BareKey::Char('u')).with_ctrl_modifier());
    assert_eq!(combo.text(), "");
}

#[test]
fn a_paste_lands_in_the_value_and_refilters() {
    let mut combo = a_combo();
    combo.insert_pasted_text("feature/log");
    assert_eq!(combo.text(), "feature/log");
    assert!(combo.is_open());
    assert_eq!(combo.dropdown.matches().len(), 2);
}

#[test]
fn accepting_a_suggestion_is_what_a_click_on_one_does() {
    let mut combo = a_combo();
    type_text(&mut combo, "fea");
    assert!(HasDropdown::commit_option(&mut combo, 2));
    assert_eq!(combo.text(), "feature/logout");
    assert!(!combo.is_open());
    assert!(
        !HasDropdown::commit_option(&mut combo, 9),
        "there is no ninth suggestion"
    );
}

#[test]
fn suggestions_that_arrive_later_filter_against_the_text_already_there() {
    let mut combo = Combo::empty();
    type_text(&mut combo, "fea");
    assert!(!combo.is_open(), "there was nothing to suggest");
    combo.open();
    combo.set_suggestions(SUGGESTIONS.iter().map(|s| s.to_string()).collect());
    assert_eq!(combo.dropdown.matches().len(), 2);
    assert_eq!(combo.text(), "fea");
}

#[test]
fn suggestions_going_away_take_the_open_list_with_them() {
    let mut combo = a_combo();
    type_text(&mut combo, "fea");
    assert!(combo.is_open());
    combo.set_suggestions(vec![]);
    assert!(!combo.is_open());
    assert_eq!(combo.text(), "fea");
}

#[test]
fn the_list_is_an_overlay_so_the_row_is_always_one_tall() {
    let mut combo = a_combo();
    assert_eq!(combo.height(), 1);
    combo.open();
    assert_eq!(combo.height(), 1);
}

#[test]
fn a_combo_that_never_rendered_covers_nothing_and_hits_nothing() {
    let mut combo = a_combo();
    combo.open();
    assert_eq!(combo.overlay_rect(), None);
    assert!(!combo.row_contains(0, 0));
    assert_eq!(combo.option_at(0, 1), None);
    assert!(!combo.marker_contains(0));
}

#[test]
fn a_row_shows_the_value_and_a_marker() {
    let mut combo = a_combo();
    type_text(&mut combo, "main");
    assert_eq!(
        combo.row_line("Base: ", 40),
        ("Base: main ▾".to_string(), false)
    );
}

#[test]
fn a_combo_with_nothing_to_suggest_is_a_text_field() {
    let mut combo = Combo::empty();
    type_text(&mut combo, "main");
    assert_eq!(
        combo.row_line("Base: ", 40),
        ("Base: main".to_string(), false)
    );
}

#[test]
fn an_empty_combo_shows_its_placeholder() {
    let combo = a_combo().placeholder("branch name");
    assert_eq!(
        combo.row_line("Base: ", 40),
        ("Base: branch name ▾".to_string(), true)
    );
}

#[test]
fn a_row_never_exceeds_its_width() {
    let mut combo = a_combo().placeholder("some rather long placeholder");
    assert!(combo.row_line("Base: ", 14).0.as_str().width() <= 14);
    combo.set_text("日本語のブランチ名前");
    let (line, _) = combo.row_line("Base: ", 14);
    assert!(line.as_str().width() <= 14, "{:?} is wider than 14", line);
}

/// Unlike a collapsed select, a combo is a field you type into.
#[test]
fn a_closed_combo_still_places_a_cursor() {
    let mut combo = a_combo();
    assert_eq!(combo.cursor_column("Base: ", 40), 6);
    type_text(&mut combo, "main");
    assert_eq!(combo.cursor_column("Base: ", 40), 10);
}

/// Everything except the arrow keys leaves a combo behaving exactly like the
/// text field it is - the value is never something the user did not type.
#[hegel::test]
fn without_the_arrows_a_combo_is_a_text_field(tc: hegel::TestCase) {
    let suggestions: Vec<String> = SUGGESTIONS.iter().map(|s| s.to_string()).collect();
    let mut combo = Combo::new(suggestions);
    let mut input = TextInput::empty();
    let keys = tc.draw(generators::vecs(generators::integers::<u8>()).max_size(40));

    for k in keys {
        let key = match k % 8 {
            0 => key(BareKey::Enter),
            1 => key(BareKey::Esc),
            2 => key(BareKey::Backspace),
            3 => key(BareKey::Left),
            _ => char_key(char::from(b'a' + (k % 26))),
        };
        combo.handle_key(key.clone());
        input.handle_key(key);
        assert_eq!(
            combo.text(),
            input.get_text(),
            "the list changed a value nobody asked it to change"
        );
    }
}

/// A highlight outside the matches is an Enter away from committing a
/// suggestion that is not on screen.
#[hegel::test]
fn the_highlight_is_either_nothing_or_a_visible_row(tc: hegel::TestCase) {
    let suggestions: Vec<String> = tc.draw(
        generators::vecs(generators::text())
            .min_size(1)
            .max_size(10),
    );
    let mut combo = Combo::new(suggestions.clone());
    let keys = tc.draw(generators::vecs(generators::integers::<u8>()).max_size(40));

    for k in keys {
        let key = match k % 8 {
            0 => key(BareKey::Down),
            1 => key(BareKey::Up),
            2 => key(BareKey::Enter),
            3 => key(BareKey::Esc),
            4 => key(BareKey::Backspace),
            _ => char_key(char::from(b'a' + (k % 26))),
        };
        combo.handle_key(key);
        match combo.dropdown.highlighted() {
            Some(_) => assert!(
                combo.highlighted_suggestion().is_some(),
                "the highlight is outside the matches"
            ),
            None => assert_eq!(combo.highlighted_suggestion(), None),
        }
        assert!(
            combo.is_open() || combo.dropdown.highlighted().is_none(),
            "a closed list still has a highlight in it"
        );
    }
}
