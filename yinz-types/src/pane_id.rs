//! A pane identity shared by the host and plugins.

use crate::internal_ids::{PluginId, TerminalId};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// A terminal and a plugin with the same number are distinct identities.
///
/// ```compile_fail
/// use yinz_types::{pane_id::PaneId, internal_ids::PluginId};
/// let _ = PaneId::Terminal(PluginId::new(0));
/// ```
///
/// ```compile_fail
/// use yinz_types::{pane_id::PaneId, internal_ids::ClientId};
/// let _ = PaneId::Plugin(ClientId::new(0));
/// ```
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PaneId {
    Terminal(TerminalId),
    Plugin(PluginId),
}

impl PaneId {
    /// Decode the legacy numeric identity and plugin flag at a protocol boundary.
    pub const fn from_legacy(id: u32, is_plugin: bool) -> Self {
        if is_plugin {
            Self::Plugin(PluginId::new(id))
        } else {
            Self::Terminal(TerminalId::new(id))
        }
    }

    /// Encode the legacy numeric identity and plugin flag at a protocol boundary.
    pub const fn legacy_parts(self) -> (u32, bool) {
        match self {
            Self::Terminal(id) => (id.get(), false),
            Self::Plugin(id) => (id.get(), true),
        }
    }
}

/// Serde adapter for existing optional `(number, is_plugin)` selectors.
pub mod legacy_tuple {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        pane: &Option<PaneId>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        pane.map(PaneId::legacy_parts).serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<PaneId>, D::Error> {
        Option::<(u32, bool)>::deserialize(deserializer)
            .map(|pane| pane.map(|(id, is_plugin)| PaneId::from_legacy(id, is_plugin)))
    }
}

/// Serde adapter for the flattened identity fields in a pane manifest.
pub mod legacy_fields {
    use super::*;
    #[derive(Serialize, Deserialize)]
    struct Identity {
        id: u32,
        is_plugin: bool,
    }
    pub fn serialize<S: serde::Serializer>(
        pane: &PaneId,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let (id, is_plugin) = pane.legacy_parts();
        Identity { id, is_plugin }.serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<PaneId, D::Error> {
        Identity::deserialize(deserializer).map(|pane| PaneId::from_legacy(pane.id, pane.is_plugin))
    }
}

/// Serde adapter for the existing IPC pane reference object.
pub mod legacy_reference {
    use super::*;
    #[derive(Serialize, Deserialize)]
    struct Reference {
        pane_id: u32,
        is_plugin: bool,
    }
    pub fn serialize<S: serde::Serializer>(
        pane: &Option<PaneId>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        pane.map(|pane| {
            let (pane_id, is_plugin) = pane.legacy_parts();
            Reference { pane_id, is_plugin }
        })
        .serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<PaneId>, D::Error> {
        Option::<Reference>::deserialize(deserializer)
            .map(|pane| pane.map(|pane| PaneId::from_legacy(pane.pane_id, pane.is_plugin)))
    }
}

impl Default for PaneId {
    fn default() -> Self {
        PaneId::Terminal(TerminalId::new(0))
    }
}

impl FromStr for PaneId {
    type Err = Box<dyn std::error::Error>;
    fn from_str(stringified_pane_id: &str) -> Result<Self, Self::Err> {
        if let Some(terminal_stringified_pane_id) = stringified_pane_id.strip_prefix("terminal_") {
            terminal_stringified_pane_id
                .parse::<u32>()
                .map(|id| PaneId::Terminal(TerminalId::new(id)))
                .map_err(|e| e.into())
        } else if let Some(plugin_pane_id) = stringified_pane_id.strip_prefix("plugin_") {
            plugin_pane_id
                .parse::<u32>()
                .map(|id| PaneId::Plugin(PluginId::new(id)))
                .map_err(|e| e.into())
        } else {
            stringified_pane_id
                .parse::<u32>()
                .map(|id| PaneId::Terminal(TerminalId::new(id)))
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

#[cfg(test)]
#[path = "unit/pane_id_tests.rs"]
mod tests;
