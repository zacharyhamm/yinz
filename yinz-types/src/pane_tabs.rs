//! Fixed plugin compositions. Container identity is independent of the active plugin.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    internal_ids::{ClientId, PluginId},
    pane_id::PaneId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PaneTabsId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaneTabDescriptor {
    pub label: String,
    pub location: String,
    #[serde(default)]
    pub configuration: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_cwd: Option<PathBuf>,
}

impl PaneTabDescriptor {
    /// Resolve aliases before identifying or loading companion plugins.
    pub fn populate_plugin_alias(&mut self, aliases: &crate::input::plugins::PluginAliases) {
        if let Some(plugin) = aliases.aliases.get(&self.location) {
            let mut plugin = plugin
                .clone()
                .merge_configuration(&Some(self.configuration.clone()));
            if self.initial_cwd.is_some() {
                plugin.initial_cwd = self.initial_cwd.clone();
            }
            self.location = plugin.location_for_serialization();
            self.configuration = plugin.configuration.inner().clone();
            self.initial_cwd = plugin.initial_cwd;
        }
    }

    pub fn add_cwd(&mut self, cwd: &Path) {
        self.initial_cwd = Some(match &self.initial_cwd {
            Some(child) => cwd.join(child),
            None => cwd.to_path_buf(),
        });
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PaneTabLoadState {
    #[default]
    Unloaded,
    Loading,
    PermissionPending,
    Ready,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaneTabInfo {
    pub descriptor: PaneTabDescriptor,
    pub plugin_id: Option<PluginId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_id: Option<crate::internal_ids::TerminalId>,
    pub state: PaneTabLoadState,
}

impl PaneTabInfo {
    pub fn pane_id(&self) -> Option<PaneId> {
        self.terminal_id
            .map(PaneId::Terminal)
            .or_else(|| self.plugin_id.map(PaneId::Plugin))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaneTabsInfo {
    /// This container is the session's configured overlay group.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub overlay_group: bool,
    pub id: PaneTabsId,
    pub pane_id: PaneId,
    pub tabs: Vec<PaneTabInfo>,
    pub active: usize,
    pub bar_focused: Vec<ClientId>,
}

/// Persist descriptors and selection, never runtime identities or guest memory.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaneTabsLayout {
    pub tabs: Vec<PaneTabDescriptor>,
    #[serde(default)]
    pub active: usize,
}

impl PaneTabsLayout {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.tabs.is_empty() || self.active >= self.tabs.len() {
            return Err("pane tabs require a nonempty list and a valid selected index");
        }
        if self
            .tabs
            .iter()
            .any(|tab| tab.label.trim().is_empty() || tab.location.trim().is_empty())
        {
            return Err("every pane tab requires a label and plugin location");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneTabsCommand {
    Mount {
        pane_id: PaneId,
        layout: PaneTabsLayout,
        existing: usize,
    },
    Select {
        id: PaneTabsId,
        index: usize,
    },
    Focus {
        id: PaneTabsId,
        bar: bool,
    },
    Retry {
        id: PaneTabsId,
        index: usize,
    },
    CloseChild {
        id: PaneTabsId,
        index: usize,
    },
    Close {
        id: PaneTabsId,
    },
}

pub type PaneTabsResponse = Result<Option<PaneTabsInfo>, String>;
