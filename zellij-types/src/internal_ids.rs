//! Canonical identities shared across crate boundaries.

use serde::{Deserialize, Serialize};
use std::fmt;

/// A terminal identity.
///
/// ```compile_fail
/// use zellij_types::internal_ids::{TerminalId, PluginId};
/// let _: TerminalId = PluginId::new(0);
/// ```
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
#[repr(transparent)]
pub struct TerminalId(u32);

impl TerminalId {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl TryFrom<i64> for TerminalId {
    type Error = std::num::TryFromIntError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        u32::try_from(value).map(Self::new)
    }
}

impl fmt::Display for TerminalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A plugin identity.
///
/// ```compile_fail
/// use zellij_types::internal_ids::{PluginId, ClientId};
/// let _: PluginId = ClientId::new(0);
/// ```
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
#[repr(transparent)]
pub struct PluginId(u32);

impl PluginId {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A client identity.
///
/// ```compile_fail
/// use zellij_types::internal_ids::{ClientId, TerminalId};
/// let _: ClientId = TerminalId::new(0);
/// ```
///
/// ```compile_fail
/// use zellij_types::internal_ids::ClientId;
/// fn for_client(_: ClientId) {}
/// for_client(1_u16);
/// ```
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
#[repr(transparent)]
pub struct ClientId(u16);

impl TryFrom<u32> for ClientId {
    type Error = std::num::TryFromIntError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        u16::try_from(value).map(Self::new)
    }
}

impl TryFrom<i64> for ClientId {
    type Error = std::num::TryFromIntError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        u16::try_from(value).map(Self::new)
    }
}

impl ClientId {
    pub const fn new(value: u16) -> Self {
        Self(value)
    }
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl fmt::Display for ClientId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A persisted recording-pane identity, including legacy signed values.
///
/// ```compile_fail
/// use zellij_types::internal_ids::{RecordingId, TerminalId};
/// let _: RecordingId = TerminalId::new(1);
/// ```
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
#[repr(transparent)]
pub struct RecordingId(i64);

impl RecordingId {
    pub const fn new(value: i64) -> Self {
        Self(value)
    }
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl fmt::Display for RecordingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for RecordingId {
    type Err = std::num::ParseIntError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self::new)
    }
}

impl std::str::FromStr for TerminalId {
    type Err = std::num::ParseIntError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse::<u32>().map(Self::new)
    }
}

impl std::str::FromStr for PluginId {
    type Err = std::num::ParseIntError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse::<u32>().map(Self::new)
    }
}

impl std::str::FromStr for ClientId {
    type Err = std::num::ParseIntError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse::<u16>().map(Self::new)
    }
}

impl TryFrom<u64> for TerminalId {
    type Error = std::num::TryFromIntError;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        u32::try_from(value).map(Self::new)
    }
}
impl TryFrom<usize> for TerminalId {
    type Error = std::num::TryFromIntError;
    fn try_from(value: usize) -> Result<Self, Self::Error> {
        u32::try_from(value).map(Self::new)
    }
}
impl TryFrom<i64> for PluginId {
    type Error = std::num::TryFromIntError;
    fn try_from(value: i64) -> Result<Self, Self::Error> {
        u32::try_from(value).map(Self::new)
    }
}
impl TryFrom<u64> for PluginId {
    type Error = std::num::TryFromIntError;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        u32::try_from(value).map(Self::new)
    }
}
impl TryFrom<usize> for PluginId {
    type Error = std::num::TryFromIntError;
    fn try_from(value: usize) -> Result<Self, Self::Error> {
        u32::try_from(value).map(Self::new)
    }
}
impl TryFrom<u64> for ClientId {
    type Error = std::num::TryFromIntError;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        u16::try_from(value).map(Self::new)
    }
}
impl TryFrom<usize> for ClientId {
    type Error = std::num::TryFromIntError;
    fn try_from(value: usize) -> Result<Self, Self::Error> {
        u16::try_from(value).map(Self::new)
    }
}

macro_rules! unsigned_identity {
    ($(#[$doc:meta])* $name:ident($raw:ty)) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        #[repr(transparent)]
        pub struct $name($raw);
        impl $name {
            pub const fn new(value: $raw) -> Self { Self(value) }
            pub const fn get(self) -> $raw { self.0 }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { self.0.fmt(f) }
        }
        impl std::str::FromStr for $name {
            type Err = std::num::ParseIntError;
            fn from_str(value: &str) -> Result<Self, Self::Err> { value.parse().map(Self::new) }
        }
        impl TryFrom<u64> for $name {
            type Error = std::num::TryFromIntError;
            fn try_from(value: u64) -> Result<Self, Self::Error> { <$raw>::try_from(value).map(Self::new) }
        }
        impl TryFrom<i64> for $name {
            type Error = std::num::TryFromIntError;
            fn try_from(value: i64) -> Result<Self, Self::Error> { <$raw>::try_from(value).map(Self::new) }
        }
    };
}

unsigned_identity!(
    /// A stable tab identity. Moving a tab does not change this value.
    ///
    /// ```compile_fail
    /// use zellij_types::internal_ids::{TabId, TabPosition};
    /// let _: TabId = TabPosition::new(0);
    /// ```
    TabId(usize)
);
unsigned_identity!(
    /// A zero-based tab position in the current display order.
    ///
    /// ```compile_fail
    /// use zellij_types::internal_ids::{TabPosition, TabNumber};
    /// let _: TabPosition = TabNumber::new(1);
    /// ```
    TabPosition(usize)
);
unsigned_identity!(
    /// A one-based tab number used by CLI actions and legacy plugin commands.
    /// Zero retains its legacy meaning: the first tab, like one.
    TabNumber(u32)
);

impl TryFrom<usize> for TabNumber {
    type Error = std::num::TryFromIntError;
    fn try_from(value: usize) -> Result<Self, Self::Error> {
        u32::try_from(value).map(Self::new)
    }
}

impl TabNumber {
    pub fn position(self) -> TabPosition {
        TabPosition::new(self.get().saturating_sub(1) as usize)
    }
}
impl TryFrom<TabPosition> for TabNumber {
    type Error = &'static str;
    fn try_from(position: TabPosition) -> Result<Self, Self::Error> {
        position
            .get()
            .checked_add(1)
            .and_then(|value| u32::try_from(value).ok())
            .map(Self::new)
            .ok_or("Tab position is outside the one-based tab number range")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_identities_preserve_values_and_check_conversion() {
        let id = TabId::new(42);
        assert_eq!(serde_json::to_string(&id).unwrap(), "42");
        assert_eq!(serde_json::from_str::<TabId>("42").unwrap(), id);
        let keyed = std::collections::BTreeMap::from([(id, "work")]);
        assert_eq!(serde_json::to_string(&keyed).unwrap(), r#"{"42":"work"}"#);
        assert!(TabId::try_from(-1_i64).is_err());
        assert_eq!(TabNumber::new(0).position(), TabPosition::new(0));
        assert_eq!(TabNumber::new(1).position(), TabPosition::new(0));
        assert_eq!(
            TabNumber::try_from(TabPosition::new(3)),
            Ok(TabNumber::new(4))
        );
        assert!(TabNumber::try_from(TabPosition::new(usize::MAX)).is_err());
    }

    #[test]
    fn ids_display_as_plain_numbers() {
        for id in [0, u32::MAX] {
            assert_eq!(TerminalId::new(id).to_string(), id.to_string());
            assert_eq!(PluginId::new(id).to_string(), id.to_string());
        }
        for id in [0, u16::MAX] {
            assert_eq!(ClientId::new(id).to_string(), id.to_string());
        }
    }

    #[test]
    fn client_ids_check_narrowing_and_preserve_map_keys() {
        use std::collections::BTreeMap;

        for value in [0, u16::MAX] {
            let client = ClientId::new(value);
            assert_eq!(ClientId::try_from(u32::from(value)), Ok(client));
            assert_eq!(ClientId::try_from(i64::from(value)), Ok(client));
            let json = format!("{{\"{value}\":\"client\"}}");
            let map = BTreeMap::from([(client, "client".to_owned())]);
            assert_eq!(serde_json::to_string(&map).unwrap(), json);
            assert_eq!(
                serde_json::from_str::<BTreeMap<ClientId, String>>(&json).unwrap(),
                map
            );
        }
        assert!(ClientId::try_from(65536_u32).is_err());
        assert!(ClientId::try_from(u32::MAX).is_err());
        assert!(ClientId::try_from(-1_i64).is_err());
        assert!(ClientId::try_from(i64::MAX).is_err());
        assert_eq!(ClientId::default(), ClientId::new(0));
    }
    #[test]
    fn ids_preserve_json_numbers_and_map_keys() {
        use std::collections::BTreeMap;
        for value in [0, u32::MAX] {
            let terminal = TerminalId::new(value);
            assert_eq!(terminal.get(), value);
            let json = value.to_string();
            assert_eq!(serde_json::to_string(&terminal).unwrap(), json);
            assert_eq!(serde_json::from_str::<TerminalId>(&json).unwrap(), terminal);
            let plugin = PluginId::new(value);
            assert_eq!(plugin.get(), value);
            assert_eq!(serde_json::to_string(&plugin).unwrap(), json);
            assert_eq!(serde_json::from_str::<PluginId>(&json).unwrap(), plugin);
            let keys = BTreeMap::from([(terminal, "pane")]);
            let json = format!("{{\"{value}\":\"pane\"}}");
            assert_eq!(serde_json::to_string(&keys).unwrap(), json);
            assert_eq!(
                serde_json::from_str::<BTreeMap<TerminalId, String>>(&json).unwrap()[&terminal],
                "pane"
            );
        }
        for value in [0, u16::MAX] {
            let client = ClientId::new(value);
            assert_eq!(client.get(), value);
            let json = value.to_string();
            assert_eq!(serde_json::to_string(&client).unwrap(), json);
            assert_eq!(serde_json::from_str::<ClientId>(&json).unwrap(), client);
        }
        assert!(serde_json::from_str::<ClientId>("65536").is_err());
        assert!(serde_json::from_str::<TerminalId>("4294967296").is_err());
        assert!(serde_json::from_str::<PluginId>("-1").is_err());
    }
}
