//! What the broker can see about a pane that the server cannot.
//!
//! The server discovers a pane's cwd and running command by inspecting the
//! process: `tcgetpgrp` on the pty master for the foreground process group, then
//! `sysinfo` for that pid's cwd and argv. Both halves are local by nature — a
//! pid means nothing off its own machine, and `/proc` describes only this one —
//! so for a remote pane the inspection has to happen here and travel as data.
//!
//! The same two calls, in the same order, against the pane this broker owns.
//! Matching the local semantics exactly is the point: agent detection is pure
//! string matching over argv and titles (`hocus-proto`'s `agents.rs`), so a
//! remote pane that reports the same argv a local one would is classified
//! identically, with nothing in the detection layer aware of the difference.

use std::os::fd::{BorrowedFd, RawFd};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use super::protocol::BrokerToServerMsg;

/// A pane's cwd and commands, as of now.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Probe {
    pub cwd: Option<String>,
    /// The argv of the process the pane was started with.
    pub shell_argv: Option<Vec<String>>,
    /// The argv of whatever currently owns the terminal, when that is something
    /// other than the pane's own process — an agent run from a shell, say.
    pub foreground_argv: Option<Vec<String>>,
}

impl Probe {
    pub fn into_report(self) -> BrokerToServerMsg {
        BrokerToServerMsg::StatusReport {
            cwd: self.cwd,
            shell_argv: self.shell_argv,
            foreground_argv: self.foreground_argv,
        }
    }
}

/// The foreground process group of the pane's terminal, if it is not the pane's
/// own process.
///
/// Mirrors `ServerOsInputOutput::get_foreground_cmds`: a pgid equal to the
/// pane's own pid means nothing else has taken the terminal, and reporting it
/// would just be the shell again.
fn foreground_pid(master_fd: RawFd, child_pid: u32) -> Option<u32> {
    let pgid = nix::unistd::tcgetpgrp(unsafe { BorrowedFd::borrow_raw(master_fd) }).ok()?;
    let pgid = pgid.as_raw();
    if pgid > 0 && pgid as u32 != child_pid {
        Some(pgid as u32)
    } else {
        None
    }
}

pub fn probe(master_fd: RawFd, child_pid: u32) -> Probe {
    let foreground = foreground_pid(master_fd, child_pid);

    let mut wanted = vec![sysinfo::Pid::from_u32(child_pid)];
    if let Some(fg) = foreground {
        wanted.push(sysinfo::Pid::from_u32(fg));
    }

    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&wanted),
        false,
        ProcessRefreshKind::nothing()
            .with_cwd(UpdateKind::Always)
            .with_cmd(UpdateKind::Always),
    );

    let argv_of = |pid: u32| -> Option<Vec<String>> {
        let cmd = system.process(sysinfo::Pid::from_u32(pid))?.cmd();
        if cmd.is_empty() {
            return None;
        }
        Some(
            cmd.iter()
                .map(|s| s.to_string_lossy().to_string())
                .collect(),
        )
    };

    // The cwd is taken from the pane's own process rather than the foreground
    // one, matching what a local pane reports.
    let cwd = system
        .process(sysinfo::Pid::from_u32(child_pid))
        .and_then(|p| p.cwd().map(|c| c.to_string_lossy().to_string()));

    Probe {
        cwd,
        shell_argv: argv_of(child_pid),
        foreground_argv: foreground.and_then(argv_of),
    }
}

#[cfg(test)]
#[path = "../../unit/pty_host_remote_status_tests.rs"]
mod pty_host_remote_status_tests;
