use super::*;

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
/// typed invisibly and turn up in the next message.
#[test]
fn the_picker_swallows_keys_it_does_not_use() {
    let mut state = picker_state(&["first"]);
    state.on_key(KeyWithModifier::new(BareKey::Char('x')));

    assert!(state.picker.is_some(), "still open");
    assert_eq!(state.input.get_text(), "", "and nothing was typed");
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
