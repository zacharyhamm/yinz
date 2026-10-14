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
pub mod envs;
pub mod errors;
pub mod home;
mod home_unix;
pub mod host_identity;
#[cfg(not(target_family = "wasm"))]
pub mod last_project;
#[cfg(not(target_family = "wasm"))]
pub mod logging;
#[cfg(not(target_family = "wasm"))]
pub mod own_binary;
#[cfg(unix)]
pub mod owned_dir;
#[cfg(not(target_family = "wasm"))]
pub mod session_checkpoint;
pub mod shared;

#[cfg(not(target_family = "wasm"))]
pub mod host_terminal_env;

pub mod agent;
#[cfg(unix)]
pub mod termios;

// Its own crate so the plugin wire API can parse plugin URLs without depending on the host
// runtime; re-exported so the `yinz_runtime::plugin_location::` paths keep working.
pub use yinz_plugin_location as plugin_location;

#[cfg(target_os = "windows")]
compile_error!("Yinz does not support Windows");
