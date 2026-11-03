//! Conservative process-exit observations for recoverable ownership transfers.
use yinz_runtime::errors::prelude::*;

/// A missing PID or an observed zombie cannot retain server descriptors. A live
/// reused PID is deliberately indistinguishable from the original owner here:
/// callers must inspect its process incarnation before taking any action.
pub fn has_exited(pid: u32) -> Result<bool> {
    let raw = i32::try_from(pid).context("invalid process id")?;
    anyhow::ensure!(raw > 1, "invalid process id");
    match nix::sys::signal::kill(nix::unistd::Pid::from_raw(raw), None) {
        Err(nix::errno::Errno::ESRCH) => return Ok(true),
        Err(error) => return Err(error).context("inspect process liveness"),
        Ok(()) => {},
    }
    let pid = sysinfo::Pid::from_u32(pid);
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[pid]),
        true,
        sysinfo::ProcessRefreshKind::nothing(),
    );
    // Failure to inspect a still-present process is uncertainty, not death.
    Ok(system
        .process(pid)
        .is_some_and(|process| process.status() == sysinfo::ProcessStatus::Zombie))
}

#[cfg(test)]
#[path = "unit/liveness.rs"]
mod tests;
