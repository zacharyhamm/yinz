use super::*;

#[test]
fn unhandled_character_chords_preserve_text_cursor_and_undo_state() {
    for modifiers in [
        vec![KeyModifier::Alt],
        vec![KeyModifier::Super],
        vec![KeyModifier::Ctrl],
        vec![KeyModifier::Alt, KeyModifier::Shift],
        vec![KeyModifier::Super, KeyModifier::Shift],
        vec![KeyModifier::Alt, KeyModifier::Super],
        vec![KeyModifier::Ctrl, KeyModifier::Alt, KeyModifier::Super],
    ] {
        for c in ['g', 'é', '界', '🦀'] {
            let mut input = TextInput::new("draft".into());
            input.insert_char('!');
            input.backspace();
            assert!(input.undo());
            input.move_left();
            let before = input.clone();

            assert_eq!(
                input.handle_key(KeyWithModifier::new_with_modifiers(
                    BareKey::Char(c),
                    modifiers.iter().copied().collect(),
                )),
                InputAction::NoAction,
            );
            assert_eq!(input.buffer, before.buffer);
            assert_eq!(input.cursor_position, before.cursor_position);
            assert_eq!(input.undo_stack, before.undo_stack);
            assert_eq!(input.redo_stack, before.redo_stack);
            assert_eq!(input.last_edit_was_insert, before.last_edit_was_insert);
        }
    }
}

#[test]
fn plain_and_shift_only_unicode_characters_insert_and_undo_together() {
    let mut input = TextInput::new("ab".into());
    input.move_left();
    for key in [
        KeyWithModifier::new(BareKey::Char('é')),
        KeyWithModifier::new(BareKey::Char('界')).with_shift_modifier(),
        KeyWithModifier::new(BareKey::Char('🦀')),
    ] {
        assert_eq!(input.handle_key(key), InputAction::Continue);
    }
    assert_eq!(input.get_text(), "aé界🦀b");
    assert_eq!(input.get_cursor_position(), 4);
    assert!(input.undo());
    assert_eq!(input.get_text(), "ab");
    assert_eq!(input.get_cursor_position(), 1);
}

#[test]
fn ctrl_and_alt_editing_shortcuts_still_work() {
    let mut input = TextInput::new("hello world".into());
    for (key, text, cursor) in [
        (
            KeyWithModifier::new(BareKey::Left).with_alt_modifier(),
            "hello world",
            6,
        ),
        (
            KeyWithModifier::new(BareKey::Char('k')).with_ctrl_modifier(),
            "hello ",
            6,
        ),
        (
            KeyWithModifier::new(BareKey::Char('z')).with_ctrl_modifier(),
            "hello world",
            6,
        ),
        (
            KeyWithModifier::new(BareKey::Backspace).with_alt_modifier(),
            "world",
            0,
        ),
    ] {
        assert_eq!(input.handle_key(key), InputAction::Continue);
        assert_eq!(input.get_text(), text);
        assert_eq!(input.get_cursor_position(), cursor);
    }
}
