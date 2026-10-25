//! Terminal key decoding and encoding.

use crate::vendored::termwiz::input::{
    KeyCode, KeyCodeEncodeModes, KeyboardEncoding, KittyKeyboardFlags, Modifiers,
};
use std::{collections::BTreeSet, str};
use yinz_types::keys::{BareKey, KeyModifier, KeyWithModifier};

#[cfg(test)]
#[path = "unit/encoding_tests.rs"]
mod encoding_tests;

#[cfg(test)]
#[path = "unit/decoding_tests.rs"]
mod decoding_tests;

#[cfg(not(target_family = "wasm"))]
impl From<&KeyModifier> for Modifiers {
    fn from(val: &KeyModifier) -> Self {
        match val {
            KeyModifier::Shift => Modifiers::SHIFT,
            KeyModifier::Alt => Modifiers::ALT,
            KeyModifier::Ctrl => Modifiers::CTRL,
            KeyModifier::Super => Modifiers::SUPER,
        }
    }
}

pub fn bare_key_from_bytes_with_u(bytes: &[u8]) -> Option<BareKey> {
    match str::from_utf8(bytes) {
        Ok("27") => Some(BareKey::Esc),
        Ok("13") => Some(BareKey::Enter),
        Ok("9") => Some(BareKey::Tab),
        Ok("8") => Some(BareKey::Backspace),
        Ok("127") => Some(BareKey::Backspace),
        Ok("57358") => Some(BareKey::CapsLock),
        Ok("57359") => Some(BareKey::ScrollLock),
        Ok("57360") => Some(BareKey::NumLock),
        Ok("57361") => Some(BareKey::PrintScreen),
        Ok("57362") => Some(BareKey::Pause),
        Ok("57363") => Some(BareKey::Menu),
        Ok("57399") => Some(BareKey::Char('0')),
        Ok("57400") => Some(BareKey::Char('1')),
        Ok("57401") => Some(BareKey::Char('2')),
        Ok("57402") => Some(BareKey::Char('3')),
        Ok("57403") => Some(BareKey::Char('4')),
        Ok("57404") => Some(BareKey::Char('5')),
        Ok("57405") => Some(BareKey::Char('6')),
        Ok("57406") => Some(BareKey::Char('7')),
        Ok("57407") => Some(BareKey::Char('8')),
        Ok("57408") => Some(BareKey::Char('9')),
        Ok("57409") => Some(BareKey::Char('.')),
        Ok("57410") => Some(BareKey::Char('/')),
        Ok("57411") => Some(BareKey::Char('*')),
        Ok("57412") => Some(BareKey::Char('-')),
        Ok("57413") => Some(BareKey::Char('+')),
        Ok("57414") => Some(BareKey::Enter),
        Ok("57415") => Some(BareKey::Char('=')),
        Ok("57417") => Some(BareKey::Left),
        Ok("57418") => Some(BareKey::Right),
        Ok("57419") => Some(BareKey::Up),
        Ok("57420") => Some(BareKey::Down),
        Ok("57421") => Some(BareKey::PageUp),
        Ok("57422") => Some(BareKey::PageDown),
        Ok("57423") => Some(BareKey::Home),
        Ok("57424") => Some(BareKey::End),
        Ok("57425") => Some(BareKey::Insert),
        Ok("57426") => Some(BareKey::Delete),
        Ok(num) => num
            .parse::<u32>()
            .ok()
            .and_then(char::from_u32)
            .map(BareKey::Char),
        _ => None,
    }
}
pub(crate) fn bare_key_from_bytes_with_tilde(bytes: &[u8]) -> Option<BareKey> {
    match str::from_utf8(bytes) {
        Ok("2") => Some(BareKey::Insert),
        Ok("3") => Some(BareKey::Delete),
        Ok("5") => Some(BareKey::PageUp),
        Ok("6") => Some(BareKey::PageDown),
        Ok("7") => Some(BareKey::Home),
        Ok("8") => Some(BareKey::End),
        Ok("11") => Some(BareKey::F(1)),
        Ok("12") => Some(BareKey::F(2)),
        Ok("13") => Some(BareKey::F(3)),
        Ok("14") => Some(BareKey::F(4)),
        Ok("15") => Some(BareKey::F(5)),
        Ok("17") => Some(BareKey::F(6)),
        Ok("18") => Some(BareKey::F(7)),
        Ok("19") => Some(BareKey::F(8)),
        Ok("20") => Some(BareKey::F(9)),
        Ok("21") => Some(BareKey::F(10)),
        Ok("23") => Some(BareKey::F(11)),
        Ok("24") => Some(BareKey::F(12)),
        Ok("25") => Some(BareKey::F(13)),
        Ok("26") => Some(BareKey::F(14)),
        Ok("28") => Some(BareKey::F(15)),
        Ok("29") => Some(BareKey::F(16)),
        Ok("31") => Some(BareKey::F(17)),
        Ok("32") => Some(BareKey::F(18)),
        Ok("33") => Some(BareKey::F(19)),
        Ok("34") => Some(BareKey::F(20)),
        Ok("42") => Some(BareKey::F(21)),
        Ok("43") => Some(BareKey::F(22)),
        Ok("44") => Some(BareKey::F(23)),
        Ok("45") => Some(BareKey::F(24)),
        _ => None,
    }
}
pub(crate) fn bare_key_from_bytes_with_no_ending_byte(bytes: &[u8]) -> Option<BareKey> {
    match str::from_utf8(bytes) {
        Ok("1D") | Ok("D") => Some(BareKey::Left),
        Ok("1C") | Ok("C") => Some(BareKey::Right),
        Ok("1A") | Ok("A") => Some(BareKey::Up),
        Ok("1B") | Ok("B") => Some(BareKey::Down),
        Ok("1H") | Ok("H") => Some(BareKey::Home),
        Ok("1F") | Ok("F") => Some(BareKey::End),
        Ok("1P") | Ok("P") => Some(BareKey::F(1)),
        Ok("1Q") | Ok("Q") => Some(BareKey::F(2)),
        Ok("1R") | Ok("R") => Some(BareKey::F(3)),
        Ok("1S") | Ok("S") => Some(BareKey::F(4)),
        Ok("Z") => Some(BareKey::Tab),
        _ => None,
    }
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct ModifierFlags: u8 {
        const SHIFT   = 0b0000_0001;
        const ALT     = 0b0000_0010;
        const CONTROL = 0b0000_0100;
        const SUPER   = 0b0000_1000;
        // we don't actually use the below, left here for completeness in case we want to add them
        // later
        const HYPER = 0b0001_0000;
        const META = 0b0010_0000;
        const CAPS_LOCK = 0b0100_0000;
        const NUM_LOCK = 0b1000_0000;
    }
}

pub(crate) fn modifiers_from_bytes(bytes: &[u8]) -> BTreeSet<KeyModifier> {
    let modifier_flags = str::from_utf8(bytes)
        .ok() // convert to string: (eg. "16")
        .and_then(|s| s.parse::<u8>().ok()) // convert to u8: (eg. 16)
        .map(|s| s.saturating_sub(1)) // subtract 1: (eg. 15)
        .and_then(ModifierFlags::from_bits); // bitflags: (0b0000_1111: Shift, Alt, Control, Super)
    let mut key_modifiers = BTreeSet::new();
    if let Some(modifier_flags) = modifier_flags {
        for name in modifier_flags.iter() {
            match name {
                ModifierFlags::SHIFT => key_modifiers.insert(KeyModifier::Shift),
                ModifierFlags::ALT => key_modifiers.insert(KeyModifier::Alt),
                ModifierFlags::CONTROL => key_modifiers.insert(KeyModifier::Ctrl),
                ModifierFlags::SUPER => key_modifiers.insert(KeyModifier::Super),
                _ => false,
            };
        }
    }
    key_modifiers
}

pub fn key_from_bytes_with_u(
    number_bytes: &[u8],
    modifier_bytes: &[u8],
) -> Option<KeyWithModifier> {
    // CSI number ; modifiers u
    let bare_key = bare_key_from_bytes_with_u(number_bytes);
    match bare_key {
        Some(bare_key) => {
            let key_modifiers = modifiers_from_bytes(modifier_bytes);
            Some(KeyWithModifier {
                bare_key,
                key_modifiers,
            })
        },
        _ => None,
    }
}
pub fn key_from_bytes_with_tilde(
    number_bytes: &[u8],
    modifier_bytes: &[u8],
) -> Option<KeyWithModifier> {
    // CSI number ; modifiers ~
    let bare_key = bare_key_from_bytes_with_tilde(number_bytes);
    match bare_key {
        Some(bare_key) => {
            let key_modifiers = modifiers_from_bytes(modifier_bytes);
            Some(KeyWithModifier {
                bare_key,
                key_modifiers,
            })
        },
        _ => None,
    }
}
pub fn key_from_bytes_with_no_ending_byte(
    number_bytes: &[u8],
    modifier_bytes: &[u8],
) -> Option<KeyWithModifier> {
    // CSI 1; modifiers [ABCDEFHPQS]
    let bare_key = bare_key_from_bytes_with_no_ending_byte(number_bytes);
    match bare_key {
        Some(bare_key) => {
            let mut key_modifiers = modifiers_from_bytes(modifier_bytes);
            // Z is BackTab, so Shift is implicit even when other modifiers are explicit.
            if number_bytes == b"Z" {
                key_modifiers.insert(KeyModifier::Shift);
            }
            Some(KeyWithModifier {
                bare_key,
                key_modifiers,
            })
        },
        _ => None,
    }
}

pub fn to_termwiz_modifiers(key: &KeyWithModifier) -> Modifiers {
    let mut modifiers = Modifiers::empty();
    for modifier in &key.key_modifiers {
        modifiers.set(modifier.into(), true);
    }
    modifiers
}
pub fn to_termwiz_keycode(key: &KeyWithModifier) -> KeyCode {
    match key.bare_key {
        BareKey::PageDown => KeyCode::PageDown,
        BareKey::PageUp => KeyCode::PageUp,
        BareKey::Left => KeyCode::LeftArrow,
        BareKey::Down => KeyCode::DownArrow,
        BareKey::Up => KeyCode::UpArrow,
        BareKey::Right => KeyCode::RightArrow,
        BareKey::Home => KeyCode::Home,
        BareKey::End => KeyCode::End,
        BareKey::Backspace => KeyCode::Backspace,
        BareKey::Delete => KeyCode::Delete,
        BareKey::Insert => KeyCode::Insert,
        BareKey::F(index) => KeyCode::Function(index),
        BareKey::Char(character) => KeyCode::Char(character),
        BareKey::Tab => KeyCode::Tab,
        BareKey::Esc => KeyCode::Escape,
        BareKey::Enter => KeyCode::Enter,
        BareKey::CapsLock => KeyCode::CapsLock,
        BareKey::ScrollLock => KeyCode::ScrollLock,
        BareKey::NumLock => KeyCode::NumLock,
        BareKey::PrintScreen => KeyCode::PrintScreen,
        BareKey::Pause => KeyCode::Pause,
        BareKey::Menu => KeyCode::Menu,
    }
}
pub fn serialize_non_kitty(key: &KeyWithModifier) -> Option<String> {
    let modifiers = to_termwiz_modifiers(key);
    let key_code_encode_modes = KeyCodeEncodeModes {
        encoding: KeyboardEncoding::Xterm,
        // all these flags are false because they have been dealt with before this
        // serialization
        application_cursor_keys: false,
        newline_mode: false,
        modify_other_keys: None,
    };
    to_termwiz_keycode(key)
        .encode(modifiers, key_code_encode_modes, true)
        .ok()
}
pub fn serialize_kitty(key: &KeyWithModifier) -> Option<String> {
    let modifiers = to_termwiz_modifiers(key);
    let key_code_encode_modes = KeyCodeEncodeModes {
        encoding: KeyboardEncoding::Kitty(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES),
        // all these flags are false because they have been dealt with before this
        // serialization
        application_cursor_keys: false,
        newline_mode: false,
        modify_other_keys: None,
    };
    to_termwiz_keycode(key)
        .encode(modifiers, key_code_encode_modes, true)
        .ok()
}

#[cfg(test)]
#[path = "unit/keys_tests.rs"]
mod tests;

use crate::vendored::termwiz::input::{InputEvent, InputParser, KeyEvent};

// used for parsing keys to plugins
pub fn parse_keys(input_bytes: &[u8]) -> Vec<KeyWithModifier> {
    let mut ret = vec![];
    let mut input_parser = InputParser::new(); // this is the termwiz InputParser
    let maybe_more = false;
    let parse_input_event = |input_event: InputEvent| {
        if let InputEvent::Key(key_event) = input_event
            && let Some(key) = cast_termwiz_key(key_event, input_bytes, |_| false)
        {
            ret.push(key);
        }
    };
    input_parser.parse(input_bytes, parse_input_event, maybe_more);
    ret
}

/// Turn a termwiz key event into yinz's key, or `None` for a key yinz has no name
/// for (media and browser keys, modifier-only presses, `Select`, `Execute`, ...).
///
/// A key with no name is dropped by the caller rather than mapped to something else:
/// this used to fall through to `Esc`, so a numpad or media key cancelled whatever mode
/// the user was in.
///
/// A raw `0x0a` is read as Ctrl-j when the user has bound it (`is_bound`), Enter otherwise.
pub fn cast_termwiz_key(
    event: KeyEvent,
    raw_bytes: &[u8],
    is_bound: impl FnOnce(&KeyWithModifier) -> bool,
) -> Option<KeyWithModifier> {
    let termwiz_modifiers = event.modifiers;

    if raw_bytes == [10] {
        let ctrl_j = KeyWithModifier::new(BareKey::Char('j')).with_ctrl_modifier();
        if is_bound(&ctrl_j) {
            return Some(ctrl_j);
        }
    }
    let mut modifiers = BTreeSet::new();
    if termwiz_modifiers.contains(Modifiers::CTRL) {
        modifiers.insert(KeyModifier::Ctrl);
    }
    if termwiz_modifiers.contains(Modifiers::ALT) {
        modifiers.insert(KeyModifier::Alt);
    }
    if termwiz_modifiers.contains(Modifiers::SHIFT) {
        modifiers.insert(KeyModifier::Shift);
    }
    if termwiz_modifiers.contains(Modifiers::SUPER) {
        modifiers.insert(KeyModifier::Super);
    }

    let bare_key = match event.key {
        KeyCode::Char('\0') => {
            // NUL character, probably ctrl-space
            modifiers.insert(KeyModifier::Ctrl);
            BareKey::Char(' ')
        },
        KeyCode::Char(c) => BareKey::Char(c),
        KeyCode::Backspace => BareKey::Backspace,
        KeyCode::LeftArrow | KeyCode::ApplicationLeftArrow => BareKey::Left,
        KeyCode::RightArrow | KeyCode::ApplicationRightArrow => BareKey::Right,
        KeyCode::UpArrow | KeyCode::ApplicationUpArrow => BareKey::Up,
        KeyCode::DownArrow | KeyCode::ApplicationDownArrow => BareKey::Down,
        KeyCode::Home | KeyCode::KeyPadHome => BareKey::Home,
        KeyCode::End | KeyCode::KeyPadEnd => BareKey::End,
        KeyCode::PageUp | KeyCode::KeyPadPageUp => BareKey::PageUp,
        KeyCode::PageDown | KeyCode::KeyPadPageDown => BareKey::PageDown,
        KeyCode::Tab => BareKey::Tab,
        KeyCode::Delete => BareKey::Delete,
        KeyCode::Insert => BareKey::Insert,
        KeyCode::Function(n) => BareKey::F(n),
        KeyCode::Escape => BareKey::Esc,
        KeyCode::Enter => BareKey::Enter,
        KeyCode::Numpad0 => BareKey::Char('0'),
        KeyCode::Numpad1 => BareKey::Char('1'),
        KeyCode::Numpad2 => BareKey::Char('2'),
        KeyCode::Numpad3 => BareKey::Char('3'),
        KeyCode::Numpad4 => BareKey::Char('4'),
        KeyCode::Numpad5 => BareKey::Char('5'),
        KeyCode::Numpad6 => BareKey::Char('6'),
        KeyCode::Numpad7 => BareKey::Char('7'),
        KeyCode::Numpad8 => BareKey::Char('8'),
        KeyCode::Numpad9 => BareKey::Char('9'),
        KeyCode::Multiply => BareKey::Char('*'),
        KeyCode::Add => BareKey::Char('+'),
        KeyCode::Subtract => BareKey::Char('-'),
        KeyCode::Decimal => BareKey::Char('.'),
        KeyCode::Divide => BareKey::Char('/'),
        KeyCode::CapsLock => BareKey::CapsLock,
        KeyCode::ScrollLock => BareKey::ScrollLock,
        KeyCode::NumLock => BareKey::NumLock,
        KeyCode::PrintScreen => BareKey::PrintScreen,
        KeyCode::Pause => BareKey::Pause,
        KeyCode::Menu => BareKey::Menu,
        _ => return None,
    };
    Some(KeyWithModifier::new_with_modifiers(bare_key, modifiers))
}

#[cfg(test)]
#[path = "unit/keys_compatibility_tests.rs"]
mod compatibility_tests;
