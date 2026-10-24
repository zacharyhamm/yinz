use super::*;

#[test]
fn numpad_page_keys_preserve_direction_and_modifiers() {
    let modes = KeyCodeEncodeModes {
        encoding: KeyboardEncoding::Xterm,
        application_cursor_keys: false,
        newline_mode: false,
        modify_other_keys: None,
    };
    for (key, prefix) in [(KeyCode::Numpad9, "\x1b[5"), (KeyCode::Numpad3, "\x1b[6")] {
        for (modifiers, suffix) in [
            (Modifiers::NONE, "~"),
            (Modifiers::SHIFT, ";2~"),
            (Modifiers::ALT, ";3~"),
            (Modifiers::CTRL, ";5~"),
            (Modifiers::CTRL | Modifiers::ALT | Modifiers::SHIFT, ";8~"),
        ] {
            assert_eq!(
                key.encode(modifiers, modes, true)
                    .expect("numpad page key encodes"),
                format!("{prefix}{suffix}"),
            );
        }
    }
}

#[test]
fn function_zero_is_an_encoding_error_with_or_without_modifiers() {
    for encoding in [
        KeyboardEncoding::Xterm,
        KeyboardEncoding::CsiU,
        KeyboardEncoding::Kitty(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES),
    ] {
        let modes = KeyCodeEncodeModes {
            encoding,
            application_cursor_keys: false,
            newline_mode: false,
            modify_other_keys: None,
        };
        for modifiers in [
            Modifiers::NONE,
            Modifiers::SHIFT,
            Modifiers::ALT,
            Modifiers::CTRL,
            Modifiers::SUPER,
            Modifiers::CTRL | Modifiers::SHIFT,
        ] {
            assert!(KeyCode::Function(0).encode(modifiers, modes, true).is_err());
            for number in 1..=24 {
                assert!(!KeyCode::Function(number)
                    .encode(modifiers, modes, true)
                    .unwrap()
                    .is_empty());
            }
        }
        assert_eq!(
            KeyCode::Function(1)
                .encode(Modifiers::NONE, modes, true)
                .unwrap(),
            "\x1bOP"
        );
        assert_eq!(
            KeyCode::Function(1)
                .encode(Modifiers::CTRL, modes, true)
                .unwrap(),
            "\x1b[1;5P"
        );
    }
}
