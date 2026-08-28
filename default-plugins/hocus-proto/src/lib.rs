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

pub mod agents;
pub mod launch;
pub mod llm;
pub mod location;
pub mod panes;
pub mod paths;
pub mod projects;
pub mod store;
pub mod text;
pub mod todos;
pub mod wire;
