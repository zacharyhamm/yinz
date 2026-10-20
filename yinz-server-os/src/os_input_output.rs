use crate::{ClientId, PaneId, TerminalId};
use yinz_types::internal_ids::{ProcessId, SessionName};
use yinz_types::style::default_palette;

use interprocess::local_socket::Stream as LocalSocketStream;

use crate::os_input_output_unix::UnixPtyBackend as PtyBackendImpl;
use crate::pty_host::client::{AgentReportCb, ChildStateCb, ReleasedPane};
use crate::pty_host::remote::{
    conn::{Reconnector, RemoteGitProbe},
    protocol::ChildSignal,
    ssh::{RemoteHosts, SshOptions},
    RemoteConn, RemoteStatus,
};
use yinz_bus::os_input_output::{
    AdoptedTerminal, AsyncReader, RemoteEndpoint, RenderBacklog, RenderReady, ResolvedSpawn,
    ServerOsApi,
};
use yinz_types::command::PaneTarget;
use yinz_types::data::RemotePaneHandoff;

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use tempfile::tempfile;
use yinz_runtime::errors::prelude::*;
use {
    yinz_ipc::ipc::ClientToServerMsg, yinz_ipc::ipc::ExitReason,
    yinz_ipc::ipc::IpcReceiverWithContext, yinz_ipc::ipc::IpcSenderWithContext,
    yinz_ipc::ipc::ServerToClientMsg, yinz_types::data::Palette,
    yinz_types::input::command::RunCommand, yinz_types::input::command::TerminalAction,
};

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    env,
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Condvar, LockResult, Mutex, MutexGuard},
    time::{Duration, Instant},
};

pub(crate) use yinz_process::command::command_exists;

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
            if payload.path.is_relative()
                && let Some(cwd) = payload.cwd.as_ref()
            {
                payload.path = cwd.join(payload.path);
            }
            let mut command = default_editor.unwrap_or_else(|| {
                PathBuf::from(
                    env::var("EDITOR")
                        .unwrap_or_else(|_| env::var("VISUAL").unwrap_or_else(|_| "vi".into())),
                )
            });

            let mut args = vec![];

            if !command.exists() {
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
/// The screen paces itself against `render_backlog_and_notify`, skipping frames while
/// every client has renders queued, so a slow client on its own sees fewer,
/// larger frames. A slow client among healthy ones is not waited for. The caps
/// below are the last resort for it, and for a client that stops reading: ordered
/// messages are bounded by `SEND_QUEUE_CONTROL_CAPACITY`, and queued render
/// allocations by `SEND_QUEUE_RENDER_BYTE_CAPACITY`. A client that hits them is
/// disconnected so it can reattach with a complete repaint.
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
    /// `Render`s in `control`.
    queued_renders: usize,
    render_ready: Option<(usize, RenderReady)>,
    pending_pane_renders: BTreeMap<yinz_types::data::PaneId, ServerToClientMsg>,
    in_flight: Option<InFlightSend>,
    closed: bool,
}

#[derive(Clone, Copy, Debug)]
struct InFlightSend {
    kind: &'static str,
    render_bytes: usize,
    started: Instant,
}

#[derive(Debug)]
struct QueueOverflow {
    reason: &'static str,
    ordered: usize,
    pending_pane_renders: usize,
    render_bytes: usize,
    rejected_render_len: usize,
    rejected_render_capacity: usize,
    in_flight: Option<InFlightSend>,
}

impl SendQueueState {
    /// The registered readiness callback, once the backlog has fallen below its threshold or
    /// the queue has closed. The caller runs it after unlocking: it re-enters the screen.
    fn take_render_ready(&mut self) -> Option<RenderReady> {
        let threshold = self.render_ready.as_ref()?.0;
        if self.closed || self.queued_renders + self.pending_pane_renders.len() < threshold {
            self.render_ready.take().map(|(_, ready)| ready)
        } else {
            None
        }
    }
    fn overflow(&self, reason: &'static str, msg: &ServerToClientMsg) -> QueueOverflow {
        let (rejected_render_len, rejected_render_capacity) = match msg {
            ServerToClientMsg::Render { content } => (content.len(), content.capacity()),
            _ => (0, 0),
        };
        QueueOverflow {
            reason,
            ordered: self.control.len(),
            pending_pane_renders: self.pending_pane_renders.len(),
            render_bytes: self.render_bytes,
            rejected_render_len,
            rejected_render_capacity,
            in_flight: self.in_flight,
        }
    }
}

fn message_size_and_kind(msg: &ServerToClientMsg) -> (&'static str, usize) {
    let kind = msg.into();
    let render_bytes = match msg {
        ServerToClientMsg::Render { content } => content.len(),
        ServerToClientMsg::PaneRenderUpdate {
            viewport,
            scrollback,
            ..
        } => viewport
            .iter()
            .chain(scrollback.iter().flatten())
            .map(String::len)
            .sum(),
        _ => 0,
    };
    (kind, render_bytes)
}

/// Which slot of the queue a message belongs in.
enum SendQueueSlot {
    PaneRender(yinz_types::data::PaneId),
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
    ControlQueueFull(QueueOverflow),
}

impl CoalescingSendQueue {
    fn new() -> Self {
        CoalescingSendQueue {
            state: Mutex::new(SendQueueState::default()),
            message_available: Condvar::new(),
        }
    }
    fn recover_state<'a>(
        &self,
        locked: LockResult<MutexGuard<'a, SendQueueState>>,
    ) -> MutexGuard<'a, SendQueueState> {
        locked.unwrap_or_else(|poisoned| {
            let mut state = poisoned.into_inner();
            // A panic may interrupt queue accounting or lose a render delta.
            // Disconnect instead of resuming with inconsistent terminal contents.
            let render_ready = state.render_ready.take();
            *state = SendQueueState {
                closed: true,
                render_ready,
                ..Default::default()
            };
            self.state.clear_poison();
            self.message_available.notify_all();
            log::warn!("closed poisoned client send queue");
            state
        })
    }
    /// Returns readiness work for the caller to run after releasing the sender registry.
    fn push_deferred(&self, msg: ServerToClientMsg) -> (SendQueuePush, Option<RenderReady>) {
        let mut state = self.recover_state(self.state.lock());
        if state.closed {
            // the writer is gone; pretend-queue so late senders don't error
            let ready = state.take_render_ready();
            return (SendQueuePush::Queued, ready);
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
                    return (
                        SendQueuePush::ControlQueueFull(state.overflow("message_count", &msg)),
                        None,
                    );
                }
                if let ServerToClientMsg::Render { content } = &msg {
                    if content.capacity() > SEND_QUEUE_RENDER_BYTE_CAPACITY {
                        return (
                            SendQueuePush::ControlQueueFull(
                                state.overflow("single_oversized_render", &msg),
                            ),
                            None,
                        );
                    }
                    if content.capacity() > SEND_QUEUE_RENDER_BYTE_CAPACITY - state.render_bytes {
                        return (
                            SendQueuePush::ControlQueueFull(
                                state.overflow("render_allocation_bytes", &msg),
                            ),
                            None,
                        );
                    }
                    state.render_bytes += content.capacity();
                    state.queued_renders += 1;
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
        let ready = state.take_render_ready();
        (outcome, ready)
    }
    #[cfg(test)]
    fn push(&self, msg: ServerToClientMsg) -> SendQueuePush {
        let (outcome, ready) = self.push_deferred(msg);
        notify(ready);
        outcome
    }
    /// Blocks until a message is available; `None` once closed and drained.
    fn pop(&self) -> Option<ServerToClientMsg> {
        let mut state = self.recover_state(self.state.lock());
        loop {
            if let Some(msg) = state.control.pop_front() {
                if let ServerToClientMsg::Render { content } = &msg {
                    state.render_bytes -= content.capacity();
                    state.queued_renders -= 1;
                }
                let (kind, render_bytes) = message_size_and_kind(&msg);
                state.in_flight = Some(InFlightSend {
                    kind,
                    render_bytes,
                    started: Instant::now(),
                });
                let ready = state.take_render_ready();
                drop(state);
                notify(ready);
                return Some(msg);
            }
            if let Some((_, msg)) = state.pending_pane_renders.pop_first() {
                let (kind, render_bytes) = message_size_and_kind(&msg);
                state.in_flight = Some(InFlightSend {
                    kind,
                    render_bytes,
                    started: Instant::now(),
                });
                let ready = state.take_render_ready();
                drop(state);
                notify(ready);
                return Some(msg);
            }
            if state.closed {
                let ready = state.take_render_ready();
                drop(state);
                notify(ready);
                return None;
            }
            state = self.recover_state(self.message_available.wait(state));
        }
    }
    fn close(&self) {
        let mut state = self.recover_state(self.state.lock());
        state.closed = true;
        self.message_available.notify_all();
        let ready = state.take_render_ready();
        drop(state);
        notify(ready);
    }
    fn finish_send(&self) {
        let mut state = self.recover_state(self.state.lock());
        state.in_flight = None;
        let ready = state.take_render_ready();
        drop(state);
        notify(ready);
    }
    #[cfg(test)]
    fn render_backlog(&self) -> RenderBacklog {
        self.render_backlog_and_notify(usize::MAX, None)
    }
    fn render_backlog_and_notify(
        &self,
        threshold: usize,
        ready: Option<&RenderReady>,
    ) -> RenderBacklog {
        let mut state = self.recover_state(self.state.lock());
        let backlog = if state.closed {
            RenderBacklog::default()
        } else {
            RenderBacklog {
                queued_renders: state.queued_renders + state.pending_pane_renders.len(),
            }
        };
        if backlog.queued_renders >= threshold
            && let Some(ready) = ready
        {
            state.render_ready = Some((threshold, ready.clone()));
        }
        let ready = state.take_render_ready();
        drop(state);
        notify(ready);
        backlog
    }
    #[cfg(test)]
    fn len(&self) -> usize {
        let state = self.recover_state(self.state.lock());
        state.control.len() + state.pending_pane_renders.len()
    }
}

/// Runs a readiness callback taken under the queue lock, once that lock is released.
fn notify(ready: Option<RenderReady>) {
    if let Some(ready) = ready {
        ready();
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
    queue: Arc<CoalescingSendQueue>,
    /// The connection's socket, kept only so that `remove_client` can shut its
    /// read half down. See `shutdown_reads`.
    stream_fd: std::os::fd::OwnedFd,
}

#[derive(Default)]
struct SlowWarnings {
    last_logged: Option<Instant>,
    suppressed: usize,
    max_suppressed: Duration,
}

impl SlowWarnings {
    fn record(&mut self, now: Instant, elapsed: Duration) -> Option<(usize, Duration)> {
        if elapsed < Duration::from_millis(100) {
            return None;
        }
        if self
            .last_logged
            .is_some_and(|last| now.duration_since(last) < Duration::from_secs(1))
        {
            self.suppressed += 1;
            self.max_suppressed = self.max_suppressed.max(elapsed);
            return None;
        }
        self.last_logged = Some(now);
        let summary = (self.suppressed, self.max_suppressed);
        self.suppressed = 0;
        self.max_suppressed = Duration::ZERO;
        Some(summary)
    }

    fn finish(&self) -> Option<(usize, Duration)> {
        (self.suppressed > 0).then_some((self.suppressed, self.max_suppressed))
    }
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
            let mut slow_warnings = SlowWarnings::default();
            while let Some(msg) = writer_queue.pop() {
                let started_at = Instant::now();
                let (message_kind, render_bytes) = message_size_and_kind(&msg);
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
                let elapsed = started_at.elapsed();
                writer_queue.finish_send();
                if let Some((suppressed, max_suppressed)) = slow_warnings.record(Instant::now(), elapsed) {
                    log::warn!(
                        "output pressure: server pid={} client_id={client_id} stage=ipc_serialization_plus_socket_delivery kind={message_kind} render_bytes={render_bytes} elapsed_ms={} suppressed={suppressed} max_suppressed_ms={} result={}",
                        std::process::id(), elapsed.as_millis(), max_suppressed.as_millis(),
                        if result.is_ok() { "ok" } else { "error" }
                    );
                }
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
            let started_at = Instant::now();
            let _ = sender.send_server_msg(ServerToClientMsg::Exit {
                exit_reason: ExitReason::Disconnect,
            });
            let elapsed = started_at.elapsed();
            if let Some((suppressed, max_suppressed)) = slow_warnings.record(Instant::now(), elapsed) {
                log::warn!("output pressure: server pid={} client_id={client_id} stage=ipc_serialization_plus_socket_delivery kind=Exit render_bytes=0 elapsed_ms={} suppressed={suppressed} max_suppressed_ms={}", std::process::id(), elapsed.as_millis(), max_suppressed.as_millis());
            }
            if let Some((suppressed, max_suppressed)) = slow_warnings.finish() {
                log::warn!("output pressure: server pid={} client_id={client_id} stage=ipc_serialization_plus_socket_delivery final suppressed={suppressed} max_suppressed_ms={}", std::process::id(), max_suppressed.as_millis());
            }
            // Last thing this thread does: from here on the socket has
            // everything it was ever going to get.
            finished_writers.deregister();
        });
        if let Err(error) = worker {
            live_writers.deregister();
            return Err(error);
        }
        Ok(ClientSender { queue, stream_fd })
    }
}

/// Finish an enqueue after releasing the sender registry, including readiness callbacks.
fn finish_enqueue(
    client_id: ClientId,
    (outcome, ready): (SendQueuePush, Option<RenderReady>),
) -> Result<()> {
    notify(ready);
    let err_context = || format!("failed to send or buffer message for client {client_id}");

    match outcome {
        SendQueuePush::Queued | SendQueuePush::CoalescedRender => Ok(()),
        SendQueuePush::ControlQueueFull(overflow) => {
            let in_flight = overflow
                .in_flight
                .map(|send| {
                    format!(
                        "kind={} render_bytes={} elapsed_ms={}",
                        send.kind,
                        send.render_bytes,
                        send.started.elapsed().as_millis()
                    )
                })
                .unwrap_or_else(|| "none".to_owned());
            log::warn!(
                "output pressure: server pid={} client_id={} queue overflow reason={} queued_messages={} ordered={}/{} pending_pane_renders={} render_allocation_bytes={}/{} rejected_render_len={} rejected_render_capacity={} in_flight={}",
                std::process::id(), client_id, overflow.reason,
                overflow.ordered + overflow.pending_pane_renders, overflow.ordered,
                SEND_QUEUE_CONTROL_CAPACITY, overflow.pending_pane_renders,
                overflow.render_bytes, SEND_QUEUE_RENDER_BYTE_CAPACITY,
                overflow.rejected_render_len, overflow.rejected_render_capacity, in_flight
            );
            Err(YinzError::ClientTooSlow { client_id }).with_context(err_context)
        },
    }
}

impl ClientSender {
    /// Stop accepting messages and let the writer thread finish what it has.
    /// Idempotent. Shutdown closes the queues themselves, outside the registry lock.
    #[cfg(test)]
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
}

#[cfg(not(windows))]
impl ServerOsInputOutput {
    fn lock_client_senders(&self) -> MutexGuard<'_, HashMap<ClientId, ClientSender>> {
        self.client_senders.lock().unwrap_or_else(|poisoned| {
            // HashMap remains valid after an interrupted operation. Each sender
            // recovers its own queue before using its possibly interrupted state.
            let senders = poisoned.into_inner();
            self.client_senders.clear_poison();
            log::warn!("recovered poisoned client sender registry");
            senders
        })
    }
    /// The client's writer queue, cloned out so the registry is not held while it is inspected.
    fn client_queue(&self, client_id: ClientId) -> Option<Arc<CoalescingSendQueue>> {
        self.lock_client_senders()
            .get(&client_id)
            .map(|sender| sender.queue.clone())
    }
    /// Resolve a local launcher, SSH endpoint, or checked Bubblewrap environment.
    fn resolve_target(
        &self,
        mut cmd: RunCommand,
        report_progress: Option<&mut dyn FnMut(&str)>,
    ) -> Result<(RunCommand, RunCommand, Option<RemoteEndpoint>)> {
        let rerun_cmd = cmd.clone();
        match &cmd.target {
            PaneTarget::Local => {
                yinz_process::command::resolve_agent_launcher(
                    yinz_runtime::own_binary::server_binary()
                        .context("find this Yinz binary for the agent launcher")?,
                    &mut cmd.command,
                    &mut cmd.args,
                )?;
                Ok((cmd, rerun_cmd, None))
            },
            PaneTarget::Ssh { host } => {
                let endpoint = RemoteEndpoint {
                    host: host.clone(),
                    ssh_options: SshOptions::default(),
                };
                Ok((cmd, rerun_cmd, Some(endpoint)))
            },
            // The command is left as asked for: the sandbox is entered where it is exec'd
            // (`exec_program_and_args`). What happens here is everything that can refuse
            // the launch, so the refusal is this spawn's answer rather than text in a pane.
            PaneTarget::Bubblewrap { root } => {
                if let Some(report_progress) = report_progress {
                    report_progress("running launch checks");
                }
                check_bubblewrap_launch(root, &cmd)?;
                Ok((cmd, rerun_cmd, None))
            },
        }
    }

    fn resolve_spawn(
        &self,
        terminal_id: TerminalId,
        terminal_action: TerminalAction,
        default_editor: Option<PathBuf>,
        report_progress: &mut dyn FnMut(&str),
    ) -> Result<ResolvedSpawn> {
        let (command, _) = build_command(terminal_action, default_editor);
        let target = command.target.clone();
        let (command, rerun_command, endpoint) = self
            .resolve_target(command, Some(report_progress))
            .map_err(|error| resolve_failed(terminal_id, &target, error))?;
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
        if resolved.endpoint.is_none() {
            let (reader, child_pid) = self.spawn_resolved_here(resolved, None, quit_cb)?;
            return Ok((terminal_id, reader, Some(child_pid)));
        }
        let reader = self.open_remote(resolved, quit_cb)?;
        // Deliberately no pid. The child's pid is meaningful only on its own
        // machine, and handing one back here would put it in
        // `id_to_child_pid`, where every consumer would treat it as local —
        // `kill`, `sysinfo`, and the plugin-facing `GetPanePid` alike.
        // `None` makes those paths skip the pane instead, which is what the
        // remote-aware replacements rely on.
        Ok((terminal_id, reader, None))
    }

    /// Start a resolved command that has no endpoint - one that runs on this machine,
    /// which a deferred spawn does only for a Bubblewrap environment.
    fn spawn_resolved_here(
        &self,
        resolved: ResolvedSpawn,
        failover_command: Option<RunCommand>,
        quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>,
    ) -> Result<(Box<dyn AsyncReader>, ProcessId)> {
        let ResolvedSpawn {
            terminal_id,
            command,
            rerun_command,
            ..
        } = resolved;
        let quit_cb = if command != rerun_command {
            Box::new(move |pane_id, status, _| quit_cb(pane_id, status, rerun_command.clone()))
                as Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send>
        } else {
            quit_cb
        };
        self.pty_backend
            .spawn_terminal(command, failover_command, quit_cb, terminal_id)
            .context("failed to spawn terminal")
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
            endpoint,
        } = resolved;
        let RemoteEndpoint { host, ssh_options } =
            endpoint.context("a local command has no endpoint to open")?;
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
        let target = command.target.clone();
        let (command, rerun_command, endpoint) = self
            .resolve_target(command, Some(report_progress))
            .map_err(|error| resolve_failed(terminal_id, &target, error))?;
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
        if resolved.endpoint.is_none() {
            let (reader, child_pid) = self.spawn_resolved_here(resolved, None, quit_cb)?;
            return Ok((reader, Some(child_pid)));
        }
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
/// `YinzError` naming the pane the message belongs in.
fn pane_start_failed(terminal_id: TerminalId, host: &str, err: anyhow::Error) -> anyhow::Error {
    if let Some(YinzError::CommandNotFound { command, .. }) = err.downcast_ref::<YinzError>() {
        return YinzError::CommandNotFound {
            terminal_id,
            command: command.clone(),
        }
        .into();
    }
    YinzError::PaneStartFailed {
        terminal_id,
        message: format!("could not open a pane on {host}: {err:#}"),
    }
    .into()
}

/// As [`pane_start_failed`], for a failure to resolve `target` itself.
fn resolve_failed(
    terminal_id: TerminalId,
    target: &PaneTarget,
    err: anyhow::Error,
) -> anyhow::Error {
    if !target.is_bubblewrap() {
        return pane_start_failed(
            terminal_id,
            target.display().as_deref().unwrap_or("remote"),
            err,
        );
    }
    if let Some(YinzError::CommandNotFound { command, .. }) = err.downcast_ref::<YinzError>() {
        return YinzError::CommandNotFound {
            terminal_id,
            command: command.clone(),
        }
        .into();
    }
    YinzError::PaneStartFailed {
        terminal_id,
        message: format!("could not start the Bubblewrap environment: {err:#}"),
    }
    .into()
}

/// Run a Bubblewrap launch's checks for the environment of the checkout at `root`.
///
/// A project program missing
/// from the sandbox's `PATH` becomes the `CommandNotFound` a missing
/// local command already is. Its terminal id is a placeholder, as a broker's is:
/// [`resolve_failed`] names the pane.
#[cfg(unix)]
fn check_bubblewrap_launch(root: &Path, cmd: &RunCommand) -> Result<()> {
    use yinz_project_config::bubblewrap::{Environment, ProgramNotFound};
    let cwd = cmd
        .cwd
        .as_deref()
        .context("a Bubblewrap pane needs a working directory")?;
    let (program, _) = yinz_types::agent::unwrapped_program_and_args(&cmd.command, &cmd.args)
        .unwrap_or((&cmd.command, &cmd.args));
    let program = program.to_path_buf();
    Environment::at_root(root)
        .check_launch(cwd, &program)
        .map_err(|error| {
            if error.downcast_ref::<ProgramNotFound>().is_some() {
                YinzError::CommandNotFound {
                    terminal_id: TerminalId::new(0),
                    command: program.to_string_lossy().into_owned(),
                }
                .into()
            } else {
                error
            }
        })
}

#[cfg(not(unix))]
fn check_bubblewrap_launch(_root: &Path, _cmd: &RunCommand) -> Result<()> {
    Err(anyhow!("Bubblewrap agent environments require Linux"))
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
        let target = cmd.target.clone();
        let (cmd, rerun_cmd, endpoint) = self
            .resolve_target(cmd, None)
            .map_err(|e| resolve_failed(terminal_id, &target, e))?;

        if endpoint.is_some() {
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
            .spawn_resolved_here(
                ResolvedSpawn {
                    terminal_id,
                    command: cmd,
                    rerun_command: rerun_cmd,
                    endpoint,
                },
                failover_cmd,
                quit_cb,
            )
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
    fn retain_agent_theme(
        &self,
        terminal_id: TerminalId,
        event: yinz_types::agent::theme::AgentThemeEvent,
    ) -> Result<()> {
        self.pty_backend.retain_agent_theme(terminal_id, event)
    }
    fn tcdrain(&self, terminal_id: TerminalId) -> Result<()> {
        self.pty_backend.tcdrain(terminal_id)
    }
    fn is_remote_pane(&self, terminal_id: TerminalId) -> bool {
        self.pty_backend.is_remote(terminal_id)
    }
    fn remote_pane_status(&self, terminal_id: TerminalId) -> Option<RemoteStatus> {
        self.pty_backend.remote_status(terminal_id)
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

        // Keep lookup and enqueue atomic with replacement/removal. Callbacks may
        // reenter the registry, so run them only after releasing its guard.
        let pending = self
            .lock_client_senders()
            .get(&client_id)
            .map(|sender| sender.queue.push_deferred(msg));
        match pending {
            Some(pending) => finish_enqueue(client_id, pending).with_context(err_context),
            None => Ok(()),
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
        // A sender this id replaces closes its queue as it drops, after the registry is released.
        let replaced = self.lock_client_senders().insert(client_id, sender);
        drop(replaced);
        Ok(receiver)
    }

    fn remove_client(&mut self, client_id: ClientId) -> Result<()> {
        // Dropped after the registry is released: closing the queue runs any armed
        // render-ready callback.
        let sender = self.lock_client_senders().remove(&client_id);
        if let Some(sender) = sender {
            // Order matters: dropping `sender` closes the queue so the writer
            // thread drains and sends its final `Exit`, and `shutdown_reads`
            // touches only the read half, so that write still lands. What it
            // does end is the router's blocked read, which nothing else here
            // could reach.
            sender.shutdown_reads();
        }
        Ok(())
    }

    fn client_render_backlog_and_notify(
        &self,
        client_id: ClientId,
        threshold: usize,
        ready: &RenderReady,
    ) -> Option<RenderBacklog> {
        self.client_queue(client_id)
            .map(|queue| queue.render_backlog_and_notify(threshold, Some(ready)))
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

        if let Some(process) = system_info.process(sysinfo_pid)
            && let Some(cwd) = process.cwd()
        {
            return Some(cwd.to_path_buf());
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
            if let Some(fpgid) = self.pty_backend.tcgetpgrp(terminal_id)
                && let Ok(fpgid) = ProcessId::try_from(fpgid)
                && fpgid.get() > 0
                && fpgid != shell_pid
            {
                terminal_to_fg_pid.insert(terminal_id, fpgid);
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
        let target = run_command.target.clone();
        let (run_command, rerun_command, endpoint) = self
            .resolve_target(run_command, None)
            .map_err(|e| resolve_failed(terminal_id, &target, e))?;
        if endpoint.is_some() {
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
        let (async_reader, child_pid) = self.spawn_resolved_here(
            ResolvedSpawn {
                terminal_id,
                command: run_command,
                rerun_command,
                endpoint,
            },
            None,
            quit_cb,
        )?;
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
    #[cfg(not(windows))]
    fn begin_confirmed_pty_host_shutdown(&self) -> Result<()> {
        self.pty_backend.begin_confirmed_pty_host_shutdown()
    }
    fn wait_for_process_exit(&self, pid: ProcessId, timeout: Duration) -> Result<()> {
        self.pty_backend.wait_for_process_exit(pid, timeout)
    }
    fn confirm_pty_host_shutdown(&self) -> Result<()> {
        self.pty_backend.confirm_pty_host_shutdown()
    }
    fn shutdown_pty_hosts(&self) {
        self.pty_backend.shutdown_pty_hosts();
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
        // `None` once the queue is closed and drained. Closed after the registry is
        // released, since closing runs any armed render-ready callback.
        let queues: Vec<_> = self
            .lock_client_senders()
            .values()
            .map(|sender| sender.queue.clone())
            .collect();
        for queue in queues {
            queue.close();
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
                // Report only the resizes that reached the pane.
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
    termios: Option<yinz_runtime::termios::WireTermios>,
) -> Result<ServerOsInputOutput, std::io::Error> {
    server_os_input_with_backend(PtyBackendImpl::with_termios(
        termios
            .as_ref()
            .map(yinz_runtime::termios::WireTermios::to_termios),
    )?)
}

fn server_os_input_with_backend(
    pty_backend: PtyBackendImpl,
) -> Result<ServerOsInputOutput, std::io::Error> {
    Ok(ServerOsInputOutput {
        pty_backend,
        remote_hosts: Arc::new(RemoteHosts::new()),
        client_senders: Arc::new(Mutex::new(HashMap::new())),
        live_writers: Arc::new(LiveWriters::default()),
        cached_resizes: Arc::new(Mutex::new(None)),
    })
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
