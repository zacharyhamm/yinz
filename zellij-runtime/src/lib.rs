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
#[cfg(not(target_family = "wasm"))]
pub mod session_checkpoint;
pub mod shared;
pub mod vendored;

#[cfg(not(target_family = "wasm"))]
pub mod keys;

#[cfg(not(target_family = "wasm"))]
pub mod host_terminal_env;

#[cfg(unix)]
pub mod termios;
