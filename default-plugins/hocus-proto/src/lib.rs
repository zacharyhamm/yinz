//! Pure logic and wire types shared by the hocus plugins.
//!
//! Nothing here calls the host, so all of it tests on the host triple without
//! the wasm dance — and the plugins that link it (`hocus`, `hocus-agentd`,
//! `hocus-store`) cannot drift apart on a message shape, which would otherwise
//! show up only as a silent deserialize failure at runtime.

pub mod agents;
pub mod panes;
pub mod projects;
pub mod todos;
pub mod wire;
