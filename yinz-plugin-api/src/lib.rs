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
pub mod action;
pub mod command;
pub mod event;
pub mod file;
pub mod input_mode;
pub mod key;
pub mod message;
pub mod pane_frame_style;
pub mod pipe_message;
pub mod plugin_command;
pub mod plugin_data;
pub mod plugin_ids;
pub mod plugin_permission;
pub mod resize;
pub mod style;
#[cfg(any(test, feature = "test-support"))]
// IPC reuses the action generators; the plugin-only generators are used by this crate's tests.
#[cfg_attr(not(test), allow(dead_code))]
pub mod test_generators;
// prost writes these, `cargo x proto` regenerates them and `cargo x assets --check` compares
// them byte for byte - lint them as the generated artifacts they are
#[allow(
    clippy::module_inception,
    clippy::large_enum_variant,
    clippy::len_without_is_empty
)]
pub mod generated_api {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/generated_plugin_api.rs"
    ));
}

pub mod authoring;
pub use prost;
