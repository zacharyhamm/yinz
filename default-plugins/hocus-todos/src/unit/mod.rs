use super::*;

#[test]
fn queued_bar_arrows_keep_focus_on_the_native_bar() {
    let mut state = State {
        tabs_focused: true,
        ..Default::default()
    };
    for key in [BareKey::Left, BareKey::Right] {
        state.on_key(KeyWithModifier::new(key));
        assert!(state.tabs_focused);
    }
    state.on_key(KeyWithModifier::new(BareKey::Char('j')));
    assert!(state.tabs_focused);
    state.on_key(KeyWithModifier::new(BareKey::Enter));
    assert!(!state.tabs_focused);
}
