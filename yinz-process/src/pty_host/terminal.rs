//! Kernel operations shared by supervised and in-process panes.

use nix::{
    errno::Errno,
    pty::{OpenptyResult, Winsize},
    sys::{signal::Signal, termios::Termios},
};
use std::{
    io,
    os::fd::{AsRawFd, RawFd},
};
use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::ProcessId;

pub fn open(size: Option<&Winsize>, termios: Option<&Termios>) -> Result<OpenptyResult> {
    let pty = nix::pty::openpty(size, termios).context("failed to open pty")?;
    // An inherited slave would suppress EOF after the pane's own child exits.
    super::fdpass::set_cloexec(pty.master.as_raw_fd()).context("failed to protect pty master")?;
    super::fdpass::set_cloexec(pty.slave.as_raw_fd()).context("failed to protect pty slave")?;
    Ok(pty)
}

nix::ioctl_write_ptr_bad!(tiocswinsz, libc::TIOCSWINSZ, Winsize);

pub fn resize(fd: RawFd, size: &Winsize) -> nix::Result<()> {
    // SAFETY: the ioctl reads one Winsize, which outlives the call.
    unsafe { tiocswinsz(fd, size) }.map(|_| ())
}

/// Attach the pane terminal and prevent unrelated descriptors from surviving exec.
///
/// # Safety
/// Call only in the child's pre_exec hook, with an open slave descriptor. Job control
/// requires the parent to have acquired this terminal in its own session.
pub unsafe fn prepare_child(slave: RawFd, job_control: bool) -> io::Result<()> {
    unsafe {
        if job_control {
            if libc::setpgid(0, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
            // SIGTTOU must stay ignored until this background group claims the terminal.
            if libc::tcsetpgrp(slave, libc::getpid()) != 0 {
                return Err(io::Error::last_os_error());
            }
            for signal in [libc::SIGTSTP, libc::SIGTTIN, libc::SIGTTOU] {
                if libc::signal(signal, libc::SIG_DFL) == libc::SIG_ERR {
                    return Err(io::Error::last_os_error());
                }
            }
            for target in [0, 1, 2] {
                if libc::dup2(slave, target) != target {
                    return Err(io::Error::last_os_error());
                }
            }
        } else if libc::login_tty(slave) != 0 {
            return Err(io::Error::last_os_error());
        }
        // Keep std's exec-error pipe usable until exec succeeds.
        close_fds::set_fds_cloexec(3, &[]);
    }
    Ok(())
}

/// Signal a pane group, falling back to its leader if it has no separate group.
pub fn signal_group_or_leader(child_pid: ProcessId, signal: Signal) -> Result<()> {
    let pid = super::process_pid(child_pid).context("resolve the pane process to signal")?;
    let group = super::process_group(pid).context("resolve the pane process group to signal")?;
    let group_error = match nix::sys::signal::kill(group, Some(signal)) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    let error = match nix::sys::signal::kill(pid, Some(signal)) {
        Ok(()) => return Ok(()),
        // The child may exit between its status report and the user's action.
        Err(Errno::ESRCH) if group_error == Errno::ESRCH => return Ok(()),
        // A gone leader does not excuse a surviving group that refused the signal.
        Err(Errno::ESRCH) => group_error,
        Err(error) => error,
    };
    Err(error).with_context(|| format!("failed to send {signal} to pane {pid}"))
}

#[cfg(test)]
#[path = "../unit/pty_host_terminal_tests.rs"]
mod tests;
