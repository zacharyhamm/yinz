//! The wire contract between the web server and its clients.
//!
//! Both ends live in this crate: `web_client` serves the protocol, while
//! `remote_attach` and `remote_terminal_loop` speak it as a client (and so does
//! the browser, via `assets/*.js`). The types and paths live here rather than in
//! either end so a field or a route can only be changed in one place.

pub mod control_message;
pub mod endpoints;
pub mod http;

#[cfg(test)]
#[path = "./unit/web_protocol_tests.rs"]
mod web_protocol_tests;
