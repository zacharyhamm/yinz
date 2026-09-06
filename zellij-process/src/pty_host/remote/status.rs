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
use zellij_types::internal_ids::ProcessId;

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
fn foreground_pid(master_fd: RawFd, child_pid: ProcessId) -> Option<ProcessId> {
    let pgid = nix::unistd::tcgetpgrp(unsafe { BorrowedFd::borrow_raw(master_fd) }).ok()?;
    let pgid = pgid.as_raw();
    let pgid = ProcessId::try_from(pgid).ok()?;
    if pgid.get() > 0 && pgid != child_pid {
        Some(pgid)
    } else {
        None
    }
}

pub fn probe(master_fd: RawFd, child_pid: ProcessId) -> Probe {
    let foreground = foreground_pid(master_fd, child_pid);

    let mut wanted = vec![sysinfo::Pid::from_u32(child_pid.get())];
    if let Some(fg) = foreground {
        wanted.push(sysinfo::Pid::from_u32(fg.get()));
    }

    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&wanted),
        false,
        ProcessRefreshKind::nothing()
            .with_cwd(UpdateKind::Always)
            .with_cmd(UpdateKind::Always),
    );

    let argv_of = |pid: ProcessId| -> Option<Vec<String>> {
        let cmd = system.process(sysinfo::Pid::from_u32(pid.get()))?.cmd();
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
        .process(sysinfo::Pid::from_u32(child_pid.get()))
        .and_then(|p| p.cwd().map(|c| c.to_string_lossy().to_string()));

    Probe {
        cwd,
        shell_argv: argv_of(child_pid),
        foreground_argv: foreground.and_then(argv_of),
    }
}

/// The raw result of asking git about a directory.
///
/// Deliberately unparsed. `hocus-agentd`'s `poll::repository` already turns
/// exactly these three things into a checkout identity, including the awkward
/// parts — a repository with no commits writes its identity to stdout and *then*
/// exits 128, so the exit code cannot gate the parse. Shipping raw output means
/// that logic stays in one place and keeps working for remote panes without
/// knowing they exist.
///
/// It reaches that parser through `PluginCommand::GetPaneGitProbe`, which
/// answers from the cache this fills — see `Pty::get_pane_git_probe`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GitProbe {
    pub cwd: String,
    pub exit_code: Option<i32>,
    pub metadata: Vec<u8>,
    pub config: Vec<u8>,
}

/// The same two invocations `hocus-agentd` runs, against a directory on this
/// machine.
///
/// Kept byte-identical to the plugin's, because the parser on the other end is
/// the plugin's too — any drift here shows up as a project that silently fails
/// to resolve rather than as an error.
pub fn probe_git(cwd: &str) -> GitProbe {
    use std::process::Command;

    let metadata = Command::new("git")
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-dir",
            "--git-common-dir",
            "--abbrev-ref",
            "HEAD",
        ])
        .current_dir(cwd)
        .output()
        .ok();
    let config = Command::new("git")
        .args([
            "config",
            "--null",
            "--get-regexp",
            r"^(remote\..*\.url|remote\.pushdefault|branch\..*\.remote)$",
        ])
        .current_dir(cwd)
        .output()
        .ok();

    GitProbe {
        cwd: cwd.to_owned(),
        // The metadata command's status is the one that matters; the config
        // command exits non-zero simply for matching nothing, which is ordinary.
        exit_code: metadata.as_ref().and_then(|o| o.status.code()),
        metadata: metadata.map(|o| o.stdout).unwrap_or_default(),
        config: config.map(|o| o.stdout).unwrap_or_default(),
    }
}

#[cfg(test)]
#[path = "../../unit/pty_host_remote_status_tests.rs"]
mod pty_host_remote_status_tests;
