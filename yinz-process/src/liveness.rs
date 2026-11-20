//! Conservative process-exit observations for recoverable ownership transfers.
use yinz_runtime::errors::prelude::*;

/// A missing PID or an observed zombie (or reaping) process cannot retain server descriptors. A live
/// reused PID is deliberately indistinguishable from the original owner here:
/// callers must inspect its process incarnation before taking any action.
pub fn has_exited(pid: u32) -> Result<bool> {
    let raw = i32::try_from(pid).context("invalid process id")?;
    anyhow::ensure!(raw > 1, "invalid process id");
    let signal_probe = || match nix::sys::signal::kill(nix::unistd::Pid::from_raw(raw), None) {
        Err(nix::errno::Errno::ESRCH) => Ok(false),
        Err(error) => Err(error).context("inspect process liveness"),
        Ok(()) => Ok(true),
    };
    let zombie_probe = || {
        let pid = sysinfo::Pid::from_u32(pid);
        let mut system = sysinfo::System::new();
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[pid]),
            true,
            sysinfo::ProcessRefreshKind::nothing(),
        );
        system
            .process(pid)
            .map(|process| is_terminated(process.status()))
    };
    observe_exit(signal_probe, zombie_probe)
}

/// While its parent reaps it, a zombie is briefly `Dead` (`X`) and still
/// accepts signal probes before it leaves the process table.
fn is_terminated(status: sysinfo::ProcessStatus) -> bool {
    matches!(
        status,
        sysinfo::ProcessStatus::Zombie | sysinfo::ProcessStatus::Dead
    )
}

/// `is_present` reports whether the PID still exists; `is_terminated` inspects its
/// status, returning `None` when the process could not be read.
fn observe_exit(
    is_present: impl Fn() -> Result<bool>,
    is_terminated: impl FnOnce() -> Option<bool>,
) -> Result<bool> {
    if !is_present()? {
        return Ok(true);
    }
    match is_terminated() {
        Some(terminated) => Ok(terminated),
        // A zombie reaped between the two probes vanishes from the process table.
        // Probe again so that reaping is not mistaken for a live process; failure
        // to inspect a still-present process is uncertainty, not death.
        None => Ok(!is_present()?),
    }
}

#[cfg(test)]
#[path = "unit/liveness.rs"]
mod tests;
