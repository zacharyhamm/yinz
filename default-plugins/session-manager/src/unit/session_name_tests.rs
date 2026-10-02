use super::*;

#[test]
fn session_names_obey_the_host_byte_limit_in_both_screens_and_when_renaming() {
    let mut state = State {
        max_session_name_bytes: Some(4),
        ..Default::default()
    };
    assert_eq!(state.session_name_error("éé"), None);
    assert_eq!(state.session_name_error(""), None);
    assert!(state.session_name_error("ééé").is_some());
    assert!(state.session_name_error("a/b").is_some());
    state.max_session_name_bytes = None;
    assert_eq!(state.session_name_error(&"x".repeat(107)), None);
    assert!(state.session_name_error(&"x".repeat(108)).is_some());
    state.max_session_name_bytes = Some(0);
    assert!(state.session_name_error("x").is_some());

    for (active_screen, rename) in [
        (ActiveScreen::NewSession, false),
        (ActiveScreen::SingleScreen, false),
        (ActiveScreen::SingleScreen, true),
        (ActiveScreen::AttachToSession, true),
    ] {
        let mut state = State {
            active_screen,
            max_session_name_bytes: Some(4),
            ..Default::default()
        };
        if rename {
            state.renaming_session_name = Some("ééé".to_owned());
        } else {
            for character in "ééé".chars() {
                state.new_session_info.add_char(character);
            }
            state.single_screen_state.search_term = "ééé".to_owned();
        }
        state.handle_selection();
        assert_eq!(
            state.error.as_deref(),
            Some("Session name must be at most 4 bytes")
        );
        if active_screen == ActiveScreen::SingleScreen && !rename {
            state.error = None;
            state.single_screen_state.mode = SingleScreenMode::SelectingLayout;
            state.handle_selection();
            assert_eq!(
                state.error.as_deref(),
                Some("Session name must be at most 4 bytes")
            );
        }
    }
}
