use super::*;

#[test]
fn keys_yinz_has_no_name_for_are_dropped_not_read_as_escape() {
    let event = |key| KeyEvent {
        key,
        modifiers: Modifiers::empty(),
    };
    // A numpad digit is the digit; it used to come out as Esc and cancel the mode.
    assert_eq!(
        cast_termwiz_key(event(KeyCode::Numpad5), b"", |_| false),
        Some(KeyWithModifier::new(BareKey::Char('5')))
    );
    assert_eq!(
        cast_termwiz_key(event(KeyCode::KeyPadPageUp), b"", |_| false),
        Some(KeyWithModifier::new(BareKey::PageUp))
    );
    assert_eq!(
        cast_termwiz_key(event(KeyCode::Menu), b"", |_| false),
        Some(KeyWithModifier::new(BareKey::Menu))
    );
    for unnamed in [
        KeyCode::VolumeUp,
        KeyCode::MediaPlayPause,
        KeyCode::BrowserBack,
        KeyCode::LeftShift,
        KeyCode::Select,
    ] {
        assert_eq!(cast_termwiz_key(event(unnamed), b"", |_| false), None);
    }
    assert!(parse_keys(b"").is_empty());
}

#[test]
fn escape_is_disambiguated_only_in_kitty_mode() {
    let escape = KeyWithModifier::new(BareKey::Esc);
    assert_eq!(serialize_kitty(&escape).as_deref(), Some("\x1b[27;1u"));
    assert_eq!(serialize_non_kitty(&escape).as_deref(), Some("\x1b"));
    assert_eq!(
        serialize_kitty(&KeyWithModifier::new(BareKey::Down)).as_deref(),
        Some("\x1b[B")
    );
    assert_eq!(
        serialize_kitty(&KeyWithModifier::new(BareKey::Enter)).as_deref(),
        Some("\r")
    );
}

#[test]
fn terminal_keys_preserve_bytes_and_binding_precedence() {
    let enter = KeyEvent {
        key: KeyCode::Enter,
        modifiers: Modifiers::empty(),
    };
    let ctrl_j = KeyWithModifier::new(BareKey::Char('j')).with_ctrl_modifier();
    assert_eq!(
        cast_termwiz_key(enter.clone(), b"\n", |key| key == &ctrl_j),
        Some(ctrl_j.clone())
    );
    assert_eq!(
        cast_termwiz_key(enter, b"\n", |_| false),
        Some(KeyWithModifier::new(BareKey::Enter))
    );
    assert_eq!(
        cast_termwiz_key(
            KeyEvent {
                key: KeyCode::Char('h'),
                modifiers: Modifiers::CTRL,
            },
            b"\x08",
            |_| panic!("only line feed consults bindings"),
        ),
        Some(KeyWithModifier::new(BareKey::Char('h')).with_ctrl_modifier())
    );
    // The existing encoder keeps Ctrl-J as line feed, including in Kitty mode.
    assert_eq!(serialize_kitty(&ctrl_j).as_deref(), Some("\n"));
    assert_eq!(serialize_non_kitty(&ctrl_j).as_deref(), Some("\n"));
    assert_eq!(
        bare_key_from_bytes_with_u(b"1114111"),
        Some(BareKey::Char('\u{10ffff}'))
    );
    for invalid in [b"1114112".as_slice(), b"55296", b"-1", b"4294967296"] {
        assert_eq!(bare_key_from_bytes_with_u(invalid), None);
    }
    assert_eq!(
        parse_keys(b"a"),
        vec![KeyWithModifier::new(BareKey::Char('a'))]
    );
}
