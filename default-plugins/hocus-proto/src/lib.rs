//! Pure logic and wire types shared by the hocus plugins.
//!
//! All of it tests on the host triple without the wasm dance — and the plugins
//! that link it (`hocus`, `hocus-agentd`, `hocus-store`) cannot drift apart on a
//! message shape, or on a stored schema, which would otherwise show up only as a
//! silent deserialize failure at runtime.
//!
//! [`store`] is the one module that reaches the host, and it is arranged so that
//! this stays true: the shims are behind a `wasm32`-only type, and everything
//! else — including the tests of the crates that link this one — goes through
//! the same trait backed by the real store engine over a temporary file.

pub mod agents;
pub mod panes;
pub mod paths;
pub mod projects;
pub mod store;
pub mod todos;
pub mod wire;
