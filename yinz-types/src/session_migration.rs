//! Versioned whole-session handoff. Release names do not define this contract.
use crate::{
    data::WarpedTab,
    internal_ids::{ClientId, SessionName, TabId},
    pane_id::PaneId,
};
use serde::{Deserialize, Serialize};

pub const SESSION_MIGRATION_FORMAT: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientFocus {
    pub client_id: ClientId,
    pub tab_id: Option<TabId>,
    pub pane_id: Option<PaneId>,
    pub tab_history: Vec<TabId>,
    pub pane_history: Vec<PaneId>,
    pub mode_info: crate::data::ModeInfo,
    pub marked_panes: Vec<PaneId>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TabInventory {
    pub tab_id: TabId,
    pub title: String,
    /// Layout slot order, including plugin and hidden companion slots.
    pub panes: Vec<PaneId>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionMigrationInventory {
    pub format: u32,
    pub session_name: SessionName,
    pub instance_id: String,
    pub tabs: Vec<TabInventory>,
    pub clients: Vec<ClientFocus>,
    pub remote_hosts: Vec<String>,
    pub global_last_active_tab: TabId,
    pub tab_group_preferences: Vec<(u32, Option<TabId>)>,
    pub paneless_plugins: Vec<crate::input::layout::RunPluginOrAlias>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MigrationTab {
    pub source_tab_id: TabId,
    pub slots: Vec<PaneId>,
    pub payload: WarpedTab,
    /// Versioned terminal state keyed by original pane identity. Kept outside
    /// ordinary warp payloads so old peers never receive an unsupported format.
    pub terminal_states: Vec<(PaneId, serde_json::Value)>,
    /// Local supervisor attachment consumes its detached output ring. Retain
    /// those bytes until commit so rollback can replay them from the original
    /// emulator snapshot even after multiple failed destination attempts.
    pub local_replay: Vec<(PaneId, Vec<u8>)>,
    pub state: TabState,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum MigrationPhase {
    Prepared,
    Releasing,
    Released,
    Adopting,
    Adopted,
    Recovery,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionMigrationSnapshot {
    pub format: u32,
    pub operation: String,
    pub inventory: SessionMigrationInventory,
    pub phase: MigrationPhase,
    pub tabs: Vec<MigrationTab>,
    /// Acknowledged ownership changes. Absent entries are never assumed adopted.
    pub mapping: SessionMigrationMap,
    pub errors: Vec<String>,
    pub ownership: Vec<(PaneId, MigrationOwnership)>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum MigrationOwnership {
    Source,
    Releasing,
    Released,
    Adopting,
    Destination {
        terminal_id: crate::internal_ids::TerminalId,
    },
    Uncertain,
}

impl SessionMigrationSnapshot {
    pub fn record_ownership(&mut self, pane: PaneId, owner: MigrationOwnership) {
        if let Some((_, current)) = self.ownership.iter_mut().find(|(id, _)| *id == pane) {
            *current = owner;
        } else {
            self.ownership.push((pane, owner));
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionMigrationMap {
    pub tabs: Vec<(TabId, TabId)>,
    pub panes: Vec<(PaneId, PaneId)>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TabState {
    pub previous_name: String,
    pub default_name: String,
    pub previous_user_named: bool,
    pub synchronize: bool,
    pub auto_layout: bool,
    pub focus_pane: Option<PaneId>,
    pub departed_focus: [Option<PaneId>; 3],
    pub fullscreen: Option<PaneId>,
    pub fullscreen_covers_ui: bool,
    pub agent_worktrees: Vec<(PaneId, std::path::PathBuf)>,
    pub swap: SwapState,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SwapState {
    // Map keys contain payloads (ExactPanes), so use arrays on the JSON wire.
    pub tiled: SwapVariants<crate::input::layout::TiledPaneLayout>,
    pub floating: SwapVariants<Vec<crate::input::layout::FloatingPaneLayout>>,
    pub current_tiled: usize,
    pub current_floating: usize,
    pub tiled_damaged: bool,
    pub floating_damaged: bool,
}

pub type SwapVariants<Layout> = Vec<(
    Vec<(crate::input::layout::LayoutConstraint, Layout)>,
    Option<String>,
)>;
