use super::*;

#[test]
fn kitty_functional_codes_decode_as_keys_including_raw_plugin_input() {
    // https://sw.kovidgoyal.net/kitty/keyboard-protocol/#functional-key-definitions
    for number in 13..=35 {
        let code = (57363 + u32::from(number)).to_string();
        assert_eq!(
            bare_key_from_bytes_with_u(code.as_bytes()),
            Some(BareKey::F(number))
        );
        for modifiers in 1..=16 {
            let expected = KeyWithModifier {
                bare_key: BareKey::F(number),
                key_modifiers: modifiers_from_bytes(modifiers.to_string().as_bytes()),
            };
            let bytes = format!("\x1b[{code};{modifiers}u");
            assert_eq!(parse_keys(bytes.as_bytes()), vec![expected.clone()]);
            assert_eq!(serialize_kitty(&expected).unwrap(), bytes);
            assert_eq!(
                parse_keys(format!("\x1b[{code};{modifiers}:2u").as_bytes()),
                vec![expected],
            );
            assert!(parse_keys(format!("\x1b[{code};{modifiers}:3u").as_bytes()).is_empty());
        }
    }
}

#[test]
fn raw_kitty_functional_keys_preserve_fragmentation_and_alt_prefixes() {
    for (sequence, key, modifiers) in [
        (
            "\x1b[57376;16u",
            KeyCode::Function(13),
            Modifiers::SHIFT | Modifiers::ALT | Modifiers::CTRL | Modifiers::SUPER,
        ),
        ("\x1b\x1b[57398u", KeyCode::Function(35), Modifiers::ALT),
        ("\x1b[57398;5:3uX", KeyCode::Char('X'), Modifiers::NONE),
        ("\x1b[57438;5uX", KeyCode::Char('X'), Modifiers::NONE),
    ] {
        for split in 0..=sequence.len() {
            let mut parser = InputParser::new();
            let mut events = parser.parse_as_vec(&sequence.as_bytes()[..split], true);
            events.extend(parser.parse_as_vec(&sequence.as_bytes()[split..], false));
            assert_eq!(
                events,
                vec![InputEvent::Key(KeyEvent { key, modifiers })],
                "{sequence:?} at {split}",
            );
        }
    }
}

#[test]
fn kitty_reserved_functional_codes_never_become_text() {
    for code in [57344, 57375, 57416, 57427, 57455, 63743]
        .into_iter()
        .chain(57428..=57454)
    {
        let number = code.to_string();
        assert_eq!(bare_key_from_bytes_with_u(number.as_bytes()), None);
        assert_eq!(
            parse_keys(format!("\x1b[{code};5uX").as_bytes()),
            vec![KeyWithModifier::new(BareKey::Char('X'))],
        );
    }
    // Literal UTF-8 private-use text does not denote a functional key.
    for character in ['\u{e000}', '\u{e020}', '\u{e06c}', '\u{f8ff}', '\u{f0000}'] {
        assert_eq!(
            parse_keys(character.to_string().as_bytes()),
            vec![KeyWithModifier::new(BareKey::Char(character))],
        );
    }
    for code in [63744, 0xf0000] {
        assert_eq!(
            bare_key_from_bytes_with_u(code.to_string().as_bytes()),
            Some(BareKey::Char(char::from_u32(code).unwrap())),
        );
    }
}

#[test]
fn extended_legacy_function_keys_round_trip_for_plugins() {
    for number in 13..=24 {
        for key in [
            KeyWithModifier::new(BareKey::F(number)),
            KeyWithModifier::new(BareKey::F(number))
                .with_ctrl_modifier()
                .with_shift_modifier(),
        ] {
            assert_eq!(
                parse_keys(serialize_non_kitty(&key).unwrap().as_bytes()),
                vec![key]
            );
        }
    }
}

#[test]
fn termwiz_super_survives_alone_and_with_other_modifiers() {
    for (modifiers, expected) in [
        (
            Modifiers::SUPER,
            KeyWithModifier::new(BareKey::Char('g')).with_super_modifier(),
        ),
        (
            Modifiers::SUPER | Modifiers::CTRL | Modifiers::ALT | Modifiers::SHIFT,
            KeyWithModifier::new(BareKey::Char('g'))
                .with_super_modifier()
                .with_ctrl_modifier()
                .with_alt_modifier()
                .with_shift_modifier(),
        ),
    ] {
        assert_eq!(
            cast_termwiz_key(
                KeyEvent {
                    key: KeyCode::Char('g'),
                    modifiers,
                },
                b"",
                |_| false,
            ),
            Some(expected),
        );
    }
}

#[test]
fn nul_adds_ctrl_without_discarding_other_modifiers() {
    let ctrl_space = KeyWithModifier::new(BareKey::Char(' ')).with_ctrl_modifier();
    for (modifiers, expected) in [
        (Modifiers::NONE, ctrl_space.clone()),
        (Modifiers::CTRL, ctrl_space.clone()),
        (Modifiers::ALT, ctrl_space.clone().with_alt_modifier()),
        (
            Modifiers::CTRL | Modifiers::ALT,
            ctrl_space.clone().with_alt_modifier(),
        ),
        (
            Modifiers::SUPER | Modifiers::ALT | Modifiers::SHIFT,
            ctrl_space
                .clone()
                .with_super_modifier()
                .with_alt_modifier()
                .with_shift_modifier(),
        ),
    ] {
        assert_eq!(
            cast_termwiz_key(
                KeyEvent {
                    key: KeyCode::Char('\0'),
                    modifiers,
                },
                b"\0",
                |_| false,
            ),
            Some(expected),
        );
    }
    assert_eq!(parse_keys(b"\0"), vec![ctrl_space.clone()]);
    assert_eq!(parse_keys(b"\x1b\0"), vec![ctrl_space.with_alt_modifier()]);
}

#[test]
fn ctrl_h_is_independent_of_batching_and_distinct_from_backspace() {
    let ctrl_h = KeyWithModifier::new(BareKey::Char('h')).with_ctrl_modifier();
    let backspace = KeyWithModifier::new(BareKey::Backspace);
    assert_eq!(parse_keys(b"\x08"), vec![ctrl_h.clone()]);
    assert_eq!(parse_keys(b"\x08\x08"), vec![ctrl_h.clone(); 2]);
    let batch = b"a\x08b\x7f\x08";
    let expected = vec![
        KeyWithModifier::new(BareKey::Char('a')),
        ctrl_h.clone(),
        KeyWithModifier::new(BareKey::Char('b')),
        backspace.clone(),
        ctrl_h,
    ];
    assert_eq!(parse_keys(batch), expected);
    assert_eq!(
        batch.chunks(1).flat_map(parse_keys).collect::<Vec<_>>(),
        expected,
    );
    for encoded in [b"\x7f".as_slice(), b"\x1b[27;1;8~", b"\x1b[27;1;127~"] {
        assert_eq!(parse_keys(encoded), vec![backspace.clone()]);
    }
    assert_eq!(
        parse_keys(b"\x1b[27;6;8~"),
        vec![backspace.with_ctrl_modifier().with_shift_modifier()],
    );
}
