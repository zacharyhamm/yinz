//! The agent plugins' persisted data: the store engine seam, the entity schemas, and the
//! project, todo and note registry built on them.
//!
//! Every agent plugin reaches these modules through `agent-proto`, which re-exports them
//! unchanged. The plugin host links this crate directly, because resolving a checkout's
//! project needs the registry and the shared namespace but nothing else agent-proto carries.
//!
//! [`store`] holds every entity definition, including the agent-run schemas
//! whose row codecs stay in agent-proto, so two declarers of one entity cannot disagree about
//! it. It is also the one module that reaches the host: the shim is behind a `wasm32`-only
//! item, with a host-triple stub that fails loudly beside it, and everything the other modules
//! decide is in the pure half that the linking crates' own tests exercise — against the real
//! store engine over a temporary file.

pub mod git_probe;
pub mod location;
pub mod notes;
pub mod paths;
pub mod projects;
pub mod store;
pub mod todos;
