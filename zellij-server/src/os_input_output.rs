#![allow(clippy::result_large_err)]
#![allow(clippy::type_complexity)]
use crate::{panes::PaneId, ClientId};

use interprocess::local_socket::Stream as LocalSocketStream;

use crate::os_input_output_unix::UnixPtyBackend as PtyBackendImpl;
use crate::pty_host::remote::{
    conn::RemoteGitProbe, protocol::ChildSignal, ssh::RemoteHosts, RemoteStatus,
};
// Re-exported: it is what `ServerOsApi::adopt_terminal` returns, so anything
// implementing that trait outside this crate — the integration tests' fake
// backend — needs to be able to name it.
pub use crate::os_input_output_unix::AdoptedTerminal;
use crate::pty_host::client::ReleasedPtyHost;

use interprocess;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use tempfile::tempfile;
use zellij_utils::{
    data::Palette,
    errors::prelude::*,
    input::command::{RunCommand, TerminalAction},
    ipc::{
        ClientToServerMsg, ExitReason, IpcReceiverWithContext, IpcSenderWithContext,
        ServerToClientMsg,
    },
    shared::default_palette,
};

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    env,
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Condvar, Mutex},
};

pub use async_trait::async_trait;

fn find_executable(candidate: &std::path::Path) -> Option<PathBuf> {
    if candidate.exists() && candidate.is_file() {
        return Some(candidate.to_path_buf());
    }
    None
}

/// Resolve a command to its absolute path, searching the working directory,
/// then PATH.
pub(crate) fn resolve_command(cmd: &RunCommand) -> Option<PathBuf> {
    let command = &cmd.command;
    match cmd.cwd.as_ref() {
        Some(cwd) => {
            if let Some(resolved) = find_executable(&cwd.join(command)) {
                return Some(resolved);
            }
        },
        None => {
            if let Some(resolved) = find_executable(command) {
                return Some(resolved);
            }
        },
    }
    if let Some(paths) = env::var_os("PATH") {
        for path in env::split_paths(&paths) {
            if let Some(resolved) = find_executable(&path.join(command)) {
                return Some(resolved);
            }
        }
    }
    None
}

pub(crate) fn command_exists(cmd: &RunCommand) -> bool {
    resolve_command(cmd).is_some()
}

// this is a utility method to separate the arguments from a pathbuf before we turn it into a
// Command. eg. "/usr/bin/vim -e" ==> "/usr/bin/vim" + "-e" (the latter will be pushed to args)
fn separate_command_arguments(command: &mut PathBuf, args: &mut Vec<String>) {
    let mut parts = vec![];
    let mut current_part = String::new();
    for part in command.display().to_string().split_ascii_whitespace() {
        current_part.push_str(part);
        if current_part.ends_with('\\') {
            let _ = current_part.pop();
            current_part.push(' ');
        } else {
            let current_part = std::mem::take(&mut current_part);
            parts.push(current_part);
        }
    }
    if !parts.is_empty() {
        *command = PathBuf::from(parts.remove(0));
        args.append(&mut parts);
    }
}

/// If a [`TerminalAction::OpenFile(file)`] is given, the text editor specified by environment variable `EDITOR`
/// (or `VISUAL`, if `EDITOR` is not set) will be started in the new terminal, with the given
/// file open.
/// If [`TerminalAction::RunCommand(RunCommand)`] is given, the command will be started
/// in the new terminal.
/// If None is given, the shell specified by environment variable `SHELL` will
/// be started in the new terminal.
///
/// Returns (cmd, failover_cmd).
fn build_command(
    terminal_action: TerminalAction,
    default_editor: Option<PathBuf>,
) -> (RunCommand, Option<RunCommand>) {
    let mut failover_cmd_args = None;
    let cmd = match terminal_action {
        TerminalAction::OpenFile(mut payload) => {
            if payload.path.is_relative() {
                if let Some(cwd) = payload.cwd.as_ref() {
                    payload.path = cwd.join(payload.path);
                }
            }
            let mut command = default_editor.unwrap_or_else(|| {
                PathBuf::from(
                    env::var("EDITOR")
                        .unwrap_or_else(|_| env::var("VISUAL").unwrap_or_else(|_| "vi".into())),
                )
            });

            let mut args = vec![];

            if !command.is_dir() {
                separate_command_arguments(&mut command, &mut args);
            }
            let file_to_open = payload
                .path
                .into_os_string()
                .into_string()
                .expect("Not valid Utf8 Encoding");
            if let Some(line_number) = payload.line_number {
                if command.ends_with("vim")
                    || command.ends_with("nvim")
                    || command.ends_with("emacs")
                    || command.ends_with("nano")
                    || command.ends_with("kak")
                {
                    failover_cmd_args = Some(vec![file_to_open.clone()]);
                    args.push(format!("+{}", line_number));
                    args.push(file_to_open);
                } else if command.ends_with("hx") || command.ends_with("helix") {
                    // at the time of writing, helix only supports this syntax
                    // and it might be a good idea to leave this here anyway
                    // to keep supporting old versions
                    args.push(format!("{}:{}", file_to_open, line_number));
                } else {
                    args.push(file_to_open);
                }
            } else {
                args.push(file_to_open);
            }
            RunCommand {
                command,
                args,
                cwd: payload.cwd,
                hold_on_close: false,
                hold_on_start: false,
                ..Default::default()
            }
        },
        TerminalAction::RunCommand(command) => command,
    };
    let failover_cmd = if let Some(failover_cmd_args) = failover_cmd_args {
        let mut failover = cmd.clone();
        failover.args = failover_cmd_args;
        Some(failover)
    } else {
        None
    };
    (cmd, failover_cmd)
}

// How many non-Render messages a client's send queue holds before the server
// gives up on the client. Control messages are small and individually
// meaningful, so a client that stops draining them for this long has genuinely
// gone away; disconnecting beats buffering without bound (and possibly having
// the whole session OOM-killed on its behalf).
const SEND_QUEUE_CONTROL_CAPACITY: usize = 5000;

/// A per-client send queue that keeps at most one pending `Render` and at most
/// one pending `PaneRenderUpdate` per pane.
///
/// A `Render` is an idempotent full-screen repaint, so when the client is not
/// draining fast enough (or at all - a stalled terminal, a suspended process),
/// older pending renders are pure backlog: replacing the pending one with the
/// newest keeps the queue flat during output floods, means render volume can
/// never disconnect a client, and lets a recovering client paint current state
/// immediately instead of replaying thousands of stale frames. The same holds
/// per pane for non-initial `PaneRenderUpdate`s, which are full-viewport
/// snapshots: only the newest per pane is kept. Initial snapshots and
/// scrollback-carrying updates are one-off responses rather than a stream of
/// repaints, so they stay - like every other message - in a bounded FIFO (see
/// `SEND_QUEUE_CONTROL_CAPACITY`).
///
/// The writer thread delivers queued control messages first and coalesced
/// renders last: control messages are order-sensitive among themselves, while
/// a coalesced render is latest-state and correct whenever it lands. This also
/// guarantees a pane's initial snapshot (FIFO) precedes its coalesced updates.
/// A pending pane render may be delivered after a `SubscribedPaneClosed` for
/// the same pane; clients must tolerate that race anyway, since pane closure
/// always races in-flight updates.
struct CoalescingSendQueue {
    state: Mutex<SendQueueState>,
    message_available: Condvar,
}

#[derive(Default)]
struct SendQueueState {
    control: VecDeque<ServerToClientMsg>,
    pending_pane_renders: BTreeMap<zellij_utils::data::PaneId, ServerToClientMsg>,
    pending_render: Option<ServerToClientMsg>,
    closed: bool,
}

/// Which slot of the queue a message belongs in.
enum SendQueueSlot {
    FullRender,
    PaneRender(zellij_utils::data::PaneId),
    Control,
}

fn slot_for(msg: &ServerToClientMsg) -> SendQueueSlot {
    match msg {
        ServerToClientMsg::Render { .. } => SendQueueSlot::FullRender,
        ServerToClientMsg::PaneRenderUpdate {
            pane_id,
            is_initial: false,
            scrollback: None,
            ..
        } => SendQueueSlot::PaneRender(*pane_id),
        _ => SendQueueSlot::Control,
    }
}

/// What `CoalescingSendQueue::push` did with the message.
enum SendQueuePush {
    Queued,
    /// The message was a render and replaced an older pending one.
    CoalescedRender,
    /// The message was dropped: the control FIFO is at capacity.
    ControlQueueFull,
}

impl CoalescingSendQueue {
    fn new() -> Self {
        CoalescingSendQueue {
            state: Mutex::new(SendQueueState::default()),
            message_available: Condvar::new(),
        }
    }
    fn push(&self, msg: ServerToClientMsg) -> SendQueuePush {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            // the writer is gone; pretend-queue so late senders don't error
            return SendQueuePush::Queued;
        }
        let outcome = match slot_for(&msg) {
            SendQueueSlot::FullRender => match state.pending_render.replace(msg) {
                Some(_) => SendQueuePush::CoalescedRender,
                None => SendQueuePush::Queued,
            },
            SendQueueSlot::PaneRender(pane_id) => {
                match state.pending_pane_renders.insert(pane_id, msg) {
                    Some(_) => SendQueuePush::CoalescedRender,
                    None => SendQueuePush::Queued,
                }
            },
            SendQueueSlot::Control => {
                if state.control.len() >= SEND_QUEUE_CONTROL_CAPACITY {
                    return SendQueuePush::ControlQueueFull;
                }
                state.control.push_back(msg);
                SendQueuePush::Queued
            },
        };
        self.message_available.notify_one();
        outcome
    }
    /// Blocks until a message is available; `None` once closed and drained.
    fn pop(&self) -> Option<ServerToClientMsg> {
        let mut state = self.state.lock().unwrap();
        loop {
            if let Some(msg) = state.control.pop_front() {
                return Some(msg);
            }
            if let Some((_, msg)) = state.pending_pane_renders.pop_first() {
                return Some(msg);
            }
            if let Some(msg) = state.pending_render.take() {
                return Some(msg);
            }
            if state.closed {
                return None;
            }
            state = self.message_available.wait(state).unwrap();
        }
    }
    fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.message_available.notify_all();
    }
    #[cfg(test)]
    fn len(&self) -> usize {
        let state = self.state.lock().unwrap();
        state.control.len()
            + state.pending_pane_renders.len()
            + usize::from(state.pending_render.is_some())
    }
}

// The ClientSender is in charge of sending messages to the client on a special thread
// This is done so that when the unix socket buffer is full, we won't block the entire router
// thread.
// Renders are coalesced rather than buffered (see CoalescingSendQueue); if the bounded
// control-message FIFO fills up regardless, we bubble up an error so that the router thread
// will give up on this client and we'll stop sending messages to it.
// If the client ever becomes responsive again, we'll send one final Disconnect message so it
// knows what happened.
struct ClientSender {
    client_id: ClientId,
    queue: Arc<CoalescingSendQueue>,
}

impl ClientSender {
    pub fn new(client_id: ClientId, mut sender: IpcSenderWithContext<ServerToClientMsg>) -> Self {
        let queue = Arc::new(CoalescingSendQueue::new());
        let writer_queue = queue.clone();
        std::thread::spawn(move || {
            let err_context = || format!("failed to send message to client {client_id}");
            while let Some(msg) = writer_queue.pop() {
                sender
                    .send_server_msg(msg)
                    .with_context(err_context)
                    .non_fatal();
            }
            let _ = sender.send_server_msg(ServerToClientMsg::Exit {
                exit_reason: ExitReason::Disconnect,
            });
        });
        ClientSender { client_id, queue }
    }
    pub fn send_or_buffer(&self, msg: ServerToClientMsg) -> Result<()> {
        let err_context = || {
            format!(
                "failed to send or buffer message for client {}",
                self.client_id
            )
        };

        match self.queue.push(msg) {
            SendQueuePush::Queued | SendQueuePush::CoalescedRender => Ok(()),
            SendQueuePush::ControlQueueFull => {
                log::warn!(
                    "client {} is processing server messages too slow",
                    self.client_id
                );
                Err(ZellijError::ClientTooSlow {
                    client_id: self.client_id,
                })
                .with_context(err_context)
            },
        }
    }
}

impl Drop for ClientSender {
    fn drop(&mut self) {
        // lets the writer thread drain what's queued and send the final
        // Disconnect message
        self.queue.close();
    }
}

#[derive(Clone)]
pub struct ServerOsInputOutput {
    pty_backend: PtyBackendImpl,
    client_senders: Arc<Mutex<HashMap<ClientId, ClientSender>>>,
    cached_resizes: Arc<Mutex<Option<BTreeMap<u32, (u16, u16, Option<u16>, Option<u16>)>>>>,
    /// ssh forwards to the machines this session has remote panes on. One per
    /// host, shared by every pane there.
    remote_hosts: Arc<RemoteHosts>,
}

/// Turn a failure to open a pane on another machine into one the pty thread can
/// show the user instead of dying on.
///
/// The terminal id is the reserved one, which only this side knows: a
/// `CommandNotFound` rebuilt from the broker's answer carries a placeholder id
/// (the far machine has never heard of ours), and everything else arrives as
/// plain `anyhow`, which the pty thread treats as fatal. Both become a
/// `ZellijError` naming the pane the message belongs in.
fn pane_start_failed(terminal_id: u32, host: &str, err: anyhow::Error) -> anyhow::Error {
    if let Some(ZellijError::CommandNotFound { command, .. }) = err.downcast_ref::<ZellijError>() {
        return ZellijError::CommandNotFound {
            terminal_id,
            command: command.clone(),
        }
        .into();
    }
    ZellijError::PaneStartFailed {
        terminal_id,
        message: format!("could not open a pane on {host}: {err:#}"),
    }
    .into()
}

/// A null `AsyncReader` for held panes (produces EOF immediately).
pub(crate) struct NullAsyncReader;

// async fn in traits is not supported by rust, so dtolnay's excellent async_trait macro is being
// used. See https://smallcultfollowing.com/babysteps/blog/2019/10/26/async-fn-in-traits-are-hard/
#[async_trait]
pub trait AsyncReader: Send + Sync {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error>;
}

#[async_trait]
impl AsyncReader for NullAsyncReader {
    async fn read(&mut self, _buf: &mut [u8]) -> Result<usize, io::Error> {
        Ok(0) // EOF
    }
}

/// The `ServerOsApi` trait represents an abstract interface to the features of an operating system that
/// Zellij server requires.
pub trait ServerOsApi: Send + Sync {
    fn set_terminal_size_using_terminal_id(
        &self,
        id: u32,
        cols: u16,
        rows: u16,
        width_in_pixels: Option<u16>,
        height_in_pixels: Option<u16>,
    ) -> Result<()>;
    /// Spawn a new terminal, with a terminal action. The returned tuple contains:
    /// - terminal_id (u32)
    /// - an async reader for the PTY output
    /// - the child process PID, if available (Option<u32>)
    fn spawn_terminal(
        &self,
        terminal_action: TerminalAction,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
        default_editor: Option<PathBuf>,
    ) -> Result<(u32, Box<dyn AsyncReader>, Option<u32>)>;
    // reserves a terminal id without actually opening a terminal
    fn reserve_terminal_id(&self) -> Result<u32> {
        unimplemented!()
    }
    /// Write bytes to the standard input of the virtual terminal referred to by `terminal_id`.
    fn write_to_tty_stdin(&self, terminal_id: u32, buf: &[u8]) -> Result<usize>;
    /// Wait until all output written to the terminal has been transmitted.
    fn tcdrain(&self, terminal_id: u32) -> Result<()>;
    /// Terminate the process with process ID `pid`. (SIGHUP)
    fn kill(&self, pid: u32) -> Result<()>;
    /// Terminate the process with process ID `pid`. (SIGKILL)
    fn force_kill(&self, pid: u32) -> Result<()>;
    /// Send SIGINT to the process with process ID `pid`
    fn send_sigint(&self, pid: u32) -> Result<()>;
    /// Returns a [`Box`] pointer to this [`ServerOsApi`] struct.
    fn box_clone(&self) -> Box<dyn ServerOsApi>;
    fn send_to_client(&self, client_id: ClientId, msg: ServerToClientMsg) -> Result<()>;
    fn new_client(
        &mut self,
        client_id: ClientId,
        stream: LocalSocketStream,
    ) -> Result<IpcReceiverWithContext<ClientToServerMsg>>;
    fn remove_client(&mut self, client_id: ClientId) -> Result<()>;
    fn load_palette(&self) -> Palette;
    /// Returns the current working directory for a given pid
    fn get_cwd(&self, pid: u32) -> Option<PathBuf>;
    /// Returns the current working directory for multiple pids
    fn get_cwds(&self, _pids: Vec<u32>) -> (HashMap<u32, PathBuf>, HashMap<u32, Vec<String>>) {
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
        _panes: &[(u32, u32)],
        _post_hook: &Option<String>,
    ) -> HashMap<u32, Vec<String>> {
        HashMap::new()
    }
    /// Writes the given buffer to a string
    fn write_to_file(&mut self, buf: String, file: Option<String>) -> Result<()>;

    fn re_run_command_in_terminal(
        &self,
        terminal_id: u32,
        run_command: RunCommand,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<(Box<dyn AsyncReader>, Option<u32>)>;
    fn clear_terminal_id(&self, terminal_id: u32) -> Result<()>;
    /// Whether new panes get a detachable pty-host supervisor. Config only
    /// arrives with the first client, long after the backend is built, so this is
    /// set once at session init. Defaulted so mock implementations are unaffected.
    fn set_pty_host_enabled(&self, _enabled: bool) {}
    /// Terminate every pty-host supervisor this server owns, on the way out.
    fn shutdown_pty_hosts(&self) {}
    /// Whether this pane's process lives on another machine.
    ///
    /// The question every pid-shaped operation has to ask first. `id_to_child_pid`
    /// holds nothing usable for a remote pane, and a pid from another machine is
    /// worse than no pid: it will happily name an unrelated *local* process, so
    /// a `kill` meant for a pane lands somewhere else entirely.
    fn is_remote_pane(&self, _terminal_id: u32) -> bool {
        false
    }
    /// What the far side last reported about a remote pane: the cwd and
    /// foreground command the local `sysinfo` and `tcgetpgrp` cannot see.
    fn remote_pane_status(&self, _terminal_id: u32) -> Option<RemoteStatus> {
        None
    }
    /// Signal a remote pane's child, by name rather than by pid.
    fn signal_remote_child(&self, _terminal_id: u32, _signal: ChildSignal) -> Result<()> {
        Err(anyhow!("this backend does not support remote panes"))
    }
    /// Every pane whose process lives on another machine.
    ///
    /// The backend's transport table is the one place that knows, so this is the
    /// single source of truth rather than a second list kept in step by hand.
    fn remote_pane_ids(&self) -> Vec<u32> {
        Vec::new()
    }
    /// What git last said about a remote pane's working directory, unparsed.
    ///
    /// Raw on purpose: `hocus-agentd`'s `poll::repository` already turns exactly
    /// this into a checkout identity, including the awkward part where a
    /// repository with no commits writes its identity to stdout and *then* exits
    /// 128. Parsing here would duplicate that and drift from it.
    fn remote_pane_git_probe(&self, _terminal_id: u32) -> Option<RemoteGitProbe> {
        None
    }
    /// The machine a pane runs on, or `None` if it runs on this one.
    fn remote_pane_host(&self, _terminal_id: u32) -> Option<String> {
        None
    }
    /// Hand a pane's supervisor over without killing it, so another session can
    /// take the running process. Defaulted to an error rather than a no-op: a
    /// backend that cannot do this must refuse the warp, not silently succeed and
    /// lose the pane.
    fn release_terminal_for_warp(&self, _terminal_id: u32) -> Result<ReleasedPtyHost> {
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
    fn cache_resizes(&mut self) {}
    fn apply_cached_resizes(&mut self) -> Vec<(u32, u16, u16)> {
        Vec::new()
    }
}

impl ServerOsApi for ServerOsInputOutput {
    fn set_terminal_size_using_terminal_id(
        &self,
        id: u32,
        cols: u16,
        rows: u16,
        width_in_pixels: Option<u16>,
        height_in_pixels: Option<u16>,
    ) -> Result<()> {
        if let Some(cached_resizes) = self.cached_resizes.lock().unwrap().as_mut() {
            cached_resizes.insert(id, (cols, rows, width_in_pixels, height_in_pixels));
            return Ok(());
        }
        self.pty_backend
            .set_terminal_size(id, cols, rows, width_in_pixels, height_in_pixels)
    }
    fn spawn_terminal(
        &self,
        terminal_action: TerminalAction,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
        default_editor: Option<PathBuf>,
    ) -> Result<(u32, Box<dyn AsyncReader>, Option<u32>)> {
        let err_context = || "failed to spawn terminal".to_string();

        let terminal_id = self
            .pty_backend
            .next_terminal_id()
            .context("no more terminal IDs left to allocate")?;

        self.pty_backend.reserve_terminal_id(terminal_id);

        let (cmd, failover_cmd) = build_command(terminal_action, default_editor);

        if let Some(host) = cmd.remote_host.clone() {
            let (conn, reader_socket) = self
                .remote_hosts
                .open_pane(&host, &cmd, None)
                .map_err(|e| pane_start_failed(terminal_id, &host, e))?;
            let async_reader = self
                .pty_backend
                .register_remote_terminal(terminal_id, conn, reader_socket, quit_cb, cmd)
                .with_context(err_context)?;
            // Deliberately no pid. The child's pid is meaningful only on its own
            // machine, and handing one back here would put it in
            // `id_to_child_pid`, where every consumer would treat it as local —
            // `kill`, `sysinfo`, and the plugin-facing `GetPanePid` alike.
            // `None` makes those paths skip the pane instead, which is what the
            // remote-aware replacements rely on.
            return Ok((terminal_id, async_reader, None));
        }

        let (async_reader, child_fd) = self
            .pty_backend
            .spawn_terminal(cmd, failover_cmd, quit_cb, terminal_id)
            .with_context(err_context)?;

        Ok((terminal_id, async_reader, Some(child_fd as u32)))
    }
    fn reserve_terminal_id(&self) -> Result<u32> {
        let terminal_id = self
            .pty_backend
            .next_terminal_id()
            .context("no more terminal IDs available")?;
        self.pty_backend.reserve_terminal_id(terminal_id);
        Ok(terminal_id)
    }
    fn write_to_tty_stdin(&self, terminal_id: u32, buf: &[u8]) -> Result<usize> {
        self.pty_backend.write_to_tty_stdin(terminal_id, buf)
    }
    fn tcdrain(&self, terminal_id: u32) -> Result<()> {
        self.pty_backend.tcdrain(terminal_id)
    }
    fn is_remote_pane(&self, terminal_id: u32) -> bool {
        self.pty_backend.is_remote(terminal_id)
    }
    fn remote_pane_status(&self, terminal_id: u32) -> Option<RemoteStatus> {
        self.pty_backend.remote_status(terminal_id)
    }
    fn signal_remote_child(&self, terminal_id: u32, signal: ChildSignal) -> Result<()> {
        self.pty_backend.signal_remote_child(terminal_id, signal)
    }
    fn remote_pane_ids(&self) -> Vec<u32> {
        self.pty_backend.remote_pane_ids()
    }
    fn remote_pane_git_probe(&self, terminal_id: u32) -> Option<RemoteGitProbe> {
        self.pty_backend.remote_git_probe(terminal_id)
    }
    fn remote_pane_host(&self, terminal_id: u32) -> Option<String> {
        self.pty_backend.remote_host(terminal_id)
    }
    fn box_clone(&self) -> Box<dyn ServerOsApi> {
        Box::new((*self).clone())
    }
    fn kill(&self, pid: u32) -> Result<()> {
        self.pty_backend.kill(pid)
    }
    fn force_kill(&self, pid: u32) -> Result<()> {
        self.pty_backend.force_kill(pid)
    }
    fn send_sigint(&self, pid: u32) -> Result<()> {
        self.pty_backend.send_sigint(pid)
    }
    fn send_to_client(&self, client_id: ClientId, msg: ServerToClientMsg) -> Result<()> {
        let err_context = || format!("failed to send message to client {client_id}");

        if let Some(sender) = self
            .client_senders
            .lock()
            .to_anyhow()
            .with_context(err_context)?
            .get_mut(&client_id)
        {
            sender.send_or_buffer(msg).with_context(err_context)
        } else {
            Ok(())
        }
    }

    fn new_client(
        &mut self,
        client_id: ClientId,
        stream: LocalSocketStream,
    ) -> Result<IpcReceiverWithContext<ClientToServerMsg>> {
        let receiver = IpcReceiverWithContext::new(stream);
        let sender = ClientSender::new(client_id, receiver.get_sender());
        self.client_senders
            .lock()
            .to_anyhow()
            .with_context(|| format!("failed to create new client {client_id}"))?
            .insert(client_id, sender);
        Ok(receiver)
    }

    fn remove_client(&mut self, client_id: ClientId) -> Result<()> {
        let mut client_senders = self
            .client_senders
            .lock()
            .to_anyhow()
            .with_context(|| format!("failed to remove client {client_id}"))?;
        if client_senders.contains_key(&client_id) {
            client_senders.remove(&client_id);
        }
        Ok(())
    }

    fn load_palette(&self) -> Palette {
        default_palette()
    }

    fn get_cwd(&self, pid: u32) -> Option<PathBuf> {
        let mut system_info = System::new();
        let sysinfo_pid = sysinfo::Pid::from_u32(pid);
        let refresh_kind = ProcessRefreshKind::nothing().with_cwd(UpdateKind::Always);
        system_info.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[sysinfo_pid]),
            false,
            refresh_kind,
        );

        if let Some(process) = system_info.process(sysinfo_pid) {
            if let Some(cwd) = process.cwd() {
                return Some(cwd.to_path_buf());
            }
        }
        None
    }

    fn get_cwds(&self, pids: Vec<u32>) -> (HashMap<u32, PathBuf>, HashMap<u32, Vec<String>>) {
        let mut system_info = System::new();
        let mut cwds = HashMap::new();
        let mut cmds = HashMap::new();

        let sysinfo_pids: Vec<sysinfo::Pid> =
            pids.iter().map(|&p| sysinfo::Pid::from_u32(p)).collect();
        let refresh_kind = ProcessRefreshKind::nothing()
            .with_cwd(UpdateKind::Always)
            .with_cmd(UpdateKind::Always);
        system_info.refresh_processes_specifics(
            ProcessesToUpdate::Some(&sysinfo_pids),
            false,
            refresh_kind,
        );

        for pid in pids {
            let sysinfo_pid = sysinfo::Pid::from_u32(pid);
            if let Some(process) = system_info.process(sysinfo_pid) {
                if let Some(cwd) = process.cwd() {
                    cwds.insert(pid, cwd.to_path_buf());
                }
                let cmd = process.cmd();
                if !cmd.is_empty() {
                    cmds.insert(
                        pid,
                        cmd.iter()
                            .map(|s| s.to_string_lossy().into_owned())
                            .collect(),
                    );
                }
            }
        }

        (cwds, cmds)
    }
    fn get_foreground_cmds(
        &self,
        panes: &[(u32, u32)],
        post_hook: &Option<String>,
    ) -> HashMap<u32, Vec<String>> {
        let mut terminal_to_fg_pid: HashMap<u32, u32> = HashMap::new();
        for &(terminal_id, shell_pid) in panes {
            if let Some(fpgid) = self.pty_backend.tcgetpgrp(terminal_id) {
                if fpgid > 0 && fpgid as u32 != shell_pid {
                    terminal_to_fg_pid.insert(terminal_id, fpgid as u32);
                }
            }
        }
        if terminal_to_fg_pid.is_empty() {
            return HashMap::new();
        }

        let sysinfo_pids: Vec<sysinfo::Pid> = terminal_to_fg_pid
            .values()
            .map(|&p| sysinfo::Pid::from_u32(p))
            .collect();
        let mut system_info = System::new();
        let refresh_kind = ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always);
        system_info.refresh_processes_specifics(
            ProcessesToUpdate::Some(&sysinfo_pids),
            false,
            refresh_kind,
        );

        let mut cmds = HashMap::new();
        for (terminal_id, fg_pid) in terminal_to_fg_pid {
            let Some(process) = system_info.process(sysinfo::Pid::from_u32(fg_pid)) else {
                continue;
            };
            let command: Vec<String> = process
                .cmd()
                .iter()
                .map(|s| s.to_string_lossy().into_owned())
                .collect();
            if command.is_empty() {
                continue;
            }
            let command = apply_post_command_hook(command, post_hook);
            cmds.insert(terminal_id, command);
        }
        cmds
    }

    fn write_to_file(&mut self, buf: String, name: Option<String>) -> Result<()> {
        let err_context = || "failed to write to file".to_string();

        let mut f: File = match name {
            Some(x) => File::create(x).with_context(err_context)?,
            None => tempfile().with_context(err_context)?,
        };
        write!(f, "{}", buf).with_context(err_context)
    }

    fn re_run_command_in_terminal(
        &self,
        terminal_id: u32,
        run_command: RunCommand,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<(Box<dyn AsyncReader>, Option<u32>)> {
        // A held remote pane re-runs on the machine it was held on. Without this
        // the command would quietly run *here* under the same pane, and the pane
        // still open on the far machine would be left to the orphan timeout.
        if let Some(host) = run_command.remote_host.clone() {
            // The pane being re-run is held, not closed, so the broker still
            // holds the session it was held from. Registering over it would drop
            // this side's only handle on it and leave it to the orphan timeout.
            self.pty_backend.clear_terminal_id(terminal_id);
            self.pty_backend.reserve_terminal_id(terminal_id);
            let (conn, reader_socket) = self
                .remote_hosts
                .open_pane(&host, &run_command, None)
                .map_err(|e| pane_start_failed(terminal_id, &host, e))?;
            let async_reader = self
                .pty_backend
                .register_remote_terminal(terminal_id, conn, reader_socket, quit_cb, run_command)
                .context("failed to re-run a command in a remote terminal")?;
            // No pid, for the same reason `spawn_terminal` gives none.
            return Ok((async_reader, None));
        }

        let (async_reader, child_fd) =
            self.pty_backend
                .spawn_terminal(run_command, None, quit_cb, terminal_id)?;
        Ok((async_reader, Some(child_fd as u32)))
    }
    fn clear_terminal_id(&self, terminal_id: u32) -> Result<()> {
        self.pty_backend.clear_terminal_id(terminal_id);
        Ok(())
    }
    #[cfg(not(windows))]
    fn set_pty_host_enabled(&self, enabled: bool) {
        self.pty_backend.set_pty_host_enabled(enabled);
    }
    #[cfg(not(windows))]
    fn shutdown_pty_hosts(&self) {
        self.pty_backend.shutdown_pty_hosts();
        // The ssh forwards go with them. The brokers and their panes on the far
        // side survive by design — only an explicit terminate ends those — but
        // leaving forwards behind would leak an ssh process and a bound socket
        // per host on the way out.
        self.remote_hosts.shutdown();
    }
    #[cfg(not(windows))]
    fn release_terminal_for_warp(&self, terminal_id: u32) -> Result<ReleasedPtyHost> {
        self.pty_backend.release_terminal_for_warp(terminal_id)
    }
    #[cfg(not(windows))]
    fn adopt_terminal(
        &self,
        control_socket: &Path,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<AdoptedTerminal> {
        self.pty_backend.adopt_terminal(control_socket, quit_cb)
    }
    fn cache_resizes(&mut self) {
        if self.cached_resizes.lock().unwrap().is_none() {
            *self.cached_resizes.lock().unwrap() = Some(BTreeMap::new());
        }
    }
    fn apply_cached_resizes(&mut self) -> Vec<(u32, u16, u16)> {
        let mut applied = Vec::new();
        let mut cached_resizes = self.cached_resizes.lock().unwrap().take();
        if let Some(cached_resizes) = cached_resizes.as_mut() {
            for (terminal_id, (cols, rows, width_in_pixels, height_in_pixels)) in
                cached_resizes.iter()
            {
                if self
                    .set_terminal_size_using_terminal_id(
                        *terminal_id,
                        *cols,
                        *rows,
                        *width_in_pixels,
                        *height_in_pixels,
                    )
                    .is_ok()
                {
                    applied.push((*terminal_id, *cols, *rows));
                }
            }
        }
        applied
    }
}

impl Clone for Box<dyn ServerOsApi> {
    fn clone(&self) -> Box<dyn ServerOsApi> {
        self.box_clone()
    }
}

pub fn get_server_os_input() -> Result<ServerOsInputOutput, std::io::Error> {
    Ok(ServerOsInputOutput {
        pty_backend: PtyBackendImpl::new()?,
        remote_hosts: Arc::new(RemoteHosts::new()),
        client_senders: Arc::new(Mutex::new(HashMap::new())),
        cached_resizes: Arc::new(Mutex::new(None)),
    })
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

fn apply_post_command_hook(command: Vec<String>, post_hook: &Option<String>) -> Vec<String> {
    let Some(post_hook) = post_hook else {
        return command;
    };
    let stringified = command.join(" ");
    let cmd = match run_command_hook(&stringified, post_hook) {
        Ok(command) => command,
        Err(e) => {
            Err::<(), _>(anyhow!("post command discovery hook failed to run: {e}")).non_fatal();
            stringified
        },
    };
    cmd.trim()
        .split_ascii_whitespace()
        .map(|p| p.to_owned())
        .collect()
}

fn run_command_hook(
    original_command: &str,
    hook_script: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(hook_script)
        .env("RESURRECT_COMMAND", original_command)
        .output()?;

    if !output.status.success() {
        return Err(format!("Hook failed: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

#[cfg(test)]
#[path = "./unit/os_input_output_tests.rs"]
mod os_input_output_tests;
