//! Key models and their user-facing string representations.

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fmt,
    hash::{Hash, Hasher},
    str::FromStr,
};
use strum_macros::Display;

impl FromStr for KeyWithModifier {
    type Err = Box<dyn std::error::Error>;
    fn from_str(key_str: &str) -> Result<Self, Self::Err> {
        let mut key_string_parts: Vec<&str> = key_str.split_ascii_whitespace().collect();
        let bare_key: BareKey = BareKey::from_str(key_string_parts.pop().ok_or("empty key")?)?;
        let mut key_modifiers: BTreeSet<KeyModifier> = BTreeSet::new();
        for stringified_modifier in key_string_parts {
            key_modifiers.insert(KeyModifier::from_str(stringified_modifier)?);
        }
        Ok(KeyWithModifier {
            bare_key,
            key_modifiers,
        })
    }
}

#[derive(Debug, Clone, Eq, Serialize, Deserialize, PartialOrd, Ord)]
pub struct KeyWithModifier {
    pub bare_key: BareKey,
    pub key_modifiers: BTreeSet<KeyModifier>,
}

impl PartialEq for KeyWithModifier {
    fn eq(&self, other: &Self) -> bool {
        match (self.bare_key, other.bare_key) {
            (BareKey::Char(self_char), BareKey::Char(other_char))
                if self_char.eq_ignore_ascii_case(&other_char) =>
            {
                let mut self_cloned = self.clone();
                let mut other_cloned = other.clone();
                if self_char.is_ascii_uppercase() {
                    self_cloned.bare_key = BareKey::Char(self_char.to_ascii_lowercase());
                    self_cloned.key_modifiers.insert(KeyModifier::Shift);
                }
                if other_char.is_ascii_uppercase() {
                    other_cloned.bare_key = BareKey::Char(self_char.to_ascii_lowercase());
                    other_cloned.key_modifiers.insert(KeyModifier::Shift);
                }
                self_cloned.bare_key == other_cloned.bare_key
                    && self_cloned.key_modifiers == other_cloned.key_modifiers
            },
            _ => self.bare_key == other.bare_key && self.key_modifiers == other.key_modifiers,
        }
    }
}

impl Hash for KeyWithModifier {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self.bare_key {
            BareKey::Char(character) if character.is_ascii_uppercase() => {
                let mut to_hash = self.clone();
                to_hash.bare_key = BareKey::Char(character.to_ascii_lowercase());
                to_hash.key_modifiers.insert(KeyModifier::Shift);
                to_hash.bare_key.hash(state);
                to_hash.key_modifiers.hash(state);
            },
            _ => {
                self.bare_key.hash(state);
                self.key_modifiers.hash(state);
            },
        }
    }
}

impl fmt::Display for KeyWithModifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.key_modifiers.is_empty() {
            write!(f, "{}", self.bare_key)
        } else {
            write!(
                f,
                "{} {}",
                self.key_modifiers
                    .iter()
                    .map(|m| m.to_string())
                    .collect::<Vec<_>>()
                    .join(" "),
                self.bare_key
            )
        }
    }
}

#[derive(Eq, Clone, Copy, Debug, PartialEq, Hash, Deserialize, Serialize, PartialOrd, Ord)]
pub enum BareKey {
    PageDown,
    PageUp,
    Left,
    Down,
    Up,
    Right,
    Home,
    End,
    Backspace,
    Delete,
    Insert,
    F(u8),
    Char(char),
    Tab,
    Esc,
    Enter,
    CapsLock,
    ScrollLock,
    NumLock,
    PrintScreen,
    Pause,
    Menu,
}

impl fmt::Display for BareKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BareKey::PageDown => write!(f, "PgDn"),
            BareKey::PageUp => write!(f, "PgUp"),
            BareKey::Left => write!(f, "←"),
            BareKey::Down => write!(f, "↓"),
            BareKey::Up => write!(f, "↑"),
            BareKey::Right => write!(f, "→"),
            BareKey::Home => write!(f, "HOME"),
            BareKey::End => write!(f, "END"),
            BareKey::Backspace => write!(f, "BACKSPACE"),
            BareKey::Delete => write!(f, "DEL"),
            BareKey::Insert => write!(f, "INS"),
            BareKey::F(index) => write!(f, "F{}", index),
            BareKey::Char(' ') => write!(f, "SPACE"),
            BareKey::Char(character) => write!(f, "{}", character),
            BareKey::Tab => write!(f, "TAB"),
            BareKey::Esc => write!(f, "ESC"),
            BareKey::Enter => write!(f, "ENTER"),
            BareKey::CapsLock => write!(f, "CAPSlOCK"),
            BareKey::ScrollLock => write!(f, "SCROLLlOCK"),
            BareKey::NumLock => write!(f, "NUMLOCK"),
            BareKey::PrintScreen => write!(f, "PRINTSCREEN"),
            BareKey::Pause => write!(f, "PAUSE"),
            BareKey::Menu => write!(f, "MENU"),
        }
    }
}

impl FromStr for BareKey {
    type Err = Box<dyn std::error::Error>;
    fn from_str(key_str: &str) -> Result<Self, Self::Err> {
        match key_str.to_ascii_lowercase().as_str() {
            // every spelling `Display` produces must parse back, so that keybind configs and
            // `yinz action send-keys` accept what the UI shows the user
            "pagedown" | "pgdn" => Ok(BareKey::PageDown),
            "pageup" | "pgup" => Ok(BareKey::PageUp),
            "left" | "←" => Ok(BareKey::Left),
            "down" | "↓" => Ok(BareKey::Down),
            "up" | "↑" => Ok(BareKey::Up),
            "right" | "→" => Ok(BareKey::Right),
            "home" => Ok(BareKey::Home),
            "end" => Ok(BareKey::End),
            "backspace" => Ok(BareKey::Backspace),
            "delete" => Ok(BareKey::Delete),
            "del" => Ok(BareKey::Delete),
            "insert" => Ok(BareKey::Insert),
            "ins" => Ok(BareKey::Insert),
            "tab" => Ok(BareKey::Tab),
            "esc" => Ok(BareKey::Esc),
            "enter" => Ok(BareKey::Enter),
            "capslock" => Ok(BareKey::CapsLock),
            "scrolllock" => Ok(BareKey::ScrollLock),
            "numlock" => Ok(BareKey::NumLock),
            "printscreen" => Ok(BareKey::PrintScreen),
            "pause" => Ok(BareKey::Pause),
            "menu" => Ok(BareKey::Menu),
            "space" => Ok(BareKey::Char(' ')),
            other => {
                // any "f<n>", not just f1-f12 - `Display` writes F(n) for every n
                if let Some(index) = other.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
                    return Ok(BareKey::F(index));
                }
                if key_str.chars().count() == 1
                    && let Some(character) = key_str.chars().next()
                {
                    return Ok(BareKey::Char(character));
                }
                Err("unsupported key".into())
            },
        }
    }
}

#[derive(
    Eq, Clone, Copy, Debug, PartialEq, Hash, Deserialize, Serialize, PartialOrd, Ord, Display,
)]
pub enum KeyModifier {
    Ctrl,
    Alt,
    Shift,
    Super,
}

impl FromStr for KeyModifier {
    type Err = Box<dyn std::error::Error>;
    fn from_str(key_str: &str) -> Result<Self, Self::Err> {
        match key_str.to_ascii_lowercase().as_str() {
            "shift" => Ok(KeyModifier::Shift),
            "alt" => Ok(KeyModifier::Alt),
            "ctrl" => Ok(KeyModifier::Ctrl),
            "super" => Ok(KeyModifier::Super),
            _ => Err("unsupported modifier".into()),
        }
    }
}

impl KeyWithModifier {
    pub fn new(bare_key: BareKey) -> Self {
        KeyWithModifier {
            bare_key,
            key_modifiers: BTreeSet::new(),
        }
    }
    pub fn new_with_modifiers(bare_key: BareKey, key_modifiers: BTreeSet<KeyModifier>) -> Self {
        KeyWithModifier {
            bare_key,
            key_modifiers,
        }
    }
    pub fn with_shift_modifier(mut self) -> Self {
        self.key_modifiers.insert(KeyModifier::Shift);
        self
    }
    pub fn with_alt_modifier(mut self) -> Self {
        self.key_modifiers.insert(KeyModifier::Alt);
        self
    }
    pub fn with_ctrl_modifier(mut self) -> Self {
        self.key_modifiers.insert(KeyModifier::Ctrl);
        self
    }
    pub fn with_super_modifier(mut self) -> Self {
        self.key_modifiers.insert(KeyModifier::Super);
        self
    }
    pub fn strip_common_modifiers(&self, common_modifiers: &[KeyModifier]) -> Self {
        let common_modifiers: BTreeSet<&KeyModifier> = common_modifiers.iter().collect();
        KeyWithModifier {
            bare_key: self.bare_key,
            key_modifiers: self
                .key_modifiers
                .iter()
                .filter(|m| !common_modifiers.contains(m))
                .cloned()
                .collect(),
        }
    }
    pub fn is_key_without_modifier(&self, key: BareKey) -> bool {
        self.bare_key == key && self.key_modifiers.is_empty()
    }
    pub fn is_key_with_ctrl_modifier(&self, key: BareKey) -> bool {
        self.bare_key == key && self.key_modifiers.contains(&KeyModifier::Ctrl)
    }
    pub fn is_key_with_alt_modifier(&self, key: BareKey) -> bool {
        self.bare_key == key && self.key_modifiers.contains(&KeyModifier::Alt)
    }
    pub fn is_key_with_shift_modifier(&self, key: BareKey) -> bool {
        self.bare_key == key && self.key_modifiers.contains(&KeyModifier::Shift)
    }
    pub fn is_key_with_super_modifier(&self, key: BareKey) -> bool {
        self.bare_key == key && self.key_modifiers.contains(&KeyModifier::Super)
    }
    pub fn is_cancel_key(&self) -> bool {
        // self.bare_key == BareKey::Esc || self.is_key_with_ctrl_modifier(BareKey::Char('c'))
        self.bare_key == BareKey::Esc
    }
    pub fn has_no_modifiers(&self) -> bool {
        self.key_modifiers.is_empty()
    }
    pub fn has_modifiers(&self, modifiers: &[KeyModifier]) -> bool {
        for modifier in modifiers {
            if !self.key_modifiers.contains(modifier) {
                return false;
            }
        }
        true
    }
    pub fn has_only_modifiers(&self, modifiers: &[KeyModifier]) -> bool {
        for modifier in modifiers {
            if !self.key_modifiers.contains(modifier) {
                return false;
            }
        }
        if self.key_modifiers.len() != modifiers.len() {
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hegel::generators::{self, Generator};

    /// Any `BareKey`, including `F` indices and characters outside the ones the
    /// parser knows about.
    ///
    /// Two characters are excluded from the `Char` arm, because the string form genuinely
    /// cannot represent them rather than because of a defect:
    /// * the arrow glyphs `←↓↑→`, which `Display` also uses for `Left`/`Down`/`Up`/`Right`,
    ///   so `Char('←')` and `Left` share a spelling and only one of them can win.
    /// * ASCII whitespace, which `KeyWithModifier::from_str` uses as its modifier
    ///   delimiter (`split_ascii_whitespace`); `BareKey::Tab` is how you spell a tab.
    #[hegel::composite]
    fn bare_keys(tc: &hegel::TestCase) -> BareKey {
        tc.draw(hegel::one_of!(
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
            generators::integers::<u8>().map(BareKey::F),
            generators::text()
                .min_size(1)
                .map(|s| s.chars().next().unwrap())
                .filter(|c| !"←↓↑→".contains(*c) && !c.is_ascii_whitespace())
                .map(BareKey::Char),
        ))
    }

    #[hegel::composite]
    fn keys_with_modifiers(tc: &hegel::TestCase) -> KeyWithModifier {
        let bare_key = tc.draw(bare_keys());
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

    #[hegel::test]
    fn key_with_modifier_roundtrips_through_its_string_form(tc: hegel::TestCase) {
        let key = tc.draw(keys_with_modifiers());
        let stringified = key.to_string();
        let parsed = KeyWithModifier::from_str(&stringified);
        // NOTE: `KeyWithModifier`'s `PartialEq` case-folds, so compare the fields.
        match parsed {
            Ok(parsed) => {
                assert_eq!(
                    parsed.bare_key, key.bare_key,
                    "{:?} stringified to {:?} and parsed back as {:?}",
                    key, stringified, parsed
                );
                assert_eq!(
                    parsed.key_modifiers, key.key_modifiers,
                    "{:?} stringified to {:?} and parsed back as {:?}",
                    key, stringified, parsed
                );
            },
            Err(e) => panic!(
                "{:?} stringified to {:?} which does not parse: {}",
                key, stringified, e
            ),
        }
    }
}

impl KeyWithModifier {
    pub fn to_keybind_string(&self) -> String {
        if self.key_modifiers.is_empty() {
            self.bare_key.to_keybind_string()
        } else {
            format!(
                "{} {}",
                self.key_modifiers
                    .iter()
                    .map(|m| m.to_string())
                    .collect::<Vec<_>>()
                    .join(" "),
                self.bare_key.to_keybind_string()
            )
        }
    }
}

impl BareKey {
    pub fn to_keybind_string(&self) -> String {
        match self {
            BareKey::PageDown => "PageDown".to_string(),
            BareKey::PageUp => "PageUp".to_string(),
            BareKey::Left => "left".to_string(),
            BareKey::Down => "down".to_string(),
            BareKey::Up => "up".to_string(),
            BareKey::Right => "right".to_string(),
            BareKey::Home => "home".to_string(),
            BareKey::End => "end".to_string(),
            BareKey::Backspace => "backspace".to_string(),
            BareKey::Delete => "del".to_string(),
            BareKey::Insert => "insert".to_string(),
            BareKey::F(index) => format!("F{}", index),
            BareKey::Char(' ') => "space".to_string(),
            BareKey::Char(character) => format!("{}", character),
            BareKey::Tab => "tab".to_string(),
            BareKey::Esc => "esc".to_string(),
            BareKey::Enter => "enter".to_string(),
            BareKey::CapsLock => "capslock".to_string(),
            BareKey::ScrollLock => "scrolllock".to_string(),
            BareKey::NumLock => "numlock".to_string(),
            BareKey::PrintScreen => "printscreen".to_string(),
            BareKey::Pause => "pause".to_string(),
            BareKey::Menu => "menu".to_string(),
        }
    }
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;

    #[test]
    fn keys_preserve_json_and_keybind_strings() {
        for (key, json, text) in [
            (
                KeyWithModifier::new(BareKey::Char('\0')),
                r#"{"bare_key":{"Char":"\u0000"},"key_modifiers":[]}"#,
                "\0",
            ),
            (
                KeyWithModifier::new(BareKey::F(u8::MAX)),
                r#"{"bare_key":{"F":255},"key_modifiers":[]}"#,
                "F255",
            ),
            (
                KeyWithModifier::new(BareKey::Left)
                    .with_ctrl_modifier()
                    .with_shift_modifier(),
                r#"{"bare_key":"Left","key_modifiers":["Ctrl","Shift"]}"#,
                "Ctrl Shift left",
            ),
        ] {
            assert_eq!(serde_json::to_string(&key).unwrap(), json);
            assert_eq!(serde_json::from_str::<KeyWithModifier>(json).unwrap(), key);
            assert_eq!(key.to_keybind_string(), text);
            assert_eq!(text.parse::<KeyWithModifier>().unwrap(), key);
        }
        assert!(serde_json::from_str::<BareKey>(r#"{"F":256}"#).is_err());
        assert!(serde_json::from_str::<BareKey>(r#"{"F":-1}"#).is_err());
    }
}
