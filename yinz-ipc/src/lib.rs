//! Client/server messages, wire encoding, and socket transport.
pub use yinz_runtime::consts::MAX_IPC_FRAME_LEN;

pub mod client_server_contract;
pub mod consts;
pub mod ipc;
mod mode_info;
pub mod nested_session;
pub mod nested_session_contract;
pub mod probe;
pub use mode_info::get_mode_info;

#[cfg(unix)]
pub mod session_ownership;
