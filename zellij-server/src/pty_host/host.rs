//! The supervisor process: owns one pane's pty and its child.
//!
//! Deliberately structured so that `serve()` takes an already-connected control
//! socket and can therefore be driven in-process from a `UnixStream::pair()` in
//! tests; `run_pty_host()` is the thin wrapper that turns an exec'd `zellij
//! --pty-host` into a call to it.
//!
//! # The one invariant
//!
//! **The supervisor reads the pty master if and only if it is `Detached`.**
//! While a server is attached it holds the only other dup of that master and is
//! the sole reader. Nothing is lost across the transition: bytes the child writes
//! in the gap sit in the kernel's pty buffer, and if that fills the child blocks
//! in `write()` rather than losing output.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nix::{
    errno::Errno,
    fcntl::{fcntl, FcntlArg, OFlag},
    poll::{poll, PollFd, PollFlags, PollTimeout},
    pty::{openpty, Winsize},
    sys::signal::{kill, Signal},
    unistd::Pid,
};
use zellij_utils::{consts::VERSION, errors::prelude::*, input::command::RunCommand};

use super::fdpass::{self, send_fd};
use super::protocol::{
    self, read_msg, write_msg, AttachRefusal, HostStatus, HostToServerMsg, ServerToHostMsg,
    SpawnFailureKind, WireTermios, WireWinsize, PROTOCOL_VERSION,
};

/// How much detached output to keep. Only reached when no server is attached, so
/// this is a bound on memory for an orphaned pane, not a scrollback setting.
const RING_CAPACITY: usize = 256 * 1024;

/// How long to let a child wind down after `SIGHUP` before resorting to
/// `SIGKILL`.
const TERMINATE_GRACE: Duration = Duration::from_secs(2);

/// A `Query` from the CLI must not be able to wedge the supervisor.
const CLI_CONNECTION_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// A server holds a dup of the master and is the sole reader.
    Attached,
    /// Nobody is attached; we drain the master so the child never blocks.
    Detached,
}

enum HostEvent {
    ChildExited(Option<i32>),
    TerminateRequested,
}

/// Fixed-capacity, drop-oldest. Drop-oldest rather than stop-reading is
/// deliberate: an orphaned pane should keep making progress (a build should
/// finish), and the cost is that its earliest output ages out.
struct RingBuffer {
    buf: VecDeque<u8>,
    dropped: u64,
}

impl RingBuffer {
    fn new() -> Self {
        RingBuffer {
            buf: VecDeque::new(),
            dropped: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        if bytes.len() >= RING_CAPACITY {
            let skip = bytes.len() - RING_CAPACITY;
            self.dropped += (self.buf.len() + skip) as u64;
            self.buf.clear();
            self.buf.extend(&bytes[skip..]);
            return;
        }
        let overflow = (self.buf.len() + bytes.len()).saturating_sub(RING_CAPACITY);
        if overflow > 0 {
            self.buf.drain(..overflow);
            self.dropped += overflow as u64;
        }
        self.buf.extend(bytes);
    }

    fn len(&self) -> usize {
        self.buf.len()
    }

    /// Hand the buffered output over and start again empty.
    ///
    /// Called once per attach. Resetting `dropped` along with the bytes is what
    /// makes the count mean "lost during *this* detachment" — a pane that is
    /// warped twice should not re-report the first gap.
    fn take(&mut self) -> (Vec<u8>, u64) {
        let dropped = std::mem::take(&mut self.dropped);
        let bytes = std::mem::take(&mut self.buf).into();
        (bytes, dropped)
    }
}

/// The live pty and child for one pane. Replaced wholesale by `Respawn`.
struct Session {
    master: OwnedFd,
    /// Held for the child's whole lifetime and closed the moment it exits —
    /// dropping the last slave is what makes an attached server's `read` on the
    /// master return `EIO`/`0`, which is how a pane learns to close.
    slave: Option<OwnedFd>,
    child_pid: u32,
    exit_status: Option<Option<i32>>,
    /// Set once the master has nothing left to give. Without it the poll loop
    /// would keep watching a hung-up descriptor that is permanently "ready", and
    /// an orphaned pane whose child has exited would spin a core forever.
    drained: bool,
}

/// Entry point for `zellij --pty-host`. Never returns.
pub fn run_pty_host(label: String) -> ! {
    // First thing, before the logger opens anything: we inherit every one of the
    // server's descriptors that is not marked CLOEXEC, and `libc::openpty` marks
    // neither end. Holding another pane's pty *slave* open for the rest of our
    // life would stop that pane ever seeing EOF when its shell exits — a pane
    // that can never be closed. Verified empirically: without this a supervisor
    // starts life holding a dozen of the server's sockets.
    unsafe {
        close_fds::close_open_fds(protocol::CONTROL_FD + 1, &[]);
    }
    zellij_utils::logging::configure_logger();

    // A new session (and therefore a new process group), so a SIGINT/SIGTERM/
    // SIGHUP aimed at the server's group cannot reach us. No double fork: staying
    // a direct child of the server keeps our pid visible to it and lets it reap
    // us. If the server dies we simply reparent to init.
    if let Err(e) = nix::unistd::setsid() {
        log::warn!("pty-host could not setsid: {e}");
    }
    // SIGHUP would otherwise kill us when the controlling terminal of whatever
    // spawned the server goes away — the one signal we must outlive.
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }

    let control = match control_stream_from_fd() {
        Ok(control) => control,
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        },
    };

    let code = match serve(
        control,
        ServeConfig {
            label,
            handle_signals: true,
        },
    ) {
        Ok(()) => 0,
        Err(e) => {
            log::error!("pty-host exiting on error: {e:#}");
            1
        },
    };
    std::process::exit(code);
}

/// Reclaim the control socket the server `dup2`'d onto fd 3 before exec.
///
/// Validated rather than assumed, so a human typing `zellij --pty-host` gets a
/// diagnostic instead of a process blocked forever on a read from their tty.
fn control_stream_from_fd() -> Result<UnixStream> {
    use nix::sys::socket::{getsockopt, sockopt};
    use std::os::fd::FromRawFd;

    let fd = protocol::CONTROL_FD;
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    let sock_type = getsockopt(&borrowed, sockopt::SockType).map_err(|e| {
        anyhow!(
            "--pty-host is an internal flag: fd {fd} is not a socket ({e}). \
             zellij spawns supervisors itself; there is nothing to run by hand."
        )
    })?;
    if sock_type != nix::sys::socket::SockType::Stream {
        return Err(anyhow!(
            "--pty-host is an internal flag: fd {fd} is not a stream socket"
        ));
    }
    // SAFETY: validated above as a stream socket, and it was placed here for us.
    Ok(unsafe { UnixStream::from_raw_fd(fd) })
}

pub struct ServeConfig {
    /// `<session>/<terminal_id>`, purely so `ps` and `pty-host list` are
    /// readable. Never the command line — that would put every pane's arguments
    /// in every user's `ps` output.
    pub label: String,
    /// Registering a SIGTERM/SIGINT handler changes the disposition for the whole
    /// process, so only the real supervisor binary does it. Tests drive `serve()`
    /// in-process and must not disturb the test harness.
    pub handle_signals: bool,
}

/// Run one supervisor to completion over an already-connected control socket.
pub fn serve(mut control: UnixStream, config: ServeConfig) -> Result<()> {
    let ServeConfig {
        label,
        handle_signals,
    } = config;
    let host_pid = std::process::id();

    // --- handshake ---
    let hello: ServerToHostMsg = read_msg(&mut control)
        .context("failed to read Hello")?
        .ok_or_else(|| anyhow!("control channel closed before Hello"))?;
    let ServerToHostMsg::Hello {
        protocol_version, ..
    } = hello
    else {
        return Err(anyhow!("expected Hello as the first message"));
    };

    let host_id = protocol::new_host_id();
    // A bind failure is not fatal: the named socket only powers `pty-host
    // list|kill` and, later, re-attach. Pane creation must not depend on it.
    let listener = match bind_listener(&host_id) {
        Ok(listener) => Some(listener),
        Err(e) => {
            log::warn!("pty-host {host_id} could not bind its control socket: {e:#}");
            None
        },
    };
    write_msg(
        &mut control,
        &HostToServerMsg::HelloAck {
            protocol_version: PROTOCOL_VERSION,
            zellij_version: VERSION.to_owned(),
            host_id: host_id.clone(),
            host_pid,
            control_socket: listener.as_ref().map(|_| protocol::socket_path(&host_id)),
        },
    )
    .context("failed to send HelloAck")?;

    if protocol_version != PROTOCOL_VERSION {
        // The server re-execs whatever binary is on disk *now*, so an upgrade
        // mid-session means an old server can spawn a new host. Refuse cleanly
        // and let the server fall back to spawning the pty in-process.
        log::warn!(
            "pty-host protocol mismatch: server speaks v{protocol_version}, we speak v{PROTOCOL_VERSION}"
        );
        cleanup_socket(&host_id);
        return Ok(());
    }

    // --- initial spawn ---
    let spawn: ServerToHostMsg = read_msg(&mut control)
        .context("failed to read Spawn")?
        .ok_or_else(|| anyhow!("control channel closed before Spawn"))?;
    let ServerToHostMsg::Spawn {
        terminal_id,
        session_name,
        cmd,
        failover_cmd,
        termios,
        initial_size,
        arg0,
    } = spawn
    else {
        cleanup_socket(&host_id);
        return Err(anyhow!("expected Spawn after the handshake"));
    };

    let mut ctx = HostContext {
        host_id,
        host_pid,
        label,
        terminal_id,
        session_name,
        cmd,
        started_at_unix: unix_now(),
        state: State::Attached,
        ring: RingBuffer::new(),
        session: None,
    };

    let (event_tx, event_rx) = mpsc::channel();
    let (wake_tx, wake_rx) = UnixStream::pair().context("failed to create the wake channel")?;
    wake_rx
        .set_nonblocking(true)
        .context("failed to set the wake channel non-blocking")?;
    if handle_signals {
        install_signal_forwarder(event_tx.clone(), wake_tx.try_clone()?);
    }

    match start_session(
        &ctx.cmd,
        failover_cmd.as_ref(),
        terminal_id,
        termios.as_ref(),
        initial_size.as_ref(),
        arg0.as_deref(),
        &event_tx,
        &wake_tx,
    ) {
        Ok(session) => {
            let child_pid = session.child_pid;
            ctx.session = Some(session);
            write_msg(&mut control, &HostToServerMsg::Spawned { child_pid })
                .context("failed to report the spawned child")?;
            let master_fd = ctx
                .session
                .as_ref()
                .map(|s| s.master.as_raw_fd())
                .expect("session was just installed");
            send_fd(&control, master_fd).context("failed to hand off the pty master")?;
        },
        Err(e) => {
            let _ = write_msg(
                &mut control,
                &HostToServerMsg::SpawnFailed {
                    kind: classify_spawn_failure(&e),
                    message: format!("{e:#}"),
                },
            );
            cleanup_socket(&ctx.host_id);
            return Err(e);
        },
    }

    let result = event_loop(&mut ctx, control, listener, event_rx, wake_rx);
    cleanup_socket(&ctx.host_id);
    result
}

struct HostContext {
    host_id: String,
    host_pid: u32,
    label: String,
    terminal_id: u32,
    session_name: Option<String>,
    cmd: RunCommand,
    started_at_unix: u64,
    state: State,
    ring: RingBuffer,
    session: Option<Session>,
}

impl HostContext {
    fn status(&self) -> HostStatus {
        HostStatus {
            host_id: self.host_id.clone(),
            session_name: self.session_name.clone(),
            terminal_id: self.terminal_id,
            command: self.cmd.to_string(),
            cwd: self.cmd.cwd.clone(),
            child_pid: self.session.as_ref().map(|s| s.child_pid),
            host_pid: self.host_pid,
            started_at_unix: self.started_at_unix,
            attached: self.state == State::Attached,
            child_exit_status: self.session.as_ref().and_then(|s| s.exit_status),
            buffered_bytes: self.ring.len(),
        }
    }
}

fn event_loop(
    ctx: &mut HostContext,
    control: UnixStream,
    listener: Option<UnixListener>,
    event_rx: Receiver<HostEvent>,
    mut wake_rx: UnixStream,
) -> Result<()> {
    // `None` once the server has gone away: that is the Attached -> Detached
    // transition, and it is the only thing that lets us start reading the master.
    let mut attached_control = Some(control);

    loop {
        // Rebuilt each pass: which descriptors are interesting depends on state,
        // and the master is polled only while detached — the read-ownership
        // invariant, expressed directly.
        //
        // The order is load-bearing, not cosmetic: `ready` below preserves it, so
        // `Control` before `Listener` means a pending `Release` is always handled
        // before a pending accept in the same wakeup. That is what makes a warp
        // race-free — the releasing server and the attaching one are different
        // processes with nothing else ordering them, and an `Attach` that arrived
        // first would be refused for a pane its owner had already let go of.
        let mut sources: Vec<PollSource> = Vec::with_capacity(4);
        if attached_control.is_some() {
            sources.push(PollSource::Control);
        }
        sources.push(PollSource::Wake);
        if listener.is_some() {
            sources.push(PollSource::Listener);
        }
        if ctx.state == State::Detached
            && ctx.session.as_ref().is_some_and(|session| !session.drained)
        {
            sources.push(PollSource::Master);
        }

        let mut poll_fds: Vec<PollFd> = sources
            .iter()
            .map(|source| {
                let fd: BorrowedFd = match source {
                    PollSource::Control => attached_control.as_ref().unwrap().as_fd(),
                    PollSource::Wake => wake_rx.as_fd(),
                    PollSource::Listener => listener.as_ref().unwrap().as_fd(),
                    PollSource::Master => ctx.session.as_ref().unwrap().master.as_fd(),
                };
                PollFd::new(fd, PollFlags::POLLIN)
            })
            .collect();

        match poll(&mut poll_fds, PollTimeout::NONE) {
            Ok(_) => {},
            Err(Errno::EINTR) => continue,
            Err(e) => return Err(e).context("pty-host poll failed"),
        }

        let ready: Vec<PollSource> = sources
            .iter()
            .zip(poll_fds.iter())
            .filter(|(_, pfd)| {
                pfd.revents()
                    .map(|r| {
                        r.intersects(PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR)
                    })
                    .unwrap_or(false)
            })
            .map(|(source, _)| *source)
            .collect();

        for source in ready {
            match source {
                PollSource::Control => {
                    let Some(stream) = attached_control.as_mut() else {
                        continue;
                    };
                    match read_msg::<ServerToHostMsg, _>(stream) {
                        Ok(Some(msg)) => match handle_server_msg(ctx, msg, stream, &event_rx)? {
                            ControlOutcome::Continue => {},
                            ControlOutcome::Exit => return Ok(()),
                            ControlOutcome::Detach => {
                                attached_control = None;
                                enter_detached(ctx)?;
                            },
                        },
                        Ok(None) => {
                            // The server went away without saying Terminate: that
                            // is a detach, not a shutdown. Take over reading the
                            // master so the child does not block on a full pty.
                            log::info!(
                                "pty-host {} ({}) detached: server closed the control channel",
                                ctx.host_id,
                                ctx.label
                            );
                            attached_control = None;
                            enter_detached(ctx)?;
                        },
                        Err(e) => {
                            log::warn!("pty-host {} control channel error: {e:#}", ctx.host_id);
                            attached_control = None;
                            enter_detached(ctx)?;
                        },
                    }
                },
                PollSource::Wake => {
                    drain_wake(&mut wake_rx);
                    while let Ok(event) = event_rx.try_recv() {
                        match event {
                            HostEvent::ChildExited(status) => {
                                on_child_exited(ctx, status, attached_control.as_mut())?;
                            },
                            HostEvent::TerminateRequested => {
                                terminate(ctx, &event_rx);
                                return Ok(());
                            },
                        }
                    }
                },
                PollSource::Listener => {
                    if let Some(listener) = listener.as_ref() {
                        match listener.accept() {
                            Ok((stream, _)) => match serve_cli_connection(ctx, stream) {
                                CliOutcome::Done => {},
                                CliOutcome::Terminate => {
                                    terminate(ctx, &event_rx);
                                    return Ok(());
                                },
                                CliOutcome::Attached(stream) => {
                                    // `attach` refuses while a server is attached, so
                                    // there is nothing here to displace. Say so rather
                                    // than silently dropping a live server's channel.
                                    debug_assert!(attached_control.is_none());
                                    attached_control = Some(stream);
                                },
                            },
                            Err(e) => {
                                log::warn!("pty-host {} accept failed: {e}", ctx.host_id)
                            },
                        }
                    }
                },
                PollSource::Master => {
                    drain_master(ctx);
                },
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PollSource {
    Control,
    Wake,
    Listener,
    Master,
}

/// What a message on the control channel asked us to do.
enum ControlOutcome {
    Continue,
    Exit,
    /// The server gave the pane up. Same end state as it crashing, reached
    /// deliberately.
    Detach,
}

fn handle_server_msg(
    ctx: &mut HostContext,
    msg: ServerToHostMsg,
    control: &mut UnixStream,
    event_rx: &Receiver<HostEvent>,
) -> Result<ControlOutcome> {
    match msg {
        ServerToHostMsg::Terminate => {
            terminate(ctx, event_rx);
            Ok(ControlOutcome::Exit)
        },
        ServerToHostMsg::Query => {
            write_msg(control, &HostToServerMsg::Status(ctx.status()))
                .context("failed to answer Query")?;
            Ok(ControlOutcome::Continue)
        },
        ServerToHostMsg::Release => {
            log::info!(
                "pty-host {} ({}) released by its server",
                ctx.host_id,
                ctx.label
            );
            Ok(ControlOutcome::Detach)
        },
        ServerToHostMsg::Attach { .. } => {
            // An `Attach` only makes sense from someone who is *not* already the
            // control channel, and the accepted connection has to become the new
            // channel, which a message here cannot do.
            log::warn!(
                "pty-host {} got an Attach on the control channel; \
                 it belongs on the named socket",
                ctx.host_id
            );
            Ok(ControlOutcome::Continue)
        },
        ServerToHostMsg::Hello { .. } | ServerToHostMsg::Spawn { .. } => {
            log::warn!("pty-host {} got a duplicate handshake message", ctx.host_id);
            Ok(ControlOutcome::Continue)
        },
    }
}

fn on_child_exited(
    ctx: &mut HostContext,
    status: Option<i32>,
    control: Option<&mut UnixStream>,
) -> Result<()> {
    if let Some(session) = ctx.session.as_mut() {
        if session.exit_status.is_some() {
            return Ok(());
        }
        session.exit_status = Some(status);
        // Order matters and matches what the server used to do in-process: drop
        // the last slave first so an attached server's master read returns
        // EIO/0 and its TerminalBytes loop ends, *then* report the status.
        session.slave = None;
    }
    if let Some(control) = control {
        write_msg(control, &HostToServerMsg::ChildExited { status })
            .context("failed to report the child's exit")?;
    }
    Ok(())
}

/// Take over reading the master. Called only when no server is attached.
fn enter_detached(ctx: &mut HostContext) -> Result<()> {
    if ctx.state == State::Detached {
        return Ok(());
    }
    ctx.state = State::Detached;
    if let Some(session) = ctx.session.as_ref() {
        // The server's RawFdAsyncReader may already have set O_NONBLOCK — that
        // flag lives on the shared open file description — but say so explicitly
        // rather than depending on it.
        let fd = session.master.as_fd();
        if let Ok(flags) = fcntl(fd, FcntlArg::F_GETFL) {
            let mut oflags = OFlag::from_bits_truncate(flags);
            oflags.insert(OFlag::O_NONBLOCK);
            let _ = fcntl(fd, FcntlArg::F_SETFL(oflags));
        }
    }
    Ok(())
}

fn drain_master(ctx: &mut HostContext) {
    let Some(session) = ctx.session.as_ref() else {
        return;
    };
    let fd = session.master.as_raw_fd();
    let mut buf = [0u8; 16 * 1024];
    // Whether the pty is finished for good, as opposed to merely empty right now.
    let exhausted = loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n > 0 {
            ctx.ring.push(&buf[..n as usize]);
            continue;
        }
        if n == 0 {
            // Every slave is closed; nothing more will ever arrive.
            break true;
        }
        match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::EINTR) => continue,
            // Just empty for now — the child may well write again.
            Some(libc::EAGAIN) => break false,
            // EIO is how Linux reports the last slave closing.
            _ => break true,
        }
    };
    if exhausted {
        if let Some(session) = ctx.session.as_mut() {
            session.drained = true;
        }
    }
}

fn drain_wake(wake_rx: &mut UnixStream) {
    let mut buf = [0u8; 64];
    loop {
        match wake_rx.read(&mut buf) {
            Ok(0) => return,
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        }
    }
}

/// What an accepted named-socket connection asked us to do.
enum CliOutcome {
    /// Answered in place; the connection is finished with.
    Done,
    Terminate,
    /// The connection has become our new control channel.
    Attached(UnixStream),
}

/// Handles one `pty-host list|kill|adopt` connection inline.
fn serve_cli_connection(ctx: &mut HostContext, mut stream: UnixStream) -> CliOutcome {
    // A wedged or malicious client must not be able to stall the supervisor.
    let _ = stream.set_read_timeout(Some(CLI_CONNECTION_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CLI_CONNECTION_TIMEOUT));
    match read_msg::<ServerToHostMsg, _>(&mut stream) {
        Ok(Some(ServerToHostMsg::Query)) => {
            let _ = write_msg(&mut stream, &HostToServerMsg::Status(ctx.status()));
            CliOutcome::Done
        },
        Ok(Some(ServerToHostMsg::Terminate)) => {
            let _ = write_msg(&mut stream, &HostToServerMsg::Status(ctx.status()));
            CliOutcome::Terminate
        },
        Ok(Some(ServerToHostMsg::Attach {
            protocol_version,
            terminal_id,
            session_name,
            ..
        })) => attach(ctx, stream, protocol_version, terminal_id, session_name),
        Ok(Some(_)) => CliOutcome::Done,
        Ok(None) => CliOutcome::Done,
        Err(e) => {
            log::warn!("pty-host {} bad CLI request: {e:#}", ctx.host_id);
            CliOutcome::Done
        },
    }
}

fn refuse_attach(
    ctx: &HostContext,
    stream: &mut UnixStream,
    reason: AttachRefusal,
    message: String,
) -> CliOutcome {
    log::warn!("pty-host {} refused an attach: {message}", ctx.host_id);
    let _ = write_msg(stream, &HostToServerMsg::AttachRefused { reason, message });
    CliOutcome::Done
}

/// Hand this pane over to a new server.
///
/// The mirror of the `Spawn` half of `serve`: same `AttachAck`-then-`send_fd`
/// shape, except the pty already exists and the ring has been catching its
/// output in the meantime.
fn attach(
    ctx: &mut HostContext,
    mut stream: UnixStream,
    protocol_version: u32,
    terminal_id: u32,
    session_name: Option<String>,
) -> CliOutcome {
    if protocol_version != PROTOCOL_VERSION {
        return refuse_attach(
            ctx,
            &mut stream,
            AttachRefusal::ProtocolMismatch,
            format!(
                "this supervisor speaks v{PROTOCOL_VERSION}, the caller speaks v{protocol_version}"
            ),
        );
    }
    if ctx.state == State::Attached {
        // Two servers holding dups of the same master would each read half the
        // pane's output. The caller retries: a server that crashed a moment ago
        // may not have been noticed yet.
        return refuse_attach(
            ctx,
            &mut stream,
            AttachRefusal::AlreadyAttached,
            "a server is still attached to this pane".to_owned(),
        );
    }
    let Some(session) = ctx.session.as_ref() else {
        return refuse_attach(
            ctx,
            &mut stream,
            AttachRefusal::NoSession,
            "this supervisor never spawned a child, so it has no pty to hand over".to_owned(),
        );
    };
    let master_fd = session.master.as_raw_fd();
    let child_pid = session.child_pid;
    let child_exited = session.exit_status;

    // Re-key before building the status, so `pty-host list` names the session
    // that owns the pane now rather than the one it came from.
    ctx.terminal_id = terminal_id;
    ctx.session_name = session_name;
    ctx.label = match ctx.session_name.as_ref() {
        Some(name) => format!("{name}/{terminal_id}"),
        None => terminal_id.to_string(),
    };
    ctx.state = State::Attached;
    let (buffered, dropped_bytes) = ctx.ring.take();
    if dropped_bytes > 0 {
        log::warn!(
            "pty-host {} dropped {dropped_bytes} bytes while detached; \
             the adopted pane is missing the middle of its output",
            ctx.host_id
        );
    }

    // `cmd` travels for `hold_on_close`; its `originating_plugin` names a plugin
    // in the session this pane is leaving, so it must not follow it.
    let mut cmd = ctx.cmd.clone();
    cmd.originating_plugin = None;

    let ack = HostToServerMsg::AttachAck {
        protocol_version: PROTOCOL_VERSION,
        zellij_version: VERSION.to_owned(),
        host_id: ctx.host_id.clone(),
        host_pid: ctx.host_pid,
        child_pid,
        cmd,
        status: ctx.status(),
        buffered_base64: protocol::encode_buffered(&buffered),
        dropped_bytes,
    };
    if let Err(e) = write_msg(&mut stream, &ack) {
        log::warn!(
            "pty-host {} could not send an AttachAck: {e:#}",
            ctx.host_id
        );
        return abandon_attach(ctx, buffered);
    }
    if let Err(e) = send_fd(&stream, master_fd) {
        log::warn!(
            "pty-host {} could not hand the pty master over: {e:#}",
            ctx.host_id
        );
        return abandon_attach(ctx, buffered);
    }

    // Back to blocking: these timeouts exist so a wedged `pty-host list` cannot
    // stall us, but a control channel that timed out mid-frame would be read as
    // a detach and would silently drop a live server.
    let _ = stream.set_read_timeout(None);
    let _ = stream.set_write_timeout(None);

    if let Some(status) = child_exited {
        // The child died while nobody was attached, so `on_child_exited` had no
        // control channel to report it on. Without this the adopted pane would
        // sit there looking alive forever.
        if let Err(e) = write_msg(&mut stream, &HostToServerMsg::ChildExited { status }) {
            log::warn!(
                "pty-host {} could not report its already-exited child: {e:#}",
                ctx.host_id
            );
        }
    }

    log::info!(
        "pty-host {} ({}) attached by a new server",
        ctx.host_id,
        ctx.label
    );
    CliOutcome::Attached(stream)
}

/// Undo a half-finished attach: go back to draining, and keep the output we had
/// already taken out of the ring so the next attacher still gets it.
fn abandon_attach(ctx: &mut HostContext, buffered: Vec<u8>) -> CliOutcome {
    ctx.state = State::Detached;
    ctx.ring.push(&buffered);
    CliOutcome::Done
}

fn kill_current_child(ctx: &mut HostContext, event_rx: &Receiver<HostEvent>) {
    let Some(session) = ctx.session.as_ref() else {
        return;
    };
    if session.exit_status.is_some() {
        ctx.session = None;
        return;
    }
    let pid = Pid::from_raw(session.child_pid as i32);
    let _ = kill(pid, Some(Signal::SIGHUP));
    let deadline = std::time::Instant::now() + TERMINATE_GRACE;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            let _ = kill(pid, Some(Signal::SIGKILL));
            // The waiter thread will observe this; give it a moment to report so
            // the child is reaped rather than left a zombie of ours.
            let _ = event_rx.recv_timeout(TERMINATE_GRACE);
            break;
        }
        match event_rx.recv_timeout(remaining) {
            Ok(HostEvent::ChildExited(_)) => break,
            Ok(HostEvent::TerminateRequested) => continue,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    ctx.session = None;
}

fn terminate(ctx: &mut HostContext, event_rx: &Receiver<HostEvent>) {
    log::info!("pty-host {} ({}) terminating", ctx.host_id, ctx.label);
    kill_current_child(ctx, event_rx);
}

fn cleanup_socket(host_id: &str) {
    let path = protocol::socket_path(host_id);
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn bind_listener(host_id: &str) -> Result<UnixListener> {
    let dir = protocol::socket_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let _ = zellij_utils::shared::set_permissions(&dir, 0o700);
    let path = protocol::socket_path(host_id);
    protocol::check_socket_path_length(&path)?;
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
    let listener =
        UnixListener::bind(&path).with_context(|| format!("failed to bind {}", path.display()))?;
    // Sticky, like the session socket: supervisors outlive everything and are
    // exactly what a runtime-dir cleaner will try to reap out from under us.
    let _ = zellij_utils::shared::set_permissions(&path, 0o1700);
    Ok(listener)
}

fn classify_spawn_failure(err: &anyhow::Error) -> SpawnFailureKind {
    match err.downcast_ref::<ZellijError>() {
        Some(ZellijError::CommandNotFound { .. }) => SpawnFailureKind::CommandNotFound,
        _ => SpawnFailureKind::Other,
    }
}

/// Forwards SIGTERM/SIGINT into the event loop so `kill <host_pid>` tears the
/// pane down properly instead of orphaning the child.
fn install_signal_forwarder(event_tx: Sender<HostEvent>, wake_tx: UnixStream) {
    let mut signals = match signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
    ]) {
        Ok(signals) => signals,
        Err(e) => {
            log::warn!("pty-host could not install its signal handler: {e}");
            return;
        },
    };
    let mut wake_tx = wake_tx;
    // the first signal is enough - it tears the host down
    std::thread::spawn(move || {
        if signals.forever().next().is_some()
            && event_tx.send(HostEvent::TerminateRequested).is_ok()
        {
            let _ = wake_tx.write_all(b"w");
        }
    });
}

/// Create the pty and fork the child. This is `handle_openpty`
/// (`os_input_output_unix.rs`) moved across the process boundary, with the
/// panics turned into errors — a panic in a `pre_exec` hook runs the panic hook
/// in a forked child of a multithreaded process, which is not safe.
#[allow(clippy::too_many_arguments)]
fn start_session(
    cmd: &RunCommand,
    failover_cmd: Option<&RunCommand>,
    terminal_id: u32,
    termios: Option<&WireTermios>,
    initial_size: Option<&WireWinsize>,
    arg0: Option<&str>,
    event_tx: &Sender<HostEvent>,
    wake_tx: &UnixStream,
) -> Result<Session> {
    let err_context = || {
        format!(
            "failed to open a pty for '{}'",
            cmd.command.to_string_lossy()
        )
    };

    // Must happen before the fork. `close_open_fds` below closes the CLOEXEC pipe
    // `std` uses to report exec failure back to the parent, so `spawn()` returns
    // Ok even when the exec fails — the caller would otherwise see a child that
    // silently exits 1. The server does the same check before spawning us at all;
    // this one is the TOCTOU backstop.
    if crate::os_input_output::resolve_command(cmd).is_none() {
        return Err(ZellijError::CommandNotFound {
            terminal_id,
            command: cmd.command.to_string_lossy().to_string(),
        })
        .with_context(err_context);
    }

    let termios = termios.map(|t| t.to_termios());
    let winsize = initial_size.map(|s| Winsize {
        ws_col: s.cols,
        ws_row: s.rows,
        ws_xpixel: s.width_in_pixels,
        ws_ypixel: s.height_in_pixels,
    });

    let pty = match openpty(winsize.as_ref(), &termios) {
        Ok(pty) => pty,
        Err(e) => {
            // Preserves the existing (odd) semantics: `failover_cmd` is consulted
            // when *openpty* fails, not when the command does. Fixing that is a
            // behavior change and does not belong in a plumbing move.
            // TODO: wire failover to exec failure instead, deliberately.
            if let Some(failover_cmd) = failover_cmd {
                return start_session(
                    failover_cmd,
                    None,
                    terminal_id,
                    None,
                    initial_size,
                    // The failover is a different command; the argv[0] chosen
                    // for the one that could not get a pty does not describe it.
                    None,
                    event_tx,
                    wake_tx,
                );
            }
            return Err(e).with_context(err_context);
        },
    };

    let master = pty.master;
    let slave = pty.slave;
    // `libc::openpty` leaves both fds inheritable. Anything we exec later — and
    // in particular the next pane's supervisor, if a descriptor ever leaked into
    // the server — must not inherit them: a leaked *slave* would keep a pane's
    // pty open after its shell exits and suppress the EOF that closes it.
    let _ = fdpass::set_cloexec(master.as_raw_fd());
    let _ = fdpass::set_cloexec(slave.as_raw_fd());

    let slave_raw = slave.as_raw_fd();
    let mut command = Command::new(&cmd.command);
    if let Some(arg0) = arg0 {
        // The whole of what makes a shell a *login* shell. Local panes pass
        // `None` and exec exactly as they always have.
        command.arg0(arg0);
    }
    if let Some(current_dir) = cmd.cwd.as_ref() {
        if current_dir.exists() && current_dir.is_dir() {
            command.current_dir(current_dir);
        } else {
            log::error!(
                "Failed to set CWD for new pane. '{}' does not exist or is not a folder",
                current_dir.display()
            );
        }
    }
    command
        .args(&cmd.args)
        .env("ZELLIJ_PANE_ID", format!("{}", terminal_id))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(move || -> std::io::Result<()> {
            // `run_pty_host` ignores these so the supervisor outlives the
            // terminal that spawned the server, and an ignored disposition
            // survives exec. Undo it or the pane's command starts with SIGHUP
            // ignored — and by POSIX so does everything its shell then runs.
            // That is both a divergence from the in-server pty path and a
            // correctness problem: closing a pane sends SIGHUP, so the pane
            // would only die via the TERMINATE_GRACE SIGKILL and its
            // descendants would be orphaned rather than hung up.
            for signal in [libc::SIGHUP, libc::SIGPIPE] {
                if libc::signal(signal, libc::SIG_DFL) == libc::SIG_ERR {
                    return Err(std::io::Error::last_os_error());
                }
            }
            if libc::login_tty(slave_raw) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // Mandatory, not hygiene: closes the master, the control socket, the
            // listener and the wake channel so none of them leak into the shell.
            close_fds::close_open_fds(3, &[]);
            Ok(())
        });
    }

    let child = command
        .spawn()
        .with_context(|| format!("failed to spawn '{}'", cmd.command.to_string_lossy()))?;
    let child_pid = child.id();

    spawn_child_waiter(child, event_tx.clone(), wake_tx.try_clone()?);

    Ok(Session {
        master,
        slave: Some(slave),
        child_pid,
        exit_status: None,
        drained: false,
    })
}

fn spawn_child_waiter(mut child: Child, event_tx: Sender<HostEvent>, wake_tx: UnixStream) {
    let mut wake_tx = wake_tx;
    std::thread::spawn(move || {
        let status = child.wait().ok().and_then(|status| status.code());
        let _ = event_tx.send(HostEvent::ChildExited(status));
        let _ = wake_tx.write_all(b"w");
    });
}

#[cfg(test)]
#[path = "../unit/pty_host_tests.rs"]
mod pty_host_tests;
