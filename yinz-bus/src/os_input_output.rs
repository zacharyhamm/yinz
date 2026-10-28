//! The server OS boundary shared by the real backend and test fakes.
//! The backend in `yinz-server-os` implements ptys, supervisors, and client socket writers.

use crate::{ClientId, PaneId};
use yinz_types::internal_ids::TerminalId;
use yinz_types::internal_ids::{ProcessId, SessionName};

use interprocess::local_socket::Stream as LocalSocketStream;

use crate::pty_host::client::{AgentReportCb, ChildStateCb, ReleasedPane};
use crate::pty_host::remote::{
    conn::RemoteGitProbe, protocol::ChildSignal, ssh::SshOptions, RemoteStatus,
};
use yinz_types::data::RemotePaneHandoff;

use yinz_runtime::errors::prelude::*;
use {
    yinz_ipc::ipc::ClientToServerMsg, yinz_ipc::ipc::IpcReceiverWithContext,
    yinz_ipc::ipc::ServerToClientMsg, yinz_types::data::Palette,
    yinz_types::input::command::RunCommand, yinz_types::input::command::TerminalAction,
};

use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    time::Duration,
};

pub use async_trait::async_trait;

/// How far behind a client's writer is: what the screen consults before rendering
/// another frame for it. See `ClientTransport::client_render_backlog_and_notify`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderBacklog {
    /// Full `Render` frames and coalesced pane snapshots queued behind the message
    /// being written, if any. Both are produced by the screen's render pass.
    pub queued_renders: usize,
}

/// Called when a client's render queue drains below the threshold it was armed at, or closes.
/// Shared, so one notifier serves every client a render pass arms.
pub type RenderReady = std::sync::Arc<dyn Fn() + Send + Sync>;

#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct ResolvedSpawn {
    pub terminal_id: TerminalId,
    pub command: RunCommand,
    pub rerun_command: RunCommand,
    /// `None` for a command that runs on this machine once resolved.
    pub endpoint: Option<RemoteEndpoint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEndpoint {
    pub host: String,
    pub ssh_options: SshOptions,
}

/// A supervisor this server has just taken over, and everything the caller needs
/// to give it a pane.
pub struct AdoptedTerminal {
    pub terminal_id: TerminalId,
    pub reader: Box<dyn AsyncReader>,
    /// `None` for a pane adopted from another machine. Its child's pid is
    /// meaningful only there, and recording one here would put it in
    /// `id_to_child_pid`, where every consumer would act on it locally.
    pub child_pid: Option<ProcessId>,
    /// The command the supervisor is running, for rebuilding a `quit_cb` that
    /// holds or closes the pane the way the original did.
    pub cmd: RunCommand,
    /// Output produced while nobody was attached, to be replayed into the pane
    /// before any live bytes.
    pub buffered: Vec<u8>,
    /// `Some` if the child has already exited — the pane should arrive held.
    pub child_exit_status: Option<Option<i32>>,
}

/// A null `AsyncReader` for held panes (produces EOF immediately).
pub struct NullAsyncReader;

pub use yinz_process::AsyncReader;

#[async_trait]
impl AsyncReader for NullAsyncReader {
    async fn read(&mut self, _buf: &mut [u8]) -> Result<usize, io::Error> {
        Ok(0) // EOF
    }
}

/// Reading, writing, and resizing an existing terminal.
pub trait TerminalIo: Send + Sync {
    fn set_terminal_size_using_terminal_id(
        &self,
        id: TerminalId,
        cols: u16,
        rows: u16,
        width_in_pixels: Option<u16>,
        height_in_pixels: Option<u16>,
    ) -> Result<()>;
    /// Write bytes to the standard input of the virtual terminal referred to by `terminal_id`.
    fn write_to_tty_stdin(&self, terminal_id: TerminalId, buf: &[u8]) -> Result<usize>;
    /// Persist observations for the active launch; unsupervised backends have
    /// no retained launch metadata to update.
    fn retain_agent_theme(
        &self,
        _terminal_id: TerminalId,
        _event: yinz_types::agent::theme::AgentThemeEvent,
    ) -> Result<()> {
        Ok(())
    }
    /// Wait until all output written to the terminal has been transmitted.
    fn tcdrain(&self, terminal_id: TerminalId) -> Result<()>;
    fn cache_resizes(&mut self) {}
    fn apply_cached_resizes(&mut self) -> Vec<(TerminalId, u16, u16)> {
        Vec::new()
    }
}

/// Allocating terminals and controlling their child processes.
pub trait TerminalProcesses: Send + Sync {
    /// Spawn a new terminal, with a terminal action. The returned tuple contains:
    /// - the terminal identity
    /// - an async reader for the PTY output
    /// - the child process identity, if available
    fn spawn_terminal(
        &self,
        terminal_action: TerminalAction,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
        default_editor: Option<PathBuf>,
    ) -> Result<(TerminalId, Box<dyn AsyncReader>, Option<ProcessId>)>;
    fn resolve_spawn(
        &self,
        _terminal_id: TerminalId,
        _terminal_action: TerminalAction,
        _default_editor: Option<PathBuf>,
        _report_progress: &mut dyn FnMut(&str),
    ) -> Result<ResolvedSpawn> {
        Err(anyhow!("this backend does not support remote panes"))
    }
    fn spawn_resolved(
        &self,
        _resolved: ResolvedSpawn,
        _quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<(TerminalId, Box<dyn AsyncReader>, Option<ProcessId>)> {
        Err(anyhow!("this backend does not support remote panes"))
    }
    /// Reserve an identity without opening a terminal.
    fn reserve_terminal_id(&self) -> Result<TerminalId> {
        Err(anyhow!(
            "this backend does not support reserving terminal IDs"
        ))
    }
    /// Terminate the process with process ID `pid`. (SIGHUP)
    fn kill(&self, pid: ProcessId) -> Result<()>;
    /// Terminate the process with process ID `pid`. (SIGKILL)
    fn force_kill(&self, pid: ProcessId) -> Result<()>;
    /// Send SIGINT to the process with process ID `pid`
    fn send_sigint(&self, pid: ProcessId) -> Result<()>;
    /// Continue a stopped process group, the way a shell's `fg` does.
    fn send_sigcont(&self, pid: ProcessId) -> Result<()>;
    fn re_run_command_in_terminal(
        &self,
        terminal_id: TerminalId,
        run_command: RunCommand,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<(Box<dyn AsyncReader>, Option<ProcessId>)>;
    fn resolve_rerun(
        &self,
        _terminal_id: TerminalId,
        _run_command: RunCommand,
        _report_progress: &mut dyn FnMut(&str),
    ) -> Result<ResolvedSpawn> {
        Err(anyhow!("this backend does not support remote panes"))
    }
    fn rerun_resolved(
        &self,
        _resolved: ResolvedSpawn,
        _quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<(Box<dyn AsyncReader>, Option<ProcessId>)> {
        Err(anyhow!("this backend does not support remote panes"))
    }
    fn clear_terminal_id(&self, terminal_id: TerminalId) -> Result<()>;
}

/// Supervised terminal lifecycle, callbacks, and ownership transfers.
pub trait TerminalSupervision: Send + Sync {
    /// Register the callback told whenever a pane's root process stops or
    /// resumes.
    ///
    /// A default no-op rather than a required method: only the real Unix backend
    /// can observe this, and no test fake has anything to say about it.
    fn set_child_state_callback(&self, _cb: ChildStateCb) {}
    /// Register the callback told whenever a coding agent's hook in a remote
    /// pane reports an event. A default no-op for the same reason as
    /// `set_child_state_callback`.
    fn set_agent_report_callback(&self, _cb: AgentReportCb) {}
    /// Whether new panes get a detachable pty-host supervisor. Config only
    /// arrives with the first client, long after the backend is built, so this is
    /// set once at session init. Defaulted so mock implementations are unaffected.
    fn set_pty_host_enabled(&self, _enabled: bool) {}
    /// Terminate every pty-host supervisor this server owns, on the way out.
    fn shutdown_pty_hosts(&self) {}
    fn begin_confirmed_pty_host_shutdown(&self) -> Result<()> {
        Ok(())
    }
    fn confirm_pty_host_shutdown(&self) -> Result<()> {
        Ok(())
    }
    /// Install this server's identity before workers clone the backend.
    fn set_session_state(&mut self, _state: yinz_runtime::envs::SessionState) {}
    /// Relabel local supervisors before publishing a session rename.
    fn rename_pty_host_session(&self, _old: &SessionName, _new: &SessionName) -> Result<()> {
        Ok(())
    }
    /// Terminate the unattached supervisors labelled with `session`, on the way
    /// out of a `KillSession` only. They are what an earlier incarnation of this
    /// session left behind when it died without shutting down; killing the
    /// session by name is the user saying those panes are done too.
    fn terminate_orphaned_pty_hosts(
        &self,
        _session: &str,
        _progress: &mut dyn FnMut(),
    ) -> Result<()> {
        Ok(())
    }
    /// Hand a pane's supervisor over without killing it, so another session can
    /// take the running process. Defaulted to an error rather than a no-op: a
    /// backend that cannot do this must refuse the warp, not silently succeed and
    /// lose the pane.
    ///
    /// `budget` is how long this may block. It is not advisory: it is what is
    /// left of the warp caller's own timeout, and a release that runs past it
    /// completes into a channel nobody is reading, leaving the pane's process
    /// running with nothing watching it. A backend that cannot finish inside the
    /// budget must return an error saying so rather than take longer.
    fn release_terminal_for_warp(
        &self,
        _terminal_id: TerminalId,
        _budget: Duration,
    ) -> Result<ReleasedPane> {
        Err(anyhow!(
            "this backend does not support moving panes between sessions"
        ))
    }
    /// Whether [`Self::release_terminal_for_warp`] would work, asked before
    /// anything has been let go.
    ///
    /// Everything the release can refuse for is knowable in advance, and it has
    /// to be asked in advance: by the time the release runs, the pane's reader
    /// task has been aborted and this server's dup of its master closed, so a
    /// refusal after that point leaves a pane on screen that no longer has a
    /// process to talk to. Must not change anything — it is asked precisely so
    /// that a "no" costs the user nothing.
    fn check_terminal_can_be_warped(&self, _terminal_id: TerminalId) -> Result<()> {
        Err(anyhow!(
            "this backend does not support moving panes between sessions"
        ))
    }
    /// Take a detached supervisor over into this session under a fresh terminal id.
    fn adopt_terminal(
        &self,
        _control_socket: &Path,
        _quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<AdoptedTerminal> {
        Err(anyhow!(
            "this backend does not support adopting panes from another session"
        ))
    }
    /// Take a released *remote* pane over into this session under a fresh
    /// terminal id, by resuming it on the broker that has been holding it.
    fn adopt_remote_terminal(
        &self,
        _handoff: &RemotePaneHandoff,
        _quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<AdoptedTerminal> {
        Err(anyhow!(
            "this backend does not support adopting remote panes from another session"
        ))
    }
}

/// Queries and signals for processes running on another machine.
pub trait RemotePanes: Send + Sync {
    /// Whether this pane's process lives on another machine.
    ///
    /// The question every pid-shaped operation has to ask first. `id_to_child_pid`
    /// holds nothing usable for a remote pane, and a pid from another machine is
    /// worse than no pid: it will happily name an unrelated *local* process, so
    /// a `kill` meant for a pane lands somewhere else entirely.
    fn is_remote_pane(&self, _terminal_id: TerminalId) -> bool {
        false
    }
    /// What the far side last reported about a remote pane: the cwd and
    /// foreground command the local `sysinfo` and `tcgetpgrp` cannot see.
    fn remote_pane_status(&self, _terminal_id: TerminalId) -> Option<RemoteStatus> {
        None
    }
    /// Signal a remote pane's child, by name rather than by pid.
    fn signal_remote_child(&self, _terminal_id: TerminalId, _signal: ChildSignal) -> Result<()> {
        Err(anyhow!("this backend does not support remote panes"))
    }
    /// Every pane whose process lives on another machine.
    ///
    /// The backend's transport table is the one place that knows, so this is the
    /// single source of truth rather than a second list kept in step by hand.
    fn remote_pane_ids(&self) -> Vec<TerminalId> {
        Vec::new()
    }
    /// What git last said about a remote pane's working directory, unparsed.
    ///
    /// Raw on purpose: `agent-service`'s `poll::repository` already turns exactly
    /// this into a checkout identity, including the awkward part where a
    /// repository with no commits writes its identity to stdout and *then* exits
    /// 128. Parsing here would duplicate that and drift from it.
    fn remote_pane_git_probe(
        &self,
        _terminal_id: TerminalId,
        _cwd: &str,
        _refresh_token: Option<&str>,
    ) -> Result<Option<RemoteGitProbe>, String> {
        Err("pane is not remote".into())
    }
    /// The machine a pane runs on, or `None` if it runs on this one.
    fn remote_pane_host(&self, _terminal_id: TerminalId) -> Option<String> {
        None
    }
}

/// Server-side client connections and ordered output delivery.
pub trait ClientTransport: Send + Sync {
    fn send_to_client(&self, client_id: ClientId, msg: ServerToClientMsg) -> Result<()>;
    fn new_client(
        &mut self,
        client_id: ClientId,
        stream: LocalSocketStream,
    ) -> Result<IpcReceiverWithContext<ClientToServerMsg>>;
    fn remove_client(&mut self, client_id: ClientId) -> Result<()>;
    /// A connected client's render backlog. The screen skips a frame while every
    /// client is behind, so dirty state accumulates into one later frame instead
    /// of queueing diffs the clients cannot drain.
    ///
    /// Atomically inspect the queue and, if it is at or above `threshold`, arm `ready` to be
    /// called once. Clone it only when arming: most clients are not behind. Invoke the callback
    /// after draining below the threshold or closing, outside locks.
    ///
    /// A default of none: test fakes have no writers to fall behind. Implementations must also
    /// return none after logging a failed backlog query; writer failure must not panic the screen.
    fn client_render_backlog_and_notify(
        &self,
        _client_id: ClientId,
        _threshold: usize,
        _ready: &RenderReady,
    ) -> Option<RenderBacklog> {
        None
    }
    /// Wait for queued client messages to reach their sockets, on the way out.
    ///
    /// `send_to_client` only queues; a per-client writer thread does the actual
    /// write. Those threads are detached, so without this the process can exit
    /// between "queued the `Exit`" and "wrote the `Exit`", and the client sees
    /// a bare hangup instead of being told why the session ended.
    fn flush_client_writers(&self, _deadline: Duration) {}
}

/// Process metadata, palette, and file access used by the server.
pub trait ServerEnvironment: Send + Sync {
    fn load_palette(&self) -> Palette;
    /// Returns the current working directory for a given pid
    fn get_cwd(&self, pid: ProcessId) -> Option<PathBuf>;
    /// Returns the current working directory for multiple pids
    fn get_cwds(
        &self,
        _pids: Vec<ProcessId>,
    ) -> (HashMap<ProcessId, PathBuf>, HashMap<ProcessId, Vec<String>>) {
        (HashMap::new(), HashMap::new())
    }
    /// Get a list of all running commands by their parent process id
    fn get_all_cmds_by_ppid(&self, _post_hook: &Option<String>) -> HashMap<String, Vec<String>> {
        HashMap::new()
    }
    /// For each `(terminal_id, shell_pid)` pane, return the foreground command running in its
    /// controlling terminal, keyed by `terminal_id`.
    fn get_foreground_cmds(
        &self,
        _panes: &[(TerminalId, ProcessId)],
        _post_hook: &Option<String>,
    ) -> HashMap<TerminalId, Vec<String>> {
        HashMap::new()
    }
    /// Writes the given buffer to a string
    fn write_to_file(&mut self, buf: String, file: Option<String>) -> Result<()>;
}

/// The complete server OS boundary. Consumers use a capability trait when they need only one part.
pub trait ServerOsApi:
    TerminalIo
    + TerminalProcesses
    + TerminalSupervision
    + RemotePanes
    + ClientTransport
    + ServerEnvironment
{
    /// Returns a [`Box`] pointer to this [`ServerOsApi`] struct.
    fn box_clone(&self) -> Box<dyn ServerOsApi>;
}

/// Every cloneable backend that provides each capability is the complete boundary.
impl<T> ServerOsApi for T
where
    T: TerminalIo
        + TerminalProcesses
        + TerminalSupervision
        + RemotePanes
        + ClientTransport
        + ServerEnvironment
        + Clone
        + 'static,
{
    fn box_clone(&self) -> Box<dyn ServerOsApi> {
        Box::new(self.clone())
    }
}

impl Clone for Box<dyn ServerOsApi> {
    fn clone(&self) -> Box<dyn ServerOsApi> {
        self.box_clone()
    }
}

use crate::pty_writer::PtyWriteInstruction;
use crate::thread_bus::ThreadSenders;

pub struct ResizeCache {
    senders: ThreadSenders,
}

impl ResizeCache {
    pub fn new(senders: ThreadSenders) -> Self {
        senders
            .send_to_pty_writer(PtyWriteInstruction::StartCachingResizes)
            .unwrap_or_else(|e| {
                log::error!("Failed to cache resizes: {}", e);
            });
        ResizeCache { senders }
    }
}

impl Drop for ResizeCache {
    fn drop(&mut self) {
        self.senders
            .send_to_pty_writer(PtyWriteInstruction::ApplyCachedResizes)
            .unwrap_or_else(|e| {
                log::error!("Failed to apply cached resizes: {}", e);
            });
    }
}
