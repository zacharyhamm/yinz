// for more info, please see: https://sw.kovidgoyal.net/kitty/keyboard-protocol

use yinz_types::keys::{BareKey, KeyModifier, KeyWithModifier};

#[derive(Debug)]
enum KittyKeysParsingState {
    Ground,
    ReceivedEscapeCharacter,
    ParsingNumber,
    /// After the first `:` in the first field: the shifted key the terminal offers under
    /// `REPORT_ALTERNATE_KEYS` - the codepoint the key actually produces with Shift held. It is
    /// collected, because under `REPORT_ALL_KEYS_AS_ESCAPE_CODES` it is the only thing that can
    /// tell `!` from a `1` with Shift. It may be empty (`CSI 97::98u`), which means the terminal
    /// is reporting only a base-layout key.
    ParsingShiftedKey,
    /// After the second `:` in the first field: the base-layout key, i.e. which physical key
    /// this is regardless of layout. Yinz resolves keys from the primary code, so it is
    /// consumed and dropped - but it has to be *recognised*, or the `:` lands in a number buffer
    /// and the whole sequence stops parsing.
    ParsingBaseLayoutKey,
    ParsingModifiers,
    /// After a `:` in the second field: the press/repeat/release discriminator the terminal
    /// sends under `REPORT_EVENT_TYPES`. Without this state a release would parse as an
    /// unmodified press of the same key, i.e. every keystroke firing twice.
    ParsingEventType,
    /// After the second `;`: the associated text under `REPORT_ASSOCIATED_TEXT`. Consumed and
    /// dropped for the same reason as the alternate keys.
    ParsingText,
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
    Complete(KeyWithModifier, KeyEventKind),
    /// A modifier key pressed or released *on its own*, which a terminal only reports under
    /// `REPORT_ALL_KEYS_AS_ESCAPE_CODES`. Deliberately not a [`KeyWithModifier`]: nothing
    /// downstream should ever be able to type a bare modifier into a pane, and keeping it out
    /// of `BareKey` - which is plugin-API surface mirrored in a proto - makes that structural
    /// rather than a rule someone has to remember.
    ModifierKey {
        modifier: KeyModifier,
        event: KeyEventKind,
    },
    /// Bytes are a valid prefix; parser keeps state. The caller retains
    /// ownership until a later call completes the sequence or it replays the
    /// whole prefix through its fallback parser.
    Incomplete,
    /// Bytes are not a Kitty sequence; parser resets to Ground.
    NoMatch,
}

/// What happened to the key, as reported under `REPORT_EVENT_TYPES`.
///
/// A terminal that was not asked for event types sends no discriminator at all, and every
/// sequence is a [`KeyEventKind::Press`] - which is why that is what an absent field means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEventKind {
    Press,
    Repeat,
    Release,
}

impl KeyEventKind {
    fn from_bytes(bytes: &[u8]) -> KeyEventKind {
        match bytes {
            b"3" => KeyEventKind::Release,
            b"2" => KeyEventKind::Repeat,
            // Includes the absent field: no discriminator means the terminal was never asked
            // for one, and everything it sends is a press.
            _ => KeyEventKind::Press,
        }
    }
}

/// The keycodes kitty gives the modifier keys themselves, under
/// `REPORT_ALL_KEYS_AS_ESCAPE_CODES`. Left and right halves of the same modifier are the same
/// answer to "is it down", so both map to one [`KeyModifier`].
///
/// Iso Level 3/5 Shift (57453, 57454) are deliberately absent: they are AltGr, which is a
/// layout mechanism rather than a chord modifier, and yinz has no `KeyModifier` for them.
fn modifier_from_keycode(bytes: &[u8]) -> Option<KeyModifier> {
    match bytes {
        b"57441" | b"57447" => Some(KeyModifier::Shift),
        b"57442" | b"57448" => Some(KeyModifier::Ctrl),
        b"57443" | b"57449" => Some(KeyModifier::Alt),
        b"57444" | b"57450" => Some(KeyModifier::Super),
        _ => None,
    }
}

/// Resolve a Shift-only key to the character the terminal says it actually produces.
///
/// Under `REPORT_ALL_KEYS_AS_ESCAPE_CODES` a printable key no longer arrives as text, and the
/// codepoint that arrives instead is the *unshifted* key on the layout: `!` comes through as a
/// `1` with Shift. `REPORT_ALTERNATE_KEYS` adds the shifted codepoint as a sub-parameter, and
/// this is where it is spent - producing the same `(bare_key, modifiers)` pair a terminal in the
/// default flag-1 mode would have produced by simply sending `!` as text.
///
/// Only when Shift is the *sole* modifier. A chord keeps the base codepoint and its Shift,
/// because that is what flag-1 mode reports for it too (`Ctrl+Shift+1` is `CSI 49;6u` either
/// way), and quietly resolving it to `!` would move the key out from under an existing keybind.
///
/// Dropping the Shift is deliberate rather than incidental: it is what makes `serialize_kitty`
/// emit `T` rather than `\x1b[84;2u` for the passthrough copy, and it is keybind-neutral because
/// `KeyWithModifier`'s `PartialEq`/`Hash` already fold `Char('F')` and `Char('f') + Shift`
/// together - which is the only reason `bind "Shift f"` works in the default path today.
fn resolve_shifted(key: KeyWithModifier, shifted_key_bytes: &[u8]) -> KeyWithModifier {
    if !key.has_only_modifiers(&[KeyModifier::Shift]) {
        return key;
    }
    let shifted_bare_key = yinz_key_codec::keys::bare_key_from_bytes_with_u(shifted_key_bytes);
    match (shifted_bare_key, key.bare_key) {
        (Some(shifted), _) => KeyWithModifier::new(shifted),
        // The terminal ignored `REPORT_ALTERNATE_KEYS`, or reported an empty shifted field. An
        // ASCII letter is the one case we can still resolve on our own, and termwiz's own
        // `normalize_shift_to_upper_case` resolves it the same way for the legacy path.
        (None, BareKey::Char(c)) if c.is_ascii_lowercase() => {
            KeyWithModifier::new(BareKey::Char(c.to_ascii_uppercase()))
        },
        // Shift+Tab, Shift+Enter, Shift+F3: no character to resolve to, so the chord stands.
        (None, _) => key,
    }
}

#[derive(Debug)]
pub struct KittyKeyboardParser {
    state: KittyKeysParsingState,
    number_bytes: Vec<u8>,
    shifted_key_bytes: Vec<u8>,
    modifier_bytes: Vec<u8>,
    event_type_bytes: Vec<u8>,
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
            shifted_key_bytes: vec![],
            modifier_bytes: vec![],
            event_type_bytes: vec![],
        }
    }

    fn reset(&mut self) {
        self.state = KittyKeysParsingState::Ground;
        self.number_bytes.clear();
        self.shifted_key_bytes.clear();
        self.modifier_bytes.clear();
        self.event_type_bytes.clear();
    }

    fn event_kind(&self) -> KeyEventKind {
        KeyEventKind::from_bytes(&self.event_type_bytes)
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
                // The modifier keys are checked before the general decode, because
                // `yinz_key_codec::keys::bare_key_from_bytes_with_u` would otherwise turn 57443 into a private-use
                // `Char` and it would be typed into whatever pane has focus.
                if let Some(modifier) = modifier_from_keycode(&self.number_bytes) {
                    let event = self.event_kind();
                    self.reset();
                    return KittyParseOutcome::ModifierKey { modifier, event };
                }
                let event = self.event_kind();
                let result = yinz_key_codec::keys::key_from_bytes_with_u(
                    &self.number_bytes,
                    &self.modifier_bytes,
                )
                .map(|k| resolve_shifted(k, &self.shifted_key_bytes));
                self.reset();
                match result {
                    Some(k) => KittyParseOutcome::Complete(k, event),
                    None => KittyParseOutcome::NoMatch,
                }
            },
            KittyKeysParsingState::DoneParsingWithTilde => {
                let event = self.event_kind();
                let result = yinz_key_codec::keys::key_from_bytes_with_tilde(
                    &self.number_bytes,
                    &self.modifier_bytes,
                )
                .map(|k| resolve_shifted(k, &self.shifted_key_bytes));
                self.reset();
                match result {
                    Some(k) => KittyParseOutcome::Complete(k, event),
                    None => KittyParseOutcome::NoMatch,
                }
            },
            KittyKeysParsingState::ParsingNumber => {
                // ParsingNumber holds either a digit run waiting for a
                // terminator (Incomplete) or a single letter that is
                // itself the terminator — `\x1b[A` etc. (Complete).
                match self.number_bytes.last().copied() {
                    Some(last) if is_csi_final_letter(last) => {
                        let event = self.event_kind();
                        let result = yinz_key_codec::keys::key_from_bytes_with_no_ending_byte(
                            &self.number_bytes,
                            &self.modifier_bytes,
                        );
                        self.reset();
                        match result {
                            Some(k) => KittyParseOutcome::Complete(k, event),
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
                        let event = self.event_kind();
                        let last_modifier = self.modifier_bytes.pop().unwrap();
                        let result = yinz_key_codec::keys::key_from_bytes_with_no_ending_byte(
                            &[last_modifier],
                            &self.modifier_bytes,
                        );
                        self.reset();
                        match result {
                            Some(k) => KittyParseOutcome::Complete(k, event),
                            None => KittyParseOutcome::NoMatch,
                        }
                    },
                    _ => KittyParseOutcome::Incomplete,
                }
            },
            // The same letter-terminated shape as `ParsingModifiers` above, reached when an
            // event type was reported: `\x1b[1;2:3A`. The letter is the last byte of the
            // event-type buffer rather than of the modifier one.
            KittyKeysParsingState::ParsingEventType => {
                match self.event_type_bytes.last().copied() {
                    Some(last) if is_csi_final_letter(last) => {
                        self.event_type_bytes.pop();
                        let event = self.event_kind();
                        let result = yinz_key_codec::keys::key_from_bytes_with_no_ending_byte(
                            &[last],
                            &self.modifier_bytes,
                        );
                        self.reset();
                        match result {
                            Some(k) => KittyParseOutcome::Complete(k, event),
                            None => KittyParseOutcome::NoMatch,
                        }
                    },
                    _ => KittyParseOutcome::Incomplete,
                }
            },
            KittyKeysParsingState::ParsingShiftedKey
            | KittyKeysParsingState::ParsingBaseLayoutKey
            | KittyKeysParsingState::ParsingText => KittyParseOutcome::Incomplete,
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
            // A `:` opens a sub-parameter. The arms for them come first, because the catch-alls
            // at the bottom would otherwise push the colon into a number buffer and the whole
            // sequence would stop parsing.
            (KittyKeysParsingState::ParsingNumber, 58) => {
                // colon: the shifted key follows, then the base-layout one
                self.state = KittyKeysParsingState::ParsingShiftedKey;
            },
            (KittyKeysParsingState::ParsingShiftedKey, 58) => {
                // colon: the base-layout key follows
                self.state = KittyKeysParsingState::ParsingBaseLayoutKey;
            },
            (KittyKeysParsingState::ParsingModifiers, 58) => {
                // colon: the press/repeat/release discriminator follows
                self.state = KittyKeysParsingState::ParsingEventType;
            },
            (
                KittyKeysParsingState::ParsingNumber
                | KittyKeysParsingState::ParsingShiftedKey
                | KittyKeysParsingState::ParsingBaseLayoutKey,
                59,
            ) => {
                // semicolon
                if self.number_bytes == [49] {
                    self.number_bytes.clear();
                }
                self.state = KittyKeysParsingState::ParsingModifiers;
            },
            (
                KittyKeysParsingState::ParsingModifiers | KittyKeysParsingState::ParsingEventType,
                59,
            ) => {
                // semicolon: the associated text follows, which yinz resolves keys without
                self.state = KittyKeysParsingState::ParsingText;
            },
            (
                KittyKeysParsingState::ParsingNumber
                | KittyKeysParsingState::ParsingShiftedKey
                | KittyKeysParsingState::ParsingBaseLayoutKey
                | KittyKeysParsingState::ParsingModifiers
                | KittyKeysParsingState::ParsingEventType
                | KittyKeysParsingState::ParsingText,
                117,
            ) => {
                // u
                self.state = KittyKeysParsingState::DoneParsingWithU;
            },
            (
                KittyKeysParsingState::ParsingNumber
                | KittyKeysParsingState::ParsingShiftedKey
                | KittyKeysParsingState::ParsingBaseLayoutKey
                | KittyKeysParsingState::ParsingModifiers
                | KittyKeysParsingState::ParsingEventType
                | KittyKeysParsingState::ParsingText,
                126,
            ) => {
                // ~
                self.state = KittyKeysParsingState::DoneParsingWithTilde;
            },
            (KittyKeysParsingState::ParsingNumber, _) => {
                self.number_bytes.push(byte);
            },
            (KittyKeysParsingState::ParsingShiftedKey, _) => {
                self.shifted_key_bytes.push(byte);
            },
            (KittyKeysParsingState::ParsingModifiers, _) => {
                self.modifier_bytes.push(byte);
            },
            (KittyKeysParsingState::ParsingEventType, _) => {
                self.event_type_bytes.push(byte);
            },
            // Collected nowhere: which physical key this is says nothing yinz resolves a key
            // from, and the text field only matters to a terminal doing its own composition.
            // They still have to be consumed, or the sequence they are part of fails to parse.
            //
            // Only the CSI parameter bytes, though - `0-9 : ; < = > ?`, which is everything
            // kitty puts in these two fields (a decimal keycode, and decimal codepoints
            // separated by colons). A catch-all here would swallow the sequence's final byte
            // as well, leaving these the only two states that can never reach a terminal
            // outcome: `terminate` reports `Incomplete` for both, so `feed` would go on
            // waiting for a sequence that already ended. An SGR mouse report is the case that
            // actually arrives - `\x1b[<0;12;34M` reaches `ParsingText` at the `M`.
            (
                KittyKeysParsingState::ParsingBaseLayoutKey | KittyKeysParsingState::ParsingText,
                0x30..=0x3f,
            ) => {},
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
        if let KittyParseOutcome::Complete(_, _) | KittyParseOutcome::NoMatch = parser.feed(&bytes)
        {
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
