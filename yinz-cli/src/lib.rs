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
//! Command line arguments, action conversion, setup, and presentation.
pub mod actions;
pub mod agent_archive;
pub mod args;
#[cfg(test)]
#[path = "unit/cross_contracts.rs"]
mod cross_contracts;
#[cfg(test)]
#[path = "unit/kdl_action_props.rs"]
mod kdl_action_props;
pub mod options;
#[cfg(test)]
#[path = "unit/options.rs"]
mod options_tests;
#[cfg(test)]
#[path = "unit/repl.rs"]
mod repl_tests;
#[cfg(test)]
#[path = "unit/session_checkpoint_tests.rs"]
mod session_checkpoint_tests;
pub mod session_picker;
#[cfg(test)]
#[path = "unit/session_picker_tests.rs"]
mod session_picker_tests;
pub mod sessions;
pub mod setup;

pub mod notes_todos;

pub mod operations;

#[cfg(test)]
#[path = "unit/plugin_configuration.rs"]
mod plugin_configuration;

#[cfg(test)]
#[path = "unit/tab_groups.rs"]
mod tab_groups;
