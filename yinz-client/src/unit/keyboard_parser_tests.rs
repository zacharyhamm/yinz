use super::*;

// =====================================================================
// Cross-chunk fragmentation tests for the long-lived feed() entry
// point. Under SSH or any kernel-boundary-fragmented stdin read, a
// single Kitty CSI sequence routinely arrives split across multiple
// chunks; feed() must keep state across calls so the sequence still
// resolves on a follow-up chunk instead of degrading to legacy CSI
// form (and losing modifier metadata).
// =====================================================================

#[test]
fn fragmented_kitty_csi_u_emits_one_event() {
    use yinz_types::keys::BareKey;
    let mut p = KittyKeyboardParser::new();
    let r1 = p.feed(b"\x1b[97;");
    assert!(matches!(r1, KittyParseOutcome::Incomplete));
    match p.feed(b"2u") {
        KittyParseOutcome::Complete(k, _) => {
            assert_eq!(
                k,
                KeyWithModifier::new(BareKey::Char('a')).with_shift_modifier()
            );
        },
        other => panic!("expected Complete, got {:?}", other),
    }
}

#[test]
fn fragmented_kitty_byte_by_byte() {
    use yinz_types::keys::BareKey;
    let full = b"\x1b[97;5u"; // ctrl+a
    let mut p = KittyKeyboardParser::new();
    for &b in &full[..full.len() - 1] {
        assert!(
            matches!(p.feed(&[b]), KittyParseOutcome::Incomplete),
            "byte 0x{:02x} should be Incomplete",
            b
        );
    }
    match p.feed(&[full[full.len() - 1]]) {
        KittyParseOutcome::Complete(k, _) => {
            assert_eq!(
                k,
                KeyWithModifier::new(BareKey::Char('a')).with_ctrl_modifier()
            );
        },
        other => panic!("expected Complete, got {:?}", other),
    }
}

// =====================================================================
// Sub-parameter handling. A terminal asked for REPORT_EVENT_TYPES,
// REPORT_ALTERNATE_KEYS or REPORT_ASSOCIATED_TEXT puts extra fields into
// the same CSI-u sequence; before these were understood, the `:` fell
// into a number buffer and the sequence stopped parsing - which for a
// release meant an unmodified press of the same key, i.e. every
// keystroke firing twice.
// =====================================================================

#[test]
fn an_event_type_is_read_off_the_modifier_field() {
    use yinz_types::keys::BareKey;
    let mut p = KittyKeyboardParser::new();
    match p.feed(b"\x1b[97;5:3u") {
        KittyParseOutcome::Complete(k, KeyEventKind::Release) => {
            assert_eq!(
                k,
                KeyWithModifier::new(BareKey::Char('a')).with_ctrl_modifier()
            );
        },
        other => panic!("expected a Release, got {:?}", other),
    }
    match p.feed(b"\x1b[97;5:2u") {
        KittyParseOutcome::Complete(_, KeyEventKind::Repeat) => {},
        other => panic!("expected a Repeat, got {:?}", other),
    }
}

#[test]
fn a_sequence_with_no_event_type_is_a_press() {
    let mut p = KittyKeyboardParser::new();
    match p.feed(b"\x1b[97;5u") {
        KittyParseOutcome::Complete(_, KeyEventKind::Press) => {},
        other => panic!("expected a Press, got {:?}", other),
    }
}

#[test]
fn the_widest_shape_kitty_sends_resolves_to_the_shifted_key() {
    // `CSI key:shifted:base ; mods:event ; text u` - the widest shape kitty sends. The
    // base-layout key and the associated text are consumed and dropped; the shifted key is what
    // the character actually is.
    let mut p = KittyKeyboardParser::new();
    match p.feed(b"\x1b[97:65:97;2:1;65u") {
        KittyParseOutcome::Complete(k, KeyEventKind::Press) => {
            assert_eq!(k, KeyWithModifier::new(BareKey::Char('A')));
        },
        other => panic!("expected an unmodified A, got {:?}", other),
    }
}

// =====================================================================
// Resolving a Shift-only key through the shifted sub-field. Under
// REPORT_ALL_KEYS_AS_ESCAPE_CODES a printable key arrives as its
// *unshifted* codepoint, so without this `!` is a `1` with Shift - which
// a pane is then written as `\x1b[49;2u` and a tab name as `[49;2u`.
// =====================================================================

#[test]
fn a_shifted_punctuation_key_resolves_to_the_character_it_types() {
    let mut p = KittyKeyboardParser::new();
    // Shift+1 on a US layout: base `1`, shifted `!`.
    match p.feed(b"\x1b[49:33;2u") {
        KittyParseOutcome::Complete(k, _) => {
            assert_eq!(k, KeyWithModifier::new(BareKey::Char('!')));
        },
        other => panic!("expected an unmodified !, got {:?}", other),
    }
}

#[test]
fn a_shifted_letter_resolves_to_its_upper_case_without_a_shift_modifier() {
    let mut p = KittyKeyboardParser::new();
    match p.feed(b"\x1b[97:65;2u") {
        KittyParseOutcome::Complete(k, _) => {
            assert_eq!(k, KeyWithModifier::new(BareKey::Char('A')));
            assert!(k.has_no_modifiers(), "{k:?} kept a modifier");
        },
        other => panic!("expected an unmodified A, got {:?}", other),
    }
}

#[test]
fn a_shifted_letter_is_resolved_even_when_the_host_reports_no_alternate_key() {
    // A host that ignored REPORT_ALTERNATE_KEYS. An ASCII letter is the one case that can still
    // be resolved without being told, which is what termwiz does for the legacy path too.
    let mut p = KittyKeyboardParser::new();
    match p.feed(b"\x1b[116;2u") {
        KittyParseOutcome::Complete(k, _) => {
            assert_eq!(k, KeyWithModifier::new(BareKey::Char('T')));
        },
        other => panic!("expected an unmodified T, got {:?}", other),
    }
}

#[test]
fn an_empty_shifted_field_leaves_the_primary_key_alone() {
    // `CSI 97::98u` - only a base-layout key, which yinz resolves nothing from.
    let mut p = KittyKeyboardParser::new();
    match p.feed(b"\x1b[97::98u") {
        KittyParseOutcome::Complete(k, _) => {
            assert_eq!(k, KeyWithModifier::new(BareKey::Char('a')));
        },
        other => panic!("expected an unmodified a, got {:?}", other),
    }
}

#[test]
fn a_chord_keeps_the_base_key_and_its_shift() {
    use yinz_types::keys::KeyModifier;
    // Ctrl+Shift+1. Flag-1 mode reports this as `CSI 49;6u` too, so resolving it to `!` would
    // move the key out from under any keybind written against it.
    let mut p = KittyKeyboardParser::new();
    match p.feed(b"\x1b[49:33;6u") {
        KittyParseOutcome::Complete(k, _) => {
            assert_eq!(k.bare_key, BareKey::Char('1'));
            assert!(
                k.has_only_modifiers(&[KeyModifier::Ctrl, KeyModifier::Shift]),
                "{k:?}"
            );
        },
        other => panic!("expected Ctrl+Shift+1, got {:?}", other),
    }
}

#[test]
fn shift_with_a_key_that_types_nothing_is_left_alone() {
    let mut p = KittyKeyboardParser::new();
    for (bytes, expected) in [
        (&b"\x1b[9;2u"[..], BareKey::Tab),
        (&b"\x1b[13;2u"[..], BareKey::Enter),
    ] {
        match p.feed(bytes) {
            KittyParseOutcome::Complete(k, _) => {
                assert_eq!(k, KeyWithModifier::new(expected).with_shift_modifier());
            },
            other => panic!("expected a shifted {expected:?}, got {:?}", other),
        }
    }
}

#[test]
fn a_shifted_key_resolves_the_same_way_when_split_across_chunks() {
    // The shifted sub-field is buffered across `feed` calls like every other one, and a stale
    // buffer would resolve the *next* key to the previous key's character.
    let full = b"\x1b[49:33:49;2:1;33u";
    for split in 0..=full.len() {
        let mut p = KittyKeyboardParser::new();
        let mut outcome = None;
        for chunk in [&full[..split], &full[split..]] {
            if let KittyParseOutcome::Complete(k, _) = p.feed(chunk) {
                outcome = Some(k);
            }
        }
        assert_eq!(
            outcome,
            Some(KeyWithModifier::new(BareKey::Char('!'))),
            "split at {split}"
        );
    }
    // The same parser, reused: the previous sequence's shifted key must not leak into this one.
    let mut p = KittyKeyboardParser::new();
    assert!(matches!(
        p.feed(b"\x1b[49:33;2u"),
        KittyParseOutcome::Complete(_, _)
    ));
    match p.feed(b"\x1b[50;2u") {
        KittyParseOutcome::Complete(k, _) => {
            assert_eq!(
                k,
                KeyWithModifier::new(BareKey::Char('2')).with_shift_modifier()
            );
        },
        other => panic!("expected a shifted 2, got {:?}", other),
    }
}

#[test]
fn a_bare_modifier_is_never_a_key() {
    use yinz_types::keys::KeyModifier;
    let mut p = KittyKeyboardParser::new();
    // Left alt down, then up. Both halves of a modifier map to the same answer.
    for (bytes, expected, event) in [
        (&b"\x1b[57443;1u"[..], KeyModifier::Alt, KeyEventKind::Press),
        (
            &b"\x1b[57449;1:3u"[..],
            KeyModifier::Alt,
            KeyEventKind::Release,
        ),
        (
            &b"\x1b[57442;1u"[..],
            KeyModifier::Ctrl,
            KeyEventKind::Press,
        ),
    ] {
        match p.feed(bytes) {
            KittyParseOutcome::ModifierKey { modifier, event: e } => {
                assert_eq!(modifier, expected);
                assert_eq!(e, event);
            },
            other => panic!("expected a bare {expected:?}, got {other:?}"),
        }
    }
}

#[test]
fn an_event_type_survives_being_split_across_chunks() {
    let mut p = KittyKeyboardParser::new();
    let full = b"\x1b[97;5:3u";
    for &b in &full[..full.len() - 1] {
        assert!(
            matches!(p.feed(&[b]), KittyParseOutcome::Incomplete),
            "byte 0x{:02x} should be Incomplete",
            b
        );
    }
    match p.feed(&[full[full.len() - 1]]) {
        KittyParseOutcome::Complete(_, KeyEventKind::Release) => {},
        other => panic!("expected a Release, got {:?}", other),
    }
}

#[test]
fn a_letter_terminated_sequence_can_carry_an_event_type() {
    use yinz_types::keys::BareKey;
    // `\x1b[1;2:3A` - shift+up, released.
    let mut p = KittyKeyboardParser::new();
    match p.feed(b"\x1b[1;2:3A") {
        KittyParseOutcome::Complete(k, KeyEventKind::Release) => {
            assert_eq!(k, KeyWithModifier::new(BareKey::Up).with_shift_modifier());
        },
        other => panic!("expected a released Up, got {:?}", other),
    }
}

#[test]
fn non_kitty_bytes_yield_nomatch_and_reset() {
    // Plain printable bytes don't form a Kitty sequence — must return
    // NoMatch (not Incomplete) so the caller falls through to termwiz
    // immediately rather than buffering forever.
    let mut p = KittyKeyboardParser::new();
    assert!(matches!(p.feed(b"hello"), KittyParseOutcome::NoMatch));
}

#[test]
fn sgr_mouse_reports_yield_nomatch_and_reset() {
    // A mouse report is a CSI too, and its parameters walk the same states a key's do: the
    // second `;` puts the parser in `ParsingText`, where the report's `M`/`m` terminator has
    // to end the parse rather than be eaten as text. Reported as NoMatch and not Incomplete,
    // because the sequence is complete - it is just not a key.
    for report in [b"\x1b[<0;12;34M".as_slice(), b"\x1b[<0;12;34m".as_slice()] {
        let mut p = KittyKeyboardParser::new();
        let outcome = p.feed(report);
        assert!(
            matches!(outcome, KittyParseOutcome::NoMatch),
            "expected NoMatch for {:?}, got {:?}",
            String::from_utf8_lossy(report),
            outcome
        );
        // ...and the parser is reusable afterwards, which is what NoMatch promises.
        assert!(matches!(
            p.feed(b"\x1b[97u"),
            KittyParseOutcome::Complete(_, _)
        ));
    }
}

#[test]
fn a_sequence_carrying_associated_text_still_parses() {
    // `\x1b[97;1;97u` - `a` with no modifiers, reported with the text it composed to. The
    // text field is walked and discarded; the `u` still terminates.
    let mut p = KittyKeyboardParser::new();
    match p.feed(b"\x1b[97;1;97u") {
        KittyParseOutcome::Complete(k, _) => {
            assert_eq!(k, KeyWithModifier::new(BareKey::Char('a')));
        },
        other => panic!("expected an `a`, got {:?}", other),
    }
}
