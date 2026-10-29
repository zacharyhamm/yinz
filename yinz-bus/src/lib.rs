//! The yinz server's thread bus.
//!
//! Every server thread owns one instruction enum and sends the others' through
//! [`thread_bus::ThreadSenders`]. This crate holds those enums, the payload types they carry, the
//! bus itself, and the code that only ever talks over it: warps, background jobs,
//! the OS boundary's contract ([`os_input_output::ServerOsApi`], which
//! `yinz-server-os` implements). Nothing here names a tab, a pane, a screen, or the plugin
//! runtime.

#[cfg(test)]
#[path = "./unit/instruction_context_tests.rs"]
mod instruction_context_tests;

#[cfg(test)]
#[path = "./unit/pty_exit_tests.rs"]
mod pty_exit_tests;

pub mod background_jobs;
pub mod frame_color_override;
pub mod global_async_runtime;
pub mod loading_indication;
pub mod nested_guest;
pub mod os_input_output;
pub mod plugins;
pub mod pty;
pub mod pty_writer;
pub mod route;
pub mod screen;
pub mod server_instruction;
pub mod session_layout_metadata;
pub mod thread_bus;
pub mod warp;

pub use server_instruction::ServerInstruction;

// Moved files keep their `crate::` paths to shared items through these re-exports, mirroring
// what `yinz-server` and `yinz-terminal` do at their roots.
pub use yinz_host_protocol::host_query;
pub use yinz_runtime::channels;
pub use yinz_types::internal_ids::{ClientId, PluginId, TerminalId};
pub use yinz_types::pane_id::PaneId;

pub mod command_palette;
