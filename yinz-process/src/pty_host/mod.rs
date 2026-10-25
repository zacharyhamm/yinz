//! Detachable panes: every pane's child process is owned by a small supervisor
//! process ("pty-host") rather than by yinz-server itself.
//!
//! The supervisor creates the pty, forks the child, and hands yinz-server a dup
//! of the pty *master* descriptor over `SCM_RIGHTS`. The server then reads,
//! writes and ioctls that descriptor exactly as it did when it owned the pty, so
//! everything downstream — `RawFdAsyncReader`, `try_write_to_fd`'s partial-write
//! backpressure, `TIOCSWINSZ`, `tcdrain`, `tcgetpgrp` — is unchanged.
//!
//! What changes is lifetime. The child is a child of the supervisor, not of the
//! server, and the supervisor holds the master open. When the server exits the
//! child gets no `SIGHUP`; the supervisor notices the control channel closed and
//! starts draining the pty so the child never blocks on a full buffer. Only an
//! explicit `Terminate` ends a supervisor, which every ordinary teardown path
//! sends.

pub mod cli;
pub mod client;
pub mod fdpass;
pub mod host;
pub mod protocol;
pub mod remote;
pub mod terminal;

pub use host::{run_pty_host, serve, ServeConfig};
use yinz_types::internal_ids::ProcessId;

/// A process identity must not become a kernel process-group selector.
///
/// PID 1 is refused along with 0: it is init, never a pane's child or a supervisor, and the
/// group paths negate what this returns — `kill(-1, …)` is every process the caller may signal.
pub fn process_pid(pid: ProcessId) -> yinz_runtime::errors::prelude::Result<nix::unistd::Pid> {
    use yinz_runtime::errors::prelude::*;
    let raw = i32::try_from(pid).context("process ID exceeds the kernel PID range")?;
    if raw <= 1 {
        return Err(anyhow!(
            "process ID {raw} does not identify a pane or supervisor process"
        ));
    }
    Ok(nix::unistd::Pid::from_raw(raw))
}

/// Locally spawned kernel PIDs are signed integers above 1 (init). Never use this for
/// IPC-derived IDs; those must pass through `process_pid`.
pub(crate) fn kernel_pid(pid: ProcessId) -> nix::unistd::Pid {
    let raw = pid.get();
    assert!(raw > 1 && raw <= i32::MAX as u32, "invalid kernel PID");
    nix::unistd::Pid::from_raw(raw as i32)
}

/// The `kill(2)` selector for the process group led by `pid`. Every group signal goes through
/// here: negating 1 gives -1, which signals every process the caller may signal (the server,
/// every pty host, the user's ssh sessions), and negating 0 gives 0, the caller's own group.
pub fn process_group(
    pid: nix::unistd::Pid,
) -> yinz_runtime::errors::prelude::Result<nix::unistd::Pid> {
    use yinz_runtime::errors::prelude::*;
    let raw = pid.as_raw();
    if raw <= 1 {
        return Err(anyhow!(
            "process ID {raw} does not lead a process group we may signal"
        ));
    }
    Ok(nix::unistd::Pid::from_raw(-raw))
}

/// Only ESRCH proves the owned group is gone. Permission and probe errors do not.
pub(crate) fn process_group_terminated(child_pid: ProcessId) -> bool {
    process_group(kernel_pid(child_pid))
        .is_ok_and(|group| nix::sys::signal::kill(group, None) == Err(nix::errno::Errno::ESRCH))
}

fn secure_socket(path: &std::path::Path, mode: u32) -> yinz_runtime::errors::prelude::Result<()> {
    secure_socket_with(path, mode, yinz_runtime::shared::set_permissions)
}

fn secure_socket_with(
    path: &std::path::Path,
    mode: u32,
    set_permissions: impl FnOnce(&std::path::Path, u32) -> std::io::Result<()>,
) -> yinz_runtime::errors::prelude::Result<()> {
    use yinz_runtime::errors::prelude::*;

    if let Err(error) = set_permissions(path, mode) {
        std::fs::remove_file(path).with_context(|| {
            format!(
                "failed to remove {} after permission finalization failed",
                path.display()
            )
        })?;
        return Err(error).with_context(|| {
            format!("failed to set permissions {mode:04o} on {}", path.display())
        });
    }
    Ok(())
}

/// The argv flag that turns this binary into a supervisor.
pub const PTY_HOST_FLAG: &str = "--pty-host";

/// The pane's stable supervisor id, inherited by everything running inside it.
///
/// Session names and terminal ids change when a pane is warped; this one does not.
pub const PANE_REF_ENV_KEY: &str = "YINZ_PANE_REF";

/// Used when the config says nothing. Kept here so there is one place to flip.
pub const DEFAULT_DETACHABLE_PANES: bool = true;

/// Detect the supervisor invocation without going through clap, so `main()` can
/// branch before the logger, the cache-folder setup and the clap builder — none
/// of which a supervisor needs, and all of which it would pay for on every pane.
///
/// Returns the label the server passed for `ps` readability, if any.
pub fn pty_host_invocation() -> Option<String> {
    invocation_from_args(std::env::args())
}

/// Split out from `pty_host_invocation` so the argv rule can be tested:
/// `std::env::args()` is process-global, and the "only as the first argument"
/// half of the rule is what `main`'s stray-`--pty-host` guard depends on.
fn invocation_from_args<I: IntoIterator<Item = String>>(args: I) -> Option<String> {
    let mut args = args.into_iter();
    let _bin = args.next();
    if args.next().as_deref() != Some(PTY_HOST_FLAG) {
        return None;
    }
    Some(args.next().unwrap_or_default())
}

#[cfg(test)]
#[path = "unit/mod_tests.rs"]
mod tests;
