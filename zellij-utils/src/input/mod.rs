pub mod actions;
pub mod cli_assets;
pub mod command;
pub mod config;
pub mod context_menu;
pub mod hooks;
pub mod keybinds;
pub mod layout;
#[cfg(not(target_family = "wasm"))]
mod lua_config;
pub mod options;
pub mod permission;
pub mod plugins;
pub mod theme;

#[cfg(not(target_family = "wasm"))]
pub use not_wasm::*;

#[cfg(not(target_family = "wasm"))]
mod not_wasm {
    use crate::{
        data::{InputMode, ModeInfo, PluginCapabilities},
        envs,
        ipc::ClientAttributes,
    };

    use super::keybinds::Keybinds;

    /// Creates a [`ModeInfo`] struct indicating the current [`InputMode`] and its keybinds
    /// (as pairs of [`String`]s).
    pub fn get_mode_info(
        mode: InputMode,
        attributes: &ClientAttributes,
        capabilities: PluginCapabilities,
        keybinds: &Keybinds,
        base_mode: Option<InputMode>,
    ) -> ModeInfo {
        let keybinds = keybinds.to_keybinds_vec();
        let session_name = envs::get_session_name().ok();

        ModeInfo {
            mode,
            base_mode,
            keybinds,
            style: attributes.style,
            capabilities,
            session_name,
            editor: None,
            shell: None,
            currently_marking_pane_group: None,
            pane_frame_style: None,
            session_dimmed: None,
            session_ancestry: vec![],
            host_fullscreen: None,
            nested_ascend_keys: vec![],
            session_ascended: None,
            nested_descend_keys: vec![],
        }
    }
}

pub use zellij_runtime::shared::expand_path;
