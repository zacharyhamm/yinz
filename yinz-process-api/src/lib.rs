//! Process contracts shared by the server bus and its local and remote backends.
//! This crate contains no spawning, transport, or supervisor implementations.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use yinz_types::input::command::RunCommand;
use yinz_types::internal_ids::{ProcessId, SupervisorId, TerminalId};

/// The object-safe output reader provided by local and remote process backends.
#[async_trait]
pub trait AsyncReader: Send + Sync {
    #[expect(
        clippy::double_must_use,
        reason = "async_trait adds must_use to an already must-use future"
    )]
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error>;
}

pub type QuitCb = Box<dyn Fn(TerminalId, Option<i32>, RunCommand) + Send>;

/// Whether a pane's root process is stopped or running.
///
/// Only transitions are ever reported, never a current state: a supervisor with
/// nobody attached has no one to tell, so a pane adopted while already stopped
/// stays unmarked until it moves again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildRunState {
    Stopped,
    Continued,
}

/// Told when a pane's root process stops or starts running again.
///
/// Deliberately *not* a second parameter on `spawn_terminal` next to `quit_cb`:
/// that signature is spelled out at some twenty-five sites, most of them test
/// fakes, and none of them has anything to say about suspension. This is
/// registered once, on the backend, and is the same closure for every pane —
/// which it can be, because unlike `quit_cb` it carries no per-pane policy.
pub type ChildStateCb = Arc<dyn Fn(TerminalId, ChildRunState) + Send + Sync>;

/// Told when a coding agent's hook in a remote pane reports an event, with the
/// pane's terminal id, the agent kind and the event JSON. Registered once on
/// the backend, like `ChildStateCb`, and handed to every remote reader.
pub type AgentReportCb = Arc<dyn Fn(TerminalId, String, String) + Send + Sync>;

/// What a caller needs to hand a released supervisor on to its next owner.
#[derive(Debug, Clone)]
pub struct ReleasedPtyHost {
    pub host_id: SupervisorId,
    pub host_pid: ProcessId,
    /// `None` if the supervisor never managed to bind its named socket, which is
    /// the one case where a pane is detachable but not transferable — there is no
    /// address for the next owner to attach to.
    pub control_socket: Option<PathBuf>,
}

/// What a caller needs to hand a *remote* pane on to its next owner.
///
/// The remote counterpart of `ReleasedPtyHost`, and deliberately not a variant
/// of it: nothing here is a path or a pid this machine could act on. The whole
/// address is "which host, and which pane the broker there is holding".
#[derive(Debug, Clone)]
pub struct ReleasedRemotePane {
    /// The ssh destination, as the pane was opened with. The adopting server
    /// builds its own forward to it rather than inheriting one.
    pub host: String,
    /// How the broker names this pane on a `Resume`. Meaningful only to that
    /// broker.
    pub pane_token: String,
    /// Output bytes this session took delivery of, sampled after its reader
    /// stopped. The broker replays from exactly here, so this is what keeps the
    /// pane's serialized screen and the replay from overlapping or leaving a
    /// gap between them.
    pub received: u64,
    /// What the pane runs.
    ///
    /// Filled in by the pty thread, which is where the command is recorded. The
    /// local path does not need this — the adopting server gets it back from
    /// the supervisor's `AttachAck` — but a broker's answer to a resume carries
    /// none, and a pane that cannot name its command cannot title itself, be
    /// re-run on the right machine when held, or be attributed to a project.
    pub cmd: RunCommand,
}

/// A pane let go for a warp, whichever machine its process is on.
///
/// An enum rather than two functions for the same reason `PaneTransport` is
/// one: every consumer of a released pane has to answer for the remote case,
/// and the compiler naming each site is better than any of them quietly
/// assuming a local supervisor.
#[derive(Debug, Clone)]
pub enum ReleasedPane {
    Held(yinz_types::data::HeldTerminal),
    Local(ReleasedPtyHost),
    Remote(ReleasedRemotePane),
}

impl ReleasedPane {
    /// How to name this pane to a user who has to go and recover it by hand.
    ///
    /// Both halves of a warp put this in their failure messages, which is why it
    /// lives with the type rather than at either call site.
    pub fn recovery_id(&self) -> String {
        match self {
            ReleasedPane::Held(_) => "held terminal".into(),
            ReleasedPane::Local(local) => local.host_id.to_string(),
            ReleasedPane::Remote(remote) => format!("{}:{}", remote.host, remote.pane_token),
        }
    }

    /// The command that gets this pane back.
    pub fn recovery_command(&self) -> String {
        match self {
            ReleasedPane::Held(_) => String::new(),
            ReleasedPane::Local(local) => recovery_command(&local.host_id, None),
            ReleasedPane::Remote(remote) => recovery_command(
                &SupervisorId::default(),
                Some((&remote.host, &remote.pane_token)),
            ),
        }
    }
}

/// The command that gets a released pane back, when nobody ended up holding it.
///
/// A pane in that state is running with nothing watching it, and this line is
/// the only thing standing between the user and losing the work in it — so it
/// has to be a command that actually applies. A remote pane is not reachable by
/// `pty-host adopt <id>`: its supervisor is on the other machine, and the id
/// that names it is a token only the broker there understands.
///
/// Written once and called from everywhere that has to say it — the release
/// path, the warp's rollback and the containment when both have failed — because
/// a second wording of this is a second chance to get it wrong.
pub fn recovery_command(local_host_id: &SupervisorId, remote: Option<(&str, &str)>) -> String {
    match remote {
        Some((host, pane_token)) => format!("yinz pty-host adopt --host {host} {pane_token}"),
        None => format!("yinz pty-host adopt {local_host_id}"),
    }
}

/// What the broker last told us about the pane, from its own machine.
///
/// Cached rather than fetched, because the alternative is a request/response
/// round trip over a link that may be dead, from `update_and_report_cwds` on the
/// pty thread. A stale-but-plausible answer is the better failure mode.
#[derive(Debug, Clone, Default)]
pub struct RemoteStatus {
    pub child_group_terminated: bool,
    pub cwd: Option<String>,
    pub shell_argv: Option<Vec<String>>,
    pub foreground_argv: Option<Vec<String>>,
}

/// What git said about a remote pane's working directory, unparsed.
///
/// Carried to the plugins that attribute panes to projects, which already know
/// how to read exactly this, by `PluginCommand::GetPaneGitProbe`. Kept raw so
/// the parsing stays in one place rather than being duplicated for remote panes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RemoteGitProbe {
    pub refresh_token: Option<String>,
    /// The machine this was run on, so two checkouts at the same path on
    /// different hosts stay distinct.
    pub host: String,
    pub cwd: String,
    pub exit_code: Option<i32>,
    pub metadata: Vec<u8>,
    pub config: Vec<u8>,
}

/// A signal to deliver to the pane's child.
///
/// Symbolic on purpose. The local server never learns the remote child's pid —
/// a pid from another machine is not merely useless locally, it is *dangerous*,
/// because it will happily name an unrelated local process. The broker resolves
/// this against the child it actually owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChildSignal {
    Hup,
    Int,
    Kill,
    /// Continue a stopped child. Only ever sent for a local pane today — a
    /// remote broker never reports a suspend, so nothing can ask for this.
    Cont,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SshOptions {
    pub identity_file: Option<PathBuf>,
    pub known_hosts_file: Option<PathBuf>,
}

impl SshOptions {
    pub fn args(&self) -> Vec<String> {
        let mut args = vec![];
        if let Some(path) = &self.identity_file {
            // `-i` takes its whole argument as the path, so it needs no quoting.
            args.extend([
                "-o".to_owned(),
                "IdentitiesOnly=yes".to_owned(),
                "-i".to_owned(),
                path.display().to_string(),
            ]);
        }
        if let Some(path) = &self.known_hosts_file {
            args.extend([
                "-o".to_owned(),
                format!("UserKnownHostsFile={}", ssh_option_value(path)),
                "-o".to_owned(),
                "StrictHostKeyChecking=yes".to_owned(),
            ]);
        }
        args
    }
}

/// A path as the value of an `-o Name=value` ssh option.
///
/// ssh reads the option string with its config-file tokenizer, which splits on
/// whitespace even when the whole `Name=value` arrived as one argv entry, so an
/// unquoted path with a space in it is silently cut at the space. Double quotes
/// keep the value whole; a quote or backslash inside the path is escaped, which
/// OpenSSH 8.7 and later understand.
pub fn ssh_option_value(path: &Path) -> String {
    let escaped = path
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
#[path = "unit/contracts.rs"]
mod tests;
