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

#[cfg(test)]
#[path = "unit/wire_tests.rs"]
mod wire_tests;

#[cfg(test)]
#[path = "unit/retired_targets.rs"]
mod retired_targets;
