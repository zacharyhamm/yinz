//! The escape-sequence traffic that crosses the boundary between a pane and the host terminal.
//!
//! [`host_query`] classifies the host-terminal queries an app inside a pane asks and Yinz
//! forwards or answers from cache. [`notifications`] covers desktop notifications: what an app
//! asked for (OSC 9, 99 and 777), how an OSC 99 identifier is namespaced per pane on the way out
//! and restored on the host's answer, and how a notification is rendered for each host.
//! [`nested_session`] carries control frames between nested Yinz sessions.
//!
//! The terminal emulator produces these types, the thread bus carries them and the screen acts
//! on them. Keeping them here lets the bus build without the emulator.

pub mod host_query;
pub mod nested_session;
pub mod nested_session_contract;
pub mod notifications;
