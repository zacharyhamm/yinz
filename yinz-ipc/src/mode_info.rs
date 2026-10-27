use crate::ipc::ClientAttributes;
use yinz_runtime::envs;
use yinz_types::{
    data::{InputMode, ModeInfo, PluginCapabilities},
    input::keybinds::Keybinds,
};
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
        max_agent_width: None,
        tab_groups: None,
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
