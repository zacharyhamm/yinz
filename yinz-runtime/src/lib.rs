//! Environment, errors, and host runtime services.

pub mod channels;
pub mod client_time;
pub mod common_path;
pub mod consts;
pub mod envs;
pub mod errors;
pub mod home;
mod home_unix;
pub mod host_identity;
pub mod last_project;
pub mod logging;
pub mod own_binary;
#[cfg(unix)]
pub mod owned_dir;
pub mod session_checkpoint;
pub mod shared;

pub mod host_terminal_env;

pub mod agent;
#[cfg(unix)]
pub mod termios;

// Its own crate so the plugin wire API can parse plugin URLs without depending on the host
// runtime; re-exported so the `yinz_runtime::plugin_location::` paths keep working.
pub use yinz_plugin_location as plugin_location;

#[cfg(target_os = "windows")]
compile_error!("Yinz does not support Windows");

pub mod remote_hosts;
