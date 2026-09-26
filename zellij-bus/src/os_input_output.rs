use crate::{ClientId, PaneId};
use zellij_types::internal_ids::TerminalId;
use zellij_types::internal_ids::{ProcessId, SessionName};
use zellij_types::style::default_palette;

use interprocess::local_socket::Stream as LocalSocketStream;

use crate::os_input_output_unix::UnixPtyBackend as PtyBackendImpl;
use crate::pty_host::remote::{
    conn::{Reconnector, RemoteGitProbe},
    protocol::ChildSignal,
    ssh::{RemoteHosts, SshOptions},
    RemoteConn, RemoteStatus,
};
// Re-exported: it is what `ServerOsApi::adopt_terminal` returns, so anything
// implementing that trait outside this crate — the integration tests' fake
// backend — needs to be able to name it.
pub use crate::os_input_output_unix::AdoptedTerminal;
use crate::pty_host::client::{AgentReportCb, ChildStateCb, ReleasedPane};
use zellij_types::command::PaneTarget;
use zellij_types::data::RemotePaneHandoff;

use interprocess;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use tempfile::tempfile;
use zellij_runtime::errors::prelude::*;
use {
    zellij_ipc::ipc::ClientToServerMsg, zellij_ipc::ipc::ExitReason,
    zellij_ipc::ipc::IpcReceiverWithContext, zellij_ipc::ipc::IpcSenderWithContext,
    zellij_ipc::ipc::ServerToClientMsg, zellij_types::data::Palette,
    zellij_types::input::command::RunCommand, zellij_types::input::command::TerminalAction,
};

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    env,
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex, RwLock,
    },
    time::{Duration, Instant},
};

pub use async_trait::async_trait;

#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct ResolvedSpawn {
    pub terminal_id: TerminalId,
    pub command: RunCommand,
    pub rerun_command: RunCommand,
    pub endpoint: RemoteEndpoint,
}

/// Where a remote command's broker is reached: the ssh host (an alias, or the
/// `ssh://` URL a Firecracker provider hands back) and the ssh options for it.
/// Kept apart from the command's `PaneTarget`, which says what the user asked
/// for and is what the pane shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEndpoint {
    pub host: String,
    pub ssh_options: SshOptions,
}

pub(crate) use zellij_process::command::command_exists;

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

// How many ordered messages a client's send queue holds before the server
// gives up on the client. Render deltas and control messages must be delivered
// in order; disconnecting beats buffering without bound (and possibly having
// the whole session OOM-killed on its behalf).
#[cfg(not(test))]
const SEND_QUEUE_CONTROL_CAPACITY: usize = 5000;
/// Small enough that the sequential-model property test reaches the full arm in a
/// handful of pushes.
#[cfg(test)]
const SEND_QUEUE_CONTROL_CAPACITY: usize = 8;

// Bound retained render allocations as well as message count: graphics uploads
// can make a single render several MiB. The writer may hold one more in flight.
#[cfg(not(test))]
const SEND_QUEUE_RENDER_BYTE_CAPACITY: usize = 64 * 1024 * 1024;
#[cfg(test)]
const SEND_QUEUE_RENDER_BYTE_CAPACITY: usize = 1024;

/// `Render` carries incremental terminal commands, including graphics uploads
/// and title changes. Every frame must arrive in order, before a later Exit or
/// SwitchSession. Only non-initial `PaneRenderUpdate`s without scrollback are
/// replaceable snapshots; those keep one pending update per pane.
///
/// Ordered messages are bounded by `SEND_QUEUE_CONTROL_CAPACITY`, and queued
/// render allocations by `SEND_QUEUE_RENDER_BYTE_CAPACITY`. A client that cannot
/// keep up is disconnected so it can reattach with a complete repaint.
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
    render_bytes: usize,
    pending_pane_renders: BTreeMap<zellij_types::data::PaneId, ServerToClientMsg>,
    closed: bool,
}

/// Which slot of the queue a message belongs in.
enum SendQueueSlot {
    PaneRender(zellij_types::data::PaneId),
    Control,
}

fn slot_for(msg: &ServerToClientMsg) -> SendQueueSlot {
    match msg {
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
    /// The message was dropped: the ordered FIFO hit its count or render-byte limit.
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
                if let ServerToClientMsg::Render { content } = &msg {
                    if content.capacity() > SEND_QUEUE_RENDER_BYTE_CAPACITY - state.render_bytes {
                        return SendQueuePush::ControlQueueFull;
                    }
                    state.render_bytes += content.capacity();
                }
                if let ServerToClientMsg::PaneRenderUpdate { pane_id, .. } = &msg {
                    // This response includes a newer viewport. A pending snapshot
                    // must not repaint older contents after it is delivered.
                    state.pending_pane_renders.remove(pane_id);
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
                if let ServerToClientMsg::Render { content } = &msg {
                    state.render_bytes -= content.capacity();
                }
                return Some(msg);
            }
            if let Some((_, msg)) = state.pending_pane_renders.pop_first() {
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
        state.control.len() + state.pending_pane_renders.len()
    }
}

// The ClientSender is in charge of sending messages to the client on a special thread
// This is done so that when the unix socket buffer is full, we won't block the entire router
// thread.
// Pane snapshots are coalesced (see CoalescingSendQueue); if the bounded
// ordered-message FIFO fills up, we bubble up an error so that the router thread
// will give up on this client and we'll stop sending messages to it.
// If the client ever becomes responsive again, we'll send one final Disconnect message so it
// knows what happened.
/// How many client writer threads are still draining.
///
/// The count has to live outside `client_senders`, because `remove_client`
/// drops the `ClientSender` — by the time the server is shutting down there is
/// no handle left to join. Each writer registers here at birth and deregisters
/// once it has written everything it was given, which is the only signal that
/// a queued `Exit` actually reached the socket.
#[derive(Default)]
struct LiveWriters {
    count: Mutex<usize>,
    drained: Condvar,
}

impl LiveWriters {
    fn register(&self) {
        *self.count.lock().unwrap() += 1;
    }

    fn deregister(&self) {
        let mut count = self.count.lock().unwrap();
        *count = count.saturating_sub(1);
        if *count == 0 {
            self.drained.notify_all();
        }
    }

    /// Wait for every writer to finish, up to `deadline`. Returns whether they
    /// all did. Bounded on purpose: a client whose socket buffer is full and
    /// which is not reading would otherwise hold the server open forever.
    fn wait_until_drained(&self, deadline: Duration) -> bool {
        let started = Instant::now();
        let mut count = self.count.lock().unwrap();
        while *count > 0 {
            let remaining = match deadline.checked_sub(started.elapsed()) {
                Some(remaining) if !remaining.is_zero() => remaining,
                _ => return false,
            };
            let (next, timeout) = self.drained.wait_timeout(count, remaining).unwrap();
            count = next;
            if timeout.timed_out() && *count > 0 {
                return false;
            }
        }
        true
    }
}

struct ClientSender {
    client_id: ClientId,
    queue: Arc<CoalescingSendQueue>,
    /// The connection's socket, kept only so that `remove_client` can shut its
    /// read half down. See `shutdown_reads`.
    stream_fd: std::os::fd::OwnedFd,
}

impl ClientSender {
    /// Stop the router thread reading this connection.
    ///
    /// Dropping a `ClientSender` unregisters the write half, but the router
    /// thread is blocked in `recv_client_msg` on a `dup` of the same socket with
    /// no read deadline, and it only leaves on EOF. Nothing else produced that
    /// EOF: a peer that connected and then held its fd open — which every
    /// liveness probe does for the moment between `ConnStatus` and the reply,
    /// and which a buggy or killed one does forever — left the router and its
    /// socket alive for the life of the process. That is what let one server
    /// here reach 131,085 threads and 65,538 open connections.
    ///
    /// `shutdown(2)` acts on the socket rather than on the file descriptor, so
    /// shutting it down here unblocks the router's `dup` too. Only the read
    /// half: the writer thread still has a final `Exit { Disconnect }` to
    /// deliver, and closing writes here would lose it.
    fn shutdown_reads(&self) {
        // Safety: a plain syscall on a fd this struct owns for its whole life.
        // A failure means the peer already went away, which is the outcome we
        // wanted anyway.
        use std::os::fd::AsRawFd;
        let _ = unsafe { libc::shutdown(self.stream_fd.as_raw_fd(), libc::SHUT_RD) };
    }
}

impl ClientSender {
    pub fn new(
        client_id: ClientId,
        mut sender: IpcSenderWithContext<ServerToClientMsg>,
        live_writers: Arc<LiveWriters>,
        stream_fd: std::os::fd::OwnedFd,
    ) -> io::Result<Self> {
        let queue = Arc::new(CoalescingSendQueue::new());
        let writer_queue = queue.clone();
        live_writers.register();
        let finished_writers = live_writers.clone();
        let worker = std::thread::Builder::new().name(format!("client-writer-{client_id}")).spawn(move || {
            let err_context = || format!("failed to send message to client {client_id}");
            let mut first_message = true;
            let mut first_render = true;
            while let Some(msg) = writer_queue.pop() {
                let started_at = Instant::now();
                let is_render = matches!(&msg, ServerToClientMsg::Render { .. });
                let trace_write = first_message || (first_render && is_render);
                if trace_write {
                    let kind = match &msg {
                        ServerToClientMsg::Render { content } => {
                            format!("first Render(bytes={})", content.len())
                        },
                        ServerToClientMsg::Connected { .. } => "Connected".to_owned(),
                        ServerToClientMsg::QueryTerminalSize => "QueryTerminalSize".to_owned(),
                        ServerToClientMsg::Exit { .. } => "Exit".to_owned(),
                        _ => "other".to_owned(),
                    };
                    log::info!(
                        "attach trace: server client writer client_id={client_id} socket write begin message={kind}"
                    );
                }
                let result = sender.send_server_msg(msg).with_context(err_context);
                if trace_write {
                    log::info!(
                        "attach trace: server client writer client_id={client_id} socket write finished after {:?} ({})",
                        started_at.elapsed(),
                        if result.is_ok() { "ok" } else { "error" }
                    );
                }
                first_message = false;
                if is_render {
                    first_render = false;
                }
                result.non_fatal();
            }
            let _ = sender.send_server_msg(ServerToClientMsg::Exit {
                exit_reason: ExitReason::Disconnect,
            });
            // Last thing this thread does: from here on the socket has
            // everything it was ever going to get.
            finished_writers.deregister();
        });
        if let Err(error) = worker {
            live_writers.deregister();
            return Err(error);
        }
        Ok(ClientSender {
            client_id,
            queue,
            stream_fd,
        })
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

impl ClientSender {
    /// Stop accepting messages and let the writer thread finish what it has.
    /// Idempotent, so shutdown can call it on senders still in the map without
    /// disturbing the ones already dropped.
    fn close(&self) {
        self.queue.close();
    }
}

impl Drop for ClientSender {
    fn drop(&mut self) {
        // lets the writer thread drain what's queued and send the final
        // Disconnect message
        self.queue.close();
    }
}

/// The Firecracker session renames waiting to be told to the provider.
///
/// One slot, one worker: a rename that arrives while an earlier one is still
/// being applied replaces it, since the provider only needs to end up on the
/// name the session has now. The worker starts with the first rename and runs
/// until `close`, so two renames can never reach the provider out of order.
#[derive(Default)]
pub(crate) struct FirecrackerRenames {
    state: Mutex<FirecrackerRenameState>,
    wake: Condvar,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

#[derive(Default)]
struct FirecrackerRenameState {
    pending: Option<String>,
    closed: bool,
}

impl FirecrackerRenames {
    /// Queue `session` as the name to rename to, starting the worker if this
    /// is the first rename. `apply` runs on the worker for each name it takes.
    fn push(
        self: &Arc<Self>,
        session: String,
        apply: impl Fn(String) + Send + 'static,
    ) -> Result<()> {
        {
            let mut state = self.state.lock().to_anyhow()?;
            if state.closed {
                return Ok(());
            }
            state.pending = Some(session);
        }
        self.wake.notify_one();
        let mut worker = self.worker.lock().to_anyhow()?;
        if worker.as_ref().is_some_and(|handle| !handle.is_finished()) {
            return Ok(());
        }
        let renames = Arc::clone(self);
        *worker = Some(
            std::thread::Builder::new()
                .name("firecracker-rename".to_owned())
                .spawn(move || renames.run(apply))
                .context("failed to start the Firecracker rename thread")?,
        );
        Ok(())
    }

    fn run(&self, apply: impl Fn(String)) {
        loop {
            let session = {
                let Ok(mut state) = self.state.lock() else {
                    return;
                };
                while state.pending.is_none() && !state.closed {
                    match self.wake.wait(state) {
                        Ok(next) => state = next,
                        Err(_) => return,
                    }
                }
                match state.pending.take() {
                    Some(session) => session,
                    None => return,
                }
            };
            apply(session);
        }
    }

    /// Stop taking renames; the worker exits after the one it is on.
    fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
        }
        self.wake.notify_all();
    }

    fn close_and_join(&self) {
        self.close();
        let handle = self.worker.lock().ok().and_then(|mut worker| worker.take());
        if let Some(handle) = handle {
            if handle.join().is_err() {
                Err::<(), _>(anyhow!("the Firecracker rename thread panicked")).non_fatal();
            }
        }
    }
}

/// A resize held back while resizes are being cached: columns, rows, and the pixel width
/// and height when the client reported them.
type CachedResize = (u16, u16, Option<u16>, Option<u16>);

#[derive(Clone)]
pub struct ServerOsInputOutput {
    pty_backend: PtyBackendImpl,
    client_senders: Arc<Mutex<HashMap<ClientId, ClientSender>>>,
    /// Writer threads that have not finished draining yet. See `LiveWriters`.
    live_writers: Arc<LiveWriters>,
    cached_resizes: Arc<Mutex<Option<BTreeMap<TerminalId, CachedResize>>>>,
    /// ssh forwards to the machines this session has remote panes on. One per
    /// host and option set, shared by every matching pane.
    remote_hosts: Arc<RemoteHosts>,
    /// The provider and the session name it keys this session's VMs on. A read
    /// guard is held across `ensure`, which can take minutes, so a rename (the
    /// only writer of the name) orders after every boot in flight and no boot
    /// ever runs under a stale name; a stop takes a read guard too, and relies
    /// on `firecracker_stopping` to cut a running `ensure` short.
    firecracker: Arc<RwLock<Option<(crate::firecracker::Provider, String)>>>,
    firecracker_stopping: Arc<AtomicBool>,
    /// Renames, applied in order by one thread. See `rename_firecracker_session`.
    firecracker_renames: Arc<FirecrackerRenames>,
}

#[cfg(not(windows))]
impl ServerOsInputOutput {
    /// Resolve where a command runs: nothing for a local one, the ssh host for an
    /// ssh one, and for a Firecracker one the endpoint the provider hands back once
    /// the VM is up. The returned command is the one to send there (a Firecracker
    /// command gets the guest cwd filled in); the second is the command as asked for,
    /// kept so a re-run can resolve it again - plus the identity the provider resolved,
    /// which is the VM's worktree and only when the caller named none. That much is not
    /// "where it ran this time" but what the pane *is*, and the pane outlives the run:
    /// it is what the restart hook names and what the held pane serializes. See the
    /// comment on the adoption below.
    fn resolve_target(
        &self,
        mut cmd: RunCommand,
        report_progress: Option<&mut dyn FnMut(&str)>,
    ) -> Result<(RunCommand, RunCommand, Option<RemoteEndpoint>)> {
        let rerun_cmd = cmd.clone();
        let (name, image, worktree) = match &cmd.target {
            PaneTarget::Local => return Ok((cmd, rerun_cmd, None)),
            PaneTarget::Ssh { host } => {
                let endpoint = RemoteEndpoint {
                    host: host.clone(),
                    ssh_options: SshOptions::default(),
                };
                return Ok((cmd, rerun_cmd, Some(endpoint)));
            },
            PaneTarget::Firecracker {
                vm,
                image,
                worktree,
            } => (vm.clone(), image.clone(), worktree.clone()),
        };
        let firecracker = self.firecracker.read().to_anyhow()?;
        if self.firecracker_stopping.load(Ordering::Acquire) {
            return Err(anyhow!("the Firecracker session is shutting down"));
        }
        let (provider, session) = firecracker.as_ref().context(
            "Firecracker is not configured; set firecracker_provider in the session config",
        )?;
        let ensured = match report_progress {
            Some(report_progress) => provider.ensure_with_progress(
                session,
                &name,
                image.as_deref(),
                worktree.as_deref(),
                &self.firecracker_stopping,
                report_progress,
            ),
            None => provider.ensure(
                session,
                &name,
                image.as_deref(),
                worktree.as_deref(),
                &self.firecracker_stopping,
            ),
        }?;
        if self.firecracker_stopping.load(Ordering::Acquire) {
            return Err(anyhow!("the Firecracker session is shutting down"));
        }
        if cmd.cwd.is_none() {
            cmd.cwd = ensured.cwd;
        }
        // The same adoption for the worktree the VM mirrors. A pane that joins a VM
        // somebody else provisioned names no worktree, and without one nothing downstream
        // can say which checkout its guest folder is a copy of: not the daemon working out
        // the pane's project, not the layout serializer writing `firecracker_worktree=`,
        // not a duplicate of the pane. The provider has it on record, so ask rather than
        // guess - and only when the caller named none, since what the caller named is what
        // the VM was ensured with.
        //
        // Both commands, because the two halves reach different readers and neither covers
        // the other. `cmd` is the transport's, which `remote_pane_worktree` reads back for
        // the manifest. `rerun_cmd` is the one `HoldPane` carries: it becomes the held
        // pane's `invoked_with`, which is what the layout serializer writes
        // `firecracker_worktree=` from, and it is what the agent restart hook reads its
        // `agent_firecracker_*` payload off - all-or-nothing on the worktree being there.
        let mut rerun_cmd = rerun_cmd;
        if let Some(worktree) = &ensured.worktree {
            for target in [&mut cmd.target, &mut rerun_cmd.target] {
                if let PaneTarget::Firecracker {
                    worktree: named, ..
                } = target
                {
                    if named.is_none() {
                        *named = Some(worktree.clone());
                    }
                }
            }
        }
        let endpoint = RemoteEndpoint {
            host: ensured.ssh_host,
            ssh_options: SshOptions {
                identity_file: ensured.ssh_identity_file,
                known_hosts_file: ensured.ssh_known_hosts_file,
            },
        };
        Ok((cmd, rerun_cmd, Some(endpoint)))
    }

    /// Tell the provider the session's new name and adopt it as the key for every later
    /// request. A provider that refuses leaves both sides on the old name, so they still
    /// agree with each other - only the CLI's view differs, which the error says.
    fn rename_firecracker_session_now(&self, session: &str) -> Result<()> {
        let mut firecracker = self.firecracker.write().to_anyhow()?;
        if self.firecracker_stopping.load(Ordering::Acquire) {
            return Ok(());
        }
        let Some((provider, current)) = firecracker.as_mut() else {
            return Ok(());
        };
        if current == session {
            return Ok(());
        }
        provider.rename_session(current, session).with_context(|| {
            format!(
                "failed to rename the Firecracker session {current:?} to {session:?}; \
                     its VMs stay under {current:?}"
            )
        })?;
        *current = session.to_owned();
        Ok(())
    }

    fn resolve_spawn(
        &self,
        terminal_id: TerminalId,
        terminal_action: TerminalAction,
        default_editor: Option<PathBuf>,
        report_progress: &mut dyn FnMut(&str),
    ) -> Result<ResolvedSpawn> {
        let (command, _) = build_command(terminal_action, default_editor);
        let display_host = command.display_host();
        let (command, rerun_command, endpoint) = self
            .resolve_target(command, Some(report_progress))
            .map_err(|error| {
                pane_start_failed(
                    terminal_id,
                    display_host.as_deref().unwrap_or("firecracker"),
                    error,
                )
            })?;
        let endpoint = endpoint.context("a local command has no endpoint to resolve")?;
        Ok(ResolvedSpawn {
            terminal_id,
            command,
            rerun_command,
            endpoint,
        })
    }

    fn spawn_resolved(
        &self,
        resolved: ResolvedSpawn,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<(TerminalId, Box<dyn AsyncReader>, Option<ProcessId>)> {
        let terminal_id = resolved.terminal_id;
        let reader = self.open_remote(resolved, quit_cb)?;
        // Deliberately no pid. The child's pid is meaningful only on its own
        // machine, and handing one back here would put it in
        // `id_to_child_pid`, where every consumer would treat it as local —
        // `kill`, `sysinfo`, and the plugin-facing `GetPanePid` alike.
        // `None` makes those paths skip the pane instead, which is what the
        // remote-aware replacements rely on.
        Ok((terminal_id, reader, None))
    }

    /// Open a pane on a resolved endpoint's broker and register its transport
    /// here: the one sequence every remote spawn and rerun ends with.
    fn open_remote(
        &self,
        resolved: ResolvedSpawn,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<Box<dyn AsyncReader>> {
        let ResolvedSpawn {
            terminal_id,
            command,
            rerun_command,
            endpoint: RemoteEndpoint { host, ssh_options },
        } = resolved;
        let (conn, reader_socket) = self
            .remote_hosts
            .open_pane_with_ssh_options(&host, &command, None, terminal_id, ssh_options)
            .map_err(|error| pane_start_failed(terminal_id, &host, error))?;
        self.pty_backend
            .register_remote_terminal(terminal_id, conn, reader_socket, quit_cb, rerun_command)
            .with_context(|| format!("failed to open a pane on {host}"))
    }

    fn resolve_rerun(
        &self,
        terminal_id: TerminalId,
        command: RunCommand,
        report_progress: &mut dyn FnMut(&str),
    ) -> Result<ResolvedSpawn> {
        let display_host = command.display_host();
        let (command, rerun_command, endpoint) = self
            .resolve_target(command, Some(report_progress))
            .map_err(|error| {
                pane_start_failed(
                    terminal_id,
                    display_host.as_deref().unwrap_or("firecracker"),
                    error,
                )
            })?;
        let endpoint = endpoint.context("a local command has no endpoint to resolve")?;
        Ok(ResolvedSpawn {
            terminal_id,
            command,
            rerun_command,
            endpoint,
        })
    }

    fn rerun_resolved(
        &self,
        resolved: ResolvedSpawn,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<(Box<dyn AsyncReader>, Option<ProcessId>)> {
        // The pane being re-run is held, not closed, so the broker still holds
        // the session it was held from. Registering over it would drop this
        // side's only handle on it and leave it to the orphan timeout; and the
        // id has to be reserved again once cleared, or the next spawn takes it.
        self.clear_terminal_id(resolved.terminal_id).non_fatal();
        self.pty_backend.reserve_terminal_id(resolved.terminal_id);
        let reader = self.open_remote(resolved, quit_cb)?;
        // No pid, for the same reason `spawn_resolved` gives none.
        Ok((reader, None))
    }

    /// Resume a warped-in remote pane on the broker holding it, and give this
    /// session a reader for it.
    ///
    /// Split out from `adopt_remote_terminal` so that the id reservation there
    /// has a single fallible body to clean up after, the way `adopt_terminal`
    /// and `adopt_into_terminal_id` are split for the local case.
    fn resume_remote_pane(
        &self,
        terminal_id: TerminalId,
        handoff: &RemotePaneHandoff,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<AdoptedTerminal> {
        let resumed = self
            .remote_hosts
            .reconnect(&handoff.host, &handoff.pane_token, handoff.received)
            .map_err(|e| pane_start_failed(terminal_id, &handoff.host, e))?;

        let conn = Arc::new(
            RemoteConn::new(
                resumed.write_socket,
                handoff.host.clone(),
                handoff.pane_token.clone(),
                handoff.cmd.clone(),
                Some(self.remote_hosts.clone() as Arc<dyn Reconnector>),
            )
            .context("failed to build a connection for an adopted remote pane")?,
        );
        // The stream is already this far in. Left at zero, a later reconnect
        // would ask the broker to replay from the beginning, on top of a grid
        // that already shows it.
        conn.restore_received(handoff.received, resumed.dropped);

        let reader = self
            .pty_backend
            .register_remote_terminal(
                terminal_id,
                conn,
                resumed.read_socket,
                quit_cb,
                handoff.cmd.clone(),
            )
            .context("failed to register an adopted remote pane")?;

        Ok(AdoptedTerminal {
            terminal_id,
            reader,
            // No pid, for the same reason `spawn_terminal` gives none for a
            // remote pane: a pid from another machine names an unrelated local
            // process rather than nothing at all.
            child_pid: None,
            cmd: handoff.cmd.clone(),
            // A replay with a hole cannot be spliced silently into the screen
            // the pane arrives with. Say so where the pane will show it, the way
            // `RemoteReader::on_reconnected` does for an outage.
            buffered: warp_replay_notice(resumed.dropped),
            // The broker reports an exit that happened while nobody was attached
            // as part of the resume, so it arrives through the reader rather
            // than being known here.
            child_exit_status: None,
        })
    }
}

/// What to put on a warped-in pane's screen about output lost in transit.
///
/// Empty in the ordinary case: a handoff measured in milliseconds does not
/// overrun a 4 MiB ring, and a pane that says nothing is the right amount to say
/// about a move that worked.
#[cfg(not(windows))]
fn warp_replay_notice(dropped: u64) -> Vec<u8> {
    if dropped == 0 {
        return Vec::new();
    }
    format!("\x1b[33m── {dropped} bytes of output were lost moving this pane ──\x1b[0m\r\n")
        .into_bytes()
}

/// Turn a failure to open a pane on another machine into one the pty thread can
/// show the user instead of dying on.
///
/// The terminal id is the reserved one, which only this side knows: a
/// `CommandNotFound` rebuilt from the broker's answer carries a placeholder id
/// (the far machine has never heard of ours), and everything else arrives as
/// plain `anyhow`, which the pty thread treats as fatal. Both become a
/// `ZellijError` naming the pane the message belongs in.
fn pane_start_failed(terminal_id: TerminalId, host: &str, err: anyhow::Error) -> anyhow::Error {
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
pub struct NullAsyncReader;

pub use zellij_process::AsyncReader;

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
        id: TerminalId,
        cols: u16,
        rows: u16,
        width_in_pixels: Option<u16>,
        height_in_pixels: Option<u16>,
    ) -> Result<()>;
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
    // reserves a terminal id without actually opening a terminal
    fn reserve_terminal_id(&self) -> Result<TerminalId> {
        unimplemented!()
    }
    /// Write bytes to the standard input of the virtual terminal referred to by `terminal_id`.
    fn write_to_tty_stdin(&self, terminal_id: TerminalId, buf: &[u8]) -> Result<usize>;
    /// Wait until all output written to the terminal has been transmitted.
    fn tcdrain(&self, terminal_id: TerminalId) -> Result<()>;
    /// Terminate the process with process ID `pid`. (SIGHUP)
    fn kill(&self, pid: ProcessId) -> Result<()>;
    /// Terminate the process with process ID `pid`. (SIGKILL)
    fn force_kill(&self, pid: ProcessId) -> Result<()>;
    /// Send SIGINT to the process with process ID `pid`
    fn send_sigint(&self, pid: ProcessId) -> Result<()>;
    /// Continue a stopped process group, the way a shell's `fg` does.
    fn send_sigcont(&self, pid: ProcessId) -> Result<()>;
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
    /// Whether new panes get a detachable pty-host supervisor. Config only
    /// arrives with the first client, long after the backend is built, so this is
    /// set once at session init. Defaulted so mock implementations are unaffected.
    fn set_pty_host_enabled(&self, _enabled: bool) {}
    fn set_firecracker_provider(
        &self,
        _provider: Option<PathBuf>,
        _timeout_secs: Option<zellij_types::units::ElapsedSeconds>,
        _session: String,
    ) -> Result<()> {
        Ok(())
    }
    fn stop_firecracker_session(&self) -> Result<()> {
        Ok(())
    }
    /// The session was renamed: move its Firecracker VMs with it, so the management
    /// commands, which resolve the session by its current name, keep finding them.
    fn rename_firecracker_session(&self, _session: String) {}
    fn begin_firecracker_shutdown(&self) {}
    /// Terminate every pty-host supervisor this server owns, on the way out.
    fn shutdown_pty_hosts(&self) {}
    fn begin_confirmed_pty_host_shutdown(&self) -> Result<()> {
        Ok(())
    }
    fn confirm_pty_host_shutdown(&self) -> Result<()> {
        Ok(())
    }
    /// Relabel local supervisors before publishing a session rename.
    fn rename_pty_host_session(&self, _old: &SessionName, new: &SessionName) -> Result<()> {
        zellij_runtime::envs::set_session_name(new.clone());
        Ok(())
    }
    /// Terminate the unattached supervisors labelled with `session`, on the way
    /// out of a `KillSession` only. They are what an earlier incarnation of this
    /// session left behind when it died without shutting down; killing the
    /// session by name is the user saying those panes are done too.
    fn terminate_orphaned_pty_hosts(&self, _session: &str) -> Result<()> {
        Ok(())
    }
    /// Wait for queued client messages to reach their sockets, on the way out.
    ///
    /// `send_to_client` only queues; a per-client writer thread does the actual
    /// write. Those threads are detached, so without this the process can exit
    /// between "queued the `Exit`" and "wrote the `Exit`", and the client sees
    /// a bare hangup instead of being told why the session ended.
    fn flush_client_writers(&self, _deadline: Duration) {}
    /// Whether this pane's process lives on another machine.
    ///
    /// The question every pid-shaped operation has to ask first. `id_to_child_pid`
    /// holds nothing usable for a remote pane, and a pid from another machine is
    /// worse than no pid: it will happily name an unrelated *local* process, so
    /// a `kill` meant for a pane lands somewhere else entirely.
    fn is_remote_pane(&self, _terminal_id: TerminalId) -> bool {
        false
    }
    /// Whether this pane runs in a Firecracker guest the provider started.
    ///
    /// The guest has no route to the session socket, so its hook reports arrive
    /// as a control sequence on the pane's own byte stream. That channel is
    /// trusted for these panes only: a program in an ordinary ssh pane could
    /// otherwise forge agent records and hook events for the plugins.
    fn is_firecracker_pane(&self, _terminal_id: TerminalId) -> bool {
        false
    }
    /// What the far side last reported about a remote pane: the cwd and
    /// foreground command the local `sysinfo` and `tcgetpgrp` cannot see.
    fn remote_pane_status(&self, _terminal_id: TerminalId) -> Option<RemoteStatus> {
        None
    }
    fn workflow_remote_panes(&self) -> Vec<serde_json::Value> {
        vec![]
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
    /// Raw on purpose: `hocus-agentd`'s `poll::repository` already turns exactly
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
    /// The host worktree a Firecracker pane's VM mirrors, as the VM was actually
    /// provisioned rather than as the caller asked.
    ///
    /// Read off the transport's own command for the reason `remote_pane_host` is: a pane
    /// that joined a VM somebody else named the worktree for asked for none, and the
    /// answer is the one `resolve_target` adopted from the provider.
    fn remote_pane_worktree(&self, _terminal_id: TerminalId) -> Option<PathBuf> {
        None
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
    fn cache_resizes(&mut self) {}
    fn apply_cached_resizes(&mut self) -> Vec<(TerminalId, u16, u16)> {
        Vec::new()
    }
}

impl ServerOsApi for ServerOsInputOutput {
    fn set_terminal_size_using_terminal_id(
        &self,
        id: TerminalId,
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
    ) -> Result<(TerminalId, Box<dyn AsyncReader>, Option<ProcessId>)> {
        let err_context = || "failed to spawn terminal".to_string();

        let terminal_id = self
            .pty_backend
            .next_terminal_id()
            .context("no more terminal IDs left to allocate")?;

        self.pty_backend.reserve_terminal_id(terminal_id);

        let (cmd, failover_cmd) = build_command(terminal_action, default_editor);
        let display_host = cmd.display_host();
        let (cmd, rerun_cmd, endpoint) = self.resolve_target(cmd, None).map_err(|e| {
            pane_start_failed(
                terminal_id,
                display_host.as_deref().unwrap_or("firecracker"),
                e,
            )
        })?;

        if let Some(endpoint) = endpoint {
            let async_reader = self.open_remote(
                ResolvedSpawn {
                    terminal_id,
                    command: cmd,
                    rerun_command: rerun_cmd,
                    endpoint,
                },
                quit_cb,
            )?;
            return Ok((terminal_id, async_reader, None));
        }

        let (async_reader, child_pid) = self
            .pty_backend
            .spawn_terminal(cmd, failover_cmd, quit_cb, terminal_id)
            .with_context(err_context)?;

        Ok((terminal_id, async_reader, Some(child_pid)))
    }
    fn resolve_spawn(
        &self,
        terminal_id: TerminalId,
        terminal_action: TerminalAction,
        default_editor: Option<PathBuf>,
        report_progress: &mut dyn FnMut(&str),
    ) -> Result<ResolvedSpawn> {
        ServerOsInputOutput::resolve_spawn(
            self,
            terminal_id,
            terminal_action,
            default_editor,
            report_progress,
        )
    }
    fn spawn_resolved(
        &self,
        resolved: ResolvedSpawn,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<(TerminalId, Box<dyn AsyncReader>, Option<ProcessId>)> {
        ServerOsInputOutput::spawn_resolved(self, resolved, quit_cb)
    }
    fn reserve_terminal_id(&self) -> Result<TerminalId> {
        let terminal_id = self
            .pty_backend
            .next_terminal_id()
            .context("no more terminal IDs available")?;
        self.pty_backend.reserve_terminal_id(terminal_id);
        Ok(terminal_id)
    }
    fn write_to_tty_stdin(&self, terminal_id: TerminalId, buf: &[u8]) -> Result<usize> {
        self.pty_backend.write_to_tty_stdin(terminal_id, buf)
    }
    fn tcdrain(&self, terminal_id: TerminalId) -> Result<()> {
        self.pty_backend.tcdrain(terminal_id)
    }
    fn is_remote_pane(&self, terminal_id: TerminalId) -> bool {
        self.pty_backend.is_remote(terminal_id)
    }
    fn is_firecracker_pane(&self, terminal_id: TerminalId) -> bool {
        self.pty_backend.is_firecracker(terminal_id)
    }
    fn remote_pane_status(&self, terminal_id: TerminalId) -> Option<RemoteStatus> {
        self.pty_backend.remote_status(terminal_id)
    }
    fn workflow_remote_panes(&self) -> Vec<serde_json::Value> {
        self.pty_backend.workflow_remote_panes()
    }
    fn signal_remote_child(&self, terminal_id: TerminalId, signal: ChildSignal) -> Result<()> {
        self.pty_backend.signal_remote_child(terminal_id, signal)
    }
    fn remote_pane_ids(&self) -> Vec<TerminalId> {
        self.pty_backend.remote_pane_ids()
    }
    fn remote_pane_git_probe(
        &self,
        terminal_id: TerminalId,
        cwd: &str,
        refresh_token: Option<&str>,
    ) -> Result<Option<RemoteGitProbe>, String> {
        self.pty_backend
            .remote_git_probe(terminal_id, cwd, refresh_token)
    }
    fn remote_pane_host(&self, terminal_id: TerminalId) -> Option<String> {
        self.pty_backend.remote_host(terminal_id)
    }
    fn remote_pane_worktree(&self, terminal_id: TerminalId) -> Option<PathBuf> {
        self.pty_backend.remote_worktree(terminal_id)
    }
    fn box_clone(&self) -> Box<dyn ServerOsApi> {
        Box::new((*self).clone())
    }
    fn kill(&self, pid: ProcessId) -> Result<()> {
        self.pty_backend.kill(pid)
    }
    fn force_kill(&self, pid: ProcessId) -> Result<()> {
        self.pty_backend.force_kill(pid)
    }
    fn send_sigint(&self, pid: ProcessId) -> Result<()> {
        self.pty_backend.send_sigint(pid)
    }
    fn send_sigcont(&self, pid: ProcessId) -> Result<()> {
        self.pty_backend.send_sigcont(pid)
    }
    fn set_child_state_callback(&self, cb: ChildStateCb) {
        self.pty_backend.set_child_state_callback(cb)
    }
    fn set_agent_report_callback(&self, cb: AgentReportCb) {
        self.pty_backend.set_agent_report_callback(cb)
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
        let stream_fd = {
            use std::os::fd::AsFd;
            // The enum has no fd accessor of its own; the unix variant is the
            // only one that exists on this platform, and it does.
            match &stream {
                LocalSocketStream::UdSocket(uds) => uds.as_fd().try_clone_to_owned(),
            }
        }
        .context("failed to duplicate client socket for cleanup")?;
        let receiver = IpcReceiverWithContext::new(stream);
        let sender = ClientSender::new(
            client_id,
            receiver
                .get_sender()
                .context("failed to duplicate client socket for writing")?,
            self.live_writers.clone(),
            stream_fd,
        )
        .context("failed to start client writer")?;
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
        if let Some(sender) = client_senders.remove(&client_id) {
            // Order matters: dropping `sender` closes the queue so the writer
            // thread drains and sends its final `Exit`, and `shutdown_reads`
            // touches only the read half, so that write still lands. What it
            // does end is the router's blocked read, which nothing else here
            // could reach.
            sender.shutdown_reads();
        }
        Ok(())
    }

    fn load_palette(&self) -> Palette {
        default_palette()
    }

    fn get_cwd(&self, pid: ProcessId) -> Option<PathBuf> {
        let mut system_info = System::new();
        let sysinfo_pid = sysinfo::Pid::from_u32(pid.get());
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

    fn get_cwds(
        &self,
        pids: Vec<ProcessId>,
    ) -> (HashMap<ProcessId, PathBuf>, HashMap<ProcessId, Vec<String>>) {
        let mut system_info = System::new();
        let mut cwds = HashMap::new();
        let mut cmds = HashMap::new();

        let sysinfo_pids: Vec<sysinfo::Pid> = pids
            .iter()
            .map(|&p| sysinfo::Pid::from_u32(p.get()))
            .collect();
        let refresh_kind = ProcessRefreshKind::nothing()
            .with_cwd(UpdateKind::Always)
            .with_cmd(UpdateKind::Always);
        system_info.refresh_processes_specifics(
            ProcessesToUpdate::Some(&sysinfo_pids),
            false,
            refresh_kind,
        );

        for pid in pids {
            let sysinfo_pid = sysinfo::Pid::from_u32(pid.get());
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
        panes: &[(TerminalId, ProcessId)],
        post_hook: &Option<String>,
    ) -> HashMap<TerminalId, Vec<String>> {
        let mut terminal_to_fg_pid: HashMap<TerminalId, ProcessId> = HashMap::new();
        for &(terminal_id, shell_pid) in panes {
            if let Some(fpgid) = self.pty_backend.tcgetpgrp(terminal_id) {
                if let Ok(fpgid) = ProcessId::try_from(fpgid) {
                    if fpgid.get() > 0 && fpgid != shell_pid {
                        terminal_to_fg_pid.insert(terminal_id, fpgid);
                    }
                }
            }
        }
        if terminal_to_fg_pid.is_empty() {
            return HashMap::new();
        }

        let sysinfo_pids: Vec<sysinfo::Pid> = terminal_to_fg_pid
            .values()
            .map(|&p| sysinfo::Pid::from_u32(p.get()))
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
            let Some(process) = system_info.process(sysinfo::Pid::from_u32(fg_pid.get())) else {
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
        terminal_id: TerminalId,
        run_command: RunCommand,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<(Box<dyn AsyncReader>, Option<ProcessId>)> {
        // A held remote pane re-runs on the machine it was held on. Without this
        // the command would quietly run *here* under the same pane, and the pane
        // still open on the far machine would be left to the orphan timeout.
        let display_host = run_command.display_host();
        let (run_command, rerun_command, endpoint) =
            self.resolve_target(run_command, None).map_err(|e| {
                pane_start_failed(
                    terminal_id,
                    display_host.as_deref().unwrap_or("firecracker"),
                    e,
                )
            })?;
        if let Some(endpoint) = endpoint {
            return self.rerun_resolved(
                ResolvedSpawn {
                    terminal_id,
                    command: run_command,
                    rerun_command,
                    endpoint,
                },
                quit_cb,
            );
        }

        self.clear_terminal_id(terminal_id).non_fatal();
        let (async_reader, child_pid) =
            self.pty_backend
                .spawn_terminal(run_command, None, quit_cb, terminal_id)?;
        Ok((async_reader, Some(child_pid)))
    }
    fn resolve_rerun(
        &self,
        terminal_id: TerminalId,
        run_command: RunCommand,
        report_progress: &mut dyn FnMut(&str),
    ) -> Result<ResolvedSpawn> {
        ServerOsInputOutput::resolve_rerun(self, terminal_id, run_command, report_progress)
    }
    fn rerun_resolved(
        &self,
        resolved: ResolvedSpawn,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<(Box<dyn AsyncReader>, Option<ProcessId>)> {
        ServerOsInputOutput::rerun_resolved(self, resolved, quit_cb)
    }
    fn clear_terminal_id(&self, terminal_id: TerminalId) -> Result<()> {
        self.pty_backend.clear_terminal_id(terminal_id)
    }
    #[cfg(not(windows))]
    fn set_pty_host_enabled(&self, enabled: bool) {
        self.pty_backend.set_pty_host_enabled(enabled);
    }
    fn set_firecracker_provider(
        &self,
        provider: Option<PathBuf>,
        timeout_secs: Option<zellij_types::units::ElapsedSeconds>,
        session: String,
    ) -> Result<()> {
        let configured = provider
            .as_deref()
            .map(|path| crate::firecracker::provider_from_options(Some(path), timeout_secs))
            .transpose()?
            .map(|provider| (provider, session));
        *self.firecracker.write().to_anyhow()? = configured;
        self.firecracker_stopping.store(false, Ordering::Release);
        Ok(())
    }
    fn stop_firecracker_session(&self) -> Result<()> {
        self.firecracker_stopping.store(true, Ordering::Release);
        // Nothing queued behind the flag can still change the name, and the
        // worker exits once it sees the queue closed; joining it here keeps
        // its provider call from racing the stop below.
        self.firecracker_renames.close_and_join();
        let guard = self.firecracker.read().to_anyhow()?;
        if let Some((provider, session)) = guard.as_ref() {
            // The clients are gone by the time this runs, so the log is the only place a
            // merge conflict can be told; the `.guest` copies in the worktree are the other.
            for warning in provider.stop_session(session)? {
                log::warn!("Firecracker worktree merge: {warning}");
            }
        }
        Ok(())
    }
    fn rename_firecracker_session(&self, session: String) {
        // `resolve_target` holds the provider lock across an `ensure`, which can
        // take minutes, and this is called from the screen thread. One worker
        // applies renames in the order they were asked for; two quick renames
        // collapse to the last name, which is the only one that matters.
        let server = self.clone();
        self.firecracker_renames
            .push(session, move |session| {
                server.rename_firecracker_session_now(&session).non_fatal()
            })
            .non_fatal();
    }
    fn begin_firecracker_shutdown(&self) {
        self.firecracker_stopping.store(true, Ordering::Release);
        self.firecracker_renames.close();
    }
    #[cfg(not(windows))]
    fn begin_confirmed_pty_host_shutdown(&self) -> Result<()> {
        self.pty_backend.begin_confirmed_pty_host_shutdown()
    }
    fn confirm_pty_host_shutdown(&self) -> Result<()> {
        self.pty_backend.confirm_pty_host_shutdown()
    }
    fn shutdown_pty_hosts(&self) {
        self.pty_backend.shutdown_pty_hosts();
        // The ssh forwards go with them. The brokers and their panes on the far
        // side survive by design — only an explicit terminate ends those, which
        // `Pty::drop` sends for Firecracker panes alone, since their VM stops
        // with the session — but leaving forwards behind would leak ssh
        // processes and bound sockets on the way out.
        self.remote_hosts.shutdown();
    }
    #[cfg(not(windows))]
    fn rename_pty_host_session(&self, old: &SessionName, new: &SessionName) -> Result<()> {
        self.pty_backend.rename_pty_host_session(old, new)
    }
    #[cfg(not(windows))]
    fn terminate_orphaned_pty_hosts(&self, session: &str) -> Result<()> {
        crate::pty_host::cli::terminate_orphans_of_session(session).map(|_| ())
    }

    fn flush_client_writers(&self, deadline: Duration) {
        // Closing each queue is what lets a writer thread finish: `pop` returns
        // `None` once the queue is closed and drained.
        for sender in self.client_senders.lock().unwrap().values() {
            sender.close();
        }
        if !self.live_writers.wait_until_drained(deadline) {
            log::warn!("timed out waiting for client writer threads to drain; some clients may not have been told why the session ended");
        }
    }
    #[cfg(not(windows))]
    fn release_terminal_for_warp(
        &self,
        terminal_id: TerminalId,
        budget: Duration,
    ) -> Result<ReleasedPane> {
        self.pty_backend
            .release_terminal_for_warp(terminal_id, budget)
    }
    #[cfg(not(windows))]
    fn check_terminal_can_be_warped(&self, terminal_id: TerminalId) -> Result<()> {
        self.pty_backend.check_terminal_can_be_warped(terminal_id)
    }
    #[cfg(not(windows))]
    fn adopt_terminal(
        &self,
        control_socket: &Path,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<AdoptedTerminal> {
        self.pty_backend.adopt_terminal(control_socket, quit_cb)
    }
    /// The remote half of `adopt_terminal`, and structurally the same operation:
    /// pick up a pane whose process someone else let go of.
    ///
    /// It lives here rather than on the backend because the reconnect belongs to
    /// *this server's* `RemoteHosts` — the adopting session builds its own ssh
    /// forward to the host rather than inheriting the releasing session's, which
    /// is what makes the two sessions independent afterwards.
    ///
    /// `RemoteHosts::reconnect` is the same call the reader makes when a link
    /// drops. From the broker's point of view a warp and an outage are the same
    /// event, which is the reason this feature is small.
    #[cfg(not(windows))]
    fn adopt_remote_terminal(
        &self,
        handoff: &RemotePaneHandoff,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<AdoptedTerminal> {
        let terminal_id = self
            .pty_backend
            .next_terminal_id()
            .ok_or_else(|| anyhow!("failed to allocate a terminal id"))?;
        self.pty_backend.reserve_terminal_id(terminal_id);
        match self.resume_remote_pane(terminal_id, handoff, quit_cb) {
            Ok(adopted) => Ok(adopted),
            Err(e) => {
                // Undo the reservation, but *not* through `clear_terminal_id` —
                // there is no transport recorded to terminate, and a failed
                // adoption must leave the pane running for the next caller, or
                // the rollback has nothing to give back.
                self.pty_backend.forget_reserved_terminal_id(terminal_id);
                Err(e)
            },
        }
    }
    fn cache_resizes(&mut self) {
        if self.cached_resizes.lock().unwrap().is_none() {
            *self.cached_resizes.lock().unwrap() = Some(BTreeMap::new());
        }
    }
    fn apply_cached_resizes(&mut self) -> Vec<(TerminalId, u16, u16)> {
        let mut applied = Vec::new();
        let mut cached_resizes = self.cached_resizes.lock().unwrap().take();
        if let Some(cached_resizes) = cached_resizes.as_mut() {
            for (terminal_id, (cols, rows, width_in_pixels, height_in_pixels)) in
                cached_resizes.iter()
            {
                // A resize that did not go through is logged and left out of `applied`,
                // so the recorder does not record a size the pane never got.
                match self.set_terminal_size_using_terminal_id(
                    *terminal_id,
                    *cols,
                    *rows,
                    *width_in_pixels,
                    *height_in_pixels,
                ) {
                    Ok(()) => applied.push((*terminal_id, *cols, *rows)),
                    Err(e) => Err::<(), _>(e)
                        .context("failed to apply a cached resize")
                        .non_fatal(),
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
    server_os_input_with_backend(PtyBackendImpl::new()?)
}

/// An absent argument samples stdin; explicit JSON null uses OS PTY defaults.
pub fn get_server_os_input_from_termios_arg(
    arg: Option<&str>,
) -> Result<ServerOsInputOutput, std::io::Error> {
    match arg {
        None => get_server_os_input(),
        Some(json) => {
            let snapshot = serde_json::from_str(json).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("invalid --server-termios: {e}"),
                )
            })?;
            get_server_os_input_with_termios(snapshot)
        },
    }
}

pub fn get_server_os_input_with_termios(
    termios: Option<zellij_runtime::termios::WireTermios>,
) -> Result<ServerOsInputOutput, std::io::Error> {
    server_os_input_with_backend(PtyBackendImpl::with_termios(
        termios
            .as_ref()
            .map(zellij_runtime::termios::WireTermios::to_termios),
    )?)
}

fn server_os_input_with_backend(
    pty_backend: PtyBackendImpl,
) -> Result<ServerOsInputOutput, std::io::Error> {
    Ok(ServerOsInputOutput {
        pty_backend,
        remote_hosts: Arc::new(RemoteHosts::new()),
        firecracker: Arc::new(RwLock::new(None)),
        firecracker_stopping: Arc::new(AtomicBool::new(false)),
        firecracker_renames: Arc::new(FirecrackerRenames::default()),
        client_senders: Arc::new(Mutex::new(HashMap::new())),
        live_writers: Arc::new(LiveWriters::default()),
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
