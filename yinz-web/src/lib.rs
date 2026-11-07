//! The mobile web interface: an HTTP/1 server on hyper that serves a session's agents to a
//! phone, behind `tailscale serve`.
//!
//! The crate knows nothing about the bus or the plugin runtime: `yinz-server` connects it to
//! the plugin thread through [`Backend`]. Authentication ([`auth`]) runs before any request
//! reaches the plugin, and public pages and assets are served natively from the frozen
//! [`yinz_types::web::Site`].
//!
//! Module declarations and re-exports only.

pub mod auth;
mod listener;
pub mod listener_file;
mod pages;
mod policy;
mod request;
pub mod server;
mod service;
mod site;

pub use server::{Backend, DispatchFuture, WebServer, WebSettings, WebStatus};
