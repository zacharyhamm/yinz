//! Canonical identities shared across crate boundaries.

use serde::{Deserialize, Serialize};
use std::fmt;

/// A terminal identity.
///
/// ```compile_fail
/// use zellij_types::internal_ids::{TerminalId, PluginId};
/// let _: TerminalId = PluginId(0);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
#[repr(transparent)]
pub struct TerminalId(pub u32);

impl TerminalId {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }
    pub const fn get(self) -> u32 {
        self.0
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
/// let _: PluginId = ClientId(0);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
#[repr(transparent)]
pub struct PluginId(pub u32);

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
/// let _: ClientId = TerminalId(0);
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
pub struct ClientId(pub u16);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_display_as_plain_numbers() {
        for id in [0, u32::MAX] {
            assert_eq!(TerminalId(id).to_string(), id.to_string());
            assert_eq!(PluginId(id).to_string(), id.to_string());
        }
        for id in [0, u16::MAX] {
            assert_eq!(ClientId(id).to_string(), id.to_string());
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
