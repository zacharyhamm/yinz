// for more info, please see: https://sw.kovidgoyal.net/kitty/keyboard-protocol
use zellij_utils::data::KeyWithModifier;

#[derive(Debug)]
enum KittyKeysParsingState {
    Ground,
    ReceivedEscapeCharacter,
    ParsingNumber,
    ParsingModifiers,
    DoneParsingWithU,
    DoneParsingWithTilde,
}

/// Three-way outcome of `KittyKeyboardParser::feed()`. Lets a long-lived
/// parser distinguish a finished sequence (consume + reset) from a
/// valid prefix (keep state, wait for the next chunk) from an unrelated
/// byte stream (reset and let a fallback parser handle it).
#[derive(Debug)]
pub enum KittyParseOutcome {
    /// Complete sequence parsed; parser resets to Ground.
    Complete(KeyWithModifier),
    /// Bytes are a valid prefix; parser keeps state. Caller should let
    /// termwiz also see them this round and call `feed()` again on the
    /// next chunk.
    Incomplete,
    /// Bytes are not a Kitty sequence; parser resets to Ground.
    NoMatch,
}

#[derive(Debug)]
pub struct KittyKeyboardParser {
    state: KittyKeysParsingState,
    number_bytes: Vec<u8>,
    modifier_bytes: Vec<u8>,
}

/// CSI final-byte range (0x40..=0x7E), minus `u` and `~` which trigger
/// the explicit `DoneParsingWith{U,Tilde}` states inside the parser.
/// A trailing letter in this range while still in
/// `ParsingNumber`/`ParsingModifiers` indicates a complete
/// letter-terminated sequence (e.g. `\x1b[A`, `\x1b[1;2A`).
fn is_csi_final_letter(b: u8) -> bool {
    (0x40..=0x7E).contains(&b) && b != b'u' && b != b'~'
}

impl KittyKeyboardParser {
    pub fn new() -> Self {
        KittyKeyboardParser {
            state: KittyKeysParsingState::Ground,
            number_bytes: vec![],
            modifier_bytes: vec![],
        }
    }

    fn reset(&mut self) {
        self.state = KittyKeysParsingState::Ground;
        self.number_bytes.clear();
        self.modifier_bytes.clear();
    }

    /// Stateful, cross-chunk-aware entry point. Drives the same
    /// state machine as `parse()` but:
    /// * resets to Ground after producing `Complete`/`NoMatch`, so the
    ///   parser can be reused for the next sequence;
    /// * preserves state on `Incomplete`, so a sequence split across
    ///   chunks still resolves on a follow-up call.
    ///
    /// The existing `parse()` wrapper is retained for the unit tests in
    /// this file, which construct `::new()` per assertion.
    pub fn feed(&mut self, bytes: &[u8]) -> KittyParseOutcome {
        for byte in bytes {
            if !self.advance(*byte) {
                self.reset();
                return KittyParseOutcome::NoMatch;
            }
        }
        match self.state {
            KittyKeysParsingState::DoneParsingWithU => {
                let result =
                    KeyWithModifier::from_bytes_with_u(&self.number_bytes, &self.modifier_bytes);
                self.reset();
                match result {
                    Some(k) => KittyParseOutcome::Complete(k),
                    None => KittyParseOutcome::NoMatch,
                }
            },
            KittyKeysParsingState::DoneParsingWithTilde => {
                let result = KeyWithModifier::from_bytes_with_tilde(
                    &self.number_bytes,
                    &self.modifier_bytes,
                );
                self.reset();
                match result {
                    Some(k) => KittyParseOutcome::Complete(k),
                    None => KittyParseOutcome::NoMatch,
                }
            },
            KittyKeysParsingState::ParsingNumber => {
                // ParsingNumber holds either a digit run waiting for a
                // terminator (Incomplete) or a single letter that is
                // itself the terminator — `\x1b[A` etc. (Complete).
                match self.number_bytes.last().copied() {
                    Some(last) if is_csi_final_letter(last) => {
                        let result = KeyWithModifier::from_bytes_with_no_ending_byte(
                            &self.number_bytes,
                            &self.modifier_bytes,
                        );
                        self.reset();
                        match result {
                            Some(k) => KittyParseOutcome::Complete(k),
                            None => KittyParseOutcome::NoMatch,
                        }
                    },
                    _ => KittyParseOutcome::Incomplete,
                }
            },
            KittyKeysParsingState::ParsingModifiers => {
                // ParsingModifiers holds either modifier digits waiting
                // for the terminator letter (Incomplete) or modifier
                // digits + a trailing letter terminator
                // — `\x1b[1;2A` etc. (Complete).
                match self.modifier_bytes.last().copied() {
                    Some(last) if is_csi_final_letter(last) => {
                        let last_modifier = self.modifier_bytes.pop().unwrap();
                        let result = KeyWithModifier::from_bytes_with_no_ending_byte(
                            &[last_modifier],
                            &self.modifier_bytes,
                        );
                        self.reset();
                        match result {
                            Some(k) => KittyParseOutcome::Complete(k),
                            None => KittyParseOutcome::NoMatch,
                        }
                    },
                    _ => KittyParseOutcome::Incomplete,
                }
            },
            KittyKeysParsingState::ReceivedEscapeCharacter => KittyParseOutcome::Incomplete,
            KittyKeysParsingState::Ground => KittyParseOutcome::NoMatch,
        }
    }

    pub fn advance(&mut self, byte: u8) -> bool {
        // returns false if we failed parsing
        match (&self.state, byte) {
            (KittyKeysParsingState::Ground, 0x1b | 0x5b) => {
                self.state = KittyKeysParsingState::ReceivedEscapeCharacter;
            },
            (KittyKeysParsingState::ReceivedEscapeCharacter, 91) => {
                self.state = KittyKeysParsingState::ParsingNumber;
            },
            (KittyKeysParsingState::ParsingNumber, 59) => {
                // semicolon
                if self.number_bytes == [49] {
                    self.number_bytes.clear();
                }
                self.state = KittyKeysParsingState::ParsingModifiers;
            },
            (
                KittyKeysParsingState::ParsingNumber | KittyKeysParsingState::ParsingModifiers,
                117,
            ) => {
                // u
                self.state = KittyKeysParsingState::DoneParsingWithU;
            },
            (
                KittyKeysParsingState::ParsingNumber | KittyKeysParsingState::ParsingModifiers,
                126,
            ) => {
                // ~
                self.state = KittyKeysParsingState::DoneParsingWithTilde;
            },
            (KittyKeysParsingState::ParsingNumber, _) => {
                self.number_bytes.push(byte);
            },
            (KittyKeysParsingState::ParsingModifiers, _) => {
                self.modifier_bytes.push(byte);
            },
            (_, _) => {
                return false;
            },
        }
        true
    }
}

#[cfg(test)]
mod props {
    use super::*;
    use hegel::generators::{self, Generator};
    use std::collections::BTreeSet;
    use zellij_utils::data::{BareKey, KeyModifier};

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
    #[hegel::test]
    fn kitty_serialization_round_trips_through_the_parser(tc: hegel::TestCase) {
        let key = tc.draw(keys_with_modifiers());
        let Some(serialized) = key.serialize_kitty() else {
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
            KittyParseOutcome::Complete(parsed) => {
                assert_eq!(
                    parsed.bare_key, key.bare_key,
                    "{:?} serialized to {:?} and parsed back as {:?}",
                    key, serialized, parsed
                );
                assert_eq!(
                    parsed.key_modifiers, key.key_modifiers,
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
        let Some(serialized) = key.serialize_kitty() else {
            return;
        };
        let bytes = serialized.as_bytes();
        let whole = match KittyKeyboardParser::new().feed(bytes) {
            KittyParseOutcome::Complete(k) => k,
            // Only sequences that parse whole can be expected to parse split.
            _ => return,
        };

        let mut splits = tc.draw(
            generators::vecs(generators::integers::<usize>().max_value(bytes.len())).max_size(4),
        );
        splits.sort_unstable();
        splits.dedup();

        let mut parser = KittyKeyboardParser::new();
        let mut prev = 0;
        let mut outcome = None;
        for split in splits.iter().copied().chain(std::iter::once(bytes.len())) {
            match parser.feed(&bytes[prev..split]) {
                KittyParseOutcome::Complete(k) => outcome = Some(k),
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
        if let KittyParseOutcome::Complete(_) | KittyParseOutcome::NoMatch = parser.feed(&bytes) {
            // Both terminal outcomes reset to Ground, so a known-good
            // sequence must still parse on the very next feed.
            assert!(
                matches!(
                    parser.feed("\u{1b}[97u".as_bytes()),
                    KittyParseOutcome::Complete(_)
                ),
                "parser did not reset after feeding {:?}",
                bytes
            );
        }
    }
}

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
    use zellij_utils::data::BareKey;
    let mut p = KittyKeyboardParser::new();
    let r1 = p.feed(b"\x1b[97;");
    assert!(matches!(r1, KittyParseOutcome::Incomplete));
    match p.feed(b"2u") {
        KittyParseOutcome::Complete(k) => {
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
    use zellij_utils::data::BareKey;
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
        KittyParseOutcome::Complete(k) => {
            assert_eq!(
                k,
                KeyWithModifier::new(BareKey::Char('a')).with_ctrl_modifier()
            );
        },
        other => panic!("expected Complete, got {:?}", other),
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
