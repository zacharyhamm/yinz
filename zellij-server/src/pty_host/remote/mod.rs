//! Panes whose process runs on another machine.
//!
//! The shape, and why it is this shape: `SCM_RIGHTS` cannot cross an ssh
//! forward. ssh relays payload bytes and drops ancillary data, so the descriptor
//! handoff in `pty_host::fdpass` — which is how a pane's pty master reaches the
//! server today — has no remote analogue. No ssh option changes that.
//!
//! So a **broker** runs on the remote machine and does locally what the server
//! would have done: it calls `pty_host::client::spawn`, receives the master over
//! `SCM_RIGHTS` *on that machine*, and then pumps that descriptor to and from
//! its socket as frames. The supervisor, the fd handoff, and the whole
//! `Terminate`/`Release`/`Query` lifecycle are untouched by any of this — from
//! `host.rs`'s point of view the broker simply is the server.
//!
//! The transport is an ssh-forwarded unix socket (`ssh -L /local:/remote`).
//! That also settles multiplexing without a multiplexer: each pane opens its own
//! connection to the forwarded socket, ssh opens a channel per connection, and
//! the broker accepts one connection per pane.

pub mod broker;
pub mod conn;
pub mod protocol;
pub mod reader;
pub mod session;
pub mod ssh;

pub use conn::{RemoteConn, RemoteStatus};
pub use reader::RemoteReader;
