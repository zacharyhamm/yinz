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
//!
//! A pane outlives its connection: it lives in the broker's `PaneRegistry`, keyed
//! by a token, and connections attach to and detach from it. That is what lets a
//! dropped link be ridden out — and, for free, what lets a pane be handed to
//! another *session*. A warp is a `Resume` by a different server, which the
//! broker cannot distinguish from a reconnect and does not need to; the two
//! servers differ only in having built their own ssh forwards. The one addition
//! a warp needed was `Release`, because a handoff must be able to name the moment
//! the pane became free, and a closed socket cannot. See `warp_address` in
//! `screen.rs` for what travels, and `ARCHITECTURE.md` for why.

pub mod broker;
pub mod conn;
pub mod env;
pub mod protocol;
pub mod reader;
pub mod session;
pub mod ssh;
pub mod status;

pub use conn::{RemoteConn, RemoteStatus};
pub use reader::RemoteReader;
