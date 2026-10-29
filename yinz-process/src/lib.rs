//! Terminal process supervision, local and SSH transport.
pub use yinz_types::internal_ids::{ClientId, PluginId, TerminalId};

pub mod agent_theme;
pub mod claude_turn;
pub mod command;
pub mod pane_context;
pub mod pty_host;
pub mod ssh;

pub use yinz_process_api::AsyncReader;
