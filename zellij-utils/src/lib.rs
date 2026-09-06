pub mod agent;
pub mod cli;
pub mod client_server_contract;
pub mod consts;
pub mod data;
pub use zellij_runtime::envs;
pub use zellij_runtime::errors;
pub use zellij_runtime::home;
pub use zellij_runtime::host_identity;

#[cfg(target_os = "windows")]
compile_error!("Zellij does not support Windows");
pub mod input;
pub use zellij_types::internal_ids;
pub mod kdl;
pub use zellij_types::kitty_placeholders;
pub mod nested_session_contract;
pub mod plugin_api;
pub mod plugin_authoring;
pub use zellij_types::plugin_data;
pub use zellij_types::position;
pub mod project_config;
pub mod session_serialization;
pub mod setup;
pub mod shared;
pub mod warped_tab;

// The following modules can't be used when targeting wasm
#[cfg(not(target_family = "wasm"))]
pub use zellij_runtime::channels; // Requires tokio
#[cfg(not(target_family = "wasm"))]
pub use zellij_runtime::common_path;
#[cfg(not(target_family = "wasm"))]
pub use zellij_runtime::downloader;
#[cfg(not(target_family = "wasm"))]
pub mod ipc; // Requires interprocess
#[cfg(not(target_family = "wasm"))]
pub use zellij_runtime::logging; // Requires log4rs
#[cfg(not(target_family = "wasm"))]
pub mod nested_session;
#[cfg(not(target_family = "wasm"))]
pub use zellij_runtime::session_checkpoint;
#[cfg(not(target_family = "wasm"))]
pub mod sessions;

// TODO(hartan): Remove this re-export for the next minor release.
pub use ::prost;

// Vendored libraries
pub use zellij_runtime::vendored;

#[cfg(all(test, not(target_family = "wasm")))]
#[path = "unit/session_checkpoint_tests.rs"]
mod session_checkpoint_tests;
