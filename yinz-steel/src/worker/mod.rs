//! A persistent interpreter process keeps native allocations and compiler failures out of the host.

mod client;
mod protocol;
mod server;

pub(crate) use client::Process;
pub use client::WorkerRuntime;
#[doc(hidden)]
pub use server::confine_process;
pub use server::run_worker;

pub const ARG: &str = "--steel-runtime-worker";
// Base64 expands a maximum 64 MiB binary body to 85.4 MiB; leave room for its metadata.
const MAX_FRAME: usize = 96 * 1024 * 1024;
const MAX_ERRORS: usize = 64 * 1024;
const DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

#[cfg(test)]
#[path = "../unit/worker.rs"]
mod tests;
