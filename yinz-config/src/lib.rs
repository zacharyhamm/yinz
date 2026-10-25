//! Configuration, layouts, themes, and plugin asset loading.
pub mod assets;
pub mod cli_assets;
pub mod config;
pub mod consts;
pub mod kdl;
pub mod layout;
pub mod permission;
pub mod plugins;
pub use yinz_project_config as project_config;
pub mod screen_config;
pub mod theme;

#[cfg(test)]
mod keybinds;

pub mod project_layout;

pub mod editor;
