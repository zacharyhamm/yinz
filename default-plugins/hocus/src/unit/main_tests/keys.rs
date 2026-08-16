use super::*;

#[test]
fn ctrl_g_is_not_handled_by_hocus() {
    let mut state = idle_state();
    state.run_local(&schedule_call(r#"{"seconds":60,"prompt":"check"}"#));

    assert!(!state.on_key(KeyWithModifier::new(BareKey::Char('g')).with_ctrl_modifier()));
    assert_eq!(state.jobs.len(), 1);
}

#[test]
fn ctrl_backtick_is_the_hocus_toggle_key() {
    assert!(State::is_toggle_key(
        &KeyWithModifier::new(BareKey::Char('`')).with_ctrl_modifier()
    ));
    assert!(!State::is_toggle_key(
        &KeyWithModifier::new(BareKey::Char('g')).with_ctrl_modifier()
    ));
}
