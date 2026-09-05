pub mod agent;
pub mod animation;
pub mod cli;
pub mod client_server_contract;
pub mod consts;
pub mod data;
pub mod envs;
pub mod errors;
pub mod home;
mod home_unix;

#[cfg(target_os = "windows")]
compile_error!("Zellij does not support Windows");
pub mod input;
pub mod kdl;
pub mod kitty_placeholders;
pub mod nested_session_contract;
pub mod pane_size;
pub mod plugin_api;
pub mod plugin_authoring;
pub mod plugin_data;
pub mod position;
pub mod project_config;
pub mod session_serialization;
pub mod setup;
pub mod shared;
pub mod warped_tab;

// The following modules can't be used when targeting wasm
#[cfg(not(target_family = "wasm"))]
pub mod channels; // Requires tokio
#[cfg(not(target_family = "wasm"))]
pub mod common_path;
#[cfg(not(target_family = "wasm"))]
pub mod data_access;
#[cfg(not(target_family = "wasm"))]
pub mod downloader; // Requires tokio
#[cfg(not(target_family = "wasm"))]
pub mod ipc; // Requires interprocess
#[cfg(not(target_family = "wasm"))]
pub mod logging; // Requires log4rs
#[cfg(not(target_family = "wasm"))]
pub mod nested_session;
#[cfg(not(target_family = "wasm"))]
pub mod plugin_data_store; // Requires rusqlite
#[cfg(not(target_family = "wasm"))]
pub mod session_checkpoint;
#[cfg(not(target_family = "wasm"))]
pub mod session_recording;
#[cfg(not(target_family = "wasm"))]
pub mod sessions;

// TODO(hartan): Remove this re-export for the next minor release.
pub use ::prost;

// Vendored libraries
pub mod vendored;
