//! Pure logic and wire types shared by the hocus plugins.
//!
//! All of it tests on the host triple without the wasm dance — and the plugins
//! that link it (`hocus`, `hocus-agents`, `hocus-agentd`,
//! `hocus-new-agent`, `hocus-store`) cannot
//! drift apart on a message shape, or on a stored schema, which would otherwise
//! show up only as a silent deserialize failure at runtime.
//!
//! [`store`] and [`launch`] are the two modules that reach the host, and both are
//! arranged so that this stays true: the shim is behind a `wasm32`-only item, with a
//! host-triple stub that fails loudly beside it, and everything either module decides
//! is in the pure half that the linking crates' own tests exercise. For [`store`] that
//! pure half runs against the real store engine over a temporary file.

/// The session's reading of a Claude Code hook event, for plugins that link only this crate.
pub use zellij_tile::agent;
pub mod agents;
pub mod completion;
pub mod decisions;
pub mod envelope;
pub mod jobs;
pub mod launch;
pub mod llm;
pub mod location;
pub mod markdown;
pub mod notes;
pub mod panes;
pub mod paths;
pub mod projects;
pub mod run_history;
pub mod store;
pub mod swamp;
pub mod tabs;
pub mod text;
pub mod todos;
pub mod wire;

pub mod vms;

pub mod panel_text;
