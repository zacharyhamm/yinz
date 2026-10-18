use crate::{
    input::layout::{Layout, Run},
    pane_size::PaneGeom,
};
use std::path::PathBuf;
#[derive(Default, Debug, Clone)]
pub struct GlobalLayoutManifest {
    pub global_cwd: Option<PathBuf>,
    pub default_shell: Option<PathBuf>,
    pub default_layout: Box<Layout>,
    pub tabs: Vec<(String, TabLayoutManifest)>,
}

#[derive(Default, Debug, Clone)]
pub struct TabLayoutManifest {
    pub tiled_panes: Vec<PaneLayoutManifest>,
    pub floating_panes: Vec<PaneLayoutManifest>,
    pub is_focused: bool,
    pub hide_floating_panes: bool,
    /// Whether the user chose the tab's name, so that it comes back holding off the project.
    pub user_named: bool,
    pub tab_group: Option<crate::tab_bar::TabGroupInfo>,
    pub group_focused: bool,
}

#[derive(Default, Debug, Clone)]
pub struct PaneLayoutManifest {
    pub pane_tabs: Option<crate::pane_tabs::PaneTabsLayout>,
    pub geom: PaneGeom,
    pub run: Option<Run>,
    pub cwd: Option<PathBuf>,
    pub is_borderless: bool,
    pub title: Option<String>,
    pub is_focused: bool,
    pub pane_contents: Option<String>,
    pub default_fg: Option<String>,
    pub default_bg: Option<String>,
}
