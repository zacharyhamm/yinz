use crate::{PluginId, TerminalId};
use zellij_utils::data::PaneId as ZellijUtilsPaneId;

/// A pane identity with distinct terminal and plugin payloads.
///
/// ```compile_fail
/// use zellij_terminal::{PaneId, PluginId};
/// let _ = PaneId::Terminal(PluginId(0));
/// ```
///
/// ```compile_fail
/// use zellij_terminal::{PaneId, TerminalId};
/// let _ = PaneId::Plugin(TerminalId(0));
/// ```
///
/// ```compile_fail
/// use zellij_terminal::{ClientId, PaneId};
/// let _ = PaneId::Terminal(ClientId(0));
/// ```
#[derive(PartialEq, Eq, Ord, PartialOrd, Hash, Clone, Copy, Debug)]
pub enum PaneId {
    Terminal(TerminalId),
    Plugin(PluginId),
}

// because crate architecture and reasons...
impl From<ZellijUtilsPaneId> for PaneId {
    fn from(zellij_utils_pane_id: ZellijUtilsPaneId) -> Self {
        match zellij_utils_pane_id {
            ZellijUtilsPaneId::Terminal(id) => PaneId::Terminal(TerminalId(id)),
            ZellijUtilsPaneId::Plugin(id) => PaneId::Plugin(PluginId(id)),
        }
    }
}

impl From<PaneId> for ZellijUtilsPaneId {
    fn from(id: PaneId) -> Self {
        match id {
            PaneId::Terminal(id) => ZellijUtilsPaneId::Terminal(id.0),
            PaneId::Plugin(id) => ZellijUtilsPaneId::Plugin(id.0),
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
    fn public_conversion_and_display_preserve_numeric_ids() {
        for id in [0, 1, u32::MAX] {
            for (public, internal, display) in [
                (
                    ZellijUtilsPaneId::Terminal(id),
                    PaneId::Terminal(TerminalId(id)),
                    format!("terminal_{id}"),
                ),
                (
                    ZellijUtilsPaneId::Plugin(id),
                    PaneId::Plugin(PluginId(id)),
                    format!("plugin_{id}"),
                ),
            ] {
                assert_eq!(PaneId::from(public), internal);
                assert_eq!(ZellijUtilsPaneId::from(internal), public);
                assert_eq!(internal.to_string(), display);
            }
        }
    }
}
