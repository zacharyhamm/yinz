use super::*;

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
