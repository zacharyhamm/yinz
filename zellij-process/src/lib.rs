//! Terminal process supervision, local and remote transport, and VM providers.
pub mod command;
pub mod firecracker;
pub mod pty_host;

use async_trait::async_trait;
use std::io;

// async fn in traits is not supported by rust, so dtolnay's excellent async_trait macro is being
// used. See https://smallcultfollowing.com/babysteps/blog/2019/10/26/async-fn-in-traits-are-hard/
#[async_trait]
pub trait AsyncReader: Send + Sync {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error>;
}
