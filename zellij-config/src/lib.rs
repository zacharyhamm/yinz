//! Configuration, layouts, themes, and plugin asset loading.
pub mod assets;
pub mod cli_assets;
pub mod config;
pub mod consts;
pub mod kdl;
pub mod layout;
pub mod permission;
pub mod plugins;
pub mod project_config;
pub mod theme;

#[cfg(test)]
mod keybinds;
#[cfg(not(target_family = "wasm"))]
mod lua_config;

pub mod project_layout;
