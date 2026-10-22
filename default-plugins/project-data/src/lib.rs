//! Shared entity schemas and pure project models.
//! The host links the registry directly; the Scheme model bridge exposes pure planners.
//! Native model tests use the real SQLite backend through [`store::Local`].

pub mod git_probe;
pub mod location;
pub mod paths;
pub mod projects;
pub mod store;
