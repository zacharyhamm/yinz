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
//! Shared models, without host facilities or configuration parsers.
// test comment

pub mod animation;
pub mod cli;
pub mod colors;
pub mod compact_bytes;
pub mod diagnostics;
pub mod envs;
pub mod internal_ids;
pub mod kitty_placeholders;
pub mod plugin_data;
pub mod position;
pub mod text;
pub mod text_input;
pub mod ui_components;

pub mod keys;

pub mod style;

pub mod pane_id;

pub mod mouse;
pub mod multiplayer;
pub mod notifications;

pub mod geometry;
pub mod multipane;
pub mod pane_size;
pub mod pane_tabs;
pub mod tab_bar;

pub mod agent;
pub mod command;
pub mod command_palette;

#[cfg(test)]
#[path = "unit/model_contracts.rs"]
mod model_contracts;

pub mod units;

pub mod data;
pub mod input;

pub mod session_manifest;

pub mod authoring;

pub mod secrets;

pub mod pick;

pub mod configuration;

pub mod project_provider;
