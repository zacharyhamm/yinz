//! Typed, single-request administration through the selected session.
use crate::plugin_data::{FetchRequest, Mutation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DataRequest {
    Catalog {
        namespace: Option<String>,
    },
    Fetch(FetchRequest),
    Count(FetchRequest),
    Apply {
        namespace: String,
        mutations: Vec<Mutation>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginOperation {
    Start,
    Reload,
    Stop,
    Inspect,
    List,
}

/// The CLI supplies its freshly read declarations as identity-selection candidates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginRequest {
    pub operation: PluginOperation,
    pub target: Option<String>,
    pub configuration: Option<std::collections::BTreeMap<String, String>>,
    pub fallback: Option<crate::input::layout::RunPlugin>,
    pub candidates: Vec<crate::input::layout::RunPlugin>,
    pub caller: Option<crate::internal_ids::TerminalId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginState {
    Loading,
    AwaitingPermission,
    Running,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginStatus {
    pub id: crate::internal_ids::PluginId,
    pub location: String,
    pub configuration: std::collections::BTreeMap<String, String>,
    pub background: bool,
    pub shared: bool,
    pub clients: Vec<crate::internal_ids::ClientId>,
    pub state: PluginState,
    pub pending_permissions: Vec<crate::data::PermissionType>,
    pub latest_error: Option<String>,
    /// Whether the instance has a pane; `None` while its declaration is not yet known.
    #[serde(default)]
    pub pane: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginResponse {
    pub plugins: Vec<PluginStatus>,
}
