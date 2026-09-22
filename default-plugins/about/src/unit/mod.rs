use super::*;

#[test]
fn about_and_release_notes_keep_navigation_without_tips() {
    for is_release_notes in [false, true] {
        let mut app = App {
            is_release_notes,
            ..App::default()
        };
        app.active_page = app.main_screen();
        assert!(!app.handle_key(KeyWithModifier::new(BareKey::Char('?'))));
        assert_eq!(app.active_page.kind, PageKind::MainScreen);
        assert!(app.handle_key(KeyWithModifier::new(BareKey::Down)));
        assert!(app.handle_key(KeyWithModifier::new(BareKey::Enter)));
        assert_eq!(app.active_page.kind, PageKind::NestedSessions);
        assert!(app.handle_key(KeyWithModifier::new(BareKey::Esc)));
        assert_eq!(app.active_page.kind, PageKind::MainScreen);
    }
}

#[test]
fn keybinding_write_results_still_update_the_confirmation_page() {
    for (event, expected) in [
        (Event::ConfigWasWrittenToDisk, ApplyStatus::Applied),
        (
            Event::FailedToWriteConfigToDisk {
                file_path: Some("/config.kdl".into()),
                reason: Some("read only".into()),
            },
            ApplyStatus::Failed {
                file_path: Some("/config.kdl".into()),
                reason: Some("read only".into()),
            },
        ),
    ] {
        let mut app = App::default();
        app.active_page = app.update_keybindings_screen();
        app.keybinding_state
            .borrow_mut()
            .set_status(ApplyStatus::Applying);
        assert!(app.update(event));
        assert_eq!(app.keybinding_state.borrow().status(), &expected);
        assert_eq!(app.active_page.kind, PageKind::UpdateKeybindings);
    }
}

#[test]
fn unrelated_config_write_results_do_not_change_about() {
    let mut app = App::default();
    assert!(!app.update(Event::ConfigWasWrittenToDisk));
    assert!(!app.update(Event::FailedToWriteConfigToDisk {
        file_path: None,
        reason: None,
    }));
    assert_eq!(app.active_page.kind, PageKind::MainScreen);
}
