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
#[path = "unit/keyboard_parser_props.rs"]
mod props;

#[cfg(test)]
#[path = "unit/keyboard_parser_tests.rs"]
mod tests;
