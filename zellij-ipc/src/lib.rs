//! Client/server messages, wire encoding, and socket transport.
/// Maximum protobuf frame length, excluding its four-byte length prefix.
/// Available on wasm so plugins can check replies before handing them to the host.
pub const MAX_IPC_FRAME_LEN: usize = 64 * 1024 * 1024;

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
