use std::collections::BTreeSet;

use winit::event::{KeyEvent, Modifiers};
use winit::keyboard::{Key, ModifiersKeyState, ModifiersState, NamedKey};
use yinz_ipc::ipc::ClientToServerMsg;
use yinz_types::data::{BareKey, KeyModifier, KeyWithModifier};
use yinz_types::input::native_terminal::OptionAsAlt;

use crate::platform::Platform;

#[derive(Debug, Clone, PartialEq)]
pub struct Press {
    pub logical: Key,
    pub unmodified: Option<Key>,
    pub modifiers: ModifiersState,
    pub option_as_alt: bool,
    pub text: Option<String>,
}

impl Press {
    pub fn of(event: &KeyEvent, modifiers: ModifiersState, option_as_alt: bool) -> Self {
        Self {
            logical: event.logical_key.clone(),
            unmodified: unmodified_key(event),
            modifiers,
            option_as_alt,
            text: event.text.as_ref().map(|text| text.to_string()),
        }
    }

    pub fn new(logical: Key, modifiers: ModifiersState) -> Self {
        Self {
            logical,
            unmodified: None,
            modifiers,
            option_as_alt: false,
            text: None,
        }
    }

    fn is_shortcut(&self) -> bool {
        self.modifiers.super_key() || (self.modifiers.control_key() && !self.modifiers.alt_key())
    }

    fn produced_text(&self) -> Option<String> {
        if self.is_shortcut() {
            return None;
        }
        let printable: String = self
            .text
            .as_deref()?
            .chars()
            .filter(|character| !character.is_control())
            .collect();
        (!printable.is_empty()).then_some(printable)
    }

    fn spelled(&self) -> Option<String> {
        match &self.logical {
            Key::Character(text) => Some(text.to_string()),
            Key::Named(NamedKey::Space) => Some(" ".to_owned()),
            _ => None,
        }
    }

    fn is_named_key(&self) -> bool {
        matches!(&self.logical, Key::Named(named) if *named != NamedKey::Space)
    }

    fn shifted_punctuation(&self) -> bool {
        let Key::Character(text) = &self.logical else {
            return false;
        };
        let Some(character) = sole_char(text) else {
            return false;
        };
        let Some(base) = shifted_punctuation_base(character) else {
            return false;
        };
        match &self.unmodified {
            Some(Key::Character(plain)) => sole_char(plain) == Some(base),
            None => true,
            _ => false,
        }
    }

    fn effective_modifiers(&self) -> ModifiersState {
        let mut modifiers = self.modifiers;
        if !self.option_as_alt
            && alt_shifted_the_character(&self.logical, self.unmodified.as_ref())
            && !(modifiers.shift_key() && self.shifted_punctuation())
        {
            modifiers.remove(ModifiersState::ALT);
        }
        modifiers
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeadKeyOutcome {
    Wait,
    Type(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Key,
    Type(String),
    TypeThenKey(String),
    Nothing,
}

pub fn dead_key_press(press: &Press) -> DeadKeyOutcome {
    match press.produced_text() {
        Some(text) => DeadKeyOutcome::Type(text),
        None => DeadKeyOutcome::Wait,
    }
}

pub fn resolve_pending_accent(press: &Press) -> Resolution {
    if matches!(press.logical, Key::Unidentified(_)) {
        return match press.produced_text() {
            Some(text) => Resolution::Type(text),
            None => Resolution::Nothing,
        };
    }
    let Some(text) = press.produced_text() else {
        return Resolution::Key;
    };
    if press.spelled().as_deref() == Some(text.as_str()) {
        return Resolution::Key;
    }
    if press.is_named_key() {
        Resolution::TypeThenKey(text)
    } else {
        Resolution::Type(text)
    }
}

pub fn unidentified_text(press: &Press) -> Option<String> {
    match press.logical {
        Key::Unidentified(_) => press.produced_text(),
        _ => None,
    }
}

pub fn leaves_the_accent_pending(press: &Press) -> bool {
    matches!(
        press.logical,
        Key::Named(
            NamedKey::Shift
                | NamedKey::Control
                | NamedKey::Alt
                | NamedKey::AltGraph
                | NamedKey::Super
                | NamedKey::Meta
                | NamedKey::Hyper
                | NamedKey::Fn
                | NamedKey::FnLock
                | NamedKey::Symbol
                | NamedKey::SymbolLock
                | NamedKey::CapsLock
                | NamedKey::NumLock
                | NamedKey::ScrollLock
        )
    )
}

pub fn leftover_accent(press: &Press) -> Option<String> {
    let Key::Character(character) = &press.logical else {
        return None;
    };
    let text = press.produced_text()?;
    (text.len() > character.len() && text.ends_with(character.as_str())).then_some(text)
}

pub fn typed_text_message(text: String) -> Option<ClientToServerMsg> {
    key_message(&Press::new(
        Key::Character(text.as_str().into()),
        ModifiersState::empty(),
    ))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AltKeys {
    pub left: bool,
    pub right: bool,
}

impl AltKeys {
    pub fn of(modifiers: &Modifiers) -> Self {
        Self {
            left: modifiers.lalt_state() == ModifiersKeyState::Pressed,
            right: modifiers.ralt_state() == ModifiersKeyState::Pressed,
        }
    }
}

pub fn option_acts_as_alt(platform: Platform, side: OptionAsAlt, held: AltKeys) -> bool {
    platform == Platform::MacOs
        && match side {
            OptionAsAlt::None => false,
            OptionAsAlt::Left => held.left,
            OptionAsAlt::Right => held.right,
            OptionAsAlt::Both => held.left || held.right,
        }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Typed {
    Key(KeyWithModifier),
    Text(String),
}

#[cfg(any(target_os = "windows", target_os = "macos", freedesktop))]
fn unmodified_key(event: &KeyEvent) -> Option<Key> {
    use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
    Some(event.key_without_modifiers())
}

#[cfg(not(any(target_os = "windows", target_os = "macos", freedesktop)))]
fn unmodified_key(_event: &KeyEvent) -> Option<Key> {
    None
}

// Winit supplies no Shift-only logical key. Recognize these known layout pairs
// rather than treating every changed character as evidence that Alt was consumed.
fn shifted_punctuation_base(character: char) -> Option<char> {
    "~!@#$%^&*()_+{}|:\"<>?"
        .chars()
        .zip("`1234567890-=[]\\;',./".chars())
        .find_map(|(shifted, base)| (shifted == character).then_some(base))
}

pub fn alt_shifted_the_character(logical: &Key, unmodified: Option<&Key>) -> bool {
    let (Key::Character(typed), Some(Key::Character(plain))) = (logical, unmodified) else {
        return false;
    };
    typed.as_str() != plain.as_str() && typed.as_str() != plain.to_uppercase()
}

pub fn claims(keys: &[KeyWithModifier], press: &Press) -> bool {
    chord(press).is_some_and(|chord| keys.contains(&chord))
}

pub fn chord(press: &Press) -> Option<KeyWithModifier> {
    let bare_key = match &press.logical {
        Key::Character(text) => BareKey::Char(sole_char(text)?.to_ascii_lowercase()),
        Key::Named(NamedKey::Space) => BareKey::Char(' '),
        Key::Named(named) => named_key(*named)?,
        Key::Dead(_) | Key::Unidentified(_) => return None,
    };
    let modifiers = press.effective_modifiers();
    let mut key_modifiers = BTreeSet::new();
    if modifiers.control_key() {
        key_modifiers.insert(KeyModifier::Ctrl);
    }
    if modifiers.alt_key() {
        key_modifiers.insert(KeyModifier::Alt);
    }
    if modifiers.super_key() {
        key_modifiers.insert(KeyModifier::Super);
    }
    if modifiers.shift_key() {
        key_modifiers.insert(KeyModifier::Shift);
    }
    Some(KeyWithModifier::new_with_modifiers(bare_key, key_modifiers))
}

pub fn key_message(press: &Press) -> Option<ClientToServerMsg> {
    typed(press).map(|typed| match typed {
        Typed::Key(key) => {
            // Encode the application key before normalizing the server binding key.
            let mut message = key_press_message(key);
            if press.modifiers.shift_key()
                && press.shifted_punctuation()
                && let ClientToServerMsg::Key { key, .. } = &mut message
            {
                key.key_modifiers.remove(&KeyModifier::Shift);
            }
            message
        },
        Typed::Text(text) => text_message(text),
    })
}

pub fn key_press_message(key: KeyWithModifier) -> ClientToServerMsg {
    match yinz_key_codec::keys::serialize_native_key(&key) {
        Some(kitty) => ClientToServerMsg::Key {
            key,
            raw_bytes: kitty.into_bytes(),
            is_kitty_keyboard_protocol: true,
        },
        None => {
            let raw_bytes = yinz_key_codec::keys::serialize_non_kitty(&key)
                .map(String::into_bytes)
                .unwrap_or_default();
            ClientToServerMsg::Key {
                key,
                raw_bytes,
                is_kitty_keyboard_protocol: false,
            }
        },
    }
}

pub fn text_message(chars: String) -> ClientToServerMsg {
    ClientToServerMsg::Text(chars)
}

pub fn typed(press: &Press) -> Option<Typed> {
    let modifiers = press.effective_modifiers();
    match &press.logical {
        Key::Character(text) => match sole_char(text) {
            Some(character) => Some(Typed::Key(with_modifiers(
                BareKey::Char(character),
                modifiers,
            ))),
            None if !text.is_empty() => Some(Typed::Text(text.to_string())),
            None => None,
        },
        Key::Named(NamedKey::Space) => {
            Some(Typed::Key(with_modifiers(BareKey::Char(' '), modifiers)))
        },
        Key::Named(named) => {
            named_key(*named).map(|bare| Typed::Key(with_modifiers(bare, modifiers)))
        },
        Key::Dead(_) | Key::Unidentified(_) => None,
    }
}

fn sole_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let character = chars.next()?;
    chars.next().is_none().then_some(character)
}

fn with_modifiers(bare_key: BareKey, modifiers: ModifiersState) -> KeyWithModifier {
    let mut key_modifiers = BTreeSet::new();
    if modifiers.control_key() {
        key_modifiers.insert(KeyModifier::Ctrl);
    }
    if modifiers.alt_key() {
        key_modifiers.insert(KeyModifier::Alt);
    }
    if modifiers.super_key() {
        key_modifiers.insert(KeyModifier::Super);
    }
    // The Kitty encoder recovers Shift only from a distinct, single lowercase scalar.
    let absorbs_shift = matches!(bare_key, BareKey::Char(character) if {
        let mut lowercase = character.to_lowercase();
        matches!((lowercase.next(), lowercase.next()), (Some(lower), None) if lower != character)
    });
    if modifiers.shift_key() && !absorbs_shift {
        key_modifiers.insert(KeyModifier::Shift);
    }
    KeyWithModifier::new_with_modifiers(bare_key, key_modifiers)
}

fn named_key(named: NamedKey) -> Option<BareKey> {
    let bare = match named {
        NamedKey::ArrowLeft => BareKey::Left,
        NamedKey::ArrowRight => BareKey::Right,
        NamedKey::ArrowUp => BareKey::Up,
        NamedKey::ArrowDown => BareKey::Down,
        NamedKey::Home => BareKey::Home,
        NamedKey::End => BareKey::End,
        NamedKey::PageUp => BareKey::PageUp,
        NamedKey::PageDown => BareKey::PageDown,
        NamedKey::Backspace => BareKey::Backspace,
        NamedKey::Delete => BareKey::Delete,
        NamedKey::Insert => BareKey::Insert,
        NamedKey::Enter => BareKey::Enter,
        NamedKey::Tab => BareKey::Tab,
        NamedKey::Escape => BareKey::Esc,
        NamedKey::CapsLock => BareKey::CapsLock,
        NamedKey::ScrollLock => BareKey::ScrollLock,
        NamedKey::NumLock => BareKey::NumLock,
        NamedKey::PrintScreen => BareKey::PrintScreen,
        NamedKey::Pause => BareKey::Pause,
        NamedKey::ContextMenu => BareKey::Menu,
        NamedKey::F1 => BareKey::F(1),
        NamedKey::F2 => BareKey::F(2),
        NamedKey::F3 => BareKey::F(3),
        NamedKey::F4 => BareKey::F(4),
        NamedKey::F5 => BareKey::F(5),
        NamedKey::F6 => BareKey::F(6),
        NamedKey::F7 => BareKey::F(7),
        NamedKey::F8 => BareKey::F(8),
        NamedKey::F9 => BareKey::F(9),
        NamedKey::F10 => BareKey::F(10),
        NamedKey::F11 => BareKey::F(11),
        NamedKey::F12 => BareKey::F(12),
        _ => return None,
    };
    Some(bare)
}

#[cfg(test)]
pub(crate) mod test_helpers;

#[cfg(test)]
#[path = "unit/keyboard_tests.rs"]
mod tests;
