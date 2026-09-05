use zellij_utils::data::PaneId as ZellijUtilsPaneId;

#[derive(PartialEq, Eq, Ord, PartialOrd, Hash, Clone, Copy, Debug)]
pub enum PaneId {
    Terminal(u32),
    Plugin(u32), // FIXME: Drop the trait object, make this a wrapper for the struct?
}

// because crate architecture and reasons...
impl From<ZellijUtilsPaneId> for PaneId {
    fn from(zellij_utils_pane_id: ZellijUtilsPaneId) -> Self {
        match zellij_utils_pane_id {
            ZellijUtilsPaneId::Terminal(id) => PaneId::Terminal(id),
            ZellijUtilsPaneId::Plugin(id) => PaneId::Plugin(id),
        }
    }
}

#[allow(clippy::from_over_into)]
impl Into<ZellijUtilsPaneId> for PaneId {
    fn into(self) -> ZellijUtilsPaneId {
        match self {
            PaneId::Terminal(id) => ZellijUtilsPaneId::Terminal(id),
            PaneId::Plugin(id) => ZellijUtilsPaneId::Plugin(id),
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
