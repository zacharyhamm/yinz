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
    /// Resolve panel aliases before identifying companions; custom plugins keep their descriptors.
    pub fn populate_plugin_alias(&mut self, aliases: &crate::input::plugins::PluginAliases) {
        if let Some(plugin) = aliases.aliases.get(&self.location) {
            if !matches!(
                plugin.location.display().as_str(),
                "yinz:agents" | "yinz:todos" | "yinz:vms" | "yinz:swamp-issues"
            ) {
                return;
            }
            let mut plugin = plugin
                .clone()
                .merge_configuration(&Some(self.configuration.clone()));
            if self.initial_cwd.is_some() {
                plugin.initial_cwd = self.initial_cwd.clone();
            }
            plugin.normalize_agent_panel();
            self.location = plugin.location.display();
            self.configuration = plugin.configuration.inner().clone();
            self.initial_cwd = plugin.initial_cwd;
        }
        self.normalize_agent_panel();
    }

    pub fn normalize_agent_panel(&mut self) {
        if let Some(location) =
            crate::input::layout::normalize_agent_panel(&self.location, &mut self.configuration)
        {
            self.location = location.into();
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
    pub state: PaneTabLoadState,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaneTabsInfo {
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
