//! Terminal process supervision, local and SSH transport.
pub use yinz_types::internal_ids::{ClientId, PluginId, TerminalId};

pub mod agent_theme;
pub mod claude_turn;
pub mod command;
pub mod pane_context;
pub mod pty_host;
pub mod ssh;

use async_trait::async_trait;
use std::io;

// async fn in traits is not supported by rust, so dtolnay's excellent async_trait macro is being
// used. See https://smallcultfollowing.com/babysteps/blog/2019/10/26/async-fn-in-traits-are-hard/
#[async_trait]
pub trait AsyncReader: Send + Sync {
    #[expect(
        clippy::double_must_use,
        reason = "async_trait adds must_use to an already must-use future"
    )]
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error>;
}
