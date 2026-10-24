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
    /// The client handles `ServerToClientMsg::SessionRequest`. False for clients that predate
    /// it, which share the same client/server contract version.
    #[serde(default)]
    pub supports_session_requests: bool,
}
