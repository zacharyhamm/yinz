//! Shared entity schemas and pure project, todo, and note models.
//! The host links the registry directly; the Scheme model bridge exposes pure planners.
//! Native model tests use the real SQLite backend through [`store::Local`].

pub mod git_probe;
pub mod location;
pub mod notes;
pub mod paths;
pub mod projects;
pub mod store;
pub mod todos;
