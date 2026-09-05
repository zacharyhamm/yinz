//! IDs used inside the host. Public plugin and wire IDs remain primitive numbers.

use std::fmt;

/// An internal terminal identity.
///
/// ```compile_fail
/// use zellij_utils::internal_ids::{TerminalId, PluginId};
/// let _: TerminalId = PluginId(0);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TerminalId(pub u32);

impl fmt::Display for TerminalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// An internal plugin identity.
///
/// ```compile_fail
/// use zellij_utils::internal_ids::{PluginId, ClientId};
/// let _: PluginId = ClientId(0);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PluginId(pub u32);

impl fmt::Display for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// An internal client identity.
///
/// ```compile_fail
/// use zellij_utils::internal_ids::{ClientId, TerminalId};
/// let _: ClientId = TerminalId(0);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClientId(pub u16);

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
}
