use crate::{data::LayoutInfo, input::options::Options, pane_size::Size};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CliAssets {
    pub config_file_path: Option<PathBuf>,
    pub config_dir: Option<PathBuf>,
    pub should_ignore_config: bool,
    pub configuration_options: Option<Options>, // merged from everywhere: there are the source of truth
    pub layout: Option<LayoutInfo>,
    pub terminal_window_size: Size,
    pub data_dir: Option<PathBuf>,
    pub is_debug: bool,
    pub max_panes: Option<usize>,
    pub force_run_layout_commands: bool,
    pub cwd: Option<PathBuf>,
    /// Host terminal variables captured by the client runtime.
    pub host_terminal_env: BTreeMap<String, String>,
    /// Timezone captured on the client; absent for older clients.
    #[serde(default)]
    pub timezone: Option<String>,
    /// The client handles `ServerToClientMsg::SessionRequest`. False for clients that predate
    /// it, which share the same client/server contract version.
    #[serde(default)]
    pub supports_session_requests: bool,
    /// Client update requests are sent only after this independent capability is advertised.
    #[serde(default)]
    pub supports_in_place_update: bool,
    /// Source artifact transfers and running-image identity are supported by this client.
    #[serde(default)]
    pub supports_source_update: bool,
    /// Private replacement server bootstrap; ordinary clients must leave this absent.
    #[serde(default)]
    pub update_target: Option<String>,
    #[serde(default)]
    pub update_source_session: Option<crate::internal_ids::SessionName>,
    /// Restores server-side client configuration before the first replacement render.
    #[serde(default)]
    pub update_resume: Option<(String, crate::data::ClientId)>,
    /// Effective configuration for a private replacement without changing the user's file path.
    #[serde(default)]
    pub update_configuration: Option<String>,
    /// Serialized effective layout for private replacement bootstrap, including tab templates.
    #[serde(default)]
    pub update_layout: Option<String>,
    /// File-sourced themes for `update_configuration`, which omits them; see
    /// `yinz_config::cli_assets::file_themes_to_string`. Older sources leave this absent.
    #[serde(default)]
    pub update_themes: Option<String>,
}
