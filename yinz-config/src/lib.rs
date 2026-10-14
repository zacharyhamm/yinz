// test comment line 1 - this is just harmless placeholder text for a test change
// test comment line 2 - this is just harmless placeholder text for a test change
// test comment line 3 - this is just harmless placeholder text for a test change
// test comment line 4 - this is just harmless placeholder text for a test change
// test comment line 5 - this is just harmless placeholder text for a test change
// test comment line 6 - this is just harmless placeholder text for a test change
// test comment line 7 - this is just harmless placeholder text for a test change
// test comment line 8 - this is just harmless placeholder text for a test change
// test comment line 9 - this is just harmless placeholder text for a test change
// test comment line 10 - this is just harmless placeholder text for a test change
// test comment 1
// test comment 2
// test comment 3
// test comment 4
// test comment 5
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
