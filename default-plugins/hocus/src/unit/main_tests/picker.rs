use super::*;

#[test]
fn unicode_case_expansions_and_contractions_complete_without_hiding_the_match() {
    for (title, filter, completed) in [
        ("İstanbul", "İ", "İstanbul"),
        ("İstanbul", "i", "i\u{307}stanbul"),
        ("Kelvin", "k", "kelvin"),
        ("İ €uro", "€", "€uro"),
    ] {
        let mut state = picker_state(&[title]);
        type_filter(&mut state, filter);
        state.picker_key(KeyWithModifier::new(BareKey::Tab));
        assert_eq!(picker(&state).filter, completed);
        assert_eq!(visible_texts(&state), [title]);
    }
}

#[test]
fn the_picker_selection_stops_at_both_ends() {
    let mut state = picker_state(&["first", "second"]);
    state.picker_key(KeyWithModifier::new(BareKey::Up));
    assert_eq!(picker(&state).selected, 0, "already at the top");

    state.picker_key(KeyWithModifier::new(BareKey::Down));
    state.picker_key(KeyWithModifier::new(BareKey::Down));
    assert_eq!(picker(&state).selected, 1, "and stops at the bottom");
}

#[test]
fn esc_closes_the_picker_and_leaves_the_transcript_alone() {
    let mut state = picker_state(&["first"]);
    let before = state.messages.clone();
    state.picker_key(KeyWithModifier::new(BareKey::Esc));

    assert!(state.picker.is_none());
    assert_eq!(state.messages, before);
    assert_eq!(state.chat_id, None, "nothing was opened");
}

/// A keystroke falling through to the input box behind the picker would be
/// typed invisibly and turn up in the next message. Printable keys are the
/// filter's now, so the box behind must stay empty either way.
#[test]
fn typing_narrows_the_list_and_never_reaches_the_input_box() {
    let mut state = picker_state(&["first", "second"]);
    state.on_key(KeyWithModifier::new(BareKey::Char('e')));

    assert!(state.picker.is_some(), "still open");
    assert_eq!(state.input.get_text(), "", "and nothing was typed");
    assert_eq!(picker(&state).filter, "e");
    assert_eq!(visible_texts(&state), ["second"], "'first' has no e");
    assert_eq!(
        entry_texts(&state),
        ["first", "second"],
        "the list is intact"
    );

    state.on_key(KeyWithModifier::new(BareKey::Backspace));
    assert_eq!(visible_texts(&state), ["first", "second"], "widened again");
}

/// The filter matches anywhere in a row, not just at the front: model ids are
/// mostly a vendor prefix, so a prefix-only filter would be no help at all.
#[test]
fn the_filter_matches_anywhere_and_ignores_case() {
    let mut state = picker_state(&["gpt-4o", "claude-opus", "o3"]);
    type_filter(&mut state, "OP");

    assert_eq!(visible_texts(&state), ["claude-opus"]);
}

/// The selection is an index into the whole list, so it must be moved onto a
/// row that is actually on screen when the filter leaves it behind - and the
/// chat ids parallel to the entries must still line up with it.
#[test]
fn narrowing_moves_the_selection_onto_a_visible_row() {
    let mut state = picker_state(&["first", "second", "third"]);
    type_filter(&mut state, "th");

    assert_eq!(picker(&state).selected, 2, "the only match");
    let PickerKind::Chats(ids) = &picker(&state).kind else {
        panic!("a chat list");
    };
    assert_eq!(ids[picker(&state).selected], 3, "the id beside 'third'");
}

/// A filter matching nothing leaves the selection alone, so backspacing out of
/// a typo puts you back on the row you were on.
#[test]
fn a_filter_that_matches_nothing_keeps_the_selection() {
    let mut state = picker_state(&["first", "second"]);
    state.picker_key(KeyWithModifier::new(BareKey::Down));
    type_filter(&mut state, "zz");

    assert!(visible_texts(&state).is_empty());
    assert_eq!(picker(&state).selected, 1);
}

#[test]
fn up_and_down_skip_the_rows_the_filter_hid() {
    let mut state = picker_state(&["a-one", "b-two", "a-three"]);
    type_filter(&mut state, "a-");

    assert_eq!(picker(&state).selected, 0);
    state.picker_key(KeyWithModifier::new(BareKey::Down));
    assert_eq!(
        picker(&state).selected,
        2,
        "'b-two' is not there to land on"
    );
    state.picker_key(KeyWithModifier::new(BareKey::Down));
    assert_eq!(picker(&state).selected, 2, "and it stops at the bottom");
    state.picker_key(KeyWithModifier::new(BareKey::Up));
    assert_eq!(picker(&state).selected, 0);
}

/// Tab types the part every match agrees on, which with one match left is the
/// whole of it.
#[test]
fn tab_completes_as_far_as_the_matches_agree() {
    let mut state = picker_state(&["claude-opus-5", "claude-sonnet-5", "gpt-4o"]);
    type_filter(&mut state, "cla");
    state.picker_key(KeyWithModifier::new(BareKey::Tab));

    assert_eq!(picker(&state).filter, "claude-", "where they part");

    state.picker_key(KeyWithModifier::new(BareKey::Char('o')));
    state.picker_key(KeyWithModifier::new(BareKey::Tab));
    assert_eq!(
        picker(&state).filter,
        "claude-opus-5",
        "all of the one left"
    );

    // and a second Tab, with nothing more to add, is a no-op that is still
    // swallowed rather than reaching the box behind
    assert!(state.picker_key(KeyWithModifier::new(BareKey::Tab)));
    assert_eq!(picker(&state).filter, "claude-opus-5");
}

/// Having typed your way into a corner, the way out is back to the full list -
/// so Esc undoes the narrowing before it closes anything.
#[test]
fn esc_clears_the_filter_before_it_closes_the_picker() {
    let mut state = picker_state(&["first", "second"]);
    type_filter(&mut state, "sec");
    state.picker_key(KeyWithModifier::new(BareKey::Esc));

    assert!(state.picker.is_some(), "still open");
    assert_eq!(picker(&state).filter, "");
    assert_eq!(visible_texts(&state), ["first", "second"]);

    state.picker_key(KeyWithModifier::new(BareKey::Esc));
    assert!(state.picker.is_none());
}

#[test]
fn ctrl_u_clears_the_whole_filter() {
    let mut state = picker_state(&["first", "second"]);
    type_filter(&mut state, "sec");
    state.picker_key(KeyWithModifier::new(BareKey::Char('u')).with_ctrl_modifier());

    assert_eq!(picker(&state).filter, "");
    assert!(state.picker.is_some());
}

/// The model list is the case the filter was written for: a long list reached
/// by name rather than by scrolling.
#[test]
fn a_filtered_model_list_switches_to_the_model_you_typed() {
    let mut state = fetching_models();
    state.on_response(
        200,
        br#"{"data":[{"id":"gpt-4o"},{"id":"o3"},{"id":"o3-mini"}]}"#,
        &models_context(),
    );
    type_filter(&mut state, "mini");
    state.picker_key(KeyWithModifier::new(BareKey::Enter));

    assert!(state.picker.is_none());
    assert_eq!(state.cfg.as_ref().unwrap().model, "o3-mini");
}

/// The list is also a display of where you are, so it must open on the model
/// in use rather than on whatever sorts first.
#[test]
fn the_model_list_opens_on_the_model_in_use() {
    let mut state = fetching_models();
    let rendered = state.on_response(
        200,
        br#"{"data":[{"id":"o3-mini"},{"id":"gpt-4o-mini"},{"id":"gpt-4o"}]}"#,
        &models_context(),
    );

    assert!(rendered);
    assert_eq!(
        entry_texts(&state),
        ["gpt-4o", "gpt-4o-mini", "o3-mini"],
        "sorted"
    );
    assert_eq!(picker(&state).selected, 1, "DEFAULT_MODEL, sorted second");
}

#[test]
fn choosing_a_model_switches_the_config_and_closes_the_picker() {
    let mut state = fetching_models();
    state.on_response(
        200,
        br#"{"data":[{"id":"gpt-4o"},{"id":"o3"}]}"#,
        &models_context(),
    );
    state.picker_key(KeyWithModifier::new(BareKey::Down));
    state.picker_key(KeyWithModifier::new(BareKey::Enter));

    assert!(state.picker.is_none());
    assert_eq!(state.cfg.as_ref().unwrap().model, "o3");
    assert_eq!(state.messages.len(), 2, "the transcript is untouched");
}

/// A dead endpoint must not leave "fetching models…" on screen forever.
#[test]
fn a_failed_fetch_closes_the_picker_and_says_why() {
    let mut state = fetching_models();
    let body = br#"{"error":{"message":"bad key"}}"#;
    let rendered = state.on_response(401, body, &models_context());

    assert!(rendered);
    assert!(state.picker.is_none());
    assert_eq!(state.messages.last().unwrap().role, Role::Error);
    assert!(state.messages.last().unwrap().content.contains("bad key"));
}

#[test]
fn an_endpoint_that_lists_nothing_closes_the_picker_too() {
    let mut state = fetching_models();
    state.on_response(200, br#"{"data":[]}"#, &models_context());

    assert!(state.picker.is_none());
    assert_eq!(state.messages.last().unwrap().role, Role::Notice);
}

/// Esc while fetching drops the picker; the reply still arrives, and must
/// not reopen it — the same rule as an aborted `web_search`.
#[test]
fn a_models_response_arriving_after_esc_is_dropped() {
    let mut state = fetching_models();
    state.picker_key(KeyWithModifier::new(BareKey::Esc));

    let rendered = state.on_response(200, br#"{"data":[{"id":"gpt-4o"}]}"#, &models_context());

    assert!(!rendered);
    assert!(state.picker.is_none());
    assert_eq!(state.cfg.as_ref().unwrap().model, api::DEFAULT_MODEL);
}
