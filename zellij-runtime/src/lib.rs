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
//! Environment, errors, and host runtime services.

#[cfg(not(target_family = "wasm"))]
pub mod channels;
#[cfg(not(target_family = "wasm"))]
pub mod common_path;
pub mod consts;
#[cfg(not(target_family = "wasm"))]
pub mod downloader;
pub mod envs;
pub mod errors;
pub mod home;
mod home_unix;
pub mod host_identity;
#[cfg(not(target_family = "wasm"))]
pub mod logging;
#[cfg(unix)]
pub mod owned_dir;
#[cfg(not(target_family = "wasm"))]
pub mod session_checkpoint;
pub mod shared;
pub mod vendored;

#[cfg(not(target_family = "wasm"))]
pub mod keys;

#[cfg(not(target_family = "wasm"))]
pub mod host_terminal_env;

pub mod agent;
#[cfg(unix)]
pub mod termios;

pub mod plugin_location;

#[cfg(target_os = "windows")]
compile_error!("Zellij does not support Windows");
