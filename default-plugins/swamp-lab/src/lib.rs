//! A client for the swamp-club Lab API (`/api/v1/lab/*`).
//!
//! Nothing here performs I/O. [`api::Client`] turns a call into an
//! [`http::Request`], the caller sends it however it can (a plugin with
//! `web_request`, a test with a socket), and the matching `parse` turns the
//! status, headers and body back into a typed value or an [`http::Error`].
//!
//! The contract is swamp-club's `routes/api/v1/lab/` and `lib/domain/lab/`; the
//! server publishes no spec for it. Credentials resolve the way the `swamp` CLI
//! resolves them ([`auth`]), so a `swamp auth login` serves both.

pub mod api;
pub mod auth;
pub mod http;
pub mod redact;
pub mod sse;
pub mod types;

pub use api::Client;
pub use auth::Credentials;
pub use http::{parse, parse_empty, Error, Method, Request};
