//! The supervisor process: owns one pane's pty and its child.
//!
//! Deliberately structured so that `serve()` takes an already-connected control
//! socket and can therefore be driven in-process from a `UnixStream::pair()` in
//! tests; `run_pty_host()` is the thin wrapper that turns an exec'd `yinz
//! --pty-host` into a call to it.
//!
//! # The one invariant
//!
//! **The supervisor reads the pty master if and only if it is `Detached`.**
//! While a server is attached it holds the only other dup of that master and is
//! the sole reader. Nothing is lost across the transition: bytes the child writes
//! in the gap sit in the kernel's pty buffer, and if that fills the child blocks
//! in `write()` rather than losing output.
//!
//! # Job control
//!
//! The supervisor owns the pane's pty as its *controlling terminal*, and the
//! child runs in a process group inside the supervisor's session. This is what
//! makes `^Z` work, and it is a deliberate replacement for `login_tty`.
//!
//! `login_tty` gives the child a session of its own, which by definition makes
//! its process group orphaned: no member has a parent in a different group of the
//! same session. Linux discards `SIGTSTP`, `SIGTTIN` and `SIGTTOU` aimed at an
//! orphaned group, so `^Z` in a command pane did nothing at all — the pane's
//! command was never suspended, however much a full-screen program that had
//! already torn its UI down in anticipation looked as if it had been.
//!
//! With the supervisor as session leader the child's group has a parent in the
//! same session, so it is not orphaned: `^Z` really stops it, `spawn_child_waiter`
//! sees the stop because it waits with `WUNTRACED`, and `SIGCONT` wakes it again.
//! See `acquire_controlling_tty`. A supervisor that cannot take the terminal
//! falls back to `login_tty` and the pane behaves exactly as it did before.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::process::{Child, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use yinz_types::internal_ids::ProcessId;
use yinz_types::internal_ids::SessionName;
use yinz_types::internal_ids::SupervisorId;
use yinz_types::internal_ids::TerminalId;
use yinz_types::units::UnixSeconds;

use nix::{
    errno::Errno,
    fcntl::{fcntl, FcntlArg, OFlag},
    poll::{poll, PollFd, PollFlags, PollTimeout},
    pty::Winsize,
    sys::signal::{kill, Signal},
    unistd::Pid,
};
use yinz_runtime::errors::prelude::*;
use yinz_runtime::shared::ensure_secure_dir;
use {yinz_runtime::consts::VERSION, yinz_types::input::command::RunCommand};

use super::fdpass::send_fd;
use super::protocol::{
    self, read_msg, write_msg, AttachRefusal, HostStatus, HostToServerMsg, ServerToHostMsg,
    SpawnFailureKind, WireTermios, WireWinsize, PROTOCOL_VERSION,
};
use super::secure_socket;

/// How much detached output to keep. Only reached when no server is attached, so
/// this is a bound on memory for an orphaned pane, not a scrollback setting.
const RING_CAPACITY: usize = 256 * 1024;

/// How long to let a child wind down after `SIGHUP` before resorting to
/// `SIGKILL`.
const TERMINATE_GRACE: Duration = Duration::from_secs(2);

// Leave time for the completion reply before the client's four-second deadline.
const TERMINATE_KILL_GRACE: Duration = Duration::from_secs(1);
const TERMINATE_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// A `Query` from the CLI must not be able to wedge the supervisor.
const CLI_CONNECTION_TIMEOUT: Duration = Duration::from_secs(2);

/// How often to look at a child we already know is stopped.
///
/// Needed because XNU will not wake a blocking `waitpid` for a continue and
/// raises no `SIGCHLD` for one either, so asking is the only way to see it.
/// Fixed rather than backed off: the resume arrives long after the stop, so an
/// escalating interval would put its worst latency exactly where the person
/// pressing the key is looking.
pub const STOPPED_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// A server holds a dup of the master and is the sole reader.
    Attached,
    /// Nobody is attached; we drain the master so the child never blocks.
    Detached,
}

enum HostEvent {
    ChildExited(Option<i32>),
    /// The child stopped, almost always because `^Z` made the line discipline
    /// send it `SIGTSTP`. A command pane has no shell above the child to `fg`
    /// it, so unless the server is told, the pane simply goes silent forever.
    ChildStopped(i32),
    /// The child was continued again — by us, or by anyone else's `SIGCONT`.
    ChildContinued,
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
    child_pid: ProcessId,
    // Dropping this sender releases the waiter's WNOWAIT reservation on the PID.
    reap_child: Option<Sender<()>>,
    child_reaped: Receiver<std::io::Result<std::process::ExitStatus>>,
    exit_status: Option<Option<i32>>,
    /// Set once the master has nothing left to give. Without it the poll loop
    /// would keep watching a hung-up descriptor that is permanently "ready", and
    /// an orphaned pane whose child has exited would spin a core forever.
    drained: bool,
}

impl Session {
    fn signal_child_group(&self, signal: Signal) -> Result<()> {
        self.reap_child
            .as_ref()
            .context("cannot signal a pane group after releasing its leader")?;
        let pid = super::kernel_pid(self.child_pid);
        // ECHILD means ownership was lost; a numeric PID alone is no longer safe.
        wait_child(pid, libc::WEXITED | libc::WNOWAIT | libc::WNOHANG)
            .context("failed to verify ownership of the pane leader")?;
        let group =
            super::process_group(pid).context("failed to select the pane's process group")?;
        match kill(group, Some(signal)) {
            Ok(()) | Err(Errno::ESRCH) => Ok(()),
            Err(error) => Err(error).with_context(|| {
                format!("failed to send {signal} to pane group {}", self.child_pid)
            }),
        }
    }
}

/// Entry point for `yinz --pty-host`. Never returns.
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
    yinz_runtime::logging::configure_logger();

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
        // Once we own the pane's pty, we are a background process for it as soon
        // as the child takes the foreground. Without these, handing the terminal
        // over — or any stray job-control signal aimed at our group — would stop
        // the supervisor itself, and the pane would freeze with nobody reading
        // its pty. The child resets all three before it execs.
        libc::signal(libc::SIGTTOU, libc::SIG_IGN);
        libc::signal(libc::SIGTTIN, libc::SIG_IGN);
        libc::signal(libc::SIGTSTP, libc::SIG_IGN);
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
/// Validated rather than assumed, so a human typing `yinz --pty-host` gets a
/// diagnostic instead of a process blocked forever on a read from their tty.
fn control_stream_from_fd() -> Result<UnixStream> {
    use nix::sys::socket::{getsockopt, sockopt};
    use std::os::fd::FromRawFd;

    let fd = protocol::CONTROL_FD;
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    let sock_type = getsockopt(&borrowed, sockopt::SockType).map_err(|e| {
        anyhow!(
            "--pty-host is an internal flag: fd {fd} is not a socket ({e}). \
             yinz spawns supervisors itself; there is nothing to run by hand."
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
    let host_pid = ProcessId::new(std::process::id());

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
            yinz_version: VERSION.to_owned(),
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
        termios,
        initial_size,
        arg0,
        env,
        login_session,
        job_control,
    } = spawn
    else {
        cleanup_socket(&host_id);
        return Err(anyhow!("expected Spawn after the handshake"));
    };

    let mut ctx = HostContext {
        host_id,
        host_pid,
        label,
        terminal_id: TerminalId::new(terminal_id),
        session_name,
        cmd,
        started_at_unix: yinz_types::units::UnixSeconds::new(unix_now()),
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

    // Spawn starts a new run; an adopted command may still carry its previous palette.
    ctx.cmd.agent_theme = None;
    let mut env = env;
    env.push(("YINZ_AGENT_THEME_REF".into(), ctx.host_id.to_string()));
    match start_session(
        &ctx.cmd,
        TerminalId::new(terminal_id),
        termios.as_ref(),
        initial_size.as_ref(),
        arg0.as_deref(),
        ChildEnv {
            vars: &env,
            login_session,
            pane_ref: if login_session {
                None
            } else {
                listener.as_ref().map(|_| ctx.host_id.as_str())
            },
        },
        job_control,
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
    host_id: SupervisorId,
    host_pid: ProcessId,
    label: String,
    terminal_id: TerminalId,
    session_name: Option<SessionName>,
    cmd: RunCommand,
    started_at_unix: UnixSeconds,
    state: State,
    ring: RingBuffer,
    session: Option<Session>,
}

impl HostContext {
    fn relabel(
        &mut self,
        expected_session: SessionName,
        session_name: SessionName,
        detached_only: bool,
    ) -> HostToServerMsg {
        if self.session_name.as_ref() != Some(&expected_session)
            || (detached_only && self.state == State::Attached)
        {
            return HostToServerMsg::RelabelRefused {
                message: format!("supervisor {} ownership changed", self.host_id),
            };
        }
        self.label = format!("{session_name}/{}", self.terminal_id);
        self.session_name = Some(session_name);
        HostToServerMsg::Relabelled
    }

    fn status(&self) -> HostStatus {
        HostStatus {
            host_id: self.host_id.clone(),
            session_name: self.session_name.clone(),
            terminal_id: self.terminal_id.get(),
            command: self.cmd.to_string(),
            cwd: self.cmd.cwd.clone(),
            child_pid: self.session.as_ref().map(|s| s.child_pid),
            host_pid: self.host_pid,
            started_at_unix: self.started_at_unix,
            attached: self.state == State::Attached,
            supports_relabel: true,
            child_exit_status: self.session.as_ref().and_then(|s| s.exit_status),
            child_group_terminated: self
                .session
                .as_ref()
                .is_some_and(|s| super::process_group_terminated(s.child_pid)),
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
                            HostEvent::ChildStopped(signal) => {
                                on_child_run_state(
                                    ctx,
                                    HostToServerMsg::ChildStopped { signal },
                                    attached_control.as_mut(),
                                )?;
                            },
                            HostEvent::ChildContinued => {
                                on_child_run_state(
                                    ctx,
                                    HostToServerMsg::ChildContinued,
                                    attached_control.as_mut(),
                                )?;
                            },
                            HostEvent::TerminateRequested => {
                                terminate(ctx, &event_rx).non_fatal();
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
                                CliOutcome::TerminateWithCompletion(mut stream) => {
                                    let error = terminate(ctx, &event_rx)
                                        .err()
                                        .map(|error| format!("{error:#}"));
                                    let reply = HostToServerMsg::TerminationComplete {
                                        terminated: true,
                                        error,
                                    };
                                    let _ = write_msg(&mut stream, &reply);
                                    return Ok(());
                                },
                                CliOutcome::Terminate => {
                                    terminate(ctx, &event_rx).non_fatal();
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
        ServerToHostMsg::Relabel {
            expected_session,
            session_name,
            detached_only,
        } => {
            let reply = ctx.relabel(expected_session, session_name, detached_only);
            write_msg(control, &reply).context("failed to acknowledge relabel")?;
            Ok(ControlOutcome::Continue)
        },
        ServerToHostMsg::Terminate => {
            terminate(ctx, event_rx).non_fatal();
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
        ServerToHostMsg::TerminateDetached { .. } | ServerToHostMsg::Attach { .. } => {
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
        ServerToHostMsg::AgentTheme(event) => {
            // Only the attached server enriches a launch. Named-socket reports
            // retain their start/end semantics, so a duplicate startup snapshot
            // cannot replace colors already learned by the server.
            if event.valid()
                && ctx
                    .session
                    .as_ref()
                    .is_some_and(|s| s.exit_status.is_none())
                && let (Some(active), Some(snapshot)) = (&mut ctx.cmd.agent_theme, event.snapshot)
                && active.launch_id == event.launch_id
            {
                active.snapshot = snapshot;
            }
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
    ctx.cmd.agent_theme = None;
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

/// Report a stop or a continue to the attached server.
///
/// Unlike an exit this changes nothing here: the child is still ours, the pty is
/// still open, and a detached supervisor has nobody to tell — the transition is
/// simply dropped, which is why the server re-derives nothing on attach.
fn on_child_run_state(
    ctx: &mut HostContext,
    msg: HostToServerMsg,
    control: Option<&mut UnixStream>,
) -> Result<()> {
    if ctx
        .session
        .as_ref()
        .is_some_and(|session| session.exit_status.is_some())
    {
        return Ok(());
    }
    if let Some(control) = control {
        write_msg(control, &msg).context("failed to report the child's run state")?;
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
    if exhausted && let Some(session) = ctx.session.as_mut() {
        session.drained = true;
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
    TerminateWithCompletion(UnixStream),
    /// The connection has become our new control channel.
    Attached(UnixStream),
}

/// Handles one `pty-host list|kill|adopt` connection inline.
fn serve_cli_connection(ctx: &mut HostContext, mut stream: UnixStream) -> CliOutcome {
    // A wedged or malicious client must not be able to stall the supervisor.
    let _ = stream.set_read_timeout(Some(CLI_CONNECTION_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CLI_CONNECTION_TIMEOUT));
    match read_msg::<ServerToHostMsg, _>(&mut stream) {
        Ok(Some(ServerToHostMsg::AgentTheme(event))) => {
            if ctx
                .session
                .as_ref()
                .is_some_and(|session| session.exit_status.is_none())
            {
                event.apply(&mut ctx.cmd.agent_theme);
            }
            let _ = write_msg(&mut stream, &HostToServerMsg::Status(ctx.status()));
            CliOutcome::Done
        },
        Ok(Some(ServerToHostMsg::Relabel {
            expected_session,
            session_name,
            detached_only,
        })) => {
            let reply = ctx.relabel(expected_session, session_name, detached_only);
            let _ = write_msg(&mut stream, &reply);
            CliOutcome::Done
        },
        Ok(Some(ServerToHostMsg::Query)) => {
            let _ = write_msg(&mut stream, &HostToServerMsg::Status(ctx.status()));
            CliOutcome::Done
        },
        Ok(Some(ServerToHostMsg::TerminateDetached { expected_session })) => {
            if ctx.state == State::Detached && ctx.session_name.as_ref() == Some(&expected_session)
            {
                CliOutcome::TerminateWithCompletion(stream)
            } else {
                let _ = write_msg(
                    &mut stream,
                    &HostToServerMsg::TerminationComplete {
                        terminated: false,
                        error: None,
                    },
                );
                CliOutcome::Done
            }
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
        })) => attach(
            ctx,
            stream,
            protocol_version,
            TerminalId::new(terminal_id),
            session_name,
        ),
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
    terminal_id: TerminalId,
    session_name: Option<SessionName>,
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
        yinz_version: VERSION.to_owned(),
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

fn kill_current_child(ctx: &mut HostContext, event_rx: &Receiver<HostEvent>) -> Result<()> {
    let Some(mut session) = ctx.session.take() else {
        return Ok(());
    };
    // Stopped group members must resume to handle the hangup.
    session.signal_child_group(Signal::SIGCONT)?;
    session.signal_child_group(Signal::SIGHUP)?;
    let grace_deadline = std::time::Instant::now() + TERMINATE_GRACE;
    let kill_deadline = grace_deadline + TERMINATE_KILL_GRACE;
    let mut leader_reaped = false;
    let mut escalated = false;
    loop {
        let now = std::time::Instant::now();
        if !escalated && now >= grace_deadline {
            // Keep the leader unreaped through the last signal, even if it exited earlier.
            session.signal_child_group(Signal::SIGKILL)?;
            session.reap_child.take();
            escalated = true;
        }
        if escalated && !leader_reaped {
            match session.child_reaped.try_recv() {
                Ok(result) => {
                    result.context("failed to reap the pane leader")?;
                    leader_reaped = true;
                },
                Err(mpsc::TryRecvError::Empty) => {},
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(anyhow!("pane waiter ended without reaping its leader"));
                },
            }
        }
        let group_gone = leader_reaped && super::process_group_terminated(session.child_pid);
        if group_gone {
            return Ok(());
        }
        if now >= kill_deadline {
            return Err(anyhow!(
                "pane {} teardown unconfirmed: leader reaped={leader_reaped}, group gone={group_gone}",
                session.child_pid
            ));
        }
        let deadline = if escalated {
            kill_deadline
        } else {
            grace_deadline
        };
        let wait = deadline
            .saturating_duration_since(now)
            .min(TERMINATE_POLL_INTERVAL);
        match event_rx.recv_timeout(wait) {
            Ok(_) | Err(RecvTimeoutError::Timeout) => {},
            // The leader's waiter can be gone while its group still needs escalation.
            Err(RecvTimeoutError::Disconnected) => std::thread::sleep(wait),
        }
    }
}

fn terminate(ctx: &mut HostContext, event_rx: &Receiver<HostEvent>) -> Result<()> {
    log::info!("pty-host {} ({}) terminating", ctx.host_id, ctx.label);
    kill_current_child(ctx, event_rx).context("failed to terminate pane")
}

fn cleanup_socket(host_id: &SupervisorId) {
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

fn bind_listener(host_id: &SupervisorId) -> Result<UnixListener> {
    let dir = protocol::socket_dir();
    ensure_secure_dir(&dir)?;
    let path = protocol::socket_path(host_id);
    protocol::check_socket_path_length(&path, 0)?;
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
    let listener =
        UnixListener::bind(&path).with_context(|| format!("failed to bind {}", path.display()))?;
    // Sticky, like the session socket: supervisors outlive everything and are
    // exactly what a runtime-dir cleaner will try to reap out from under us.
    secure_socket(&path, 0o1700)?;
    Ok(listener)
}

fn classify_spawn_failure(err: &anyhow::Error) -> SpawnFailureKind {
    match err.downcast_ref::<YinzError>() {
        Some(YinzError::CommandNotFound { .. }) => SpawnFailureKind::CommandNotFound,
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

/// Everything the child's environment is made of, as decided elsewhere.
///
/// Bundled rather than passed as two more arguments because `start_session` is
/// already at the `too_many_arguments` threshold, and because the two always
/// travel together: `login_session` is only ever meaningful alongside a `vars`
/// the broker composed.
#[derive(Clone, Copy, Default)]
struct ChildEnv<'a> {
    vars: &'a [(String, String)],
    login_session: bool,
    /// Present only when the named socket exists, because that socket is how a
    /// descendant resolves the reference after the pane moves.
    pane_ref: Option<&'a str>,
}

/// Create the pty and fork the child. This is `handle_openpty`
/// (`os_input_output_unix.rs`) moved across the process boundary, with the
/// panics turned into errors — a panic in a `pre_exec` hook runs the panic hook
/// in a forked child of a multithreaded process, which is not safe.
#[allow(clippy::too_many_arguments)]
fn start_session(
    cmd: &RunCommand,
    terminal_id: TerminalId,
    termios: Option<&WireTermios>,
    initial_size: Option<&WireWinsize>,
    arg0: Option<&str>,
    child_env: ChildEnv<'_>,
    // Whether the caller wants — and can finish — job control. See
    // `ServerToHostMsg::Spawn::job_control`.
    job_control: bool,
    event_tx: &Sender<HostEvent>,
    wake_tx: &UnixStream,
) -> Result<Session> {
    let err_context = || {
        format!(
            "failed to open a pty for '{}'",
            cmd.command.to_string_lossy()
        )
    };

    // Give missing commands the structured error the server displays in the pane.
    // `spawn()` below still reports failures such as a missing script interpreter
    // or a command removed after this check.
    // Against the `PATH` the *child* will get, not this supervisor's: for a remote
    // pane those differ, and `Command` execs against the environment it was given.
    // Searching the wrong one would reject a command that would have exec'd fine.
    if !crate::command::can_exec(cmd, crate::pty_host::remote::env::path_in(child_env.vars)) {
        return Err(YinzError::CommandNotFound {
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

    let pty =
        super::terminal::open(winsize.as_ref(), termios.as_ref()).with_context(err_context)?;
    let master = pty.master;
    let slave = pty.slave;
    let slave_raw = slave.as_raw_fd();
    let mut command = crate::command::prepare_command(cmd).with_context(err_context)?;
    if let Some(arg0) = arg0 {
        // The whole of what makes a shell a *login* shell. Local panes pass
        // `None` and exec exactly as they always have.
        command.arg0(arg0);
    }
    if child_env.login_session {
        // Whatever ssh session started this machine's broker left its own
        // `SSH_*` in the environment every pane inherits. They describe a
        // connection this pane has no part in, and a stale one at that.
        for name in crate::pty_host::remote::env::SCRUBBED_VARS {
            command.env_remove(name);
        }
    }
    // A nested yinz inside a remote pane inherits that pane's broker token, and unlike the
    // supervisor reference it sits beside a `YINZ_PANE_ID` that *is* this machine's. A hook
    // in the inner session tests the token first - by design, because in a remote pane the
    // pane id is the asking session's - so an inherited one would report the outer pane. Only
    // a spawn that names a token gets one, and the broker names it in `vars`, just below.
    command.env_remove(crate::pty_host::remote::broker::PANE_TOKEN_ENV);
    // A new pane has no inline hooks unless its own launch explicitly supplies them.
    command.env_remove("YINZ_CLAUDE_INLINE_HOOKS");
    for (name, value) in child_env.vars {
        command.env(name, value);
    }
    match child_env.pane_ref {
        Some(pane_ref) => {
            command.env(super::PANE_REF_ENV_KEY, pane_ref);
        },
        None => {
            // A nested yinz can inherit its outer pane's reference. Never let
            // a child without its own named supervisor route reports through it.
            command.env_remove(super::PANE_REF_ENV_KEY);
        },
    }
    if child_env.login_session {
        // The one variable only this process can answer, and only now: the pane's
        // pty exists, and nothing upstream of here can name it.
        match nix::unistd::ttyname(&slave) {
            Ok(path) => {
                command.env("SSH_TTY", path);
            },
            // Not fatal - a pane without SSH_TTY is what every remote pane had
            // until now.
            Err(e) => log::warn!("could not name the pane's pty for SSH_TTY: {e}"),
        }
    }
    command
        .env("YINZ_PANE_ID", format!("{}", terminal_id))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Take the pane's pty as *our* controlling terminal, so the child can have a
    // process group instead of a whole session — see `acquire_controlling_tty`.
    let job_control = job_control && acquire_controlling_tty(slave_raw);

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
            super::terminal::prepare_child(slave_raw, job_control)
        });
    }

    let child_wake = wake_tx
        .try_clone()
        .context("failed to clone the child wake channel")?;
    let child = command
        .spawn()
        .with_context(|| format!("failed to spawn '{}'", cmd.command.to_string_lossy()))?;
    let child_pid = ProcessId::new(child.id());
    if job_control {
        // Both sides do this, because either one alone is a race: the child may
        // not have run yet when we return, and it may already have exec'd and
        // read from the tty before we get here. Both are idempotent.
        let kernel_pid = super::kernel_pid(child_pid).as_raw();
        unsafe {
            libc::setpgid(kernel_pid, kernel_pid);
            libc::tcsetpgrp(slave_raw, kernel_pid);
        }
    }

    let (reap_child, child_reaped) = spawn_child_waiter(child, event_tx.clone(), child_wake);

    Ok(Session {
        master,
        slave: Some(slave),
        child_pid,
        reap_child: Some(reap_child),
        child_reaped,
        exit_status: None,
        drained: false,
    })
}

/// Make the pane's pty this supervisor's controlling terminal.
///
/// This is what makes `^Z` work at all. `login_tty` — what this replaces — puts
/// the child in a *new session*, which makes its process group orphaned by
/// definition: no member has a parent in a different group of the same session.
/// Linux discards `SIGTSTP`/`SIGTTIN`/`SIGTTOU` aimed at an orphaned group, so
/// `^Z` in a command pane did nothing whatsoever — the pane's command was never
/// suspended, however much it behaved as if it had been.
///
/// The supervisor is already a session leader with no controlling terminal
/// (`run_pty_host` calls `setsid`), which is exactly the shape job control wants:
/// it takes the tty, and the child gets a process group inside *our* session,
/// with us as its parent in a different group. Not orphaned, so `^Z` stops it and
/// `SIGCONT` wakes it.
///
/// Returns false if we could not take the terminal — a second `Spawn` on the same
/// supervisor, or `serve()` driven in-process by a test, where this process is
/// somebody else's session member. The caller then falls back to `login_tty` and
/// the pane behaves exactly as it did before.
fn acquire_controlling_tty(slave_raw: RawFd) -> bool {
    // Only a session leader may take a controlling terminal, and asking when we
    // are not one is not merely futile: `serve()` is driven in-process by tests,
    // and a process that happened to qualify would hand its whole session's
    // terminal over to a test pane.
    let is_session_leader = unsafe { libc::getsid(0) } == unsafe { libc::getpid() };
    if !is_session_leader {
        log::info!("pty-host is not a session leader; the pane will not have job control");
        return false;
    }
    if unsafe { libc::ioctl(slave_raw, libc::TIOCSCTTY as _, 0) } != 0 {
        log::info!(
            "pty-host could not take the pane's pty as its controlling terminal ({}); \
             the pane will not have job control",
            std::io::Error::last_os_error()
        );
        return false;
    }
    true
}

/// Watch the child for good, not just for its exit.
///
/// `Child::wait` cannot observe job control: it is `waitpid` without `WUNTRACED`, so a
/// stopped child is indistinguishable from a busy one and `^Z` in a command pane
/// looks exactly like a hang. Observe exits with WNOWAIT to reserve the leader's
/// PID until the session releases it after the last group signal. Dropping the
/// release sender on an error also lets this thread reap the child when it exits.
///
/// Once the child *is* stopped the wait has to become a poll, because
/// `WCONTINUED` is not enough on every kernel. On XNU a blocking `waitpid` is
/// never woken by a continue — not even one entered after the `SIGCONT` — and no
/// `SIGCHLD` is raised for one either. Only `WNOHANG` returns the status.
///
/// Missing that report is not a cosmetic loss. The pane's `suspended` flag is
/// cleared by nothing else, and while it is set
/// `TerminalPane::adjust_input_to_terminal` swallows every key, so the pane is
/// input-dead for the rest of its life: `Enter` sends another `SIGCONT` to a
/// process that already resumed, and nothing else gets through.
///
/// The poll is confined to that window. A running child is still waited on
/// blocking, which is the whole of the common case, and the cost while suspended
/// is one timer per pane. Done on every unix rather than behind a `target_os`,
/// because a platform-split loop would leave whichever arm this machine is not
/// running untested — there is no CI here to run the other one.
///
/// One gap remains, and it is the kernel's: XNU holds a pending continue only
/// until the process next `exec`s, which discards it. A child stopped in the
/// instant between `fork` and `exec` — a shell that has not yet become the
/// command it was told to run — and resumed there will therefore still be
/// reported as stopped. Polling narrows that to the interval below; closing it
/// entirely would mean reading the process's state rather than waiting on it.
fn spawn_child_waiter(
    mut child: Child,
    event_tx: Sender<HostEvent>,
    wake_tx: UnixStream,
) -> (
    Sender<()>,
    Receiver<std::io::Result<std::process::ExitStatus>>,
) {
    let (reap_tx, reap_rx) = mpsc::channel();
    let (reaped_tx, reaped_rx) = mpsc::channel();
    let mut wake_tx = wake_tx;
    std::thread::spawn(move || {
        let pid = Pid::from_raw(child.id() as i32);
        let flags = libc::WEXITED | libc::WSTOPPED | libc::WCONTINUED | libc::WNOWAIT;
        let mut stopped = false;
        loop {
            let flags = if stopped {
                flags | libc::WNOHANG
            } else {
                flags
            };
            let observed = wait_child(pid, flags).and_then(|event| match event {
                Some(HostEvent::ChildStopped(_) | HostEvent::ChildContinued) => {
                    // Consume job-control events, but never an exit that races with them.
                    wait_child(pid, libc::WSTOPPED | libc::WCONTINUED | libc::WNOHANG).or_else(
                        |error| {
                            if error == Errno::ECHILD {
                                // An exited child is ineligible for a job-control-only wait.
                                wait_child(pid, libc::WEXITED | libc::WNOWAIT | libc::WNOHANG)
                            } else {
                                Err(error)
                            }
                        },
                    )
                },
                other => Ok(other),
            });
            let event = match observed {
                Ok(Some(event)) => event,
                // No pending state, and the only arm that sleeps —
                // which is what keeps the poll from becoming a spin. The first
                // poll after a stop is deliberately not delayed: the kernel
                // holds a continue only until the process next `exec`s, so the
                // sooner it is asked the smaller that window is.
                Ok(None) => {
                    std::thread::sleep(STOPPED_POLL_INTERVAL);
                    continue;
                },
                Err(Errno::EINTR) => continue,
                // ECHILD and friends: there is nothing left to wait for, and a
                // waiter that returned without reporting would leave the pane
                // open forever. Report an unknown status instead.
                Err(error) => {
                    Err::<(), _>(error)
                        .context("failed to observe the pane child")
                        .non_fatal();
                    HostEvent::ChildExited(None)
                },
            };
            // Recomputed every time, so a continue or an exit puts the loop back
            // on the blocking wait.
            stopped = matches!(event, HostEvent::ChildStopped(_));
            let is_exit = matches!(event, HostEvent::ChildExited(_));
            let _ = event_tx.send(event);
            let _ = wake_tx.write_all(b"w");
            if is_exit {
                break;
            }
        }
        let _ = reap_rx.recv();
        let _ = reaped_tx.send(child.wait());
    });
    (reap_tx, reaped_rx)
}

// nix does not expose waitid on macOS; libc supports the same WNOWAIT contract there.
fn wait_child(pid: Pid, flags: i32) -> nix::Result<Option<HostEvent>> {
    // Zeroing also handles kernels that leave siginfo untouched for WNOHANG.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    Errno::result(unsafe {
        libc::waitid(libc::P_PID, pid.as_raw() as libc::id_t, &mut info, flags)
    })?;
    decode_child_event(&info)
}

fn decode_child_event(info: &libc::siginfo_t) -> nix::Result<Option<HostEvent>> {
    // XNU can report CLD_CONTINUED with a zero PID.
    if info.si_signo == 0 {
        return Ok(None);
    }
    let status = unsafe { info.si_status() };
    match info.si_code {
        libc::CLD_EXITED => Ok(Some(HostEvent::ChildExited(Some(status)))),
        libc::CLD_KILLED | libc::CLD_DUMPED => Ok(Some(HostEvent::ChildExited(None))),
        libc::CLD_STOPPED | libc::CLD_TRAPPED => Ok(Some(HostEvent::ChildStopped(status))),
        libc::CLD_CONTINUED => Ok(Some(HostEvent::ChildContinued)),
        _ => Err(Errno::EINVAL),
    }
}

#[cfg(test)]
#[path = "../unit/pty_host_tests.rs"]
mod pty_host_tests;
