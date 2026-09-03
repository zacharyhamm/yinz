use crate::home::default_layout_dir;
use crate::input::actions::{Action, RunCommandAction};
use crate::input::command::RunCommand;
use crate::input::config::{ConversionError, KdlError};
use crate::input::hooks::HookPoint;
use crate::input::keybinds::Keybinds;
use crate::input::layout::{
    FloatingPaneLayout, Layout, PercentOrFixed, Run, RunPlugin, RunPluginLocation,
    RunPluginOrAlias, TiledPaneLayout,
};
pub use crate::input::options::PaneFrameStyle;
use crate::pane_size::PaneGeom;
// The data store's types are part of the plugin API, so they belong in the prelude alongside
// everything else here. They live in their own module because the engine behind them cannot build
// for wasm and these types must.
pub use crate::plugin_data::*;
use crate::position::Position;
use crate::shared::colors as default_colors;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::fs::Metadata;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::str::{self, FromStr};
use std::time::Duration;
use strum_macros::{Display, EnumDiscriminants, EnumIter, EnumString};
use unicode_width::UnicodeWidthChar;

#[cfg(not(target_family = "wasm"))]
use crate::vendored::termwiz::{
    input::KittyKeyboardFlags,
    input::{KeyCode, KeyCodeEncodeModes, KeyboardEncoding, Modifiers},
};

pub type ClientId = u16; // TODO: merge with crate type?

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnblockCondition {
    /// Unblock only when exit status is 0 (success)
    OnExitSuccess,
    /// Unblock only when exit status is non-zero (failure)
    OnExitFailure,
    /// Unblock on any exit (success or failure)
    OnAnyExit,
}

impl UnblockCondition {
    /// Check if the condition is met for the given exit status
    pub fn is_met(&self, exit_status: i32) -> bool {
        match self {
            UnblockCondition::OnExitSuccess => exit_status == 0,
            UnblockCondition::OnExitFailure => exit_status != 0,
            UnblockCondition::OnAnyExit => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandOrPlugin {
    Command(RunCommandAction),
    Plugin(RunPluginOrAlias),
    File(FileToOpen), // open file in configured editor
}

impl CommandOrPlugin {
    pub fn new_command(command: Vec<String>) -> Self {
        CommandOrPlugin::Command(RunCommandAction::new(command))
    }
}

pub fn client_id_to_colors(
    client_id: ClientId,
    colors: MultiplayerColors,
) -> Option<(PaletteColor, PaletteColor)> {
    // (primary color, secondary color)
    let black = PaletteColor::EightBit(default_colors::BLACK);
    match client_id {
        1 => Some((colors.player_1, black)),
        2 => Some((colors.player_2, black)),
        3 => Some((colors.player_3, black)),
        4 => Some((colors.player_4, black)),
        5 => Some((colors.player_5, black)),
        6 => Some((colors.player_6, black)),
        7 => Some((colors.player_7, black)),
        8 => Some((colors.player_8, black)),
        9 => Some((colors.player_9, black)),
        10 => Some((colors.player_10, black)),
        _ => None,
    }
}

pub fn single_client_color(colors: Palette) -> (PaletteColor, PaletteColor) {
    (colors.green, colors.black)
}

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
            // `zellij action send-keys` accept what the UI shows the user
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
                if key_str.chars().count() == 1 {
                    if let Some(character) = key_str.chars().next() {
                        return Ok(BareKey::Char(character));
                    }
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

impl BareKey {
    pub fn from_bytes_with_u(bytes: &[u8]) -> Option<Self> {
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
    pub fn from_bytes_with_tilde(bytes: &[u8]) -> Option<Self> {
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
    pub fn from_bytes_with_no_ending_byte(bytes: &[u8]) -> Option<Self> {
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

impl KeyModifier {
    pub fn from_bytes(bytes: &[u8]) -> BTreeSet<KeyModifier> {
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
    pub fn from_bytes_with_u(number_bytes: &[u8], modifier_bytes: &[u8]) -> Option<Self> {
        // CSI number ; modifiers u
        let bare_key = BareKey::from_bytes_with_u(number_bytes);
        match bare_key {
            Some(bare_key) => {
                let key_modifiers = KeyModifier::from_bytes(modifier_bytes);
                Some(KeyWithModifier {
                    bare_key,
                    key_modifiers,
                })
            },
            _ => None,
        }
    }
    pub fn from_bytes_with_tilde(number_bytes: &[u8], modifier_bytes: &[u8]) -> Option<Self> {
        // CSI number ; modifiers ~
        let bare_key = BareKey::from_bytes_with_tilde(number_bytes);
        match bare_key {
            Some(bare_key) => {
                let key_modifiers = KeyModifier::from_bytes(modifier_bytes);
                Some(KeyWithModifier {
                    bare_key,
                    key_modifiers,
                })
            },
            _ => None,
        }
    }
    pub fn from_bytes_with_no_ending_byte(
        number_bytes: &[u8],
        modifier_bytes: &[u8],
    ) -> Option<Self> {
        // CSI 1; modifiers [ABCDEFHPQS]
        let bare_key = BareKey::from_bytes_with_no_ending_byte(number_bytes);
        match bare_key {
            Some(bare_key) => {
                let mut key_modifiers = KeyModifier::from_bytes(modifier_bytes);
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
    #[cfg(not(target_family = "wasm"))]
    pub fn to_termwiz_modifiers(&self) -> Modifiers {
        let mut modifiers = Modifiers::empty();
        for modifier in &self.key_modifiers {
            modifiers.set(modifier.into(), true);
        }
        modifiers
    }
    #[cfg(not(target_family = "wasm"))]
    pub fn to_termwiz_keycode(&self) -> KeyCode {
        match self.bare_key {
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
    #[cfg(not(target_family = "wasm"))]
    pub fn serialize_non_kitty(&self) -> Option<String> {
        let modifiers = self.to_termwiz_modifiers();
        let key_code_encode_modes = KeyCodeEncodeModes {
            encoding: KeyboardEncoding::Xterm,
            // all these flags are false because they have been dealt with before this
            // serialization
            application_cursor_keys: false,
            newline_mode: false,
            modify_other_keys: None,
        };
        self.to_termwiz_keycode()
            .encode(modifiers, key_code_encode_modes, true)
            .ok()
    }
    #[cfg(not(target_family = "wasm"))]
    pub fn serialize_kitty(&self) -> Option<String> {
        let modifiers = self.to_termwiz_modifiers();
        let key_code_encode_modes = KeyCodeEncodeModes {
            encoding: KeyboardEncoding::Kitty(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES),
            // all these flags are false because they have been dealt with before this
            // serialization
            application_cursor_keys: false,
            newline_mode: false,
            modify_other_keys: None,
        };
        self.to_termwiz_keycode()
            .encode(modifiers, key_code_encode_modes, true)
            .ok()
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

#[derive(
    Eq, Clone, Copy, Debug, PartialEq, Hash, Deserialize, Serialize, PartialOrd, Ord, Default,
)]
pub enum Direction {
    #[default]
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub fn invert(&self) -> Direction {
        match *self {
            Direction::Left => Direction::Right,
            Direction::Down => Direction::Up,
            Direction::Up => Direction::Down,
            Direction::Right => Direction::Left,
        }
    }

    pub fn is_horizontal(&self) -> bool {
        matches!(self, Direction::Left | Direction::Right)
    }

    pub fn is_vertical(&self) -> bool {
        matches!(self, Direction::Down | Direction::Up)
    }
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Direction::Left => write!(f, "←"),
            Direction::Right => write!(f, "→"),
            Direction::Up => write!(f, "↑"),
            Direction::Down => write!(f, "↓"),
        }
    }
}

impl FromStr for Direction {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Left" | "left" => Ok(Direction::Left),
            "Right" | "right" => Ok(Direction::Right),
            "Up" | "up" => Ok(Direction::Up),
            "Down" | "down" => Ok(Direction::Down),
            _ => Err(format!(
                "Failed to parse Direction. Unknown Direction: {}",
                s
            )),
        }
    }
}

/// Resize operation to perform.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Deserialize, Serialize, Default)]
pub enum Resize {
    #[default]
    Increase,
    Decrease,
}

impl Resize {
    pub fn invert(&self) -> Self {
        match self {
            Resize::Increase => Resize::Decrease,
            Resize::Decrease => Resize::Increase,
        }
    }
}

impl fmt::Display for Resize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Resize::Increase => write!(f, "+"),
            Resize::Decrease => write!(f, "-"),
        }
    }
}

impl FromStr for Resize {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Increase" | "increase" | "+" => Ok(Resize::Increase),
            "Decrease" | "decrease" | "-" => Ok(Resize::Decrease),
            _ => Err(format!(
                "failed to parse resize type. Unknown specifier '{}'",
                s
            )),
        }
    }
}

/// Container type that fully describes resize operations.
///
/// This is best thought of as follows:
///
/// - `resize` commands how the total *area* of the pane will change as part of this resize
///   operation.
/// - `direction` has two meanings:
///     - `None` means to resize all borders equally
///     - Anything else means to move the named border to achieve the change in area
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Deserialize, Serialize)]
pub struct ResizeStrategy {
    /// Whether to increase or resize total area
    pub resize: Resize,
    /// With which border, if any, to change area
    pub direction: Option<Direction>,
    /// If set to true (default), increasing resizes towards a viewport border will be inverted.
    /// I.e. a scenario like this ("increase right"):
    ///
    /// ```text
    /// +---+---+
    /// |   | X |->
    /// +---+---+
    /// ```
    ///
    /// turns into this ("decrease left"):
    ///
    /// ```text
    /// +---+---+
    /// |   |-> |
    /// +---+---+
    /// ```
    pub invert_on_boundaries: bool,
}

impl From<Direction> for ResizeStrategy {
    fn from(direction: Direction) -> Self {
        ResizeStrategy::new(Resize::Increase, Some(direction))
    }
}

impl From<Resize> for ResizeStrategy {
    fn from(resize: Resize) -> Self {
        ResizeStrategy::new(resize, None)
    }
}

impl ResizeStrategy {
    pub fn new(resize: Resize, direction: Option<Direction>) -> Self {
        ResizeStrategy {
            resize,
            direction,
            invert_on_boundaries: true,
        }
    }

    pub fn invert(&self) -> ResizeStrategy {
        let resize = match self.resize {
            Resize::Increase => Resize::Decrease,
            Resize::Decrease => Resize::Increase,
        };
        let direction = self.direction.map(|direction| direction.invert());

        ResizeStrategy::new(resize, direction)
    }

    pub fn resize_type(&self) -> Resize {
        self.resize
    }

    pub fn direction(&self) -> Option<Direction> {
        self.direction
    }

    pub fn direction_horizontal(&self) -> bool {
        matches!(
            self.direction,
            Some(Direction::Left) | Some(Direction::Right)
        )
    }

    pub fn direction_vertical(&self) -> bool {
        matches!(self.direction, Some(Direction::Up) | Some(Direction::Down))
    }

    pub fn resize_increase(&self) -> bool {
        self.resize == Resize::Increase
    }

    pub fn resize_decrease(&self) -> bool {
        self.resize == Resize::Decrease
    }

    pub fn move_left_border_left(&self) -> bool {
        (self.resize == Resize::Increase) && (self.direction == Some(Direction::Left))
    }

    pub fn move_left_border_right(&self) -> bool {
        (self.resize == Resize::Decrease) && (self.direction == Some(Direction::Left))
    }

    pub fn move_lower_border_down(&self) -> bool {
        (self.resize == Resize::Increase) && (self.direction == Some(Direction::Down))
    }

    pub fn move_lower_border_up(&self) -> bool {
        (self.resize == Resize::Decrease) && (self.direction == Some(Direction::Down))
    }

    pub fn move_upper_border_up(&self) -> bool {
        (self.resize == Resize::Increase) && (self.direction == Some(Direction::Up))
    }

    pub fn move_upper_border_down(&self) -> bool {
        (self.resize == Resize::Decrease) && (self.direction == Some(Direction::Up))
    }

    pub fn move_right_border_right(&self) -> bool {
        (self.resize == Resize::Increase) && (self.direction == Some(Direction::Right))
    }

    pub fn move_right_border_left(&self) -> bool {
        (self.resize == Resize::Decrease) && (self.direction == Some(Direction::Right))
    }

    pub fn move_all_borders_out(&self) -> bool {
        (self.resize == Resize::Increase) && self.direction.is_none()
    }

    pub fn move_all_borders_in(&self) -> bool {
        (self.resize == Resize::Decrease) && self.direction.is_none()
    }
}

impl fmt::Display for ResizeStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let resize = match self.resize {
            Resize::Increase => "increase",
            Resize::Decrease => "decrease",
        };
        let border = match self.direction {
            Some(Direction::Left) => "left",
            Some(Direction::Down) => "bottom",
            Some(Direction::Up) => "top",
            Some(Direction::Right) => "right",
            None => "every",
        };

        write!(f, "{} size on {} border", resize, border)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
// FIXME: This should be extended to handle different button clicks (not just
// left click) and the `ScrollUp` and `ScrollDown` events could probably be
// merged into a single `Scroll(isize)` event.
pub enum Mouse {
    ScrollUp(usize),   // number of lines
    ScrollDown(usize), // number of lines
    ScrollLeft(usize),
    ScrollRight(usize),
    LeftClick(isize, usize),  // line and column
    RightClick(isize, usize), // line and column
    Hold(isize, usize),       // line and column
    Release(isize, usize),    // line and column
    Hover(isize, usize),      // line and column
}

impl Mouse {
    pub fn position(&self) -> Option<(usize, usize)> {
        // (line, column)
        match self {
            Mouse::LeftClick(line, column) => Some((*line as usize, *column)),
            Mouse::RightClick(line, column) => Some((*line as usize, *column)),
            Mouse::Hold(line, column) => Some((*line as usize, *column)),
            Mouse::Release(line, column) => Some((*line as usize, *column)),
            Mouse::Hover(line, column) => Some((*line as usize, *column)),
            _ => None,
        }
    }
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileMetadata {
    pub is_dir: bool,
    pub is_file: bool,
    pub is_symlink: bool,
    pub len: u64,
}

/// One entry of an `FsList` answer: the entry's path as the plugin would name it, and its
/// metadata - `None` when the entry exists but could not be stat'ed, which is an answer and
/// not an error.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsEntry {
    pub path: String,
    pub metadata: Option<FileMetadata>,
}

impl From<Metadata> for FileMetadata {
    fn from(metadata: Metadata) -> Self {
        FileMetadata {
            is_dir: metadata.is_dir(),
            is_file: metadata.is_file(),
            is_symlink: metadata.is_symlink(),
            len: metadata.len(),
        }
    }
}

#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyledText {
    pub text: String,
    pub indices: Vec<Vec<usize>>,
}

/// These events can be subscribed to with subscribe method exported by `zellij-tile`.
/// Once subscribed to, they will trigger the `update` method of the `ZellijPlugin` trait.
// `ModeUpdate` is the widest variant, but this enum is the plugin-facing event API and is
// mirrored in the protobuf contract: boxing a variant would break the `update()` match arm of
// every plugin built against it, in and out of this repo.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, EnumDiscriminants, Display, Serialize, Deserialize)]
#[strum_discriminants(derive(EnumString, EnumIter, Display, Hash, Serialize, Deserialize))]
#[strum_discriminants(name(EventType))]
#[non_exhaustive]
pub enum Event {
    ModeUpdate(ModeInfo),
    TabUpdate(Vec<TabInfo>),
    PaneUpdate(PaneManifest),
    /// A key was pressed while the user is focused on this plugin's pane
    Key(KeyWithModifier),
    /// A mouse event happened while the user is focused on this plugin's pane
    Mouse(Mouse),
    /// A timer expired set by the `set_timeout` method exported by `zellij-tile`.
    Timer(f64),
    /// Text was copied to the clipboard anywhere in the app
    CopyToClipboard(CopyDestination),
    /// Failed to copy text to clipboard anywhere in the app
    SystemClipboardFailure,
    /// Input was received anywhere in the app
    InputReceived,
    /// This plugin became visible or invisible
    Visible(bool),
    /// A message from one of the plugin's workers
    CustomMessage(
        String, // message
        String, // payload
    ),
    /// A file was created somewhere in the Zellij CWD folder
    FileSystemCreate(Vec<(PathBuf, Option<FileMetadata>)>),
    /// A file was accessed somewhere in the Zellij CWD folder
    FileSystemRead(Vec<(PathBuf, Option<FileMetadata>)>),
    /// A file was modified somewhere in the Zellij CWD folder
    FileSystemUpdate(Vec<(PathBuf, Option<FileMetadata>)>),
    /// A file was deleted somewhere in the Zellij CWD folder
    FileSystemDelete(Vec<(PathBuf, Option<FileMetadata>)>),
    /// A Result of plugin permission request
    PermissionRequestResult(PermissionStatus),
    SessionUpdate(
        Vec<SessionInfo>,
        Vec<(String, Duration)>, // resurrectable sessions
    ),
    RunCommandResult(Option<i32>, Vec<u8>, Vec<u8>, BTreeMap<String, String>), // exit_code, STDOUT, STDERR,
    // context
    WebRequestResult(
        u16,
        BTreeMap<String, String>,
        Vec<u8>,
        BTreeMap<String, String>,
    ), // status,
    // headers,
    // body,
    // context
    /// A chunk of a streamed web request body (see `web_request_stream`). Chunks arrive in
    /// order after the leading `WebRequestResult`, and are always terminated by exactly one
    /// chunk with `done: true` - on clean EOF, on network error and on cancellation alike.
    WebRequestStreamChunk {
        stream_id: String,
        chunk: Vec<u8>,
        /// no further chunks will arrive for this stream_id
        done: bool,
        /// set only on the final chunk, when the stream was aborted rather than completed
        error: Option<String>,
    },
    CommandPaneOpened(u32, Context), // u32 - terminal_pane_id
    CommandPaneExited(u32, Option<i32>, Context), // u32 - terminal_pane_id, Option<i32> -
    // exit_code
    PaneClosed(PaneId),
    EditPaneOpened(u32, Context),              // u32 - terminal_pane_id
    EditPaneExited(u32, Option<i32>, Context), // u32 - terminal_pane_id, Option<i32> - exit code
    CommandPaneReRun(u32, Context),            // u32 - terminal_pane_id, Option<i32> -
    FailedToWriteConfigToDisk(Option<String>), // String -> the file path we failed to write
    ListClients(Vec<ClientInfo>),
    HostFolderChanged(PathBuf),               // PathBuf -> new host folder
    FailedToChangeHostFolder(Option<String>), // String -> the error we got when changing
    PastedText(String),
    ConfigWasWrittenToDisk,
    BeforeClose,
    InterceptedKeyPress(KeyWithModifier),
    /// An action was performed by the user (requires InterceptInput permission)
    UserAction(Action, ClientId, Option<u32>, Option<ClientId>), // Action, client_id, terminal_id, cli_client_id
    PaneRenderReport(HashMap<PaneId, PaneContents>),
    ActionComplete(Action, Option<PaneId>, BTreeMap<String, String>), // Action, pane_id, context
    CwdChanged(PaneId, PathBuf, Vec<ClientId>), // pane_id, cwd, focused_client_ids
    CommandChanged(PaneId, Vec<String>, bool, Vec<ClientId>), // pane_id, command, is_foreground, focused_client_ids
    AvailableLayoutInfo(Vec<LayoutInfo>, Vec<LayoutWithError>),
    PluginConfigurationChanged(BTreeMap<String, String>),
    HighlightClicked {
        pane_id: PaneId,
        pattern: String,
        matched_string: String,
        context: BTreeMap<String, String>,
    },
    /// A right-click asked for a context menu. The bound `context_menu` hook plugin answers with
    /// `set_context_menu`; anything else that receives this should ignore it.
    ContextMenuRequested(Box<ContextMenuContext>),
    /// An item the plugin supplied - one carrying `context` - was chosen.
    ///
    /// Only items with a non-empty `context` produce this. An item that is only a list of
    /// `Action`s is run by the server and the plugin hears nothing, the same way a keybinding's
    /// actions are not narrated back to anyone.
    ContextMenuItemSelected {
        label: String,
        /// The item's own `context`, echoed verbatim.
        item_context: BTreeMap<String, String>,
        /// The click the menu was opened by, so a stateless plugin still knows where it came from.
        context: Box<ContextMenuContext>,
    },
    /// Initial keybindings sent once on plugin load and on reconfiguration.
    /// Plugins that subscribe to this event signal they cache keybindings
    /// and can handle lightweight ModeUpdate events without keybindings.
    InitialKeybinds(KeybindsVec),
    /// The host terminal indicated its color palette theme mode (CSI 2031 / DSR 997).
    HostTerminalThemeChanged(HostTerminalThemeMode),
    HintText(BTreeMap<usize, StyledText>),
    ActivePaneScroll(Option<(usize, usize)>),
    /// The results of a watch registered with `data_watch` have changed.
    ///
    /// Sent only when the result set actually differs, so an idempotent write is silent. The rows
    /// are subject to the same limits as a fetch, and `truncated` says when they hit one.
    PluginDataChanged {
        watch_id: u64,
        rows: DataRows,
        truncated: bool,
    },
    /// A job this plugin enqueued with `EnqueueQueueJob` has started running.
    ///
    /// Sent only to the plugin that enqueued the job; `context` is what it passed, echoed
    /// verbatim. `plugin_id` is the job's own plugin instance.
    QueueJobStarted {
        queue: String,
        job_id: u64,
        plugin_id: u32,
        context: BTreeMap<String, String>,
    },
    /// A job this plugin enqueued has finished, one way or another. Sent only to the enqueuer.
    ///
    /// `payload` is what the job passed to `FinishQueueJob`, or the error for a job that failed
    /// to load or crashed; a job that was cancelled or simply unloaded itself has none.
    QueueJobFinished {
        queue: String,
        job_id: u64,
        outcome: QueueJobOutcome,
        payload: Option<String>,
        context: BTreeMap<String, String>,
    },
    /// A queue's contents or concurrency changed. Broadcast to every subscriber.
    QueueChanged {
        queue: String,
        concurrency: usize,
        pending: Vec<QueueJobInfo>,
        running: Vec<QueueJobInfo>,
    },
    /// A hook point with a headless shape was reached. Sent only to the plugin bound to `point`
    /// in `hooks { }`, which is loaded (without a pane) the first time it is needed and receives
    /// every later firing. The payload keys are documented per point in `docs/LUA_PLUGINS.md`.
    Hook {
        point: HookPoint,
        payload: BTreeMap<String, String>,
    },
}

impl EventType {
    /// The events `zellij events` streams: the session's state as it is reported to plugins.
    /// Not here, on purpose: one client's input (`Key`, `Mouse`, `PastedText`), one plugin's
    /// replies (`RunCommandResult`, `WebRequestResult`, `CustomMessage`, `Timer`) and the
    /// render report, which has its own stream in `zellij subscribe`.
    pub const CLI_STREAMABLE: &'static [EventType] = &[
        EventType::ModeUpdate,
        EventType::TabUpdate,
        EventType::PaneUpdate,
        EventType::SessionUpdate,
        EventType::CommandPaneOpened,
        EventType::CommandPaneExited,
        EventType::CommandPaneReRun,
        EventType::EditPaneOpened,
        EventType::EditPaneExited,
        EventType::PaneClosed,
        EventType::CwdChanged,
        EventType::CommandChanged,
        EventType::HostFolderChanged,
        EventType::FileSystemCreate,
        EventType::FileSystemRead,
        EventType::FileSystemUpdate,
        EventType::FileSystemDelete,
        EventType::PluginConfigurationChanged,
        EventType::QueueChanged,
    ];

    pub fn is_cli_streamable(&self) -> bool {
        Self::CLI_STREAMABLE.contains(self)
    }

    /// A streamable event by either of its spellings, `PaneUpdate` or `pane_update`. `None` for
    /// a name that is not an event, or an event that is not streamed.
    pub fn from_cli_name(name: &str) -> Option<EventType> {
        let name = name.trim();
        let pascal: String = match name.contains('_') {
            true => name
                .split('_')
                .map(|part| {
                    let mut chars = part.chars();
                    match chars.next() {
                        Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                        None => String::new(),
                    }
                })
                .collect(),
            false => name.to_owned(),
        };
        pascal
            .parse::<EventType>()
            .ok()
            .filter(EventType::is_cli_streamable)
    }

    pub fn cli_streamable_names() -> Vec<String> {
        Self::CLI_STREAMABLE
            .iter()
            .map(EventType::to_string)
            .collect()
    }
}

/// How a queued job ended.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QueueJobOutcome {
    /// The job called `FinishQueueJob` with `success: true`.
    Succeeded,
    /// The job called `FinishQueueJob` with `success: false`, failed to load, or crashed.
    Failed,
    /// Someone called `CancelQueueJob` on it.
    Cancelled,
    /// The job's plugin unloaded without reporting: it closed itself, or its pane was closed.
    Aborted,
}

/// Where a queued job is in its life.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QueueJobState {
    Pending,
    Running,
}

/// One job on a work queue, as reported by `QueueStatus` and `Event::QueueChanged`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueJobInfo {
    pub job_id: u64,
    pub queue: String,
    pub plugin_url: String,
    /// The job's plugin instance, once it is running.
    pub plugin_id: Option<u32>,
    pub enqueuer_plugin_id: u32,
    pub context: BTreeMap<String, String>,
    pub state: QueueJobState,
}

/// One work queue and everything on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueSnapshot {
    pub queue: String,
    pub concurrency: usize,
    pub pending: Vec<QueueJobInfo>,
    pub running: Vec<QueueJobInfo>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HostTerminalThemeMode {
    Dark,
    Light,
}

#[derive(
    Debug,
    PartialEq,
    Eq,
    Hash,
    Copy,
    Clone,
    EnumDiscriminants,
    Display,
    Serialize,
    Deserialize,
    PartialOrd,
    Ord,
)]
#[strum_discriminants(derive(
    EnumString,
    EnumIter,
    Hash,
    Serialize,
    Deserialize,
    Display,
    PartialOrd,
    Ord
))]
#[strum_discriminants(name(PermissionType))]
#[non_exhaustive]
pub enum Permission {
    ReadApplicationState,
    ChangeApplicationState,
    OpenFiles,
    RunCommands,
    OpenTerminalsOrPlugins,
    WriteToStdin,
    WebAccess,
    ReadCliPipes,
    MessageAndLaunchOtherPlugins,
    Reconfigure,
    FullHdAccess,
    InterceptInput,
    ReadPaneContents,
    RunActionsAsUser,
    WriteToClipboard,
    ReadSessionEnvironmentVariables,
    PluginData,
    SharedPluginData,
    ReadFiles,
    WriteFiles,
    /// Write to `/plugins`, the user's own plugin folder, and reload plugins by url. The
    /// authority a plugin that writes other plugins needs, and nothing wider: `WriteFiles`
    /// still covers the write itself, so a plugin that only reads `/plugins` needs `ReadFiles`.
    AuthorPlugins,
}

impl PermissionType {
    pub fn display_name(&self) -> String {
        match self {
            PermissionType::ReadApplicationState => {
                "Access Zellij state (Panes, Tabs and UI)".to_owned()
            },
            PermissionType::ChangeApplicationState => {
                "Change Zellij state (Panes, Tabs and UI)".to_owned()
            },
            PermissionType::OpenFiles => "Open files (eg. for editing)".to_owned(),
            PermissionType::RunCommands => "Run commands".to_owned(),
            PermissionType::OpenTerminalsOrPlugins => "Start new terminals and plugins".to_owned(),
            PermissionType::WriteToStdin => "Write to standard input (STDIN)".to_owned(),
            PermissionType::WebAccess => "Make web requests".to_owned(),
            PermissionType::ReadCliPipes => "Control command line pipes and output".to_owned(),
            PermissionType::MessageAndLaunchOtherPlugins => {
                "Send messages to and launch other plugins".to_owned()
            },
            PermissionType::Reconfigure => "Change Zellij runtime configuration".to_owned(),
            PermissionType::FullHdAccess => "Full access to the hard-drive".to_owned(),
            PermissionType::InterceptInput => "Intercept Input (keyboard & mouse)".to_owned(),
            PermissionType::ReadPaneContents => {
                "Read pane contents (viewport and selection)".to_owned()
            },
            PermissionType::RunActionsAsUser => "Execute actions as the user".to_owned(),
            PermissionType::WriteToClipboard => "Write to clipboard".to_owned(),
            PermissionType::ReadSessionEnvironmentVariables => {
                "Read environment variables present upon session creation".to_owned()
            },
            PermissionType::PluginData => "Store and query its own structured data".to_owned(),
            PermissionType::SharedPluginData => {
                "Store and query data shared with other plugins".to_owned()
            },
            PermissionType::ReadFiles => "Read files in its own folders".to_owned(),
            PermissionType::WriteFiles => "Write files in its own folders".to_owned(),
            PermissionType::AuthorPlugins => {
                "Write and reload plugins in your plugin folder".to_owned()
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct PluginPermission {
    pub name: String,
    pub permissions: Vec<PermissionType>,
}

impl PluginPermission {
    pub fn new(name: String, permissions: Vec<PermissionType>) -> Self {
        PluginPermission { name, permissions }
    }
}

/// Describes the different input modes, which change the way that keystrokes will be interpreted.
#[derive(
    Debug,
    PartialEq,
    Eq,
    Hash,
    Copy,
    Clone,
    EnumIter,
    Serialize,
    Deserialize,
    ValueEnum,
    PartialOrd,
    Ord,
    Default,
)]
pub enum InputMode {
    /// In `Normal` mode, input is always written to the terminal, except for the shortcuts leading
    /// to other modes
    #[default]
    #[serde(alias = "normal")]
    Normal,
    /// In `Locked` mode, input is always written to the terminal and all shortcuts are disabled
    /// except the one leading back to normal mode
    #[serde(alias = "locked")]
    Locked,
    /// `Resize` mode allows resizing the different existing panes.
    #[serde(alias = "resize")]
    Resize,
    /// `Pane` mode allows creating and closing panes, as well as moving between them.
    #[serde(alias = "pane")]
    Pane,
    /// `Tab` mode allows creating and closing tabs, as well as moving between them.
    #[serde(alias = "tab")]
    Tab,
    /// `Scroll` mode allows scrolling up and down within a pane.
    #[serde(alias = "scroll")]
    Scroll,
    /// `EnterSearch` mode allows for typing in the needle for a search in the scroll buffer of a pane.
    #[serde(alias = "entersearch")]
    EnterSearch,
    /// `Search` mode allows for searching a term in a pane (superset of `Scroll`).
    #[serde(alias = "search")]
    Search,
    /// `RenameTab` mode allows assigning a new name to a tab.
    #[serde(alias = "renametab")]
    RenameTab,
    /// `RenamePane` mode allows assigning a new name to a pane.
    #[serde(alias = "renamepane")]
    RenamePane,
    /// `Session` mode allows detaching sessions
    #[serde(alias = "session")]
    Session,
    /// `Move` mode allows moving the different existing panes within a tab
    #[serde(alias = "move")]
    Move,
    /// `Prompt` mode allows interacting with active prompts.
    #[serde(alias = "prompt")]
    Prompt,
    /// `Tmux` mode allows for basic tmux keybindings functionality
    #[serde(alias = "tmux")]
    Tmux,
    /// `Agent` mode gathers the hocus agent actions behind a single leader.
    #[serde(alias = "agent")]
    Agent,
    /// `Select` mode extends a text selection in the focused pane.
    #[serde(alias = "select")]
    Select,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default, ValueEnum)]
pub enum ThemeHue {
    #[serde(alias = "light")]
    Light,
    #[default]
    #[serde(alias = "dark")]
    Dark,
}

impl FromStr for ThemeHue {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "light" => Ok(ThemeHue::Light),
            "dark" => Ok(ThemeHue::Dark),
            e => Err(format!(
                "Unknown theme hue: '{}' (expected 'dark' or 'light')",
                e
            )),
        }
    }
}

impl fmt::Display for ThemeHue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ThemeHue::Light => write!(f, "light"),
            ThemeHue::Dark => write!(f, "dark"),
        }
    }
}

impl From<ThemeHue> for HostTerminalThemeMode {
    fn from(hue: ThemeHue) -> Self {
        match hue {
            ThemeHue::Light => HostTerminalThemeMode::Light,
            ThemeHue::Dark => HostTerminalThemeMode::Dark,
        }
    }
}

impl From<HostTerminalThemeMode> for ThemeHue {
    fn from(mode: HostTerminalThemeMode) -> Self {
        match mode {
            HostTerminalThemeMode::Light => ThemeHue::Light,
            HostTerminalThemeMode::Dark => ThemeHue::Dark,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum PaletteColor {
    Rgb((u8, u8, u8)),
    EightBit(u8),
}
impl Default for PaletteColor {
    fn default() -> PaletteColor {
        PaletteColor::EightBit(0)
    }
}

/// Priority layer for plugin-supplied regex highlights.
/// Higher-priority layers take visual precedence over lower ones
/// when highlights overlap.  Built-in highlights (mouse selection,
/// search results) always take precedence over all plugin layers.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default,
)]
pub enum HighlightLayer {
    #[default]
    Hint, // lowest: pure pattern matching (paths, URLs, IPs)
    Tool,           // middle: backed by runtime domain knowledge (git, docker, k8s)
    ActionFeedback, // highest: result of an explicit user action (search, bookmarks)
}

/// Style for a plugin-supplied regex highlight.
/// Theme-based variants reference `style.colors.text_unselected.*`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HighlightStyle {
    None,      // no color override — use with bold/italic/underline for style-only highlights
    Emphasis0, // fg = emphasis_0, no bg override
    Emphasis1, // fg = emphasis_1, no bg override
    Emphasis2, // fg = emphasis_2, no bg override
    Emphasis3, // fg = emphasis_3, no bg override
    BackgroundEmphasis0, // bg = emphasis_0, fg = background
    BackgroundEmphasis1, // bg = emphasis_1, fg = background
    BackgroundEmphasis2, // bg = emphasis_2, fg = background
    BackgroundEmphasis3, // bg = emphasis_3, fg = background
    CustomRgb {
        fg: Option<(u8, u8, u8)>,
        bg: Option<(u8, u8, u8)>,
    },
    CustomIndex {
        fg: Option<u8>,
        bg: Option<u8>,
    },
}

/// One pattern + style pair sent by a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegexHighlight {
    pub pattern: String, // key for upsert; also the regex source
    pub style: HighlightStyle,
    pub layer: HighlightLayer,
    pub context: BTreeMap<String, String>, // arbitrary data echoed back verbatim on click
    pub on_hover: bool, // if true, only rendered when the cursor overlaps this match
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub tooltip_text: Option<String>, // shown at bottom of pane frame when hovering over match
}

/// One row of a right-click context menu.
///
/// A menu is a list of these and nothing else: the server draws them, hit-tests them, and runs
/// `actions` against the pane the click landed in. There is no widget tree and no nesting - see
/// `zellij-server/src/ui/context_menu.rs` for the drawing and `input::context_menu` for the
/// configured half of where they come from.
///
/// **`actions` does not cross the plugin boundary, and `id` is why it does not have to.** A good
/// many `Action` variants have no protobuf representation at all - `Action::Copy` among them - so
/// a wire format carrying actions would drop some items' behaviour in silence. Instead the server
/// keeps the action lists and sends the plugin an `id` per configured item; a plugin returning an
/// item with that `id` gets the actions re-attached on arrival. An item the plugin *invented* has
/// no `id`, no actions, and acts through its `context` instead - which reaches it as
/// `Event::ContextMenuItemSelected`, from where `run_action` can do anything an action could.
///
/// `separator` is a variant spelled as a flag rather than as an enum because a separator is a row
/// like any other everywhere it matters - layout, hit-testing, and the wire format - and an enum
/// would put a match in each of those for one uninteresting case.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMenuItem {
    /// Identifies a configured item across the plugin boundary. `None` for an item a plugin made
    /// up, and for one a plugin returned without the `id` it was given - which is how a plugin
    /// deliberately strips a configured item's behaviour, if it ever wants to.
    #[serde(default)]
    pub id: Option<u64>,
    /// Defaulted rather than required: a separator genuinely has no label, and making every
    /// plugin write `label = ""` for one would be a papercut with nothing behind it.
    #[serde(default)]
    pub label: String,
    /// Drawn right-aligned, dimmed. Purely a hint - nothing binds it.
    #[serde(default)]
    pub shortcut: Option<String>,
    /// Run in order, against the pane the menu was opened over. Never sent to a plugin; see the
    /// type's own documentation for why.
    #[serde(default, skip_serializing)]
    pub actions: Vec<Action>,
    /// Opaque plugin data echoed back to the hook plugin when this item is chosen, exactly as
    /// `RegexHighlight::context` is echoed on click. This is how a plugin-supplied item does
    /// something no `Action` can express.
    #[serde(default)]
    pub context: BTreeMap<String, String>,
    /// Drawn dimmed and not selectable.
    #[serde(default)]
    pub disabled: bool,
    /// A horizontal rule. `label`, `actions` and the rest are ignored.
    #[serde(default)]
    pub separator: bool,
}

impl ContextMenuItem {
    pub fn new<L: Into<String>>(label: L, actions: Vec<Action>) -> Self {
        ContextMenuItem {
            label: label.into(),
            actions,
            ..Default::default()
        }
    }
    pub fn separator() -> Self {
        ContextMenuItem {
            separator: true,
            ..Default::default()
        }
    }
    pub fn with_shortcut<S: Into<String>>(mut self, shortcut: S) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }
    pub fn with_id(mut self, id: u64) -> Self {
        self.id = Some(id);
        self
    }
    /// Whether the keyboard and mouse may land on this row.
    pub fn is_selectable(&self) -> bool {
        !self.separator && !self.disabled
    }
    /// This item as the plugin should see it: identified, but without the behaviour the server
    /// keeps to itself.
    pub fn without_actions(&self) -> Self {
        ContextMenuItem {
            actions: vec![],
            ..self.clone()
        }
    }
}

/// A `link`-plugin regex match sitting under the cursor when the menu was opened.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMenuLink {
    /// The plugin that registered the highlight, so an item can be aimed back at it.
    pub plugin_id: u32,
    pub pattern: String,
    pub matched: String,
    pub context: BTreeMap<String, String>,
}

/// Everything the server could cheaply learn about a right-click, handed to the hook plugin so it
/// can decide what the menu should say.
///
/// "Cheaply" is the whole design rule. Two things a menu might plausibly want are absent for the
/// same reason - getting them would mean blocking the screen thread, which every mouse *motion*
/// also passes through:
///
/// - **cwd** lives in the pty thread (`PtyInstruction::GetPaneCwd`), reachable only through a
///   blocking `recv_timeout`. A plugin asks for it itself with `get_pane_cwd`.
/// - **scrollback** can be megabytes. `get_pane_scrollback` already exists for a plugin that
///   wants it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMenuContext {
    /// Correlates the plugin's answer with the menu that asked. An answer naming a menu that has
    /// since closed is dropped.
    pub request_id: u64,
    pub client_id: ClientId,
    pub pane_id: PaneId,
    pub tab_position: usize,
    /// The clicked cell in surface coordinates - where the menu is anchored.
    pub click: (usize, usize),
    /// The same cell relative to the pane, which is what a plugin reasoning about pane content
    /// wants. Both are carried because neither can be derived from the other here.
    pub click_in_pane: (usize, usize),
    /// The click landed on the pane's frame rather than its content.
    pub on_frame: bool,
    pub pane_title: String,
    pub pane_command: Option<String>,
    pub pane_exited: bool,
    pub pane_exit_status: Option<i32>,
    pub pane_is_held: bool,
    pub pane_is_floating: bool,
    pub selected_text: Option<String>,
    pub word_under_cursor: Option<String>,
    pub line_under_cursor: Option<String>,
    pub link: Option<ContextMenuLink>,
    /// The items the configuration declared, in the order it declared them. The plugin filters and
    /// augments this rather than building a menu from nothing.
    pub items: Vec<ContextMenuItem>,
}

impl FromStr for InputMode {
    type Err = ConversionError;

    fn from_str(s: &str) -> Result<Self, ConversionError> {
        match s {
            "normal" | "Normal" => Ok(InputMode::Normal),
            "locked" | "Locked" => Ok(InputMode::Locked),
            "resize" | "Resize" => Ok(InputMode::Resize),
            "pane" | "Pane" => Ok(InputMode::Pane),
            "tab" | "Tab" => Ok(InputMode::Tab),
            "search" | "Search" => Ok(InputMode::Search),
            "scroll" | "Scroll" => Ok(InputMode::Scroll),
            "renametab" | "RenameTab" => Ok(InputMode::RenameTab),
            "renamepane" | "RenamePane" => Ok(InputMode::RenamePane),
            "session" | "Session" => Ok(InputMode::Session),
            "move" | "Move" => Ok(InputMode::Move),
            "prompt" | "Prompt" => Ok(InputMode::Prompt),
            "tmux" | "Tmux" => Ok(InputMode::Tmux),
            "agent" | "Agent" => Ok(InputMode::Agent),
            "select" | "Select" => Ok(InputMode::Select),
            "entersearch" | "Entersearch" | "EnterSearch" => Ok(InputMode::EnterSearch),
            e => Err(ConversionError::UnknownInputMode(e.into())),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub enum PaletteSource {
    #[default]
    Default,
    Xresources,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub struct Palette {
    pub source: PaletteSource,
    pub theme_hue: ThemeHue,
    pub fg: PaletteColor,
    pub bg: PaletteColor,
    pub black: PaletteColor,
    pub red: PaletteColor,
    pub green: PaletteColor,
    pub yellow: PaletteColor,
    pub blue: PaletteColor,
    pub magenta: PaletteColor,
    pub cyan: PaletteColor,
    pub white: PaletteColor,
    pub orange: PaletteColor,
    pub gray: PaletteColor,
    pub purple: PaletteColor,
    pub gold: PaletteColor,
    pub silver: PaletteColor,
    pub pink: PaletteColor,
    pub brown: PaletteColor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Style {
    pub colors: Styling,
    pub rounded_corners: bool,
    pub hide_session_name: bool,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub enum Coloration {
    NoStyling,
    Styled(StyleDeclaration),
}

impl Coloration {
    pub fn with_fallback(&self, fallback: StyleDeclaration) -> StyleDeclaration {
        match &self {
            Coloration::NoStyling => fallback,
            Coloration::Styled(style) => *style,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct Styling {
    pub text_unselected: StyleDeclaration,
    pub text_selected: StyleDeclaration,
    pub ribbon_unselected: StyleDeclaration,
    pub ribbon_selected: StyleDeclaration,
    pub table_title: StyleDeclaration,
    pub table_cell_unselected: StyleDeclaration,
    pub table_cell_selected: StyleDeclaration,
    pub list_unselected: StyleDeclaration,
    pub list_selected: StyleDeclaration,
    pub frame_unselected: Option<StyleDeclaration>,
    pub frame_selected: StyleDeclaration,
    pub frame_highlight: StyleDeclaration,
    pub exit_code_success: StyleDeclaration,
    pub exit_code_error: StyleDeclaration,
    pub multiplayer_user_colors: MultiplayerColors,
}

#[derive(Debug, Copy, Default, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct StyleDeclaration {
    pub base: PaletteColor,
    pub background: PaletteColor,
    pub emphasis_0: PaletteColor,
    pub emphasis_1: PaletteColor,
    pub emphasis_2: PaletteColor,
    pub emphasis_3: PaletteColor,
}

#[derive(Debug, Copy, Default, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct MultiplayerColors {
    pub player_1: PaletteColor,
    pub player_2: PaletteColor,
    pub player_3: PaletteColor,
    pub player_4: PaletteColor,
    pub player_5: PaletteColor,
    pub player_6: PaletteColor,
    pub player_7: PaletteColor,
    pub player_8: PaletteColor,
    pub player_9: PaletteColor,
    pub player_10: PaletteColor,
}

pub const DEFAULT_STYLES: Styling = Styling {
    text_unselected: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::BRIGHT_GRAY),
        emphasis_0: PaletteColor::EightBit(default_colors::ORANGE),
        emphasis_1: PaletteColor::EightBit(default_colors::CYAN),
        emphasis_2: PaletteColor::EightBit(default_colors::GREEN),
        emphasis_3: PaletteColor::EightBit(default_colors::MAGENTA),
        background: PaletteColor::EightBit(default_colors::GRAY),
    },
    text_selected: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::BRIGHT_GRAY),
        emphasis_0: PaletteColor::EightBit(default_colors::ORANGE),
        emphasis_1: PaletteColor::EightBit(default_colors::CYAN),
        emphasis_2: PaletteColor::EightBit(default_colors::GREEN),
        emphasis_3: PaletteColor::EightBit(default_colors::MAGENTA),
        background: PaletteColor::EightBit(default_colors::GRAY),
    },
    ribbon_unselected: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::BLACK),
        emphasis_0: PaletteColor::EightBit(default_colors::RED),
        emphasis_1: PaletteColor::EightBit(default_colors::WHITE),
        emphasis_2: PaletteColor::EightBit(default_colors::BLUE),
        emphasis_3: PaletteColor::EightBit(default_colors::MAGENTA),
        background: PaletteColor::EightBit(default_colors::GRAY),
    },
    ribbon_selected: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::BLACK),
        emphasis_0: PaletteColor::EightBit(default_colors::RED),
        emphasis_1: PaletteColor::EightBit(default_colors::ORANGE),
        emphasis_2: PaletteColor::EightBit(default_colors::MAGENTA),
        emphasis_3: PaletteColor::EightBit(default_colors::BLUE),
        background: PaletteColor::EightBit(default_colors::GREEN),
    },
    exit_code_success: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::GREEN),
        emphasis_0: PaletteColor::EightBit(default_colors::CYAN),
        emphasis_1: PaletteColor::EightBit(default_colors::BLACK),
        emphasis_2: PaletteColor::EightBit(default_colors::MAGENTA),
        emphasis_3: PaletteColor::EightBit(default_colors::BLUE),
        background: PaletteColor::EightBit(default_colors::GRAY),
    },
    exit_code_error: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::RED),
        emphasis_0: PaletteColor::EightBit(default_colors::YELLOW),
        emphasis_1: PaletteColor::EightBit(default_colors::GOLD),
        emphasis_2: PaletteColor::EightBit(default_colors::SILVER),
        emphasis_3: PaletteColor::EightBit(default_colors::PURPLE),
        background: PaletteColor::EightBit(default_colors::GRAY),
    },
    frame_unselected: None,
    frame_selected: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::GREEN),
        emphasis_0: PaletteColor::EightBit(default_colors::ORANGE),
        emphasis_1: PaletteColor::EightBit(default_colors::CYAN),
        emphasis_2: PaletteColor::EightBit(default_colors::MAGENTA),
        emphasis_3: PaletteColor::EightBit(default_colors::BROWN),
        background: PaletteColor::EightBit(default_colors::GRAY),
    },
    frame_highlight: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::ORANGE),
        emphasis_0: PaletteColor::EightBit(default_colors::MAGENTA),
        emphasis_1: PaletteColor::EightBit(default_colors::PURPLE),
        emphasis_2: PaletteColor::EightBit(default_colors::GREEN),
        emphasis_3: PaletteColor::EightBit(default_colors::GREEN),
        background: PaletteColor::EightBit(default_colors::GREEN),
    },
    table_title: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::GREEN),
        emphasis_0: PaletteColor::EightBit(default_colors::ORANGE),
        emphasis_1: PaletteColor::EightBit(default_colors::CYAN),
        emphasis_2: PaletteColor::EightBit(default_colors::GREEN),
        emphasis_3: PaletteColor::EightBit(default_colors::MAGENTA),
        background: PaletteColor::EightBit(default_colors::GRAY),
    },
    table_cell_unselected: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::BRIGHT_GRAY),
        emphasis_0: PaletteColor::EightBit(default_colors::ORANGE),
        emphasis_1: PaletteColor::EightBit(default_colors::CYAN),
        emphasis_2: PaletteColor::EightBit(default_colors::GREEN),
        emphasis_3: PaletteColor::EightBit(default_colors::MAGENTA),
        background: PaletteColor::EightBit(default_colors::GRAY),
    },
    table_cell_selected: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::GREEN),
        emphasis_0: PaletteColor::EightBit(default_colors::ORANGE),
        emphasis_1: PaletteColor::EightBit(default_colors::CYAN),
        emphasis_2: PaletteColor::EightBit(default_colors::RED),
        emphasis_3: PaletteColor::EightBit(default_colors::MAGENTA),
        background: PaletteColor::EightBit(default_colors::GRAY),
    },
    list_unselected: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::BRIGHT_GRAY),
        emphasis_0: PaletteColor::EightBit(default_colors::ORANGE),
        emphasis_1: PaletteColor::EightBit(default_colors::CYAN),
        emphasis_2: PaletteColor::EightBit(default_colors::GREEN),
        emphasis_3: PaletteColor::EightBit(default_colors::MAGENTA),
        background: PaletteColor::EightBit(default_colors::GRAY),
    },
    list_selected: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::GREEN),
        emphasis_0: PaletteColor::EightBit(default_colors::ORANGE),
        emphasis_1: PaletteColor::EightBit(default_colors::CYAN),
        emphasis_2: PaletteColor::EightBit(default_colors::RED),
        emphasis_3: PaletteColor::EightBit(default_colors::MAGENTA),
        background: PaletteColor::EightBit(default_colors::GRAY),
    },
    multiplayer_user_colors: MultiplayerColors {
        player_1: PaletteColor::EightBit(default_colors::MAGENTA),
        player_2: PaletteColor::EightBit(default_colors::BLUE),
        player_3: PaletteColor::EightBit(default_colors::PURPLE),
        player_4: PaletteColor::EightBit(default_colors::YELLOW),
        player_5: PaletteColor::EightBit(default_colors::CYAN),
        player_6: PaletteColor::EightBit(default_colors::GOLD),
        player_7: PaletteColor::EightBit(default_colors::RED),
        player_8: PaletteColor::EightBit(default_colors::SILVER),
        player_9: PaletteColor::EightBit(default_colors::PINK),
        player_10: PaletteColor::EightBit(default_colors::BROWN),
    },
};

impl Default for Styling {
    fn default() -> Self {
        DEFAULT_STYLES
    }
}

impl From<Styling> for Palette {
    fn from(styling: Styling) -> Self {
        Palette {
            theme_hue: ThemeHue::Dark,
            source: PaletteSource::Default,
            fg: styling.ribbon_unselected.background,
            bg: styling.text_unselected.background,
            red: styling.exit_code_error.base,
            green: styling.text_unselected.emphasis_2,
            yellow: styling.exit_code_error.emphasis_0,
            blue: styling.ribbon_unselected.emphasis_2,
            magenta: styling.text_unselected.emphasis_3,
            orange: styling.text_unselected.emphasis_0,
            cyan: styling.text_unselected.emphasis_1,
            black: styling.ribbon_unselected.base,
            white: styling.ribbon_unselected.emphasis_1,
            gray: styling.list_unselected.background,
            purple: styling.multiplayer_user_colors.player_3,
            gold: styling.multiplayer_user_colors.player_6,
            silver: styling.multiplayer_user_colors.player_8,
            pink: styling.multiplayer_user_colors.player_9,
            brown: styling.multiplayer_user_colors.player_10,
        }
    }
}

impl From<Palette> for Styling {
    fn from(palette: Palette) -> Self {
        let (fg, bg) = match palette.theme_hue {
            ThemeHue::Light => (palette.black, palette.white),
            ThemeHue::Dark => (palette.white, palette.black),
        };
        Styling {
            text_unselected: StyleDeclaration {
                base: fg,
                emphasis_0: palette.orange,
                emphasis_1: palette.cyan,
                emphasis_2: palette.green,
                emphasis_3: palette.magenta,
                background: bg,
            },
            text_selected: StyleDeclaration {
                base: fg,
                emphasis_0: palette.orange,
                emphasis_1: palette.cyan,
                emphasis_2: palette.green,
                emphasis_3: palette.magenta,
                background: palette.bg,
            },
            ribbon_unselected: StyleDeclaration {
                base: palette.black,
                emphasis_0: palette.red,
                emphasis_1: palette.white,
                emphasis_2: palette.blue,
                emphasis_3: palette.magenta,
                background: palette.fg,
            },
            ribbon_selected: StyleDeclaration {
                base: palette.black,
                emphasis_0: palette.red,
                emphasis_1: palette.orange,
                emphasis_2: palette.magenta,
                emphasis_3: palette.blue,
                background: palette.green,
            },
            exit_code_success: StyleDeclaration {
                base: palette.green,
                emphasis_0: palette.cyan,
                emphasis_1: palette.black,
                emphasis_2: palette.magenta,
                emphasis_3: palette.blue,
                background: Default::default(),
            },
            exit_code_error: StyleDeclaration {
                base: palette.red,
                emphasis_0: palette.yellow,
                emphasis_1: palette.gold,
                emphasis_2: palette.silver,
                emphasis_3: palette.purple,
                background: Default::default(),
            },
            frame_unselected: None,
            frame_selected: StyleDeclaration {
                base: palette.green,
                emphasis_0: palette.orange,
                emphasis_1: palette.cyan,
                emphasis_2: palette.magenta,
                emphasis_3: palette.brown,
                background: Default::default(),
            },
            frame_highlight: StyleDeclaration {
                base: palette.orange,
                emphasis_0: palette.magenta,
                emphasis_1: palette.purple,
                emphasis_2: palette.orange,
                emphasis_3: palette.orange,
                background: Default::default(),
            },
            table_title: StyleDeclaration {
                base: palette.green,
                emphasis_0: palette.orange,
                emphasis_1: palette.cyan,
                emphasis_2: palette.green,
                emphasis_3: palette.magenta,
                background: palette.gray,
            },
            table_cell_unselected: StyleDeclaration {
                base: fg,
                emphasis_0: palette.orange,
                emphasis_1: palette.cyan,
                emphasis_2: palette.green,
                emphasis_3: palette.magenta,
                background: palette.black,
            },
            table_cell_selected: StyleDeclaration {
                base: fg,
                emphasis_0: palette.orange,
                emphasis_1: palette.cyan,
                emphasis_2: palette.green,
                emphasis_3: palette.magenta,
                background: palette.bg,
            },
            list_unselected: StyleDeclaration {
                base: palette.white,
                emphasis_0: palette.orange,
                emphasis_1: palette.cyan,
                emphasis_2: palette.green,
                emphasis_3: palette.magenta,
                background: palette.black,
            },
            list_selected: StyleDeclaration {
                base: palette.white,
                emphasis_0: palette.orange,
                emphasis_1: palette.cyan,
                emphasis_2: palette.green,
                emphasis_3: palette.magenta,
                background: palette.bg,
            },
            multiplayer_user_colors: MultiplayerColors {
                player_1: palette.magenta,
                player_2: palette.blue,
                player_3: palette.purple,
                player_4: palette.yellow,
                player_5: palette.cyan,
                player_6: palette.gold,
                player_7: palette.red,
                player_8: palette.silver,
                player_9: palette.pink,
                player_10: palette.brown,
            },
        }
    }
}

// FIXME: Poor devs hashtable since HashTable can't derive `Default`...
pub type KeybindsVec = Vec<(InputMode, Vec<(KeyWithModifier, Vec<Action>)>)>;

/// Provides information helpful in rendering the Zellij controls for UI bars
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeInfo {
    pub mode: InputMode,
    pub base_mode: Option<InputMode>,
    pub keybinds: KeybindsVec,
    pub style: Style,
    pub capabilities: PluginCapabilities,
    pub session_name: Option<String>,
    pub editor: Option<PathBuf>,
    pub shell: Option<PathBuf>,
    pub currently_marking_pane_group: Option<bool>,
    pub pane_frame_style: Option<PaneFrameStyle>,
    pub session_dimmed: Option<bool>,
    pub session_ancestry: Vec<String>,
    pub host_fullscreen: Option<bool>,
    pub nested_ascend_keys: Vec<KeyWithModifier>,
    pub session_ascended: Option<bool>,
    pub nested_descend_keys: Vec<KeyWithModifier>,
}

impl ModeInfo {
    pub fn get_mode_keybinds(&self) -> Vec<(KeyWithModifier, Vec<Action>)> {
        self.get_keybinds_for_mode(self.mode)
    }

    pub fn get_keybinds_for_mode(&self, mode: InputMode) -> Vec<(KeyWithModifier, Vec<Action>)> {
        for (vec_mode, map) in &self.keybinds {
            if mode == *vec_mode {
                return map.to_vec();
            }
        }
        vec![]
    }
    pub fn update_keybinds(&mut self, keybinds: Keybinds) {
        self.keybinds = keybinds.to_keybinds_vec();
    }
    pub fn update_default_mode(&mut self, new_default_mode: InputMode) {
        self.base_mode = Some(new_default_mode);
    }
    pub fn update_theme(&mut self, theme: Styling) {
        self.style.colors = theme;
    }
    pub fn update_rounded_corners(&mut self, rounded_corners: bool) {
        self.style.rounded_corners = rounded_corners;
    }
    pub fn update_arrow_fonts(&mut self, should_support_arrow_fonts: bool) {
        // it is honestly quite baffling to me how "arrow_fonts: false" can mean "I support arrow
        // fonts", but since this is a public API... ¯\_(ツ)_/¯
        self.capabilities.arrow_fonts = !should_support_arrow_fonts;
    }
    pub fn update_hide_session_name(&mut self, hide_session_name: bool) {
        self.style.hide_session_name = hide_session_name;
    }
    pub fn change_to_default_mode(&mut self) {
        if let Some(base_mode) = self.base_mode {
            self.mode = base_mode;
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SessionInfo {
    pub name: String,
    pub tabs: Vec<TabInfo>,
    pub panes: PaneManifest,
    pub connected_clients: usize,
    pub is_current_session: bool,
    pub available_layouts: Vec<LayoutInfo>,
    pub plugins: BTreeMap<u32, PluginInfo>,
    pub tab_history: BTreeMap<ClientId, Vec<usize>>,
    pub pane_history: BTreeMap<ClientId, Vec<PaneId>>,
    pub creation_time: Duration,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PluginInfo {
    pub location: String,
    pub configuration: BTreeMap<String, String>,
}

impl From<RunPlugin> for PluginInfo {
    fn from(run_plugin: RunPlugin) -> Self {
        PluginInfo {
            location: run_plugin.location.display(),
            configuration: run_plugin.configuration.inner().clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum LayoutInfo {
    BuiltIn(String),
    File(String, LayoutMetadata),
    Url(String),
    Stringified(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LayoutWithError {
    pub layout_name: String,
    pub error: LayoutParsingError,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum LayoutParsingError {
    KdlError {
        // boxed to keep this variant from making every `Result<_, LayoutParsingError>` wide
        kdl_error: Box<KdlError>,
        file_name: String,
        source_code: String,
    },
    SyntaxError,
}

impl AsRef<LayoutInfo> for LayoutInfo {
    fn as_ref(&self) -> &LayoutInfo {
        self
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LayoutMetadata {
    pub tabs: Vec<TabMetadata>,
    pub creation_time: String,
    pub update_time: String,
}

impl From<&PathBuf> for LayoutMetadata {
    fn from(path: &PathBuf) -> LayoutMetadata {
        match Layout::stringified_from_path(path) {
            Ok((path_str, stringified_layout, _swap_layouts)) => {
                match Layout::from_kdl(&stringified_layout, Some(path_str), None, None) {
                    Ok(layout) => {
                        let layout_tabs = layout.tabs();
                        let tabs = if layout_tabs.is_empty() {
                            let (tiled_pane_layout, floating_pane_layout) = layout.new_tab();
                            vec![TabMetadata::from(&(
                                None,
                                tiled_pane_layout,
                                floating_pane_layout,
                            ))]
                        } else {
                            layout
                                .tabs()
                                .into_iter()
                                .map(|tab| TabMetadata::from(&tab))
                                .collect()
                        };

                        // Get file metadata for creation and modification times as Unix epochs
                        let (creation_time, update_time) =
                            LayoutMetadata::creation_and_update_times(path);

                        LayoutMetadata {
                            tabs,
                            creation_time,
                            update_time,
                        }
                    },
                    Err(e) => {
                        log::error!("Failed to parse layout: {}", e);
                        LayoutMetadata::default()
                    },
                }
            },
            Err(e) => {
                log::error!("Failed to read layout file: {}", e);
                LayoutMetadata::default()
            },
        }
    }
}

impl LayoutMetadata {
    fn creation_and_update_times(path: &PathBuf) -> (String, String) {
        // (creation_time, update_time) returns stringified unix epoch
        match std::fs::metadata(path) {
            Ok(metadata) => {
                let creation_time = metadata
                    .created()
                    .ok()
                    .and_then(|t| {
                        t.duration_since(std::time::UNIX_EPOCH)
                            .ok()
                            .map(|d| d.as_secs().to_string())
                    })
                    .unwrap_or_default();

                let update_time = metadata
                    .modified()
                    .ok()
                    .and_then(|t| {
                        t.duration_since(std::time::UNIX_EPOCH)
                            .ok()
                            .map(|d| d.as_secs().to_string())
                    })
                    .unwrap_or_default();

                (creation_time, update_time)
            },
            Err(_) => (String::new(), String::new()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TabMetadata {
    pub panes: Vec<PaneMetadata>,
    pub name: Option<String>,
}

impl
    From<&(
        Option<String>,
        crate::input::layout::TiledPaneLayout,
        Vec<crate::input::layout::FloatingPaneLayout>,
    )> for TabMetadata
{
    fn from(
        tab: &(
            Option<String>,
            crate::input::layout::TiledPaneLayout,
            Vec<crate::input::layout::FloatingPaneLayout>,
        ),
    ) -> Self {
        let (tab_name, tiled_pane_layout, floating_panes) = tab;

        // Collect panes from tiled layout (only leaf nodes are real panes)
        let mut panes = Vec::new();
        collect_leaf_panes(tiled_pane_layout, &mut panes);

        // Collect panes from floating panes
        for floating_pane in floating_panes {
            panes.push(PaneMetadata::from(floating_pane));
        }

        TabMetadata {
            panes,
            name: tab_name.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PaneMetadata {
    pub name: Option<String>,
    pub is_plugin: bool,
    pub is_builtin_plugin: bool,
}

impl From<&crate::input::layout::TiledPaneLayout> for PaneMetadata {
    fn from(pane: &crate::input::layout::TiledPaneLayout) -> Self {
        let mut is_plugin = false;
        let mut is_builtin_plugin = false;

        // Try to get the name from the pane's name field first
        let name = if let Some(ref name) = pane.name {
            Some(name.clone())
        } else if let Some(ref run) = pane.run {
            // If no explicit name, glean it from the run configuration
            match run {
                Run::Command(cmd) => {
                    // Use the command name
                    Some(cmd.command.to_string_lossy().to_string())
                },
                Run::EditFile(path, _line, _cwd) => {
                    // Use the file name
                    path.file_name().map(|n| n.to_string_lossy().to_string())
                },
                Run::Plugin(plugin) => {
                    is_plugin = true;
                    is_builtin_plugin = plugin.is_builtin_plugin();
                    Some(plugin.location_string())
                },
                Run::Cwd(_) => None,
            }
        } else {
            None
        };

        PaneMetadata {
            name,
            is_plugin,
            is_builtin_plugin,
        }
    }
}

impl From<&crate::input::layout::FloatingPaneLayout> for PaneMetadata {
    fn from(pane: &crate::input::layout::FloatingPaneLayout) -> Self {
        let mut is_plugin = false;
        let mut is_builtin_plugin = false;

        // Try to get the name from the pane's name field first
        let name = if let Some(ref name) = pane.name {
            Some(name.clone())
        } else if let Some(ref run) = pane.run {
            // If no explicit name, glean it from the run configuration
            match run {
                Run::Command(cmd) => {
                    // Use the command name
                    Some(cmd.command.to_string_lossy().to_string())
                },
                Run::EditFile(path, _line, _cwd) => {
                    // Use the file name
                    path.file_name().map(|n| n.to_string_lossy().to_string())
                },
                Run::Plugin(plugin) => {
                    is_plugin = true;
                    is_builtin_plugin = match plugin {
                        crate::input::layout::RunPluginOrAlias::RunPlugin(run_plugin) => {
                            matches!(run_plugin.location, RunPluginLocation::Zellij(_))
                        },
                        crate::input::layout::RunPluginOrAlias::Alias(_) => false,
                    };
                    // Use the plugin location string
                    Some(plugin.location_string())
                },
                Run::Cwd(_) => None,
            }
        } else {
            None
        };

        PaneMetadata {
            name,
            is_plugin,
            is_builtin_plugin,
        }
    }
}

// Helper function to recursively collect leaf panes from TiledPaneLayout
fn collect_leaf_panes(
    pane: &crate::input::layout::TiledPaneLayout,
    result: &mut Vec<PaneMetadata>,
) {
    if pane.children.is_empty() {
        // This is a leaf node (actual pane)
        result.push(PaneMetadata::from(pane));
    } else {
        // This is a container, recurse into children
        for child in &pane.children {
            collect_leaf_panes(child, result);
        }
    }
}

impl LayoutInfo {
    pub fn name(&self) -> &str {
        match self {
            LayoutInfo::BuiltIn(name) => name,
            LayoutInfo::File(name, _) => name,
            LayoutInfo::Url(url) => url,
            LayoutInfo::Stringified(layout) => layout,
        }
    }
    pub fn is_builtin(&self) -> bool {
        match self {
            LayoutInfo::BuiltIn(_name) => true,
            LayoutInfo::File(_name, _) => false,
            LayoutInfo::Url(_url) => false,
            LayoutInfo::Stringified(_stringified) => false,
        }
    }
    pub fn from_cli(
        layout_dir: &Option<PathBuf>,
        maybe_layout_path: &Option<PathBuf>,
        cwd: PathBuf,
    ) -> Option<Self> {
        // If we're not given a layout path, fall back to "default". Since we cannot tell ahead of
        // time whether the user has a layout named "default.kdl" in their layout directory, we
        // cannot blindly assume that this is indeed the builtin default layout. The layout
        // resolution below will correctly handle this.
        // The docs promise this behavior, so we have to abide:
        // <https://zellij.dev/documentation/layouts.html#layout-default-directory>
        let layout_path = maybe_layout_path
            .clone()
            .unwrap_or(PathBuf::from("default"));

        if layout_path.starts_with("http://") || layout_path.starts_with("https://") {
            Some(LayoutInfo::Url(layout_path.display().to_string()))
        } else if layout_path.extension().is_some() || layout_path.components().count() > 1 {
            let layout_dir = cwd;
            let file_path = layout_dir.join(layout_path);
            Some(LayoutInfo::File(
                // layout_dir.join(layout_path).display().to_string(),
                file_path.display().to_string(),
                LayoutMetadata::from(&file_path),
            ))
        } else {
            // Attempt to interpret the layout as bare layout name from the layout application
            // directory. This is described in the docs:
            // <https://zellij.dev/documentation/layouts.html#layout-default-directory>
            if let Some(layout_dir) = layout_dir.clone().or_else(default_layout_dir) {
                let file_path = layout_dir.join(&layout_path);
                if file_path.exists() {
                    return Some(LayoutInfo::File(
                        file_path.display().to_string(),
                        LayoutMetadata::from(&file_path),
                    ));
                }
                let file_path_with_ext = file_path.with_extension("kdl");
                if file_path_with_ext.exists() {
                    return Some(LayoutInfo::File(
                        file_path_with_ext.display().to_string(),
                        LayoutMetadata::from(&file_path_with_ext),
                    ));
                }
            }
            // Assume a builtin layout by default
            Some(LayoutInfo::BuiltIn(layout_path.display().to_string()))
        }
    }
    pub fn from_config(
        layout_dir: &Option<PathBuf>,
        maybe_layout_path: &Option<PathBuf>,
    ) -> Option<Self> {
        // If we're not given a layout path, fall back to "default". Since we cannot tell ahead of
        // time whether the user has a layout named "default.kdl" in their layout directory, we
        // cannot blindly assume that this is indeed the builtin default layout. The layout
        // resolution below will correctly handle this.
        // The docs promise this behavior, so we have to abide:
        // <https://zellij.dev/documentation/layouts.html#layout-default-directory>
        let layout_path = maybe_layout_path
            .clone()
            .unwrap_or(PathBuf::from("default"));

        if layout_path.starts_with("http://") || layout_path.starts_with("https://") {
            Some(LayoutInfo::Url(layout_path.display().to_string()))
        } else if layout_path.extension().is_some() || layout_path.components().count() > 1 {
            let layout_dir = layout_dir.clone().or_else(default_layout_dir)?;
            let file_path = layout_dir.join(layout_path);
            Some(LayoutInfo::File(
                // layout_dir.join(layout_path).display().to_string(),
                file_path.display().to_string(),
                LayoutMetadata::from(&file_path),
            ))
        } else {
            // Attempt to interpret the layout as bare layout name from the layout application
            // directory. This is described in the docs:
            // <https://zellij.dev/documentation/layouts.html#layout-default-directory>
            if let Some(layout_dir) = layout_dir.clone().or_else(default_layout_dir) {
                let file_path = layout_dir.join(&layout_path);
                if file_path.exists() {
                    return Some(LayoutInfo::File(
                        file_path.display().to_string(),
                        LayoutMetadata::from(&file_path),
                    ));
                }
                let file_path_with_ext = file_path.with_extension("kdl");
                if file_path_with_ext.exists() {
                    return Some(LayoutInfo::File(
                        file_path_with_ext.display().to_string(),
                        LayoutMetadata::from(&file_path_with_ext),
                    ));
                }
            }
            // Assume a builtin layout by default
            Some(LayoutInfo::BuiltIn(layout_path.display().to_string()))
        }
    }
}

#[allow(clippy::derived_hash_with_manual_eq)]
impl Hash for SessionInfo {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl SessionInfo {
    pub fn new(name: String) -> Self {
        SessionInfo {
            name,
            ..Default::default()
        }
    }
    pub fn update_tab_info(&mut self, new_tab_info: Vec<TabInfo>) {
        self.tabs = new_tab_info;
    }
    pub fn update_pane_info(&mut self, new_pane_info: PaneManifest) {
        self.panes = new_pane_info;
    }
    pub fn update_connected_clients(&mut self, new_connected_clients: usize) {
        self.connected_clients = new_connected_clients;
    }
    pub fn populate_plugin_list(&mut self, plugins: BTreeMap<u32, RunPlugin>) {
        // u32 - plugin_id
        let mut plugin_list = BTreeMap::new();
        for (plugin_id, run_plugin) in plugins {
            plugin_list.insert(plugin_id, run_plugin.into());
        }
        self.plugins = plugin_list;
    }
}

/// What the agent in a pane is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
pub enum AgentState {
    /// Working.
    Busy,
    /// Stopped and blocked on the user: a permission prompt, a question, a form.
    Waiting,
    /// Finished its turn and at its prompt.
    Idle,
    /// The agent's process exited; the pane holds what it left.
    Exited,
}

impl AgentState {
    /// `busy` as well as `Busy`.
    pub fn parse_lenient(name: &str) -> Option<AgentState> {
        let mut chars = name.trim().chars();
        let first = chars.next()?.to_uppercase().collect::<String>();
        let rest = chars.as_str().to_lowercase();
        format!("{first}{rest}").parse().ok()
    }
}

/// Who wrote a pane's agent record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
pub enum AgentSource {
    /// The pane was opened as an agent pane, and nothing has been heard from the agent since:
    /// the state is what the session can see (running, exited), not what the agent says.
    Spawned,
    /// A person named the pane's agent by hand (hocus's `agent_register`), and nothing has been
    /// heard from the agent itself: the kind is theirs, the state is a placeholder.
    Registered,
    /// The agent itself said so — its hooks through `zellij action agent-report`, or a plugin
    /// speaking for it.
    Reported,
}

/// The agent running in a pane: what it is, what it is doing, and who said so.
///
/// Carried on [`PaneInfo::agent`], so every reader of the pane manifest — plugins, `zellij
/// events`, `list-panes --json` — sees the same answer to "is this an agent, and is it waiting
/// on me". Written by the spawn path, by `PluginCommand::SetPaneAgent`, and by `zellij action
/// agent-report`, which is what coding-agent hooks call. A record is complete every time it is
/// set; nothing merges — with one stated exception, [`AgentInfo::run_id`], which the agent
/// never reports and the session therefore carries forward for it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentInfo {
    /// `claude`, `codex`, … — the program, not a version of it.
    pub kind: String,
    pub state: AgentState,
    #[serde(default = "AgentSource::spawned")]
    pub source: AgentSource,
    /// A few words on what it is doing: the tool it is running, mostly.
    #[serde(default)]
    pub detail: Option<String>,
    /// What it is blocked on, in one line, when the state is `Waiting`: the tool a permission
    /// prompt is about, the question it asked, the sentence an MCP server wrote.
    ///
    /// A field of its own rather than a reuse of `detail`, because the two are cleared by
    /// different events and read in different places: `detail` names a tool in a status row,
    /// this names a question to somebody who has to answer it.
    #[serde(default)]
    pub waiting_on: Option<String>,
    /// Whether this wait is one a person has to answer, when the state is `Waiting`.
    ///
    /// `false` where the permission mode is about to answer the prompt itself — see
    /// `agent::waiting_needs_a_person`, which is where this is decided, and which is the
    /// only place it is worked out. Read by whatever would interrupt somebody over the
    /// wait; the state, the record and the agent strip all take the agent at its word
    /// regardless, so a suppressed wait is still a wait everywhere it is drawn.
    #[serde(default)]
    pub wait_needs_a_person: bool,
    /// The folder the agent is working in, which is not always the pane's: an agent told to
    /// work in a worktree moved without the shell under it hearing.
    #[serde(default)]
    pub cwd: Option<String>,
    /// The agent's own session id, where it has one, so a restart in the same pane can be
    /// told from a pane that merely went quiet.
    #[serde(default)]
    pub session_id: Option<String>,
    /// The id of the run record a plugin is keeping for this pane's agent, where one is
    /// keeping one.
    ///
    /// Opaque to the session: written by whoever holds the agent's history, read back by
    /// whoever picks the pane up. That is what lets a warped pane's run continue in the
    /// session that adopted it — an agent that supplies no session id has nothing else
    /// stable to be found by.
    ///
    /// The one field of a record the agent itself never says, and so the one field that is
    /// carried forward rather than re-supplied: a report that leaves it `None` inherits the
    /// pane's, and only clearing the whole record clears it. See `apply_pane_agent_report`.
    #[serde(default)]
    pub run_id: Option<String>,
}

impl AgentInfo {
    /// Whether this record is a wait that a person has to answer.
    ///
    /// The two halves have to be read together: `wait_needs_a_person` is only meaningful while
    /// the state is `Waiting`, and a record that has moved on carries whatever the last report
    /// happened to set.
    pub fn is_waiting_on_a_person(&self) -> bool {
        self.state == AgentState::Waiting && self.wait_needs_a_person
    }
}

impl AgentSource {
    fn spawned() -> AgentSource {
        AgentSource::Spawned
    }
}

/// Contains all the information for a currently opened tab.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct TabInfo {
    /// The Tab's 0 indexed position
    pub position: usize,
    /// The name of the tab as it appears in the UI (if there's enough room for it)
    pub name: String,
    /// Whether this tab is focused
    pub active: bool,
    /// The number of suppressed panes this tab has
    pub panes_to_hide: usize,
    /// Whether there's one pane taking up the whole display area on this tab
    pub is_fullscreen_active: bool,
    /// Whether input sent to this tab will be synced to all panes in it
    pub is_sync_panes_active: bool,
    pub are_floating_panes_visible: bool,
    pub other_focused_clients: Vec<ClientId>,
    pub active_swap_layout_name: Option<String>,
    /// Whether the user manually changed the layout, moving out of the swap layout scheme
    pub is_swap_layout_dirty: bool,
    /// Row count in the viewport (including all non-ui panes, eg. will exclude the status bar)
    pub viewport_rows: usize,
    /// Column count in the viewport (including all non-ui panes, eg. will exclude the status bar)
    pub viewport_columns: usize,
    /// Row count in the display area (including all panes, will typically be larger than the
    /// viewport)
    pub display_area_rows: usize,
    /// Column count in the display area (including all panes, will typically be larger than the
    /// viewport)
    pub display_area_columns: usize,
    /// The number of selectable (eg. not the UI bars) tiled panes currently in this tab
    pub selectable_tiled_panes_count: usize,
    /// The number of selectable (eg. not the UI bars) floating panes currently in this tab
    pub selectable_floating_panes_count: usize,
    /// The stable identifier for this tab
    pub tab_id: usize,
    /// Whether this tab has an active (persistent) bell notification
    pub has_bell_notification: bool,
    /// Whether this tab is currently flashing its bell (transient 400ms state)
    pub is_flashing_bell: bool,
}

/// The `PaneManifest` contains a dictionary of panes, indexed by the tab position (0 indexed).
/// Panes include all panes in the relevant tab, including `tiled` panes, `floating` panes and
/// `suppressed` panes.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PaneManifest {
    pub panes: HashMap<usize, Vec<PaneInfo>>, // usize is the tab position
}

/// Contains all the information for a currently open pane
///
/// # Difference between coordinates/size and content coordinates/size
///
/// The pane basic coordinates and size (eg. `pane_x` or `pane_columns`) are the entire space taken
/// up by this pane - including its frame and title if it has a border.
///
/// The pane content coordinates and size (eg. `pane_content_x` or `pane_content_columns`)
/// represent the area taken by the pane's content, excluding its frame and title if it has a
/// border.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct PaneInfo {
    /// The id of the pane, unique to all panes of this kind (eg. id in terminals or id in panes)
    pub id: u32,
    /// Whether this pane is a plugin (`true`) or a terminal (`false`), used along with `id` can represent a unique pane ID across
    /// the running session
    pub is_plugin: bool,
    /// Whether the pane is focused in its layer (tiled or floating)
    pub is_focused: bool,
    pub is_fullscreen: bool,
    /// Whether a pane is floating or tiled (embedded)
    pub is_floating: bool,
    /// Whether a pane is suppressed - suppressed panes are not visible to the user, but still run
    /// in the background
    pub is_suppressed: bool,
    /// The full title of the pane as it appears in the UI (if there is room for it)
    pub title: String,
    /// Whether a pane exited or not, note that most panes close themselves before setting this
    /// flag, so this is only relevant to command panes
    pub exited: bool,
    /// The exit status of a pane if it did exit and is still in the UI
    pub exit_status: Option<i32>,
    /// A "held" pane is a paused pane that is waiting for user input (eg. a command pane that
    /// exited and is waiting to be re-run or closed)
    pub is_held: bool,
    pub pane_x: usize,
    pub pane_content_x: usize,
    pub pane_y: usize,
    pub pane_content_y: usize,
    pub pane_rows: usize,
    pub pane_content_rows: usize,
    pub pane_columns: usize,
    pub pane_content_columns: usize,
    /// The coordinates of the cursor - if this pane is focused - relative to the pane's
    /// coordinates
    pub cursor_coordinates_in_pane: Option<(usize, usize)>, // x, y if cursor is visible
    /// If this is a command pane, this will show the stringified version of the command and its
    /// arguments
    pub terminal_command: Option<String>,
    /// The URL from which this plugin was loaded (eg. `zellij:strider` for the built-in `strider`
    /// plugin or `file:/path/to/my/plugin.wasm` for a local plugin)
    pub plugin_url: Option<String>,
    /// Unselectable panes are often used for UI elements that do not have direct user interaction
    /// (eg. the default `status-bar` or `tab-bar`).
    pub is_selectable: bool,
    /// Grouped panes (usually through an explicit user action) that are staged for a bulk action
    /// the index is kept track of in order to preserve the pane group order
    pub index_in_pane_group: BTreeMap<ClientId, usize>,
    /// The default foreground color of this pane, if set (e.g. "#00e000")
    pub default_fg: Option<String>,
    /// The default background color of this pane, if set (e.g. "#001a3a")
    pub default_bg: Option<String>,
    /// The folder this pane's process is in right now, for a terminal pane.
    ///
    /// Carried on every manifest rather than only announced by `CwdChanged`: an
    /// announcement is heard once, so a plugin that loaded after it — or missed
    /// it — had no second way to find out and stayed wrong about the pane for as
    /// long as it lived. `None` for a plugin pane, which is not a process in a
    /// folder.
    #[serde(default)]
    pub cwd: Option<String>,
    /// The machine this pane's process runs on, if it is not this one.
    ///
    /// `None` means local, which is every pane unless it was opened with
    /// `--remote-host`. It is here so a pane's *identity* is complete: two panes
    /// sitting in `/home/me/work` on different machines are in different folders,
    /// and anything keying on the path alone would quietly merge them.
    #[serde(default)]
    pub host: Option<String>,
    /// The machine this pane's process runs on, structurally: what `host`
    /// shows, plus a Firecracker pane's image and worktree.
    #[serde(default)]
    pub target: PaneTarget,
    /// The agent running in this pane, if the session knows of one. See [`AgentInfo`].
    #[serde(default)]
    pub agent: Option<AgentInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct PaneListEntry {
    #[serde(flatten)]
    pub pane_info: PaneInfo,
    pub tab_id: usize,
    pub tab_position: usize,
    pub tab_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane_cwd: Option<String>,
}

pub type ListPanesResponse = Vec<PaneListEntry>;
pub type ListTabsResponse = Vec<TabInfo>;

/// A toast notification received by the current live session.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct NotificationInfo {
    pub id: u64,
    pub message: String,
    pub received_at_unix_ms: u64,
}

/// A recorded pane, as listed by `zellij_utils::session_recording::list_recordings`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct RecordingSummary {
    pub id: i64,
    pub session_name: String,
    pub tab_name: String,
    pub terminal_id: u32,
    pub pane_title: String,
    pub started_at_us: i64,
    pub duration_us: i64,
    pub is_running: bool,
    pub capture_input: bool,
    pub error: Option<String>,
}

/// A match from `zellij_utils::session_recording::search_recordings`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SearchHit {
    /// Same id `list_recordings` and `export_recording` use (a `recording_panes` row).
    pub recording_id: i64,
    pub session_name: String,
    pub tab_name: String,
    pub pane_title: String,
    /// Offset into the recording, for seeking within the exported asciicast.
    pub elapsed_us: i64,
    pub started_at_us: i64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ClientInfo {
    pub client_id: ClientId,
    pub pane_id: PaneId,
    pub running_command: String,
    pub is_current_client: bool,
}

impl ClientInfo {
    pub fn new(
        client_id: ClientId,
        pane_id: PaneId,
        running_command: String,
        is_current_client: bool,
    ) -> Self {
        ClientInfo {
            client_id,
            pane_id,
            running_command,
            is_current_client,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PaneRenderReport {
    pub all_pane_contents: HashMap<ClientId, HashMap<PaneId, PaneContents>>,
    pub all_pane_contents_with_ansi: HashMap<ClientId, HashMap<PaneId, PaneContents>>,
}

impl PaneRenderReport {
    pub fn add_pane_contents(
        &mut self,
        client_ids: &[ClientId],
        pane_id: PaneId,
        pane_contents: PaneContents,
    ) {
        for client_id in client_ids {
            let p = self.all_pane_contents.entry(*client_id).or_default();
            p.insert(pane_id, pane_contents.clone());
        }
    }
    pub fn add_pane_contents_with_ansi(
        &mut self,
        client_ids: &[ClientId],
        pane_id: PaneId,
        pane_contents: PaneContents,
    ) {
        for client_id in client_ids {
            let p = self
                .all_pane_contents_with_ansi
                .entry(*client_id)
                .or_default();
            p.insert(pane_id, pane_contents.clone());
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PaneContents {
    // NOTE: both lines_above_viewport and lines_below_viewport are only populated if explicitly
    // requested (eg. with get_full_scrollback true in the plugin command) this is for performance
    // reasons
    pub lines_above_viewport: Vec<String>,
    pub lines_below_viewport: Vec<String>,
    pub viewport: Vec<String>,
    pub selected_text: Option<SelectedText>,
    pub cursor: Option<(usize, usize)>,
    /// Milliseconds since this pane's process last wrote to its pty, if it ever did.
    ///
    /// The only signal that separates "the program produced something" from "the
    /// picture moved": scrolling, reflowing and re-rendering all change what a pane
    /// shows without a single byte arriving from the process.
    #[serde(default)]
    pub last_output_ms_ago: Option<u64>,
    /// Whether the user has scrolled this pane up out of its live tail.
    ///
    /// A scrolled pane shows old output, so its viewport says nothing about what the
    /// process is doing right now.
    #[serde(default)]
    pub is_scrolled: bool,
}

/// Extract text from a line between two column positions, accounting for wide characters
fn extract_text_by_columns(line: &str, start_col: usize, end_col: usize) -> String {
    let mut current_col = 0;
    let mut result = String::new();
    let mut capturing = false;

    for ch in line.chars() {
        let char_width = ch.width().unwrap_or(0);

        // Start capturing when we reach start_col
        if current_col >= start_col && !capturing {
            capturing = true;
        }

        // Stop if we've reached or passed end_col
        if current_col >= end_col {
            break;
        }

        // Capture character if we're in the range
        if capturing {
            result.push(ch);
        }

        current_col += char_width;
    }

    result
}

/// Extract text from a line starting at a column position, accounting for wide characters
fn extract_text_from_column(line: &str, start_col: usize) -> String {
    let mut current_col = 0;
    let mut result = String::new();
    let mut capturing = false;

    for ch in line.chars() {
        let char_width = ch.width().unwrap_or(0);

        if current_col >= start_col {
            capturing = true;
        }

        if capturing {
            result.push(ch);
        }

        current_col += char_width;
    }

    result
}

/// Extract text from a line up to a column position, accounting for wide characters
fn extract_text_to_column(line: &str, end_col: usize) -> String {
    let mut current_col = 0;
    let mut result = String::new();

    for ch in line.chars() {
        let char_width = ch.width().unwrap_or(0);

        if current_col >= end_col {
            break;
        }

        result.push(ch);
        current_col += char_width;
    }

    result
}

impl PaneContents {
    pub fn new(viewport: Vec<String>, selection_start: Position, selection_end: Position) -> Self {
        PaneContents {
            viewport,
            selected_text: SelectedText::from_positions(selection_start, selection_end),
            ..Default::default()
        }
    }
    pub fn new_with_scrollback(
        viewport: Vec<String>,
        selection_start: Position,
        selection_end: Position,
        lines_above_viewport: Vec<String>,
        lines_below_viewport: Vec<String>,
    ) -> Self {
        PaneContents {
            viewport,
            selected_text: SelectedText::from_positions(selection_start, selection_end),
            lines_above_viewport,
            lines_below_viewport,
            ..Default::default()
        }
    }

    /// Returns the actual text content of the selection, if any exists.
    /// Selection only occurs within the viewport.
    pub fn get_selected_text(&self) -> Option<String> {
        let selected_text = self.selected_text?;

        let start_line = selected_text.start.line() as usize;
        let start_col = selected_text.start.column();
        let end_line = selected_text.end.line() as usize;
        let end_col = selected_text.end.column();

        // Handle out of bounds
        if start_line >= self.viewport.len() || end_line >= self.viewport.len() {
            return None;
        }

        if start_line == end_line {
            // Single line selection
            let line = &self.viewport[start_line];
            Some(extract_text_by_columns(line, start_col, end_col))
        } else {
            // Multi-line selection
            let mut result = String::new();

            // First line - from start column to end of line
            let first_line = &self.viewport[start_line];
            result.push_str(&extract_text_from_column(first_line, start_col));
            result.push('\n');

            // Middle lines - complete lines
            for i in (start_line + 1)..end_line {
                result.push_str(&self.viewport[i]);
                result.push('\n');
            }

            // Last line - from start to end column
            let last_line = &self.viewport[end_line];
            result.push_str(&extract_text_to_column(last_line, end_col));

            Some(result)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PaneScrollbackResponse {
    Ok(PaneContents),
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GetPanePidResponse {
    Ok(i32),
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GetPaneRunningCommandResponse {
    Ok(Vec<String>),
    Err(String),
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionListSnapshot {
    pub live_sessions: Vec<SessionInfo>,
    pub resurrectable_sessions: Vec<(String, Duration)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GetSessionListResponse {
    Ok(SessionListSnapshot),
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum KillSessionsResponse {
    Ok,
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeleteDeadSessionResponse {
    Ok,
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeleteAllDeadSessionsResponse {
    Ok,
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GetPaneCwdResponse {
    Ok(PathBuf),
    Err(String),
}

/// What Git said about a remote pane's working directory, unparsed.
///
/// Raw on purpose. `hocus-agentd`'s `poll::repository` already turns exactly
/// these three things into a checkout identity, including the awkward parts — a
/// repository with no commits writes its identity to stdout and *then* exits
/// 128, so the exit code cannot gate the parse. Shipping the bytes means that
/// logic stays in one place and keeps working for remote panes without knowing
/// they exist.
///
/// A projection of the server's `RemoteGitProbe`, which lives in `zellij-server`
/// and could never cross the wasm boundary.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PaneGitProbe {
    /// The machine this was run on, so two checkouts at the same path on
    /// different hosts stay distinct.
    pub host: String,
    /// The folder the probe was run in, which is a path on `host`.
    ///
    /// Not necessarily the pane's folder *now*: the far machine probes on its
    /// own clock, so a pane that has just moved reports the folder it was in.
    /// Whoever files the answer keys it by this, not by where the pane is.
    pub cwd: String,
    pub exit_code: Option<i32>,
    pub metadata: Vec<u8>,
    pub config: Vec<u8>,
}

/// The answer to `get_pane_git_probe`.
///
/// A local pane is an `Err`: nothing on this machine has a cached probe to give,
/// and a plugin wanting one runs Git itself. That is not a failure worth
/// special-casing — callers branch on `PaneInfo::host` before asking — but it is
/// worth saying rather than returning an empty answer that reads as "not a
/// repository".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GetPaneGitProbeResponse {
    Ok(PaneGitProbe),
    Err(String),
}

/// The outcome of moving a pane between sessions, in either direction.
///
/// Both variants carry prose meant to be shown to whoever asked. The failures in
/// particular are not codes to be matched on: when a warp fails after the pane's
/// supervisor has been released, the message is the only place the recovery
/// instructions exist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WarpPaneResponse {
    Ok(String),
    Err(String),
}

/// A pane whose session died but whose process is still running.
///
/// A projection of the server's own `HostStatus` rather than that type itself,
/// which lives in `zellij-server` and could never cross the wasm boundary — and
/// which carries a host pid and a socket path a plugin has no use for.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct OrphanedPane {
    /// The id `adopt_orphaned_pane` takes, and that `zellij pty-host adopt` on
    /// the command line takes too.
    pub host_id: String,
    /// The session it belonged to, if the supervisor was ever told one.
    pub session_name: Option<String>,
    pub command: String,
    pub cwd: Option<PathBuf>,
    pub child_pid: Option<u32>,
    pub started_at_unix: u64,
    /// `None` while the child is still running; `Some(None)` once it has been
    /// killed by a signal; `Some(Some(code))` once it has exited.
    pub exit_status: Option<Option<i32>>,
    /// Output the supervisor caught while nobody was reading, which the adopting
    /// session will replay.
    pub buffered_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ListOrphanedPanesResponse {
    Ok(Vec<OrphanedPane>),
    Err(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum GetFocusedPaneInfoResponse {
    Ok { tab_index: usize, pane_id: PaneId },
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaveLayoutResponse {
    Ok(()),
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeleteLayoutResponse {
    Ok(()),
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenameLayoutResponse {
    Ok(()),
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EditLayoutResponse {
    Ok(()),
    Err(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedText {
    pub start: Position,
    pub end: Position,
}

impl SelectedText {
    pub fn new(start: Position, end: Position) -> Self {
        // Normalize: ensure start <= end
        let (normalized_start, normalized_end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };

        // Normalize negative line values to 0
        // (column is already usize so can't be negative)
        let normalized_start = Position::new(
            normalized_start.line().max(0) as i32,
            normalized_start.column() as u16,
        );
        let normalized_end = Position::new(
            normalized_end.line().max(0) as i32,
            normalized_end.column() as u16,
        );

        SelectedText {
            start: normalized_start,
            end: normalized_end,
        }
    }

    pub fn from_positions(start: Position, end: Position) -> Option<Self> {
        if start == end {
            None
        } else {
            Some(Self::new(start, end))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct PluginIds {
    pub plugin_id: u32,
    pub zellij_pid: u32,
    pub initial_cwd: PathBuf,
    pub client_id: ClientId,
    /// Short hostname of the machine running this session's server, empty if unavailable
    pub hostname: String,
}

/// Tag used to identify the plugin in layout and config kdl files
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Deserialize, Serialize, PartialOrd, Ord)]
pub struct PluginTag(String);

impl PluginTag {
    pub fn new(url: impl Into<String>) -> Self {
        PluginTag(url.into())
    }
}

impl From<PluginTag> for String {
    fn from(tag: PluginTag) -> Self {
        tag.0
    }
}

impl fmt::Display for PluginTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct PluginCapabilities {
    pub arrow_fonts: bool,
    /// Whether the host terminal can display kitty graphics, so a plugin knows whether drawing an
    /// image is worth doing or whether it should fall back to text. Unlike `arrow_fonts` this is
    /// not known at startup: it is learned from the client's probe reply and pushed out in a later
    /// `ModeUpdate`, so it starts `false` and turns on.
    pub kitty_graphics: bool,
}

impl Default for PluginCapabilities {
    fn default() -> PluginCapabilities {
        PluginCapabilities {
            arrow_fonts: true,
            kitty_graphics: false,
        }
    }
}

/// Represents a Clipboard type
#[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
pub enum CopyDestination {
    Command,
    Primary,
    System,
}

#[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
pub enum PermissionStatus {
    Granted,
    Denied,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileToOpen {
    pub path: PathBuf,
    pub line_number: Option<usize>,
    pub cwd: Option<PathBuf>,
}

impl FileToOpen {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        FileToOpen {
            path: path.as_ref().to_path_buf(),
            ..Default::default()
        }
    }
    pub fn with_line_number(mut self, line_number: usize) -> Self {
        self.line_number = Some(line_number);
        self
    }
    pub fn with_cwd(mut self, cwd: PathBuf) -> Self {
        self.cwd = Some(cwd);
        self
    }
}

/// Where a pane's process runs.
///
/// Carried alongside the command rather than as a new `Run` variant: it is an
/// attribute of *where* the command runs rather than of what it is. `Local` is
/// the default, so every existing layout and serialized session parses
/// unchanged.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PaneTarget {
    /// This machine.
    #[default]
    Local,
    /// A machine reached over ssh; `host` is the `--remote-host` argument
    /// (an ssh alias, `user@host`, or an `ssh://` URL).
    Ssh { host: String },
    /// A named Firecracker VM owned by this session.
    Firecracker {
        /// The VM's name, unique within the session.
        vm: String,
        /// Immutable image selected for the VM, or the provider's default.
        image: Option<String>,
        /// Canonical host worktree mirrored into the VM, or the pane's cwd.
        worktree: Option<PathBuf>,
    },
}

/// The prefix `PaneTarget::display` puts before a Firecracker VM's name.
///
/// `validate_firecracker_vm_name` never admits a `:`, and `validate_ssh_host`
/// rejects hosts that start with it, so the encoding is unambiguous.
pub const FIRECRACKER_HOST_PREFIX: &str = "firecracker:";

impl PaneTarget {
    /// An ssh target. The host is trimmed; a blank host is refused, because it
    /// would silently become a local pane somewhere down the line.
    pub fn ssh(host: &str) -> Result<Self, String> {
        let host = host.trim();
        if host.is_empty() {
            return Err("remote_host must not be empty".to_owned());
        }
        Ok(PaneTarget::Ssh {
            host: host.to_owned(),
        })
    }

    /// A Firecracker target, carrying every rule the layout, keybind, CLI and
    /// plugin spellings share: the VM and image names use the charset
    /// `validate_firecracker_vm_name` admits, blank image and worktree values
    /// mean "not given", and a worktree is an absolute path.
    pub fn firecracker(
        vm: &str,
        image: Option<&str>,
        worktree: Option<&Path>,
    ) -> Result<Self, String> {
        let vm = vm.trim();
        validate_firecracker_vm_name(vm)?;
        let image = match image.map(str::trim).filter(|image| !image.is_empty()) {
            Some(image) => {
                validate_firecracker_vm_name(image)
                    .map_err(|error| error.replacen("VM name", "image name", 1))?;
                Some(image.to_owned())
            },
            None => None,
        };
        let worktree = match worktree.filter(|path| !path.as_os_str().is_empty()) {
            Some(path) if path.is_absolute() => Some(path.to_path_buf()),
            Some(path) => {
                return Err(format!(
                    "firecracker_worktree must be an absolute path, got {}",
                    path.display()
                ))
            },
            None => None,
        };
        Ok(PaneTarget::Firecracker {
            vm: vm.to_owned(),
            image,
            worktree,
        })
    }

    /// Build a target from the loose optional spellings every parser starts
    /// from: `None`/`None` is local, an ssh host and a VM name together are
    /// refused, and an image or worktree without a VM is refused too.
    pub fn from_parts(
        remote_host: Option<&str>,
        firecracker_vm: Option<&str>,
        firecracker_image: Option<&str>,
        firecracker_worktree: Option<&Path>,
    ) -> Result<Self, String> {
        let remote_host = remote_host.map(str::trim).filter(|host| !host.is_empty());
        let firecracker_vm = firecracker_vm.map(str::trim).filter(|vm| !vm.is_empty());
        let has_image = firecracker_image
            .map(str::trim)
            .is_some_and(|image| !image.is_empty());
        let has_worktree = firecracker_worktree.is_some_and(|path| !path.as_os_str().is_empty());
        match (remote_host, firecracker_vm) {
            (Some(_), Some(_)) => {
                Err("a pane cannot use both firecracker_vm and remote_host".to_owned())
            },
            (None, Some(vm)) => {
                PaneTarget::firecracker(vm, firecracker_image, firecracker_worktree)
            },
            (_, None) if has_image || has_worktree => Err(match (has_image, has_worktree) {
                (true, true) => {
                    "firecracker_image and firecracker_worktree need a firecracker_vm".to_owned()
                },
                (true, false) => "firecracker_image needs a firecracker_vm".to_owned(),
                _ => "firecracker_worktree needs a firecracker_vm".to_owned(),
            }),
            (Some(host), None) => PaneTarget::ssh(host),
            (None, None) => Ok(PaneTarget::Local),
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, PaneTarget::Local)
    }
    /// Runs on another machine, over ssh or in a VM.
    pub fn is_remote(&self) -> bool {
        !self.is_local()
    }
    pub fn is_firecracker(&self) -> bool {
        matches!(self, PaneTarget::Firecracker { .. })
    }
    /// The ssh host of an `Ssh` target.
    pub fn ssh_host(&self) -> Option<&str> {
        match self {
            PaneTarget::Ssh { host } => Some(host),
            _ => None,
        }
    }
    /// The VM name of a `Firecracker` target.
    pub fn firecracker_vm(&self) -> Option<&str> {
        match self {
            PaneTarget::Firecracker { vm, .. } => Some(vm),
            _ => None,
        }
    }
    pub fn firecracker_image(&self) -> Option<&str> {
        match self {
            PaneTarget::Firecracker { image, .. } => image.as_deref(),
            _ => None,
        }
    }
    pub fn firecracker_worktree(&self) -> Option<&Path> {
        match self {
            PaneTarget::Firecracker { worktree, .. } => worktree.as_deref(),
            _ => None,
        }
    }
    /// What to show a user for this target: `None` for a local pane, the ssh
    /// host, or `firecracker:<vm>`. The VM's ssh endpoint is a provider detail
    /// and never appears here.
    pub fn display(&self) -> Option<String> {
        match self {
            PaneTarget::Local => None,
            PaneTarget::Ssh { host } => Some(host.clone()),
            PaneTarget::Firecracker { vm, .. } => Some(format!("{FIRECRACKER_HOST_PREFIX}{vm}")),
        }
    }

    /// The inverse of `display`, for readers that only have the string.
    pub fn parse_display(host: Option<&str>) -> Self {
        match host {
            None => PaneTarget::Local,
            Some(host) => match host.strip_prefix(FIRECRACKER_HOST_PREFIX) {
                Some(vm) => PaneTarget::Firecracker {
                    vm: vm.to_owned(),
                    image: None,
                    worktree: None,
                },
                None => PaneTarget::Ssh {
                    host: host.to_owned(),
                },
            },
        }
    }

    /// Fill what this target leaves unsaid from `base`, the way a layout's
    /// pane inherits from its template: a local target takes the base
    /// wholesale, and a Firecracker target without an image or a worktree takes
    /// each from a Firecracker base individually.
    pub fn merge_from(&mut self, base: &PaneTarget) {
        match (self, base) {
            (this @ PaneTarget::Local, base) => *this = base.clone(),
            (
                PaneTarget::Firecracker {
                    image, worktree, ..
                },
                PaneTarget::Firecracker {
                    image: base_image,
                    worktree: base_worktree,
                    ..
                },
            ) => {
                if image.is_none() {
                    image.clone_from(base_image);
                }
                if worktree.is_none() {
                    worktree.clone_from(base_worktree);
                }
            },
            _ => {},
        }
    }
}

/// The charset and length a Firecracker VM or image name must satisfy.
pub fn validate_firecracker_vm_name(name: &str) -> Result<(), String> {
    if (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        Ok(())
    } else {
        Err(format!(
            "invalid Firecracker VM name {name:?}: use 1-64 letters, digits, '.', '_' or '-'"
        ))
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct CommandToRun {
    pub path: PathBuf,
    pub args: Vec<String>,
    /// Where to start the command. Locally, `/host`, `/data`, `/cache` and `/tmp`
    /// name the plugin mounts and a relative path is joined onto the plugin's
    /// cwd. With a remote `target`, the path is interpreted on that machine
    /// exactly as written, and `None` means the login directory there.
    pub cwd: Option<PathBuf>,
    /// The machine the command runs on.
    pub target: PaneTarget,
    /// Ignore `path` and run the login shell of the machine named by
    /// `target`, resolved there. See `RunCommand::remote_default_shell`:
    /// which shell a user has is a fact about that machine, so a plugin has
    /// nothing useful to put in `path` and says so with this instead.
    pub remote_default_shell: bool,
}

/// `CommandToRun` as it arrives from serde callers (Lua plugins, JSON): either
/// a `target`, or the flat `remote_host`/`firecracker_*` keys the API accepted
/// before `PaneTarget`, which still describe one.
#[derive(Default, Deserialize)]
#[serde(default)]
struct CommandToRunWire {
    path: PathBuf,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    target: Option<PaneTarget>,
    remote_host: Option<String>,
    firecracker_vm: Option<String>,
    firecracker_image: Option<String>,
    firecracker_worktree: Option<PathBuf>,
    remote_default_shell: bool,
}

impl<'de> Deserialize<'de> for CommandToRun {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = CommandToRunWire::deserialize(deserializer)?;
        let target = match wire.target {
            Some(target) => target,
            None => PaneTarget::from_parts(
                wire.remote_host.as_deref(),
                wire.firecracker_vm.as_deref(),
                wire.firecracker_image.as_deref(),
                wire.firecracker_worktree.as_deref(),
            )
            .map_err(serde::de::Error::custom)?,
        };
        Ok(CommandToRun {
            path: wire.path,
            args: wire.args,
            cwd: wire.cwd,
            target,
            remote_default_shell: wire.remote_default_shell,
        })
    }
}

/// Git setup Zellij performs before opening a command pane.
///
/// `New` in both branch variants means *the one named*, created when it is not
/// there already: a branch or a worktree that exists is reused rather than
/// refused, and `base` then has nothing to say - an existing branch has its own
/// history. See `prepare_git_checkout_in` in the server for the exact order the
/// two kinds of reuse are tried in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitCheckout {
    Current,
    /// The branch, in the checkout the request came from. Reuse means switching
    /// to it, or - when another worktree holds it, which git allows only one of
    /// - running in that worktree instead.
    NewBranch {
        branch: String,
        base: String,
    },
    /// The branch, in a worktree of its own at `path`. A worktree already at
    /// `path` is used as it stands; failing that, the worktree already holding
    /// `branch` is.
    NewWorktree {
        branch: String,
        base: String,
        path: PathBuf,
    },
}

impl CommandToRun {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        CommandToRun {
            path: path.as_ref().to_path_buf(),
            ..Default::default()
        }
    }
    pub fn new_with_args<P: AsRef<Path>, A: AsRef<str>>(path: P, args: Vec<A>) -> Self {
        CommandToRun {
            path: path.as_ref().to_path_buf(),
            args: args.into_iter().map(|a| a.as_ref().to_owned()).collect(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MessageToPlugin {
    pub plugin_url: Option<String>,
    pub destination_plugin_id: Option<u32>,
    pub plugin_config: BTreeMap<String, String>,
    pub message_name: String,
    pub message_payload: Option<String>,
    pub message_args: BTreeMap<String, String>,
    /// these will only be used in case we need to launch a new plugin to send this message to,
    /// since none are running
    pub new_plugin_args: Option<NewPluginArgs>,
    pub floating_pane_coordinates: Option<FloatingPaneCoordinates>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NewPluginArgs {
    pub should_float: Option<bool>,
    pub pane_id_to_replace: Option<PaneId>,
    pub pane_title: Option<String>,
    pub cwd: Option<PathBuf>,
    pub skip_cache: bool,
    pub should_focus: Option<bool>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PaneId {
    Terminal(u32),
    Plugin(u32),
}

impl Default for PaneId {
    fn default() -> Self {
        PaneId::Terminal(0)
    }
}

impl FromStr for PaneId {
    type Err = Box<dyn std::error::Error>;
    fn from_str(stringified_pane_id: &str) -> Result<Self, Self::Err> {
        if let Some(terminal_stringified_pane_id) = stringified_pane_id.strip_prefix("terminal_") {
            terminal_stringified_pane_id
                .parse::<u32>()
                .map(PaneId::Terminal)
                .map_err(|e| e.into())
        } else if let Some(plugin_pane_id) = stringified_pane_id.strip_prefix("plugin_") {
            plugin_pane_id
                .parse::<u32>()
                .map(PaneId::Plugin)
                .map_err(|e| e.into())
        } else {
            stringified_pane_id
                .parse::<u32>()
                .map(PaneId::Terminal)
                .map_err(|e| e.into())
        }
    }
}

impl std::fmt::Display for PaneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PaneId::Terminal(id) => write!(f, "terminal_{}", id),
            PaneId::Plugin(id) => write!(f, "plugin_{}", id),
        }
    }
}

impl MessageToPlugin {
    pub fn new(message_name: impl Into<String>) -> Self {
        MessageToPlugin {
            message_name: message_name.into(),
            ..Default::default()
        }
    }
    pub fn with_plugin_url(mut self, url: impl Into<String>) -> Self {
        self.plugin_url = Some(url.into());
        self
    }
    pub fn with_destination_plugin_id(mut self, destination_plugin_id: u32) -> Self {
        self.destination_plugin_id = Some(destination_plugin_id);
        self
    }
    pub fn with_plugin_config(mut self, plugin_config: BTreeMap<String, String>) -> Self {
        self.plugin_config = plugin_config;
        self
    }
    pub fn with_payload(mut self, payload: impl Into<String>) -> Self {
        self.message_payload = Some(payload.into());
        self
    }
    pub fn with_args(mut self, args: BTreeMap<String, String>) -> Self {
        self.message_args = args;
        self
    }
    pub fn with_floating_pane_coordinates(
        mut self,
        floating_pane_coordinates: FloatingPaneCoordinates,
    ) -> Self {
        self.floating_pane_coordinates = Some(floating_pane_coordinates);
        self
    }
    pub fn new_plugin_instance_should_float(mut self, should_float: bool) -> Self {
        let new_plugin_args = self.new_plugin_args.get_or_insert_with(Default::default);
        new_plugin_args.should_float = Some(should_float);
        self
    }
    pub fn new_plugin_instance_should_replace_pane(mut self, pane_id: PaneId) -> Self {
        let new_plugin_args = self.new_plugin_args.get_or_insert_with(Default::default);
        new_plugin_args.pane_id_to_replace = Some(pane_id);
        self
    }
    pub fn new_plugin_instance_should_have_pane_title(
        mut self,
        pane_title: impl Into<String>,
    ) -> Self {
        let new_plugin_args = self.new_plugin_args.get_or_insert_with(Default::default);
        new_plugin_args.pane_title = Some(pane_title.into());
        self
    }
    pub fn new_plugin_instance_should_have_cwd(mut self, cwd: PathBuf) -> Self {
        let new_plugin_args = self.new_plugin_args.get_or_insert_with(Default::default);
        new_plugin_args.cwd = Some(cwd);
        self
    }
    pub fn new_plugin_instance_should_skip_cache(mut self) -> Self {
        let new_plugin_args = self.new_plugin_args.get_or_insert_with(Default::default);
        new_plugin_args.skip_cache = true;
        self
    }
    pub fn new_plugin_instance_should_be_focused(mut self) -> Self {
        let new_plugin_args = self.new_plugin_args.get_or_insert_with(Default::default);
        new_plugin_args.should_focus = Some(true);
        self
    }
    /// The other half of `new_plugin_instance_should_be_focused`, for a message that opens a
    /// plugin the sender wants running rather than looked at. Distinct from leaving
    /// `should_focus` unset, which lets the launch decide.
    pub fn new_plugin_instance_should_not_be_focused(mut self) -> Self {
        let new_plugin_args = self.new_plugin_args.get_or_insert_with(Default::default);
        new_plugin_args.should_focus = Some(false);
        self
    }
    pub fn has_cwd(&self) -> bool {
        self.new_plugin_args
            .as_ref()
            .map(|n| n.cwd.is_some())
            .unwrap_or(false)
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectToSession {
    pub name: Option<String>,
    pub tab_position: Option<usize>,
    pub pane_id: Option<(u32, bool)>, // (id, is_plugin)
    pub layout: Option<LayoutInfo>,
    pub cwd: Option<PathBuf>,
}

impl ConnectToSession {
    pub fn apply_layout_dir(&mut self, layout_dir: &Path) {
        if let Some(LayoutInfo::File(file_path, _layout_metadata)) = self.layout.as_mut() {
            *file_path = Path::join(layout_dir, &file_path)
                .to_string_lossy()
                .to_string();
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginMessage {
    pub name: String,
    pub payload: String,
    pub worker_name: Option<String>,
}

impl PluginMessage {
    pub fn new_to_worker(worker_name: &str, message: &str, payload: &str) -> Self {
        PluginMessage {
            name: message.to_owned(),
            payload: payload.to_owned(),
            worker_name: Some(worker_name.to_owned()),
        }
    }
    pub fn new_to_plugin(message: &str, payload: &str) -> Self {
        PluginMessage {
            name: message.to_owned(),
            payload: payload.to_owned(),
            worker_name: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HttpVerb {
    Get,
    Post,
    Put,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PipeSource {
    Cli(String), // String is the pipe_id of the CLI pipe (used for blocking/unblocking)
    Plugin(u32), // u32 is the lugin id
    Keybind,     // TODO: consider including the actual keybind here?
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PipeMessage {
    pub source: PipeSource,
    pub name: String,
    pub payload: Option<String>,
    pub args: BTreeMap<String, String>,
    pub is_private: bool,
    /// The pane `zellij pipe` was run in, when the source is a CLI pipe and the caller was
    /// inside one: the client reads `ZELLIJ_PANE_ID` and the server carries it here, so no
    /// integration has to hand-thread a `pane_id=` argument. Convenience, not authentication —
    /// the variable is as trustworthy as the process that set it.
    #[serde(default)]
    pub pane_id: Option<PaneId>,
}

impl PipeMessage {
    pub fn new(
        source: PipeSource,
        name: impl Into<String>,
        payload: &Option<String>,
        args: &Option<BTreeMap<String, String>>,
        is_private: bool,
    ) -> Self {
        PipeMessage {
            source,
            name: name.into(),
            payload: payload.clone(),
            args: args.clone().unwrap_or_default(),
            is_private,
            pane_id: None,
        }
    }
}

/// Where a warped or adopted pane should land in the session receiving it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, Default)]
pub enum WarpPlacement {
    /// A tab of its own. The default, because it is the only placement that
    /// cannot disturb a layout the receiving session already has.
    #[default]
    NewTab,
    /// A tiled split in whatever tab the receiving session has focused.
    FocusedTab,
    Floating,
}

/// Everything that has to travel with a pane moving between sessions, beyond what
/// the pane's own supervisor can already report.
///
/// Deliberately small. The running process, its command and its exit status are
/// all held by the pty-host, and the adopting server learns them from the
/// `AttachAck` — so this carries what lives in the *terminal emulator* plus the
/// server-side agent record attached to that process.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Default)]
pub struct WarpedPane {
    /// Identifies the supervisor, for error messages and `pty-host adopt`.
    /// Empty for a remote pane, whose supervisor is not on this machine.
    pub host_id: String,
    /// Where to attach. A supervisor that failed to bind a socket has no address
    /// to hand out, which is the one way a pane can be detachable but not
    /// transferable — the source refuses the warp rather than sending this empty.
    /// Empty for a remote pane; `remote` is its address instead.
    pub control_socket: PathBuf,
    /// The pane's title, so it does not arrive renamed.
    pub title: Option<String>,
    /// The screen and scrollback as an ANSI dump, replayed into the new pane the
    /// same way session resurrection restores one.
    pub contents: Option<String>,
    pub borderless: bool,
    /// `Some` when the pane's process is on another machine, and then the only
    /// address that means anything: the two fields above name a local
    /// supervisor socket, and there is none.
    pub remote: Option<RemotePaneHandoff>,
    /// The agent record belongs to the running process, so it moves with it.
    #[serde(default)]
    pub agent: Option<AgentInfo>,
}

/// How to pick a remote pane back up from the broker that has been holding it.
///
/// The remote counterpart of `WarpedPane`'s `control_socket`, and the reason it
/// cannot simply *be* one: a broker is reached by ssh, not by a path, and the
/// adopting server builds its own forward rather than inheriting the releasing
/// server's.
///
/// Carries the command, which the local path does not have to: the adopting
/// server learns a local pane's command from its supervisor's `AttachAck`, but
/// the broker's answer to a resume carries none. Without it the pane would
/// arrive unable to say what it runs, unable to re-run on the right machine when
/// held, and unable to title itself.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Default)]
pub struct RemotePaneHandoff {
    /// The ssh destination the pane was opened against.
    pub host: String,
    /// How the broker on that host names this pane. Meaningful only there.
    pub pane_token: String,
    /// Output bytes the releasing session took delivery of. The broker replays
    /// from exactly here, which is what stops the pane's serialized screen and
    /// the replay from overlapping or leaving a gap between them.
    pub received: u64,
    pub cmd: RunCommand,
}

/// Everything that has to travel with a whole tab moving between sessions.
///
/// The arrangement travels as a layout rather than as pane geometry, which is
/// what lets a tab land in a session of a different size: a layout is
/// proportional, and the receiving session positions it against its own
/// viewport. This is the same representation session resurrection uses to
/// survive a resize.
///
/// The panes are split into two lists for the same reason `apply_layout` takes
/// two: tiled and floating slots are filled from separate queues, in separate
/// orders.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Default)]
pub struct WarpedTab {
    /// The tab's name, so it does not arrive renamed.
    pub name: Option<String>,
    pub tiled_layout: TiledPaneLayout,
    pub floating_layout: Vec<FloatingPaneLayout>,
    /// One per released supervisor, ordered to match the layout's terminal slots
    /// — that is, `tiled_layout.extract_run_instructions()` order, which is what
    /// the receiving side's `position_new_panes` consumes ids in. Plugin panes
    /// are not here: they have no process, and are recreated from their
    /// `Run::Plugin` node instead.
    ///
    /// Only the parts of a `WarpedPane` the layout cannot express are filled in:
    /// which supervisor to attach to, and the pane's screen. Titles and borders
    /// travel as layout nodes, because that is where the receiving session looks
    /// for them when it builds the tab.
    pub tiled_panes: Vec<WarpedPane>,
    /// Ordered to match `floating_layout`, minus its plugin panes.
    pub floating_panes: Vec<WarpedPane>,
    pub hide_floating_panes: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, Default)]
pub struct FloatingPaneCoordinates {
    pub x: Option<PercentOrFixed>,
    pub y: Option<PercentOrFixed>,
    pub width: Option<PercentOrFixed>,
    pub height: Option<PercentOrFixed>,
    pub pinned: Option<bool>,
    pub borderless: Option<bool>,
}

impl FloatingPaneCoordinates {
    pub fn new(
        x: Option<String>,
        y: Option<String>,
        width: Option<String>,
        height: Option<String>,
        pinned: Option<bool>,
        borderless: Option<bool>,
    ) -> Option<Self> {
        // Parse x/y coordinates - allows 0% or 0
        let x = x.and_then(|x| PercentOrFixed::from_str(&x).ok());
        let y = y.and_then(|y| PercentOrFixed::from_str(&y).ok());

        // Parse width/height - reject 0% or 0
        let width = width.and_then(|w| {
            PercentOrFixed::from_str(&w)
                .ok()
                .and_then(|size| match size {
                    PercentOrFixed::Percent(0) => None,
                    PercentOrFixed::Fixed(0) => None,
                    _ => Some(size),
                })
        });
        let height = height.and_then(|h| {
            PercentOrFixed::from_str(&h)
                .ok()
                .and_then(|size| match size {
                    PercentOrFixed::Percent(0) => None,
                    PercentOrFixed::Fixed(0) => None,
                    _ => Some(size),
                })
        });

        if x.is_none()
            && y.is_none()
            && width.is_none()
            && height.is_none()
            && pinned.is_none()
            && borderless.is_none()
        {
            None
        } else {
            Some(FloatingPaneCoordinates {
                x,
                y,
                width,
                height,
                pinned,
                borderless,
            })
        }
    }
    pub fn with_x_fixed(mut self, x: usize) -> Self {
        self.x = Some(PercentOrFixed::Fixed(x));
        self
    }
    pub fn with_x_percent(mut self, x: usize) -> Self {
        if x > 100 {
            eprintln!("x must be between 0 and 100");
            return self;
        }
        self.x = Some(PercentOrFixed::Percent(x));
        self
    }
    pub fn with_y_fixed(mut self, y: usize) -> Self {
        self.y = Some(PercentOrFixed::Fixed(y));
        self
    }
    pub fn with_y_percent(mut self, y: usize) -> Self {
        if y > 100 {
            eprintln!("y must be between 0 and 100");
            return self;
        }
        self.y = Some(PercentOrFixed::Percent(y));
        self
    }
    pub fn with_width_fixed(mut self, width: usize) -> Self {
        self.width = Some(PercentOrFixed::Fixed(width));
        self
    }
    pub fn with_width_percent(mut self, width: usize) -> Self {
        if width > 100 {
            eprintln!("width must be between 0 and 100");
            return self;
        }
        self.width = Some(PercentOrFixed::Percent(width));
        self
    }
    pub fn with_height_fixed(mut self, height: usize) -> Self {
        self.height = Some(PercentOrFixed::Fixed(height));
        self
    }
    pub fn with_height_percent(mut self, height: usize) -> Self {
        if height > 100 {
            eprintln!("height must be between 0 and 100");
            return self;
        }
        self.height = Some(PercentOrFixed::Percent(height));
        self
    }
}

impl From<PaneGeom> for FloatingPaneCoordinates {
    fn from(pane_geom: PaneGeom) -> Self {
        FloatingPaneCoordinates {
            x: Some(PercentOrFixed::Fixed(pane_geom.x)),
            y: Some(PercentOrFixed::Fixed(pane_geom.y)),
            width: Some(PercentOrFixed::Fixed(pane_geom.cols.as_usize())),
            height: Some(PercentOrFixed::Fixed(pane_geom.rows.as_usize())),
            pinned: Some(pane_geom.is_pinned),
            borderless: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OriginatingPlugin {
    pub plugin_id: u32,
    pub client_id: ClientId,
    pub context: Context,
}

/// Marks command panes opened by the Hocus new/duplicate agent actions.
pub const HOCUS_AGENT_PANE_CONTEXT: &str = "hocus_agent_pane";

impl OriginatingPlugin {
    pub fn new(plugin_id: u32, client_id: ClientId, context: Context) -> Self {
        OriginatingPlugin {
            plugin_id,
            client_id,
            context,
        }
    }
}

/// What a pane drawn over the whole session surface does when the user's attention moves
/// elsewhere.
///
/// The distinction used to be a string in the plugin's own configuration map, read back out of
/// `Pane::invoked_with()` at every decision point - which meant only a plugin pane could ever be
/// modal, and that a caller which rebuilt a pane's coordinates silently dropped it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OverlayKind {
    /// Watches `PaneUpdate` and closes itself once it sees it is no longer focused, so focus may
    /// be taken from it freely - `file-finder`.
    #[default]
    Dismissible,
    /// Closes only on Esc, on submit, or on `Event::Visible(false)`. It has no way to learn that
    /// it lost focus, so taking focus from it strands it on screen while the keyboard goes
    /// somewhere else - `hocus-new-agent`, `quit-confirmation`.
    Modal,
}

impl OverlayKind {
    pub fn is_modal(&self) -> bool {
        matches!(self, OverlayKind::Modal)
    }
}

impl FromStr for OverlayKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "dismissible" | "Dismissible" | "overlay" => Ok(OverlayKind::Dismissible),
            "modal" | "Modal" => Ok(OverlayKind::Modal),
            _ => Err(format!("No such overlay kind: {s}")),
        }
    }
}

/// A request to place a pane on the session surface rather than on a tab's floating layer.
///
/// The placement is the thing that says what the pane is, so
/// that overlay-ness cannot be lost by a caller that rebuilds the coordinates.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayPlacement {
    pub kind: OverlayKind,
    /// Where on the surface. `None` centres it at a default size.
    #[serde(default)]
    pub coordinates: Option<FloatingPaneCoordinates>,
}

impl OverlayPlacement {
    pub fn new(kind: OverlayKind, coordinates: Option<FloatingPaneCoordinates>) -> Self {
        OverlayPlacement { kind, coordinates }
    }
    pub fn modal(coordinates: Option<FloatingPaneCoordinates>) -> Self {
        OverlayPlacement::new(OverlayKind::Modal, coordinates)
    }
    pub fn dismissible(coordinates: Option<FloatingPaneCoordinates>) -> Self {
        OverlayPlacement::new(OverlayKind::Dismissible, coordinates)
    }
    pub fn is_modal(&self) -> bool {
        self.kind.is_modal()
    }
    /// Where on the surface this overlay sits, defaulted if it did not say.
    ///
    /// Only the geometry: what makes it an overlay is the placement itself, and which kind of
    /// overlay is `kind`. Coordinates cannot express either.
    pub fn as_floating_pane_coordinates(&self) -> FloatingPaneCoordinates {
        self.coordinates.clone().unwrap_or_default()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum NewPanePlacement {
    NoPreference {
        borderless: Option<bool>,
    },
    Tiled {
        direction: Option<Direction>,
        borderless: Option<bool>,
    },
    Floating(Option<FloatingPaneCoordinates>),
    InPlace {
        pane_id_to_replace: Option<PaneId>,
        close_replaced_pane: bool,
        borderless: Option<bool>,
    },
    Stacked {
        pane_id_to_stack_under: Option<PaneId>,
        borderless: Option<bool>,
    },
    /// Over the whole session surface - not on the tab's floating layer.
    Overlay(OverlayPlacement),
}

impl Default for NewPanePlacement {
    fn default() -> Self {
        NewPanePlacement::NoPreference { borderless: None }
    }
}

impl NewPanePlacement {
    /// The placement a `--floating` CLI invocation asks for.
    ///
    /// `--modal` and `--overlay` both put the pane on the session surface rather than on the
    /// tab's floating layer; they differ only in whether focus may be taken from it. `--modal`
    /// wins if both are given, being the stricter of the two.
    #[allow(clippy::too_many_arguments)]
    pub fn floating_or_overlay(
        x: Option<String>,
        y: Option<String>,
        width: Option<String>,
        height: Option<String>,
        pinned: Option<bool>,
        borderless: Option<bool>,
        overlay: Option<bool>,
        modal: Option<bool>,
    ) -> Self {
        let is_modal = modal.unwrap_or(false);
        let is_overlay = is_modal || overlay.unwrap_or(false);
        let coordinates = FloatingPaneCoordinates::new(x, y, width, height, pinned, borderless);
        if is_overlay {
            NewPanePlacement::Overlay(OverlayPlacement {
                kind: if is_modal {
                    OverlayKind::Modal
                } else {
                    OverlayKind::Dismissible
                },
                coordinates,
            })
        } else {
            NewPanePlacement::Floating(coordinates)
        }
    }
    pub fn with_floating_pane_coordinates(
        floating_pane_coordinates: Option<FloatingPaneCoordinates>,
    ) -> Self {
        NewPanePlacement::Floating(floating_pane_coordinates)
    }
    pub fn with_should_be_in_place(
        self,
        should_be_in_place: bool,
        close_replaced_pane: bool,
    ) -> Self {
        if should_be_in_place {
            NewPanePlacement::InPlace {
                pane_id_to_replace: None,
                close_replaced_pane,
                borderless: None,
            }
        } else {
            self
        }
    }
    pub fn with_pane_id_to_replace(
        pane_id_to_replace: Option<PaneId>,
        close_replaced_pane: bool,
    ) -> Self {
        NewPanePlacement::InPlace {
            pane_id_to_replace,
            close_replaced_pane,
            borderless: None,
        }
    }
    pub fn should_float(&self) -> Option<bool> {
        match self {
            NewPanePlacement::Floating(_) => Some(true),
            NewPanePlacement::Tiled { .. } => Some(false),
            _ => None,
        }
    }
    /// Where on a surface this pane sits, for the two placements that have coordinates at all:
    /// the tab's floating layer and the session surface an overlay is drawn on.
    pub fn floating_pane_coordinates(&self) -> Option<FloatingPaneCoordinates> {
        match self {
            NewPanePlacement::Floating(floating_pane_coordinates) => {
                floating_pane_coordinates.clone()
            },
            NewPanePlacement::Overlay(overlay) => overlay.coordinates.clone(),
            _ => None,
        }
    }
    pub fn should_stack(&self) -> bool {
        matches!(self, NewPanePlacement::Stacked { .. })
    }
    pub fn id_of_stack_root(&self) -> Option<PaneId> {
        match self {
            NewPanePlacement::Stacked {
                pane_id_to_stack_under,
                ..
            } => *pane_id_to_stack_under,
            _ => None,
        }
    }
    pub fn get_borderless(&self) -> Option<bool> {
        match self {
            NewPanePlacement::NoPreference { borderless } => *borderless,
            NewPanePlacement::Tiled { borderless, .. } => *borderless,
            NewPanePlacement::Floating(coords) => coords.as_ref().and_then(|c| c.borderless),
            NewPanePlacement::InPlace { borderless, .. } => *borderless,
            NewPanePlacement::Stacked { borderless, .. } => *borderless,
            NewPanePlacement::Overlay(overlay) => {
                overlay.coordinates.as_ref().and_then(|c| c.borderless)
            },
        }
    }
}

type Context = BTreeMap<String, String>;

/// Bytes that may be written as a string.
///
/// `PluginCommand`'s serde representation exists for exactly one caller - the Lua runtime, which
/// builds commands with `from_value` (the wasm side uses protobuf and never touches this). Lua
/// has no byte-array type: text and bytes are both the one string type. So a plugin writing
/// `zellij.fs.write(path, "hello")` hands over a string, and the default `Vec<u8>` deserializer,
/// which wants a sequence of numbers, would refuse it.
///
/// Accepting both keeps the natural spelling working without giving up binary writes: a plugin
/// that really does have bytes can still pass an array of numbers.
mod bytes_or_string {
    use serde::de::{Deserializer, Error, SeqAccess, Visitor};
    use serde::Serializer;
    use std::fmt;

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(bytes)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        struct BytesOrString;

        impl<'de> Visitor<'de> for BytesOrString {
            type Value = Vec<u8>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string or an array of byte values")
            }

            fn visit_str<E: Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(value.as_bytes().to_vec())
            }

            fn visit_bytes<E: Error>(self, value: &[u8]) -> Result<Self::Value, E> {
                Ok(value.to_vec())
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut bytes = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(byte) = seq.next_element::<u8>()? {
                    bytes.push(byte);
                }
                Ok(bytes)
            }
        }

        deserializer.deserialize_any(BytesOrString)
    }
}

#[derive(Debug, Clone, PartialEq, EnumDiscriminants, Display, Serialize, Deserialize)]
#[strum_discriminants(derive(EnumString, EnumIter, Display, Hash, Serialize, Deserialize))]
#[strum_discriminants(name(CommandType))]
#[allow(clippy::large_enum_variant)]
pub enum PluginCommand {
    Subscribe(HashSet<EventType>),
    Unsubscribe(HashSet<EventType>),
    SetSelectable(bool),
    ShowCursor(Option<(usize, usize)>),
    GetPluginIds,
    GetZellijVersion,
    OpenFile(FileToOpen, Context),
    OpenFileFloating(FileToOpen, Option<FloatingPaneCoordinates>, Context),
    OpenTerminal(FileToOpen), // only used for the path as cwd
    OpenTerminalFloating(FileToOpen, Option<FloatingPaneCoordinates>), // only used for the path as cwd
    OpenCommandPane(CommandToRun, Context),
    OpenCommandPaneFloating(CommandToRun, Option<FloatingPaneCoordinates>, Context),
    /// Open a command pane directly onto the session surface, above the tab's own panes. A terminal
    /// pane carries no plugin configuration, so the placement is the only thing that can say it
    /// is an overlay - and which kind.
    OpenCommandPaneOverlay(CommandToRun, OverlayPlacement, Context),
    SwitchTabTo(u32), // tab index
    SetTimeout(f64),  // seconds
    ExecCmd(Vec<String>),
    PostMessageTo(PluginMessage),
    PostMessageToPlugin(PluginMessage),
    HideSelf,
    ShowSelf(bool), // bool - should float if hidden
    SwitchToMode(InputMode),
    NewTabsWithLayout(String), // raw kdl layout
    NewTab {
        name: Option<String>,
        cwd: Option<String>,
    },
    NewTabUnfocused {
        name: Option<String>,
        cwd: Option<String>,
    },
    NewTiledPaneInTab {
        tab_position: usize,
    },
    ToggleFloatingPanes {
        tab_id: Option<u64>,
    },
    NewPane,
    GoToNextTab,
    GoToPreviousTab,
    Resize(Resize),
    ResizeWithDirection(ResizeStrategy),
    FocusNextPane,
    FocusPreviousPane,
    FocusLastPane,
    MoveFocus(Direction),
    MoveFocusOrTab(Direction),
    Detach,
    EditScrollback,
    Write(Vec<u8>), // bytes
    WriteChars(String),
    ToggleTab,
    MovePane,
    MovePaneWithDirection(Direction),
    ClearScreen,
    ScrollUp,
    ScrollDown,
    ScrollToTop,
    ScrollToBottom,
    PageScrollUp,
    PageScrollDown,
    ToggleFocusFullscreen,
    ToggleFocusNoUiFullscreen,
    TogglePaneFrames,
    SetPaneFrameStyle(PaneFrameStyle),
    TogglePaneEmbedOrEject,
    UndoRenamePane,
    CloseFocus,
    ToggleActiveTabSync,
    CloseFocusedTab,
    UndoRenameTab,
    QuitZellij,
    PreviousSwapLayout,
    NextSwapLayout,
    GoToTabName(String),
    FocusOrCreateTab(String),
    GoToTab(u32),                       // tab index
    StartOrReloadPlugin(String),        // plugin url (eg. file:/path/to/plugin.wasm)
    CloseTerminalPane(u32),             // terminal pane id
    ClosePluginPane(u32),               // plugin pane id
    FocusTerminalPane(u32, bool, bool), // terminal pane id, should_float_if_hidden, should_be_in_place_if_hidden
    FocusPluginPane(u32, bool, bool), // plugin pane id, should_float_if_hidden, should_be_in_place_if_hidden
    RenameTerminalPane(u32, String),  // terminal pane id, new name
    RenamePluginPane(u32, String),    // plugin pane id, new name
    RenameTab(u32, String),           // tab index, new name
    ReportPanic(String),              // stringified panic
    RequestPluginPermissions(Vec<PermissionType>),
    SwitchSession(ConnectToSession),
    DeleteDeadSession(String),       // String -> session name
    DeleteAllDeadSessions,           // String -> session name
    OpenTerminalInPlace(FileToOpen), // only used for the path as cwd
    OpenFileInPlace(FileToOpen, Context),
    OpenCommandPaneInPlace(CommandToRun, Context),
    RunCommand(
        Vec<String>,              // command
        BTreeMap<String, String>, // env_variables
        PathBuf,                  // cwd
        BTreeMap<String, String>, // context
        Option<usize>,            // max bytes returned for each output stream
    ),
    WebRequest(
        String, // url
        HttpVerb,
        BTreeMap<String, String>, // headers
        Vec<u8>,                  // body
        BTreeMap<String, String>, // context
        Option<String>,           // stream_id: Some(_) => stream the response body back as
        // WebRequestStreamChunk events under this id
        Option<String>, // proxy: Some(_) => send this request through the given
                        // proxy URL (e.g. "socks5://host:port") instead of direct
    ),
    CancelWebRequest(String),       // String -> stream_id
    RenameSession(String),          // String -> new session name
    Notify(String, Option<PaneId>), // String -> message, Option<PaneId> -> pane to focus on click
    UnblockCliPipeInput(String),    // String => pipe name
    BlockCliPipeInput(String),      // String => pipe name
    CliPipeOutput(String, String),  // String => pipe name, String => output
    MessageToPlugin(MessageToPlugin),
    DisconnectOtherClients,
    KillSessions(Vec<String>), // one or more session names
    ScanHostFolder(PathBuf),   // TODO: rename to ScanHostFolder
    WatchFilesystem,
    /// Read a file, answering with its bytes.
    ///
    /// This and its `Fs*` siblings are the deliberate counterpart to the wasm runtime's WASI
    /// preopens: a wasm plugin reaches `/host`, `/data`, `/cache` and `/tmp` through `std::fs`,
    /// and a plugin with no filesystem of its own (Lua) reaches the same four roots through
    /// these. Paths are resolved and confined by `resolve_plugin_fs_path` on the server, which,
    /// unlike `translate_plugin_path`, refuses a path that escapes its root. Reaching outside the
    /// four roots needs `FullHdAccess` on top of the read/write permission.
    ///
    /// Answers synchronously, so that a plugin can read a config file in one expression rather
    /// than through a subscription. `max_bytes` caps the read; the host applies its own ceiling
    /// regardless, so this can only ask for less.
    FsRead {
        path: PathBuf,
        max_bytes: Option<usize>,
    },
    /// List a directory. Answers with one entry per child, its path spelled in the same virtual
    /// root the request used, so a plugin can feed the answer straight back in.
    FsList {
        path: PathBuf,
    },
    /// Metadata for one path, or nothing if it does not exist.
    FsMetadata {
        path: PathBuf,
    },
    /// Write a file, replacing it or appending to it.
    FsWrite {
        path: PathBuf,
        #[serde(with = "bytes_or_string")]
        contents: Vec<u8>,
        append: bool,
        create_dirs: bool,
    },
    /// Remove a file, or a directory when `recursive`.
    FsRemove {
        path: PathBuf,
        recursive: bool,
    },
    /// Create a directory. `all` creates missing parents and succeeds if it already exists.
    FsCreateDir {
        path: PathBuf,
        all: bool,
    },
    /// Rename a path. Both endpoints are resolved and confined independently.
    FsRename {
        from: PathBuf,
        to: PathBuf,
    },
    /// Grant `permissions` to the plugin at `plugin_url` ahead of it asking, as if the user had
    /// answered its prompt. Builtins only: this is how a plugin that authors other plugins
    /// carries the user's approval of the source over to the permissions that source asks for.
    GrantPluginPermissions {
        plugin_url: String,
        permissions: Vec<PermissionType>,
    },
    DumpSessionLayout {
        tab_index: Option<usize>,
    },
    CloseSelf,
    NewTabsWithLayoutInfo(LayoutInfo),
    Reconfigure(String, bool), // String -> stringified configuration, bool -> save configuration
    // file to disk
    HidePaneWithId(PaneId),
    ShowPaneWithId(PaneId, bool, bool), // bools -> should_float_if_hidden, should_focus_pane
    OpenCommandPaneBackground(CommandToRun, Context),
    RerunCommandPane(u32), // u32  - terminal pane id
    /// Run a different command in an existing (held) command pane, reusing its pane id. The
    /// `context` round-trips on the resulting `CommandPaneExited`, so each step of a sequence can
    /// carry its own correlation keys. When the pane's current command is still running, the
    /// replacement starts when it exits (last write wins).
    RunCommandInPane {
        pane_id: PaneId,
        command: CommandToRun,
        context: Context,
        #[serde(default)]
        keep_scrollback: bool,
    },
    ResizePaneIdWithDirection(ResizeStrategy, PaneId),
    EditScrollbackForPaneWithId(PaneId),
    GetPaneScrollback {
        pane_id: PaneId,
        get_full_scrollback: bool,
    },
    WriteToPaneId(Vec<u8>, PaneId),
    WriteCharsToPaneId(String, PaneId),
    SendSigintToPaneId(PaneId),
    SendSigkillToPaneId(PaneId),
    GetPanePid {
        pane_id: PaneId,
    },
    GetPaneRunningCommand {
        pane_id: PaneId,
    },
    GetPaneCwd {
        pane_id: PaneId,
    },
    /// What Git says about a *remote* pane's folder, as its own machine sees it.
    GetPaneGitProbe {
        pane_id: PaneId,
    },
    MovePaneWithPaneId(PaneId),
    MovePaneWithPaneIdInDirection(PaneId, Direction),
    ClearScreenForPaneId(PaneId),
    ScrollUpInPaneId(PaneId),
    ScrollDownInPaneId(PaneId),
    ScrollToTopInPaneId(PaneId),
    ScrollToBottomInPaneId(PaneId),
    PageScrollUpInPaneId(PaneId),
    PageScrollDownInPaneId(PaneId),
    TogglePaneIdFullscreen(PaneId),
    TogglePaneEmbedOrEjectForPaneId(PaneId),
    CloseTabWithIndex(usize), // usize - tab_index
    BreakPanesToNewTab(Vec<PaneId>, Option<String>, bool), // bool -
    // should_change_focus_to_new_tab,
    // Option<String> - optional name for
    // the new tab
    BreakPanesToTabWithIndex(Vec<PaneId>, usize, bool), // usize - tab_index, bool -
    // should_change_focus_to_new_tab
    SwitchTabToId(u64),           // u64 - tab_id
    GoToTabWithId(u64),           // u64 - tab_id
    CloseTabWithId(u64),          // u64 - tab_id
    RenameTabWithId(u64, String), // u64 - tab_id, String - new name
    /// The project each tab is working on, keyed by tab id.
    ///
    /// Advisory rather than a rename: the server shows these to fill in tabs the
    /// user has not named, and never writes them to the tab. Sent as the whole
    /// map, so a tab left out of it has no project.
    SetTabProjects(BTreeMap<u64, String>),
    BreakPanesToTabWithId(Vec<PaneId>, u64, bool), // u64 - tab_id, bool -
    // should_change_focus_to_target_tab
    ReloadPlugin(u32), // u32 - plugin pane id
    LoadNewPlugin {
        url: String,
        config: BTreeMap<String, String>,
        load_in_background: bool,
        skip_plugin_cache: bool,
    },
    RebindKeys {
        keys_to_rebind: Vec<(InputMode, KeyWithModifier, Vec<Action>)>,
        keys_to_unbind: Vec<(InputMode, KeyWithModifier)>,
        write_config_to_disk: bool,
    },
    RegisterDefaultKeybindings(String),
    ListClients,
    ChangeHostFolder(PathBuf),
    SetFloatingPanePinned(PaneId, bool), // bool -> should be pinned
    StackPanes(Vec<PaneId>),
    ChangeFloatingPanesCoordinates(Vec<(PaneId, FloatingPaneCoordinates)>),
    /// Move or resize an overlay on the session surface. The overlay's kind was decided when it
    /// was born and is not touched here.
    ChangeOverlayCoordinates {
        placements: Vec<(PaneId, FloatingPaneCoordinates)>,
        /// The placement is a user gesture made through the plugin, so it may reclaim a position
        /// the user dragged and a size they resized (`OverlayCarry::position_is_user_stated` /
        /// `size_is_user_stated` on the server) rather than being overruled by them.
        override_user_placement: bool,
    },
    TogglePaneBorderless(PaneId),
    SetPaneBorderless(PaneId, bool),
    OpenCommandPaneNearPlugin(CommandToRun, Context),
    OpenCommandPaneInGitCheckoutNearPlugin(
        CommandToRun,
        PathBuf,
        GitCheckout,
        #[serde(default)] bool,
    ),
    OpenTerminalNearPlugin(FileToOpen),
    OpenTerminalFloatingNearPlugin(FileToOpen, Option<FloatingPaneCoordinates>),
    OpenTerminalInPlaceOfPlugin(FileToOpen, bool), // bool -> close_plugin_after_replace
    OpenCommandPaneFloatingNearPlugin(CommandToRun, Option<FloatingPaneCoordinates>, Context),
    OpenCommandPaneInPlaceOfPlugin(CommandToRun, bool, Context), // bool ->
    // close_plugin_after_replace
    OpenFileNearPlugin(FileToOpen, Context),
    OpenFileFloatingNearPlugin(FileToOpen, Option<FloatingPaneCoordinates>, Context),
    OpenFileInPlaceOfPlugin(FileToOpen, bool, Context), // bool -> close_plugin_after_replace
    GroupAndUngroupPanes(Vec<PaneId>, Vec<PaneId>, bool), // panes to group, panes to ungroup,
    // bool -> for all clients
    HighlightAndUnhighlightPanes(Vec<PaneId>, Vec<PaneId>), // panes to highlight, panes to
    // unhighlight
    /// Bring a pane into view for this client and highlight it, without taking
    /// keyboard focus. The `String` is a label drawn in place of the pane's
    /// title for as long as the highlight is up.
    RevealPaneWithId(PaneId, Option<String>),
    CloseMultiplePanes(Vec<PaneId>),
    FloatMultiplePanes(Vec<PaneId>),
    EmbedMultiplePanes(Vec<PaneId>),
    SetSelfMouseSelectionSupport(bool),
    InterceptKeyPresses,
    ClearKeyPressesIntercepts,
    ReplacePaneWithExistingPane(PaneId, PaneId, bool), // (pane id to replace, pane id of existing,
    // suppress_replaced_pane)
    RunAction(Action, BTreeMap<String, String>),
    CopyToClipboard(String), // text to copy
    OverrideLayout(
        LayoutInfo,
        bool,                     // retain_existing_terminal_panes
        bool,                     // retain_existing_plugin_panes
        bool,                     // apply_only_to_active_tab,
        BTreeMap<String, String>, // context
    ),
    SaveLayout {
        layout_name: String,
        layout_kdl: String,
        overwrite: bool,
    },
    DeleteLayout {
        layout_name: String,
    },
    RenameLayout {
        old_layout_name: String,
        new_layout_name: String,
    },
    EditLayout {
        layout_name: String,
        context: Context,
    },
    GenerateRandomName,
    DumpLayout(String),
    ParseLayout(String), // String contains raw KDL layout
    GetLayoutDir,
    /// The folder holding the user's own plugins. A plugin that writes one needs the absolute
    /// path, and a wasm plugin cannot discover it: its WASI preopens are `/host`, `/data`,
    /// `/cache` and `/tmp`, and a config dir is none of them.
    GetUserPluginDir,
    GetFocusedPaneInfo,
    SaveSession,
    CurrentSessionLastSavedTime,
    GetPaneInfo(PaneId),
    GetTabInfo(usize), // tab_id
    GetSessionEnvironmentVariables,
    /// The KDL configuration block declared for one plugin alias, by alias name.
    ///
    /// A plugin's own configuration already reaches it through `load`; this is for
    /// reading a *different* plugin's, which nothing else answers. `SessionInfo.plugins`
    /// comes closest but is keyed on running plugins, so it cannot speak for an alias
    /// whose plugin has never been launched.
    GetPluginAliasConfiguration(String), // alias name
    OpenCommandPaneInNewTab(CommandToRun, Context),
    OpenPluginPaneInNewTab {
        plugin_url: String,
        configuration: BTreeMap<String, String>,
        context: Context,
    },
    OpenEditorPaneInNewTab(FileToOpen, Context),
    OpenCommandPaneInPlaceOfPaneId(PaneId, CommandToRun, bool, Context), // bool = close_replaced_pane
    OpenTerminalPaneInPlaceOfPaneId(PaneId, FileToOpen, bool),
    OpenEditPaneInPlaceOfPaneId(PaneId, FileToOpen, bool, Context),
    HideFloatingPanes {
        tab_id: Option<usize>,
    },
    ShowFloatingPanes {
        tab_id: Option<usize>,
    },
    SetPaneColor(PaneId, Option<String>, Option<String>), // (pane_id, fg, bg)
    SetPaneRegexHighlights(PaneId, Vec<RegexHighlight>),
    /// Answer a `Event::ContextMenuRequested`: this is what the menu should say.
    ///
    /// `request_id` is echoed from the request. Answering a menu that has since been dismissed or
    /// replaced is harmless - the server drops it - so a plugin never has to track whether its
    /// answer is still wanted.
    SetContextMenu {
        request_id: u64,
        items: Vec<ContextMenuItem>,
    },
    ClearPaneHighlights(PaneId),
    OpenPluginPaneFloating {
        plugin_url: String,
        configuration: BTreeMap<String, String>,
        floating_pane_coordinates: Option<FloatingPaneCoordinates>,
        context: BTreeMap<String, String>,
    },
    GetSessionList,
    KillSessionsAndReply(Vec<String>), // one or more session names; sends a response back
    DeleteDeadSessionAndReply(String), // session name; sends a response back
    DeleteAllDeadSessionsAndReply,     // no payload; sends a response back
    FocusHostSession,
    ListRecordings,
    ListNotifications,
    SearchRecordings {
        query: String,
        limit: usize,
    },
    /// Move a pane out of this session and into another one. Sends a response
    /// back, and blocks until the pane has landed or the move has been undone.
    WarpPaneToSession {
        pane_id: PaneId,
        to_session: String,
        placement: WarpPlacement,
    },
    /// Ask another session for one of its panes. Sends a response back.
    WarpPaneFromSession {
        pane_id: PaneId,
        from_session: String,
        placement: WarpPlacement,
    },
    /// Move a whole tab out of this session and into another one. Sends a
    /// response back, and blocks until the tab has landed or the move has been
    /// undone.
    WarpTabToSession {
        tab_index: usize,
        to_session: String,
    },
    /// Ask another session for one of its tabs. Sends a response back.
    WarpTabFromSession {
        tab_index: usize,
        from_session: String,
    },
    /// Panes whose session died but whose process is still running. Sends a
    /// response back.
    ListOrphanedPanes,
    /// Take one of those panes into this session. Sends a response back.
    AdoptOrphanedPane {
        host_id: String,
        placement: WarpPlacement,
    },
    /// Declare an entity in the data store, creating or migrating its storage to match. Sends a
    /// response back.
    DataDefineEntity {
        namespace: Option<String>,
        def: EntityDef,
        allow_destructive: bool,
    },
    /// Apply a batch of writes in one transaction, all or nothing. Sends a response back.
    DataApply {
        namespace: Option<String>,
        mutations: Vec<Mutation>,
    },
    /// Read rows from the data store. Sends a response back.
    DataFetch {
        request: FetchRequest,
    },
    /// Count the rows a fetch would match. Sends a response back.
    DataCount {
        request: FetchRequest,
    },
    /// Register a standing fetch, and be sent an [`Event::PluginDataChanged`] whenever its results
    /// change. Sends the initial rows back.
    DataWatch {
        request: FetchRequest,
        mode: WatchMode,
    },
    /// Stop a watch registered with `DataWatch`. Sends a response back.
    DataUnwatch {
        watch_id: u64,
    },
    /// What entities exist, and what their columns are. Sends a response back.
    ///
    /// Unlike every other `Data*` command, `None` means *every namespace this plugin may see* —
    /// its own, plus the `shared:` ones if it holds `SharedPluginData` — rather than its own
    /// alone. A catalog of only what the caller declared itself would have nothing to tell it,
    /// and discovering another plugin's published entities is the whole point. `Some(namespace)`
    /// names one `shared:` namespace, on the same terms a fetch does.
    DataCatalog {
        namespace: Option<String>,
    },
    /// Compose a decoration in front of a pane's title, or drop it with `None`.
    ///
    /// Not a rename: the pane's name, its own OSC title and the title a layout gave
    /// it are all untouched, and so is the `title` every plugin reads off the pane
    /// manifest. The prefix only shows where a human looks at the pane — its frame
    /// and its row in a stack list — and survives every rename, which is the point:
    /// it carries a fact about the pane that the pane's name cannot be trusted to
    /// keep. Terminal panes only.
    SetPaneTitlePrefix {
        pane_id: PaneId,
        prefix: Option<String>,
    },
    /// Put a plugin on a named work queue, to be run as a fresh headless instance when the queue
    /// has a free slot. Sends the job id back.
    ///
    /// The queue is created with concurrency 1 if it does not exist. `config` becomes the job's
    /// configuration (the host adds `zellij_queue`, `zellij_queue_job_id` and
    /// `zellij_queue_enqueuer_plugin_id`); `context` is not given to the job but echoed back in
    /// [`Event::QueueJobStarted`] and [`Event::QueueJobFinished`], which only the enqueuer receives.
    EnqueueQueueJob {
        queue: String,
        plugin_url: String,
        config: BTreeMap<String, String>,
        context: BTreeMap<String, String>,
    },
    /// Set how many jobs `queue` may run at once, creating it if needed. Lowering the limit below
    /// the number of running jobs evicts none of them; it just stops new ones starting.
    ConfigureQueue {
        queue: String,
        concurrency: usize,
    },
    /// Drop a pending job, or unload a running one. Its enqueuer is told `Cancelled`.
    CancelQueueJob {
        job_id: u64,
    },
    /// Sends back every queue, or just the named one, with its pending and running jobs.
    QueueStatus {
        queue: Option<String>,
    },
    /// Called by a job to report its outcome. The host then unloads the calling plugin.
    ///
    /// Ignored when the caller is not a queued job.
    FinishQueueJob {
        success: bool,
        payload: Option<String>,
    },
    /// Compile `source` as a Lua plugin without running it, and answer with the compile error if
    /// there is one. `name` is what the error names, as the loader's own error would
    /// (`keys.lua:3: ...`). Nothing is written or loaded: this is the check a writer runs before
    /// a reload, so a plugin that cannot parse is refused where the writer can see it rather than
    /// reported in a pane nobody is looking at.
    CheckLuaPlugin {
        name: String,
        source: String,
    },
    /// Set what agent runs in `pane_id` and what it is doing, or clear it with `None`. The
    /// record replaces the pane's whole one; every reader of the manifest sees it next report.
    SetPaneAgent {
        pane_id: PaneId,
        agent: Option<AgentInfo>,
    },
    /// Set the id of the run record this plugin is keeping for `pane_id`'s agent, or clear it
    /// with `None`.
    ///
    /// Additive where [`PluginCommand::SetPaneAgent`] is wholesale: this touches one field and
    /// leaves the rest of the record as the agent last reported it, which is what a plugin
    /// holding an agent's history needs — it is not the authority on what the agent is doing,
    /// only on which run it is doing it in. A run id for a pane with no record is dropped:
    /// it is bookkeeping about a record, not a record.
    SetPaneAgentRun {
        pane_id: PaneId,
        run_id: Option<String>,
    },
    /// What this plugin wants said, and decided, when the session is about to end.
    ///
    /// The `notice` is one line the quit confirmation shows, or `None` to withdraw whatever this
    /// plugin last said. `guard` is whether closing the session's *last* pane should raise that
    /// confirmation instead of ending the session there and then.
    ///
    /// The two are independent: a plugin may have something worth saying about a quit without
    /// wanting to stand in the way of one - hocus with three settled conversations open says so
    /// and lets the quit through, and only guards while a reply or a tool is still outstanding.
    ///
    /// Held per plugin and dropped when its pane closes, so a plugin that goes away cannot leave
    /// a guard behind that nothing can lift.
    SetQuitNotice {
        notice: Option<String>,
        guard: bool,
    },
    /// Reload every running instance of the plugin at `url` in place, opening nothing. For a
    /// plugin that is not running this does nothing; `StartOrReloadPlugin` is the one that opens
    /// a pane. Fires `BeforeClose` on the old instance first, for state worth keeping.
    ReloadPluginByUrl(String),
    /// The last `lines` lines of zellij's own log about the plugin `name`: what its
    /// `zellij.log`/`print` wrote, tagged `[lua:<file>]`, and any loader error naming its file.
    /// `name` is a plugin name as `plugin_write` spells it (`keys`, `hocus/weather`), resolved in
    /// the user's plugin folder the way `zellij plugins log` resolves it, or a builtin's tag.
    ///
    /// Answers synchronously with the lines, or the reason there are none. The log file is
    /// outside every root a plugin can read, and the server holds the only copy of its path,
    /// which is why this is a command and not a file a plugin greps for itself.
    ReadPluginLog {
        name: String,
        lines: usize,
    },
}

/// The job id handed back by `EnqueueQueueJob`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnqueueQueueJobResponse {
    Ok(u64),
    Err(String),
}

/// The queues reported by `QueueStatus`.
pub type QueueStatusResponse = Vec<QueueSnapshot>;

/// The outcome of declaring an entity in the data store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataDefineResponse {
    Ok(DataSchemaInfo),
    Err(String),
}

/// The outcome of a batch of writes to the data store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataApplyResponse {
    Ok(ApplyReport),
    Err(String),
}

/// Rows read from the data store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DataFetchResponse {
    Ok(DataRows),
    Err(String),
}

/// How many rows a fetch matched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataCountResponse {
    Ok(u64),
    Err(String),
}

/// The entities a plugin may see, with their definitions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DataCatalogResponse {
    Ok(Vec<DataEntityInfo>),
    Err(String),
}

/// A registered watch, and the rows at the moment it was registered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DataWatchResponse {
    Ok(DataWatchHandle),
    Err(String),
}

/// The outcome of cancelling a watch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataUnwatchResponse {
    Ok,
    Err(String),
}

// Response type for plugin API methods that open a pane in a new tab
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenPaneInNewTabResponse {
    pub tab_id: Option<usize>,
    pub pane_id: Option<PaneId>,
}

// Response types for plugin API methods that create tabs
pub type NewTabResponse = Option<usize>;
pub type NewTabUnfocusedResponse = Option<usize>;
pub type NewTabsResponse = Vec<usize>;
pub type FocusOrCreateTabResponse = Option<usize>;
pub type BreakPanesToNewTabResponse = Option<usize>;
pub type BreakPanesToTabWithIndexResponse = Option<usize>;
pub type BreakPanesToTabWithIdResponse = Option<usize>;

// Response types for plugin API methods that create panes
pub type OpenFileResponse = Option<PaneId>;
pub type OpenFileFloatingResponse = Option<PaneId>;
pub type OpenFileInPlaceResponse = Option<PaneId>;
pub type OpenFileNearPluginResponse = Option<PaneId>;
pub type OpenFileFloatingNearPluginResponse = Option<PaneId>;
pub type OpenFileInPlaceOfPluginResponse = Option<PaneId>;

pub type OpenTerminalResponse = Option<PaneId>;
pub type OpenTerminalFloatingResponse = Option<PaneId>;
pub type OpenTerminalInPlaceResponse = Option<PaneId>;
pub type OpenTerminalNearPluginResponse = Option<PaneId>;
pub type OpenTerminalFloatingNearPluginResponse = Option<PaneId>;
pub type OpenTerminalInPlaceOfPluginResponse = Option<PaneId>;
pub type NewTiledPaneInTabResponse = Option<PaneId>;

pub type OpenCommandPaneResponse = Option<PaneId>;
pub type OpenCommandPaneFloatingResponse = Option<PaneId>;
pub type OpenCommandPaneOverlayResponse = Option<PaneId>;
pub type OpenCommandPaneInPlaceResponse = Option<PaneId>;
pub type OpenCommandPaneNearPluginResponse = Option<PaneId>;
pub type OpenCommandPaneInGitCheckoutResponse = Result<PaneId, String>;
pub type OpenCommandPaneFloatingNearPluginResponse = Option<PaneId>;
pub type OpenCommandPaneInPlaceOfPluginResponse = Option<PaneId>;
pub type OpenCommandPaneBackgroundResponse = Option<PaneId>;
pub type OpenCommandPaneInPlaceOfPaneIdResponse = Option<PaneId>;
pub type OpenTerminalPaneInPlaceOfPaneIdResponse = Option<PaneId>;
pub type OpenEditPaneInPlaceOfPaneIdResponse = Option<PaneId>;
pub type OpenPluginPaneFloatingResponse = Option<PaneId>;

#[test]
pub fn can_parse_unicode_bare_keys() {
    let key = "1087"; // п
    assert_eq!(
        BareKey::from_bytes_with_u(key.as_bytes()),
        Some(BareKey::Char('п')),
        "Can parse a bare 'п' keypress"
    );
    let key = "1255"; // ӧ
    assert_eq!(
        BareKey::from_bytes_with_u(key.as_bytes()),
        Some(BareKey::Char('ӧ')),
        "Can parse a bare 'ӧ' keypress"
    );
    let key = "1098"; // ъ
    assert_eq!(
        BareKey::from_bytes_with_u(key.as_bytes()),
        Some(BareKey::Char('ъ')),
        "Can parse a bare 'ъ' keypress"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use hegel::generators::{self, Generator};

    #[test]
    fn modified_backtab_keeps_implicit_shift() {
        assert_eq!(
            KeyWithModifier::from_bytes_with_no_ending_byte(b"Z", b"5"),
            Some(
                KeyWithModifier::new(BareKey::Tab)
                    .with_ctrl_modifier()
                    .with_shift_modifier()
            )
        );
    }

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
