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
