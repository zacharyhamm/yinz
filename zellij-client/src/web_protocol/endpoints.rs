//! The web server's routes, named once.
//!
//! The server registers these with axum and the remote client builds its URLs
//! from them, so a renamed route breaks at compile time rather than at runtime.

pub const ROOT: &str = "/";
pub const LOGIN: &str = "/command/login";
pub const SESSION: &str = "/session";
pub const SESSION_LIST: &str = "/session-list";
pub const WS_TERMINAL: &str = "/ws/terminal";
pub const WS_CONTROL: &str = "/ws/control";
pub const VERSION: &str = "/info/version";

// Route patterns, server-side only: the `{...}` segments are axum syntax, not
// something a client can request verbatim. `concat!` only takes literals, so
// `WS_TERMINAL_WITH_SESSION` restates its prefix - a unit test keeps the two in
// agreement.
pub const WS_TERMINAL_WITH_SESSION: &str = concat!("/ws/terminal", "/{session}");
pub const SESSION_PAGE: &str = "/{session}";
pub const STATIC_ASSETS: &str = "/assets/{*path}";
