use {
    crate::client_server_contract::client_server_contract::BareKey as ProtoBareKey,
    crate::client_server_contract::client_server_contract::KeyModifier as ProtoKeyModifier,
    yinz_runtime::errors::prelude::*, yinz_types::data::BareKey, yinz_types::data::KeyModifier,
};

#[cfg(test)]
#[path = "unit/key_tests.rs"]
mod tests;

// BareKey conversions
impl From<BareKey> for ProtoBareKey {
    fn from(key: BareKey) -> Self {
        match key {
            BareKey::PageDown => ProtoBareKey::PageDown,
            BareKey::PageUp => ProtoBareKey::PageUp,
            BareKey::Left => ProtoBareKey::Left,
            BareKey::Down => ProtoBareKey::Down,
            BareKey::Up => ProtoBareKey::Up,
            BareKey::Right => ProtoBareKey::Right,
            BareKey::Home => ProtoBareKey::Home,
            BareKey::End => ProtoBareKey::End,
            BareKey::Backspace => ProtoBareKey::Backspace,
            BareKey::Delete => ProtoBareKey::Delete,
            BareKey::Insert => ProtoBareKey::Insert,
            BareKey::F(1) => ProtoBareKey::F1,
            BareKey::F(2) => ProtoBareKey::F2,
            BareKey::F(3) => ProtoBareKey::F3,
            BareKey::F(4) => ProtoBareKey::F4,
            BareKey::F(5) => ProtoBareKey::F5,
            BareKey::F(6) => ProtoBareKey::F6,
            BareKey::F(7) => ProtoBareKey::F7,
            BareKey::F(8) => ProtoBareKey::F8,
            BareKey::F(9) => ProtoBareKey::F9,
            BareKey::F(10) => ProtoBareKey::F10,
            BareKey::F(11) => ProtoBareKey::F11,
            BareKey::F(12) => ProtoBareKey::F12,
            BareKey::F(13) => ProtoBareKey::F13,
            BareKey::F(14) => ProtoBareKey::F14,
            BareKey::F(15) => ProtoBareKey::F15,
            BareKey::F(16) => ProtoBareKey::F16,
            BareKey::F(17) => ProtoBareKey::F17,
            BareKey::F(18) => ProtoBareKey::F18,
            BareKey::F(19) => ProtoBareKey::F19,
            BareKey::F(20) => ProtoBareKey::F20,
            BareKey::F(21) => ProtoBareKey::F21,
            BareKey::F(22) => ProtoBareKey::F22,
            BareKey::F(23) => ProtoBareKey::F23,
            BareKey::F(24) => ProtoBareKey::F24,
            BareKey::F(25) => ProtoBareKey::F25,
            BareKey::F(26) => ProtoBareKey::F26,
            BareKey::F(27) => ProtoBareKey::F27,
            BareKey::F(28) => ProtoBareKey::F28,
            BareKey::F(29) => ProtoBareKey::F29,
            BareKey::F(30) => ProtoBareKey::F30,
            BareKey::F(31) => ProtoBareKey::F31,
            BareKey::F(32) => ProtoBareKey::F32,
            BareKey::F(33) => ProtoBareKey::F33,
            BareKey::F(34) => ProtoBareKey::F34,
            BareKey::F(35) => ProtoBareKey::F35,
            BareKey::F(_) => ProtoBareKey::Unspecified, // Unsupported F-key
            BareKey::Char(_) => ProtoBareKey::Char,     // Character stored separately
            BareKey::Tab => ProtoBareKey::Tab,
            BareKey::Esc => ProtoBareKey::Esc,
            BareKey::Enter => ProtoBareKey::Enter,
            BareKey::CapsLock => ProtoBareKey::CapsLock,
            BareKey::ScrollLock => ProtoBareKey::ScrollLock,
            BareKey::NumLock => ProtoBareKey::NumLock,
            BareKey::PrintScreen => ProtoBareKey::PrintScreen,
            BareKey::Pause => ProtoBareKey::Pause,
            BareKey::Menu => ProtoBareKey::Menu,
        }
    }
}

impl TryFrom<ProtoBareKey> for BareKey {
    type Error = anyhow::Error;

    fn try_from(key: ProtoBareKey) -> Result<Self> {
        match key {
            ProtoBareKey::PageDown => Ok(BareKey::PageDown),
            ProtoBareKey::PageUp => Ok(BareKey::PageUp),
            ProtoBareKey::Left => Ok(BareKey::Left),
            ProtoBareKey::Down => Ok(BareKey::Down),
            ProtoBareKey::Up => Ok(BareKey::Up),
            ProtoBareKey::Right => Ok(BareKey::Right),
            ProtoBareKey::Home => Ok(BareKey::Home),
            ProtoBareKey::End => Ok(BareKey::End),
            ProtoBareKey::Backspace => Ok(BareKey::Backspace),
            ProtoBareKey::Delete => Ok(BareKey::Delete),
            ProtoBareKey::Insert => Ok(BareKey::Insert),
            ProtoBareKey::F1 => Ok(BareKey::F(1)),
            ProtoBareKey::F2 => Ok(BareKey::F(2)),
            ProtoBareKey::F3 => Ok(BareKey::F(3)),
            ProtoBareKey::F4 => Ok(BareKey::F(4)),
            ProtoBareKey::F5 => Ok(BareKey::F(5)),
            ProtoBareKey::F6 => Ok(BareKey::F(6)),
            ProtoBareKey::F7 => Ok(BareKey::F(7)),
            ProtoBareKey::F8 => Ok(BareKey::F(8)),
            ProtoBareKey::F9 => Ok(BareKey::F(9)),
            ProtoBareKey::F10 => Ok(BareKey::F(10)),
            ProtoBareKey::F11 => Ok(BareKey::F(11)),
            ProtoBareKey::F12 => Ok(BareKey::F(12)),
            ProtoBareKey::F13 => Ok(BareKey::F(13)),
            ProtoBareKey::F14 => Ok(BareKey::F(14)),
            ProtoBareKey::F15 => Ok(BareKey::F(15)),
            ProtoBareKey::F16 => Ok(BareKey::F(16)),
            ProtoBareKey::F17 => Ok(BareKey::F(17)),
            ProtoBareKey::F18 => Ok(BareKey::F(18)),
            ProtoBareKey::F19 => Ok(BareKey::F(19)),
            ProtoBareKey::F20 => Ok(BareKey::F(20)),
            ProtoBareKey::F21 => Ok(BareKey::F(21)),
            ProtoBareKey::F22 => Ok(BareKey::F(22)),
            ProtoBareKey::F23 => Ok(BareKey::F(23)),
            ProtoBareKey::F24 => Ok(BareKey::F(24)),
            ProtoBareKey::F25 => Ok(BareKey::F(25)),
            ProtoBareKey::F26 => Ok(BareKey::F(26)),
            ProtoBareKey::F27 => Ok(BareKey::F(27)),
            ProtoBareKey::F28 => Ok(BareKey::F(28)),
            ProtoBareKey::F29 => Ok(BareKey::F(29)),
            ProtoBareKey::F30 => Ok(BareKey::F(30)),
            ProtoBareKey::F31 => Ok(BareKey::F(31)),
            ProtoBareKey::F32 => Ok(BareKey::F(32)),
            ProtoBareKey::F33 => Ok(BareKey::F(33)),
            ProtoBareKey::F34 => Ok(BareKey::F(34)),
            ProtoBareKey::F35 => Ok(BareKey::F(35)),
            ProtoBareKey::Char => Err(anyhow!("Character key needs character data")),
            ProtoBareKey::Tab => Ok(BareKey::Tab),
            ProtoBareKey::Esc => Ok(BareKey::Esc),
            ProtoBareKey::Enter => Ok(BareKey::Enter),
            ProtoBareKey::CapsLock => Ok(BareKey::CapsLock),
            ProtoBareKey::ScrollLock => Ok(BareKey::ScrollLock),
            ProtoBareKey::NumLock => Ok(BareKey::NumLock),
            ProtoBareKey::PrintScreen => Ok(BareKey::PrintScreen),
            ProtoBareKey::Pause => Ok(BareKey::Pause),
            ProtoBareKey::Menu => Ok(BareKey::Menu),
            ProtoBareKey::Unspecified => Err(anyhow!("Unspecified bare key")),
        }
    }
}

// KeyModifier conversions
impl From<KeyModifier> for ProtoKeyModifier {
    fn from(modifier: KeyModifier) -> Self {
        match modifier {
            KeyModifier::Ctrl => ProtoKeyModifier::Ctrl,
            KeyModifier::Alt => ProtoKeyModifier::Alt,
            KeyModifier::Shift => ProtoKeyModifier::Shift,
            KeyModifier::Super => ProtoKeyModifier::Super,
        }
    }
}

impl TryFrom<ProtoKeyModifier> for KeyModifier {
    type Error = anyhow::Error;

    fn try_from(modifier: ProtoKeyModifier) -> Result<Self> {
        match modifier {
            ProtoKeyModifier::Ctrl => Ok(KeyModifier::Ctrl),
            ProtoKeyModifier::Alt => Ok(KeyModifier::Alt),
            ProtoKeyModifier::Shift => Ok(KeyModifier::Shift),
            ProtoKeyModifier::Super => Ok(KeyModifier::Super),
            ProtoKeyModifier::Unspecified => Err(anyhow!("Unspecified key modifier")),
        }
    }
}

// Helper functions for converting between protobuf i32 and enum types
pub fn bare_key_to_proto_i32(key: BareKey) -> i32 {
    ProtoBareKey::from(key) as i32
}

pub fn bare_key_from_proto_i32(value: i32) -> Result<BareKey> {
    let proto_key =
        ProtoBareKey::try_from(value).map_err(|_| anyhow!("Invalid BareKey value: {}", value))?;
    proto_key.try_into()
}

pub fn key_modifier_to_proto_i32(modifier: KeyModifier) -> i32 {
    ProtoKeyModifier::from(modifier) as i32
}

pub fn key_modifier_from_proto_i32(value: i32) -> Result<KeyModifier> {
    let proto_modifier = ProtoKeyModifier::try_from(value)
        .map_err(|_| anyhow!("Invalid KeyModifier value: {}", value))?;
    proto_modifier.try_into()
}
