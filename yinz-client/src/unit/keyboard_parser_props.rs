use super::*;
use hegel::generators::{self, Generator};
use std::collections::BTreeSet;
use {yinz_types::data::BareKey, yinz_types::data::KeyModifier};

#[hegel::composite]
fn keys_with_modifiers(tc: &hegel::TestCase) -> KeyWithModifier {
    let bare_key = tc.draw(hegel::one_of!(
        generators::sampled_from(vec![
            BareKey::PageDown,
            BareKey::PageUp,
            BareKey::Left,
            BareKey::Down,
            BareKey::Up,
            BareKey::Right,
            BareKey::Home,
            BareKey::End,
            BareKey::Backspace,
            BareKey::Delete,
            BareKey::Insert,
            BareKey::Tab,
            BareKey::Esc,
            BareKey::Enter,
            BareKey::CapsLock,
            BareKey::ScrollLock,
            BareKey::NumLock,
            BareKey::PrintScreen,
            BareKey::Pause,
            BareKey::Menu,
        ]),
        // F1-F35 is the range the Kitty protocol defines; F(0) trips an
        // `unreachable!` inside termwiz's encoder.
        generators::integers::<u8>()
            .min_value(1)
            .max_value(35)
            .map(BareKey::F),
        generators::characters().map(BareKey::Char),
    ));
    let key_modifiers: BTreeSet<KeyModifier> = tc
        .draw(generators::vecs(generators::sampled_from(vec![
            KeyModifier::Ctrl,
            KeyModifier::Alt,
            KeyModifier::Shift,
            KeyModifier::Super,
        ])))
        .into_iter()
        .collect();
    KeyWithModifier {
        bare_key,
        key_modifiers,
    }
}

/// The property behind the ~1800 lines of hand-written
/// `assert_eq!(parse(b"\x1b[97u"), Some(key))` rows this replaced:
/// whatever `serialize_kitty` emits, the parser must read back.
///
/// "The same key" is `KeyWithModifier`'s own equality, which folds `Char('A')` and
/// `Char('a') + Shift` together. It has to be: `serialize_kitty` emits no alternate-key
/// sub-field, so a shifted letter comes back through [`resolve_shifted`]'s fallback as the
/// upper-case character - the same shape the legacy path produces, and the same key as far
/// as anything that looks one up is concerned.
#[hegel::test]
fn kitty_serialization_round_trips_through_the_parser(tc: hegel::TestCase) {
    let key = tc.draw(keys_with_modifiers());
    // BareKey::Char control characters (e.g. '\r', '\n', '\t') collide with
    // dedicated BareKeys (Enter, Tab, etc.) when serialized as Kitty CSI
    // (both encode as the same codepoint, e.g. 13 for '\r' vs Enter).
    // The parser cannot distinguish them, so skip those ambiguous cases;
    // they are better represented as the dedicated BareKey variants.
    if let BareKey::Char(c) = key.bare_key
        && c.is_control()
    {
        return;
    }
    let Some(serialized) = yinz_key_codec::keys::serialize_kitty(&key) else {
        // Not every keycode has a Kitty encoding; nothing to check.
        return;
    };
    // Keys that already have an unambiguous legacy encoding (Backspace ->
    // 0x7f, plain Enter -> \r, ...) are emitted in that form rather than as
    // CSI, and this parser only claims to read CSI sequences.
    if !serialized.starts_with("\u{1b}[") {
        return;
    }
    match KittyKeyboardParser::new().feed(serialized.as_bytes()) {
        KittyParseOutcome::Complete(parsed, _) => {
            assert_eq!(
                parsed, key,
                "{:?} serialized to {:?} and parsed back as {:?}",
                key, serialized, parsed
            );
        },
        other => panic!(
            "{:?} serialized to {:?} which did not parse: {:?}",
            key, serialized, other
        ),
    }
}

/// Generalizes `fragmented_kitty_csi_u_emits_one_event` and
/// `fragmented_kitty_byte_by_byte`: any split point, not just the
/// two that were typed out by hand.
#[hegel::test]
fn splitting_a_sequence_across_chunks_parses_the_same_key(tc: hegel::TestCase) {
    let key = tc.draw(keys_with_modifiers());
    let Some(serialized) = yinz_key_codec::keys::serialize_kitty(&key) else {
        return;
    };
    let bytes = serialized.as_bytes();
    let whole = match KittyKeyboardParser::new().feed(bytes) {
        KittyParseOutcome::Complete(k, _) => k,
        // Only sequences that parse whole can be expected to parse split.
        _ => return,
    };

    let mut splits = tc
        .draw(generators::vecs(generators::integers::<usize>().max_value(bytes.len())).max_size(4));
    splits.sort_unstable();
    splits.dedup();

    let mut parser = KittyKeyboardParser::new();
    let mut prev = 0;
    let mut outcome = None;
    for split in splits.iter().copied().chain(std::iter::once(bytes.len())) {
        match parser.feed(&bytes[prev..split]) {
            KittyParseOutcome::Complete(k, _) => outcome = Some(k),
            // The generator only draws real keys, never the modifier keycodes.
            KittyParseOutcome::ModifierKey { modifier, .. } => {
                panic!("{serialized:?} parsed as a bare {modifier:?}")
            },
            KittyParseOutcome::Incomplete => {},
            KittyParseOutcome::NoMatch => {
                // An empty chunk lands in Ground and reports NoMatch;
                // that is not a parse failure of the sequence itself.
                if prev != split {
                    panic!(
                        "{:?} split at {:?} failed to parse at chunk {}..{}",
                        serialized, splits, prev, split
                    );
                }
            },
        }
        prev = split;
    }
    assert_eq!(
        outcome.map(|k| (k.bare_key, k.key_modifiers.clone())),
        Some((whole.bare_key, whole.key_modifiers.clone())),
        "{:?} parsed as {:?} whole but differently when split at {:?}",
        serialized,
        whole,
        splits
    );
}

/// Generalizes `non_kitty_bytes_yield_nomatch_and_reset`. stdin is
/// untrusted: no byte stream may panic the parser, and a parser that
/// reported a terminal outcome must be reusable afterwards.
#[hegel::test]
fn arbitrary_bytes_never_panic_and_always_leave_a_reusable_parser(tc: hegel::TestCase) {
    let bytes = tc.draw(generators::binary());
    let mut parser = KittyKeyboardParser::new();
    if let KittyParseOutcome::Complete(_, _) | KittyParseOutcome::NoMatch = parser.feed(&bytes) {
        // Both terminal outcomes reset to Ground, so a known-good
        // sequence must still parse on the very next feed.
        assert!(
            matches!(
                parser.feed("\u{1b}[97u".as_bytes()),
                KittyParseOutcome::Complete(_, _)
            ),
            "parser did not reset after feeding {:?}",
            bytes
        );
    }
}
