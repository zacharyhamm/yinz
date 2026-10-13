//! Client/server messages, wire encoding, and socket transport.
pub use yinz_runtime::consts::MAX_IPC_FRAME_LEN;

pub mod client_server_contract;
pub mod consts;
#[cfg(not(target_family = "wasm"))]
pub mod ipc;
#[cfg(not(target_family = "wasm"))]
mod mode_info;
#[cfg(not(target_family = "wasm"))]
pub mod nested_session;
pub mod nested_session_contract;
#[cfg(not(target_family = "wasm"))]
pub mod probe;
#[cfg(not(target_family = "wasm"))]
pub use mode_info::get_mode_info;

#[cfg(unix)]
pub mod session_ownership;
