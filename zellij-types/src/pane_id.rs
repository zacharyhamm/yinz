//! A pane identity shared by the host and plugins.

use crate::internal_ids::{PluginId, TerminalId};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// A terminal and a plugin with the same number are distinct identities.
///
/// ```compile_fail
/// use zellij_types::{pane_id::PaneId, internal_ids::PluginId};
/// let _ = PaneId::Terminal(PluginId::new(0));
/// ```
///
/// ```compile_fail
/// use zellij_types::{pane_id::PaneId, internal_ids::ClientId};
/// let _ = PaneId::Plugin(ClientId::new(0));
/// ```
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PaneId {
    Terminal(TerminalId),
    Plugin(PluginId),
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
mod tests {
    use super::*;

    #[test]
    fn pane_ids_preserve_json_display_and_parsing() {
        assert_eq!(PaneId::default(), PaneId::Terminal(TerminalId::new(0)));
        for value in [0, 1, u32::MAX] {
            for (id, display, json) in [
                (
                    PaneId::Terminal(TerminalId::new(value)),
                    format!("terminal_{value}"),
                    format!("{{\"Terminal\":{value}}}"),
                ),
                (
                    PaneId::Plugin(PluginId::new(value)),
                    format!("plugin_{value}"),
                    format!("{{\"Plugin\":{value}}}"),
                ),
            ] {
                assert_eq!(id.to_string(), display);
                assert_eq!(display.parse::<PaneId>().unwrap(), id);
                assert_eq!(serde_json::to_string(&id).unwrap(), json);
                assert_eq!(serde_json::from_str::<PaneId>(&json).unwrap(), id);
            }
            assert_eq!(
                value.to_string().parse::<PaneId>().unwrap(),
                PaneId::Terminal(TerminalId::new(value))
            );
        }
        for invalid in [
            "",
            "-1",
            "terminal_-1",
            "plugin_4294967296",
            "4294967296",
            "pane_0",
        ] {
            assert!(invalid.parse::<PaneId>().is_err(), "{invalid}");
        }
        for invalid in [r#"{"Terminal":-1}"#, r#"{"Plugin":4294967296}"#] {
            assert!(serde_json::from_str::<PaneId>(invalid).is_err());
        }
    }
}
