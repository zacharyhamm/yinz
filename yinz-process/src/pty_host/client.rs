//! Server side of the pty-host relationship: spawn a supervisor for a pane, take
//! the pty master it hands back, and translate its lifecycle messages into the
//! same `quit_cb` the in-process waiter thread used to invoke.

use std::net::Shutdown;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use yinz_types::internal_ids::ProcessId;
use yinz_types::internal_ids::SessionName;
use yinz_types::internal_ids::SupervisorId;
use yinz_types::internal_ids::TerminalId;

use nix::sys::signal::{kill, Signal};
use yinz_runtime::errors::prelude::*;
use {yinz_runtime::consts::VERSION, yinz_types::input::command::RunCommand};

use super::fdpass::{clear_cloexec, recv_fd};
use super::protocol::{
    self, read_msg, write_msg, AttachRefusal, HostStatus, HostToServerMsg, ServerToHostMsg,
    SpawnFailureKind, WireTermios, WireWinsize, CONTROL_FD, PROTOCOL_VERSION,
};
use super::PTY_HOST_FLAG;

/// `spawn_terminal` runs synchronously on the pty thread, which also services
/// pane close, tab close and layout dumps. A supervisor that wedges during its
/// handshake must not take the session with it.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long to let a supervisor wind itself down after `Terminate` before we
/// SIGKILL it. It gives its own child a 2s grace period first.
const HOST_EXIT_GRACE: Duration = Duration::from_secs(4);

pub struct SpawnParams<'a> {
    pub host_bin: &'a Path,
    pub terminal_id: TerminalId,
    pub session_name: Option<SessionName>,
    pub cmd: RunCommand,
    pub termios: Option<WireTermios>,
    pub initial_size: Option<WireWinsize>,
    /// See `ServerToHostMsg::Spawn::arg0`. `None` everywhere but the remote
    /// broker, which uses it to start a login shell.
    pub arg0: Option<String>,
    /// Overrides for the inherited environment, including the current local session
    /// identity or the remote broker's composed environment.
    pub env: Vec<(String, String)>,
    /// See `ServerToHostMsg::Spawn::login_session`. Only the remote broker sets
    /// it - a local pane is not a login and must not grow an `SSH_TTY`.
    pub login_session: bool,
    /// See `ServerToHostMsg::Spawn::job_control`. Set by the local server, which
    /// listens for the resulting stops; never by the broker, which does not.
    pub job_control: bool,
}

/// A supervisor that is up and has handed over its pty, but whose lifecycle
/// messages nobody is listening to yet.
///
/// Split out from `SpawnedHost` so that every fallible step happens *before* the
/// caller commits its `quit_cb`: if starting a supervisor fails, the caller still
/// owns the callback and can fall back to opening the pty in-process. Dropping a
/// `PendingHost` without calling `finish` kills the supervisor.
pub struct PendingHost {
    control: Option<UnixStream>,
    reader_stream: Option<UnixStream>,
    host_child: Option<Child>,
    host_id: SupervisorId,
    host_pid: ProcessId,
    control_socket: Option<PathBuf>,
    master: Option<OwnedFd>,
    child_pid: ProcessId,
    terminal_id: TerminalId,
    cmd: RunCommand,
}

/// How this server can observe the supervisor process itself.
///
/// Only ever used for reaping. A supervisor's *death* is detected the same way
/// either side of this split: the control channel hits EOF.
enum HostProcess {
    /// We forked it, so we can and must `waitpid` it.
    Spawned(Child),
    /// Adopted over a named socket. Not our child — `waitpid` would return
    /// `ECHILD` — so liveness is probed with `kill(pid, 0)` and the reaping is
    /// left to init.
    Foreign,
}

impl PendingHost {
    pub fn finish(mut self, quit_cb: QuitCb, child_state_cb: Option<ChildStateCb>) -> SpawnedHost {
        let reported = Arc::new(AtomicBool::new(false));
        let released = Arc::new(AtomicBool::new(false));
        spawn_reader_thread(
            self.reader_stream
                .take()
                .expect("reader stream taken twice"),
            HostProcess::Spawned(self.host_child.take().expect("host child taken twice")),
            self.host_id.clone(),
            self.host_pid,
            self.terminal_id,
            self.cmd.clone(),
            quit_cb,
            child_state_cb,
            reported.clone(),
            released.clone(),
        );
        SpawnedHost {
            handle: PtyHostHandle {
                host_id: std::mem::take(&mut self.host_id),
                host_pid: self.host_pid,
                child_pid: self.child_pid,
                control_socket: self.control_socket.take(),
                control: self.control.take().expect("control taken twice"),
                reported,
                released,
            },
            master: self.master.take().expect("master taken twice"),
            child_pid: self.child_pid,
        }
    }
}

impl Drop for PendingHost {
    fn drop(&mut self) {
        if let Some(mut child) = self.host_child.take() {
            // Never consumed, so no reader thread will ever reap it.
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub struct SpawnedHost {
    pub handle: PtyHostHandle,
    pub master: OwnedFd,
    pub child_pid: ProcessId,
}

/// The server's end of one supervisor. Dropping it does **not** kill the pane —
/// that is the whole point. Only `terminate()` does.
pub struct PtyHostHandle {
    pub child_pid: ProcessId,
    pub host_id: SupervisorId,
    pub host_pid: ProcessId,
    pub control_socket: Option<PathBuf>,
    control: UnixStream,
    /// Set once the pane's exit has been reported, so `ChildExited` and an
    /// unexpected supervisor death cannot both fire `quit_cb`.
    reported: Arc<AtomicBool>,
    /// Set when the pane is being handed to someone else rather than closed, so
    /// the reader thread winds itself down instead of reaping the supervisor.
    released: Arc<AtomicBool>,
}

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

#[cfg(test)]
impl PtyHostHandle {
    /// A handle with a socketpair where a supervisor would be, so a test can
    /// build the things that own one — `PaneSession`, mostly — without forking
    /// anything.
    ///
    /// The caller must keep the peer end alive for as long as the handle. A
    /// failed write in `terminate()` falls back to signalling `host_pid`, and
    /// this handle's is not a real supervisor's.
    pub(crate) fn for_test(control: UnixStream) -> Self {
        Self {
            host_id: SupervisorId::new("test"),
            host_pid: ProcessId::new(std::process::id()),
            child_pid: ProcessId::new(std::process::id()),
            control_socket: None,
            control,
            reported: Arc::new(AtomicBool::new(false)),
            released: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl PtyHostHandle {
    /// Enrich the active launch without starting or ending a launch. The
    /// supervisor consumes this before EOF if the owning server crashes.
    pub fn retain_agent_theme(
        &mut self,
        event: yinz_types::agent::theme::AgentThemeEvent,
    ) -> Result<()> {
        if !event.valid() || event.snapshot.is_none() {
            bail!("invalid agent palette update");
        }
        if serde_json::to_vec(&event)
            .context("encode learned agent palette")?
            .len()
            > yinz_types::agent::theme::MAX_SNAPSHOT_BYTES
        {
            bail!("agent palette update exceeds metadata limit");
        }
        self.control
            .set_write_timeout(Some(HANDSHAKE_TIMEOUT))
            .context("bound agent palette update")?;
        write_msg(&mut self.control, &ServerToHostMsg::AgentTheme(event))
            .context("send learned palette to supervisor")
    }

    /// Kill the pane's process and shut the supervisor down.
    ///
    /// Deliberately synchronous and deliberately not a channel send: this runs on
    /// paths that end with the server process exiting, and a queued message would
    /// die with it. A blocking `write` lands the bytes in the kernel socket
    /// buffer, and the supervisor reads them before it observes EOF — which is
    /// what makes "the server asked me to stop" distinguishable from "the server
    /// crashed" at all.
    pub fn terminate(&mut self) {
        // Suppress the synthesized close from the reader thread: this exit is
        // expected and the pane is already being torn down.
        self.reported.store(true, Ordering::SeqCst);
        let _ = self.control.set_write_timeout(Some(HANDSHAKE_TIMEOUT));
        if let Err(e) = write_msg(&mut self.control, &ServerToHostMsg::Terminate) {
            log::warn!(
                "could not ask pty-host {} to terminate ({e:#}); killing it",
                self.host_id
            );
            super::process_pid(self.host_pid)
                .and_then(|pid| kill(pid, Some(Signal::SIGKILL)).context("failed to kill pty host"))
                .non_fatal();
        }
        // Unblocks the reader thread so it can reap the supervisor and exit.
        let _ = self.control.shutdown(Shutdown::Both);
    }

    /// Hand this pane's supervisor to someone else.
    ///
    /// The opposite of `terminate` in every way that matters: the supervisor and
    /// its child keep running, and the next owner picks them up with
    /// `attach()`. Two flags have to be set first, and both are load-bearing:
    ///
    /// * `reported`, so the reader thread's "the supervisor died without telling
    ///   us" fallback does not fire `quit_cb` and close the pane on its way out.
    ///   The pane is leaving, not dying.
    /// * `released`, so the reader thread skips `reap_host`, which waits
    ///   `HOST_EXIT_GRACE` for an exit that is never coming and then `SIGKILL`s.
    ///   Without it a warp would work, and four seconds later the pane would die
    ///   in the session that just adopted it.
    pub fn release(mut self) -> ReleasedPtyHost {
        self.reported.store(true, Ordering::SeqCst);
        self.released.store(true, Ordering::SeqCst);
        let _ = self.control.set_write_timeout(Some(HANDSHAKE_TIMEOUT));
        if let Err(e) = write_msg(&mut self.control, &ServerToHostMsg::Release) {
            // Not fatal: the shutdown below detaches it anyway, just without the
            // ordering guarantee that makes a hand-off race-free.
            log::warn!(
                "could not tell pty-host {} it was released ({e:#}); \
                 it will notice when the channel closes",
                self.host_id
            );
        }
        // Acts on the socket rather than one descriptor, so the reader thread's
        // `try_clone` sees EOF too. Without this the supervisor keeps one live
        // dup, never leaves `Attached`, and refuses every attempt to attach.
        let _ = self.control.shutdown(Shutdown::Both);
        ReleasedPtyHost {
            host_id: std::mem::take(&mut self.host_id),
            host_pid: self.host_pid,
            control_socket: self.control_socket.take(),
        }
    }
}

/// Spawn a supervisor for one pane and take the pty master back from it.
///
/// Any failure leaves nothing behind: the half-started supervisor is killed and
/// the caller is free to fall back to opening the pty in-process.
pub fn spawn(params: SpawnParams<'_>) -> Result<PendingHost> {
    let err_context = || {
        format!(
            "failed to start a pty-host for '{}'",
            params.cmd.command.to_string_lossy()
        )
    };

    let (server_end, host_end) = UnixStream::pair().with_context(err_context)?;

    let label = match params.session_name.as_ref().map(|name| name.as_str()) {
        Some(session) => format!("{session}/{}", params.terminal_id),
        None => format!("{}", params.terminal_id),
    };

    let host_raw = host_end.as_raw_fd();
    let mut command = Command::new(params.host_bin);
    command
        .arg(PTY_HOST_FLAG)
        .arg(&label)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(move || -> std::io::Result<()> {
            place_control_fd(host_raw)?;
            Ok(())
        });
    }
    let host_child = command.spawn().with_context(err_context)?;
    // The parent must not keep the supervisor's end open, or our own reads would
    // never see EOF when it dies.
    drop(host_end);

    let mut guard = HostGuard {
        child: Some(host_child),
    };

    let (child_pid, master, host_id, host_pid, control_socket) =
        handshake_and_spawn(&server_end, &params).with_context(err_context)?;

    // Back to blocking reads now the handshake is done; the reader thread should
    // sit on this socket indefinitely.
    let _ = server_end.set_read_timeout(None);
    let _ = server_end.set_write_timeout(None);
    let reader_stream = server_end.try_clone().with_context(err_context)?;

    Ok(PendingHost {
        control: Some(server_end),
        reader_stream: Some(reader_stream),
        host_child: Some(guard.take()),
        host_id,
        host_pid,
        control_socket,
        master: Some(master),
        child_pid,
        terminal_id: params.terminal_id,
        cmd: params.cmd,
    })
}

/// Kills a half-started supervisor if we bail out before handing it over to the
/// reader thread.
struct HostGuard {
    child: Option<Child>,
}

impl HostGuard {
    fn take(&mut self) -> Child {
        self.child.take().expect("host child taken twice")
    }
}

impl Drop for HostGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
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

fn handshake_and_spawn(
    control: &UnixStream,
    params: &SpawnParams<'_>,
) -> Result<(ProcessId, OwnedFd, SupervisorId, ProcessId, Option<PathBuf>)> {
    control
        .set_read_timeout(Some(HANDSHAKE_TIMEOUT))
        .context("failed to bound the handshake")?;
    control
        .set_write_timeout(Some(HANDSHAKE_TIMEOUT))
        .context("failed to bound the handshake")?;

    let mut stream = control;
    write_msg(
        &mut stream,
        &ServerToHostMsg::Hello {
            protocol_version: PROTOCOL_VERSION,
            yinz_version: VERSION.to_owned(),
        },
    )
    .context("failed to send Hello")?;

    let ack: HostToServerMsg = read_msg(&mut stream)
        .context("failed to read HelloAck")?
        .ok_or_else(|| anyhow!("pty-host closed the control channel during the handshake"))?;
    let HostToServerMsg::HelloAck {
        protocol_version,
        host_id,
        host_pid,
        control_socket,
        ..
    } = ack
    else {
        return Err(anyhow!("expected HelloAck, got {ack:?}"));
    };
    if protocol_version != PROTOCOL_VERSION {
        // `current_exe()` re-execs whatever is on disk now, so a package upgrade
        // mid-session lands here. Not an error worth failing a pane over: the
        // caller falls back to opening the pty in-process.
        return Err(anyhow!(
            "pty-host speaks protocol v{protocol_version}, we speak v{PROTOCOL_VERSION}"
        ));
    }

    write_msg(
        &mut stream,
        &ServerToHostMsg::Spawn {
            terminal_id: params.terminal_id.get(),
            session_name: params.session_name.clone(),
            cmd: params.cmd.clone(),
            termios: params.termios.clone(),
            initial_size: params.initial_size,
            arg0: params.arg0.clone(),
            env: params.env.clone(),
            login_session: params.login_session,
            job_control: params.job_control,
        },
    )
    .context("failed to send Spawn")?;

    let reply: HostToServerMsg = read_msg(&mut stream)
        .context("failed to read the spawn result")?
        .ok_or_else(|| anyhow!("pty-host closed the control channel during spawn"))?;
    match reply {
        HostToServerMsg::Spawned { child_pid } => {
            let master = recv_fd(control).context("failed to take the pty master")?;
            Ok((child_pid, master, host_id, host_pid, control_socket))
        },
        HostToServerMsg::SpawnFailed { kind, message } => match kind {
            // Preserves the exact error eight call sites in pty.rs downcast for.
            // The server checks this before spawning a supervisor at all, so
            // reaching it means the command vanished in between.
            SpawnFailureKind::CommandNotFound => Err(YinzError::CommandNotFound {
                terminal_id: params.terminal_id,
                command: params.cmd.command.to_string_lossy().to_string(),
            }
            .into()),
            SpawnFailureKind::Other => Err(anyhow!("pty-host failed to spawn: {message}")),
        },
        other => Err(anyhow!("expected Spawned, got {other:?}")),
    }
}

/// How long to keep retrying an attach that is refused because someone is still
/// attached. A server that was `kill -9`'d leaves its supervisor holding a socket
/// it has not yet noticed is dead, and that takes a scheduling quantum, not a
/// human timescale.
const ATTACH_RETRY_WINDOW: Duration = Duration::from_millis(1000);
const ATTACH_RETRY_INTERVAL: Duration = Duration::from_millis(50);

pub struct AttachParams<'a> {
    pub control_socket: &'a Path,
    /// The terminal id this pane will have in the *adopting* session. Ids are
    /// per-server, so the one it had before means nothing here.
    pub terminal_id: TerminalId,
    pub session_name: Option<SessionName>,
}

/// A supervisor that has handed its pty over to us, but whose lifecycle messages
/// nobody is listening to yet. The attach-side mirror of `PendingHost`.
pub struct PendingAttach {
    control: Option<UnixStream>,
    reader_stream: Option<UnixStream>,
    host_id: SupervisorId,
    host_pid: ProcessId,
    control_socket: PathBuf,
    master: Option<OwnedFd>,
    child_pid: ProcessId,
    terminal_id: TerminalId,
    cmd: RunCommand,
    buffered: Vec<u8>,
    status: HostStatus,
}

impl PendingAttach {
    /// The command the supervisor reported, for rebuilding a `quit_cb` that holds
    /// or closes the pane the way the original did.
    pub fn cmd(&self) -> &RunCommand {
        &self.cmd
    }

    pub fn status(&self) -> &HostStatus {
        &self.status
    }

    pub fn finish(mut self, quit_cb: QuitCb, child_state_cb: Option<ChildStateCb>) -> AttachedHost {
        let reported = Arc::new(AtomicBool::new(false));
        let released = Arc::new(AtomicBool::new(false));
        spawn_reader_thread(
            self.reader_stream
                .take()
                .expect("reader stream taken twice"),
            HostProcess::Foreign,
            self.host_id.clone(),
            self.host_pid,
            self.terminal_id,
            self.cmd.clone(),
            quit_cb,
            child_state_cb,
            reported.clone(),
            released.clone(),
        );
        AttachedHost {
            handle: PtyHostHandle {
                host_id: std::mem::take(&mut self.host_id),
                host_pid: self.host_pid,
                child_pid: self.child_pid,
                control_socket: Some(std::mem::take(&mut self.control_socket)),
                control: self.control.take().expect("control taken twice"),
                reported,
                released,
            },
            master: self.master.take().expect("master taken twice"),
            child_pid: self.child_pid,
            cmd: std::mem::take(&mut self.cmd),
            buffered: std::mem::take(&mut self.buffered),
            status: self.status.clone(),
        }
    }
}

/// Deliberately *not* a mirror of `Drop for PendingHost`, which kills the
/// supervisor it half-started. An attach that falls over halfway has not created
/// anything: the supervisor belonged to someone before us and, if we walk away,
/// belongs to whoever attaches next. Closing the socket is the whole job — that
/// puts it straight back to draining into its ring.
impl Drop for PendingAttach {
    fn drop(&mut self) {
        if let Some(control) = self.control.take() {
            let _ = control.shutdown(Shutdown::Both);
        }
    }
}

pub struct AttachedHost {
    pub handle: PtyHostHandle,
    pub master: OwnedFd,
    pub child_pid: ProcessId,
    pub cmd: RunCommand,
    /// Everything the child wrote while nobody was attached. Replayed into the
    /// new pane before any live output, so the hand-off costs no bytes.
    pub buffered: Vec<u8>,
    pub status: HostStatus,
}

/// Take over a detached supervisor and get its pty master.
///
/// The counterpart of `spawn`, and the same shape once the process exists:
/// handshake, then a descriptor over `SCM_RIGHTS`. Failure leaves the supervisor
/// exactly as it was found — running, detached, and available to the next caller.
pub fn attach(params: AttachParams<'_>) -> Result<PendingAttach> {
    let err_context = || {
        format!(
            "failed to attach to the pty-host at {}",
            params.control_socket.display()
        )
    };
    let deadline = Instant::now() + ATTACH_RETRY_WINDOW;
    loop {
        match try_attach(&params) {
            Ok(pending) => return Ok(pending),
            Err(e) => {
                let still_attached = matches!(
                    e.downcast_ref::<AttachRefusedError>(),
                    Some(AttachRefusedError {
                        reason: AttachRefusal::AlreadyAttached,
                        ..
                    })
                );
                if !still_attached || Instant::now() >= deadline {
                    return Err(e).with_context(err_context);
                }
                // The previous owner is on its way out but the supervisor has not
                // processed the closed channel yet. Give it a moment.
                std::thread::sleep(ATTACH_RETRY_INTERVAL);
            },
        }
    }
}

/// A refusal from the supervisor, kept typed so `attach` can tell "try again in a
/// moment" apart from "this will never work".
#[derive(Debug)]
struct AttachRefusedError {
    reason: AttachRefusal,
    message: String,
}

impl std::fmt::Display for AttachRefusedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the pty-host refused to hand the pane over: {}",
            self.message
        )
    }
}

impl std::error::Error for AttachRefusedError {}

fn try_attach(params: &AttachParams<'_>) -> Result<PendingAttach> {
    let control = UnixStream::connect(params.control_socket)
        .with_context(|| format!("failed to connect to {}", params.control_socket.display()))?;
    control
        .set_read_timeout(Some(HANDSHAKE_TIMEOUT))
        .context("failed to bound the attach handshake")?;
    control
        .set_write_timeout(Some(HANDSHAKE_TIMEOUT))
        .context("failed to bound the attach handshake")?;

    let mut stream = &control;
    write_msg(
        &mut stream,
        &ServerToHostMsg::Attach {
            protocol_version: PROTOCOL_VERSION,
            yinz_version: VERSION.to_owned(),
            terminal_id: params.terminal_id.get(),
            session_name: params.session_name.clone(),
        },
    )
    .context("failed to send Attach")?;

    let reply: HostToServerMsg = read_msg(&mut stream)
        .context("failed to read the attach result")?
        .ok_or_else(|| anyhow!("pty-host closed the connection during the attach"))?;
    let (host_id, host_pid, child_pid, cmd, status, buffered_base64, dropped_bytes) = match reply {
        HostToServerMsg::AttachAck {
            protocol_version,
            host_id,
            host_pid,
            child_pid,
            cmd,
            status,
            buffered_base64,
            dropped_bytes,
            ..
        } => {
            if protocol_version != PROTOCOL_VERSION {
                return Err(anyhow!(
                    "pty-host speaks protocol v{protocol_version}, we speak v{PROTOCOL_VERSION}"
                ));
            }
            (
                host_id,
                host_pid,
                child_pid,
                cmd,
                status,
                buffered_base64,
                dropped_bytes,
            )
        },
        HostToServerMsg::AttachRefused { reason, message } => {
            return Err(AttachRefusedError { reason, message }.into());
        },
        other => return Err(anyhow!("expected AttachAck, got {other:?}")),
    };

    let master = recv_fd(&control).context("failed to take the pty master")?;
    let buffered = protocol::decode_buffered(&buffered_base64)?;
    if dropped_bytes > 0 {
        log::warn!(
            "pty-host {host_id} dropped {dropped_bytes} bytes while it was detached; \
             this pane is missing part of its output"
        );
    }

    // Back to blocking reads now the handshake is done; the reader thread should
    // sit on this socket indefinitely.
    let _ = control.set_read_timeout(None);
    let _ = control.set_write_timeout(None);
    let reader_stream = control
        .try_clone()
        .context("failed to clone the control channel")?;

    Ok(PendingAttach {
        control: Some(control),
        reader_stream: Some(reader_stream),
        host_id,
        host_pid,
        control_socket: params.control_socket.to_path_buf(),
        master: Some(master),
        child_pid,
        terminal_id: params.terminal_id,
        cmd,
        buffered,
        status,
    })
}

/// Put the control socket on fd 3 for the supervisor to find after exec.
///
/// Runs between fork and exec, so it must stay to raw syscalls.
fn place_control_fd(host_raw: RawFd) -> std::io::Result<()> {
    if host_raw == CONTROL_FD {
        // `dup2(fd, fd)` is a no-op that does *not* clear FD_CLOEXEC, so the
        // supervisor would exec with fd 3 already closed and then block forever
        // on a read. Clear it explicitly instead.
        clear_cloexec(host_raw).map_err(|e| std::io::Error::other(format!("{e:#}")))?;
        return Ok(());
    }
    if unsafe { libc::dup2(host_raw, CONTROL_FD) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn spawn_reader_thread(
    mut stream: UnixStream,
    host_process: HostProcess,
    host_id: SupervisorId,
    host_pid: ProcessId,
    terminal_id: TerminalId,
    cmd: RunCommand,
    quit_cb: QuitCb,
    child_state_cb: Option<ChildStateCb>,
    reported: Arc<AtomicBool>,
    released: Arc<AtomicBool>,
) {
    std::thread::Builder::new()
        .name(format!("pty-host-{host_id}"))
        .spawn(move || {
            loop {
                match read_msg::<HostToServerMsg, _>(&mut stream) {
                    Ok(Some(HostToServerMsg::ChildExited { status })) => {
                        if !reported.swap(true, Ordering::SeqCst) {
                            quit_cb(terminal_id, status, cmd.clone());
                        }
                        // Keep reading: the supervisor lingers until it is told to
                        // go, and its EOF is what tells us the pane is finished.
                    },
                    Ok(Some(HostToServerMsg::ChildStopped { .. })) => {
                        if let Some(cb) = child_state_cb.as_ref() {
                            cb(terminal_id, ChildRunState::Stopped);
                        }
                    },
                    Ok(Some(HostToServerMsg::ChildContinued)) => {
                        if let Some(cb) = child_state_cb.as_ref() {
                            cb(terminal_id, ChildRunState::Continued);
                        }
                    },
                    Ok(Some(other)) => {
                        log::debug!("pty-host {host_id} sent an unexpected {other:?}");
                    },
                    Ok(None) => break,
                    Err(e) => {
                        log::warn!("pty-host {host_id} control channel error: {e:#}");
                        break;
                    },
                }
            }
            if released.load(Ordering::SeqCst) {
                // Not a death: we handed this pane to another server, and the
                // supervisor is meant to outlive us. So no synthesized close and
                // emphatically no `reap_host` — its grace-then-SIGKILL would kill
                // the pane the new owner just adopted. Still wait, so it does not
                // zombie here when that owner eventually terminates it. The cost
                // is one parked thread per warped pane.
                if let HostProcess::Spawned(mut child) = host_process {
                    let _ = child.wait();
                }
                return;
            }
            // The supervisor is gone. If it died without reporting an exit, the
            // pane's child is now unsupervised and nobody will ever `waitpid` it,
            // so the pane would hang open forever. Close it out with an unknown
            // status rather than leaving a dead-looking live pane on screen.
            if !reported.swap(true, Ordering::SeqCst) {
                log::warn!("pty-host {host_id} died without reporting an exit status");
                quit_cb(terminal_id, None, cmd.clone());
            }
            reap_host(host_process, host_pid, &host_id);
        })
        .map(|_| ())
        .unwrap_or_else(|e| log::error!("could not start the pty-host reader thread: {e}"));
}

/// Reap the supervisor so it does not become a zombie. A supervisor we spawned is
/// a direct child of ours, which is exactly what lets us do this — and why no
/// double fork.
///
/// An *adopted* supervisor is not our child, so `waitpid` would only return
/// `ECHILD`. It still has to be held to the same deadline, since this runs on the
/// paths where the pane really is finished; liveness is probed with a null signal
/// instead, and the reaping is left to init once it exits.
fn reap_host(host_process: HostProcess, host_pid: ProcessId, host_id: &SupervisorId) {
    let Some(pid) = super::process_pid(host_pid).to_log().ok() else {
        return;
    };
    let mut host_process = host_process;
    let deadline = Instant::now() + HOST_EXIT_GRACE;
    loop {
        match &mut host_process {
            HostProcess::Spawned(child) => match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => {},
                Err(e) => {
                    log::warn!("could not wait on pty-host {host_id}: {e}");
                    return;
                },
            },
            HostProcess::Foreign => {
                if kill(pid, None).is_err() {
                    // ESRCH: it is gone, and whatever reparented it will reap it.
                    return;
                }
            },
        }
        if Instant::now() >= deadline {
            log::warn!("pty-host {host_id} did not exit in time; killing it");
            let _ = kill(pid, Some(Signal::SIGKILL));
            if let HostProcess::Spawned(child) = &mut host_process {
                let _ = child.wait();
            }
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Whether a socket file is left over from a supervisor that is gone.
///
/// Only a refused (or absent) connect proves that: anything else — a timeout, a
/// full backlog — can happen to a perfectly healthy supervisor, and treating that
/// as death would unlink the socket of a live pane.
pub fn socket_is_dead(socket: &Path) -> bool {
    match yinz_ipc::consts::ipc_connect(socket) {
        Ok(_) => false,
        Err(e) => matches!(
            e.kind(),
            std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
        ),
    }
}

/// Ask a supervisor for its status over its named socket. Used by
/// `yinz pty-host list`.
pub fn query(socket: &Path) -> Result<super::protocol::HostStatus> {
    use interprocess::local_socket::traits::Stream;
    let mut stream = yinz_ipc::consts::ipc_connect(socket)
        .with_context(|| format!("failed to connect to {}", socket.display()))?;
    stream
        .set_recv_timeout(Some(HANDSHAKE_TIMEOUT))
        .context("bound supervisor query read")?;
    stream
        .set_send_timeout(Some(HANDSHAKE_TIMEOUT))
        .context("bound supervisor query write")?;
    write_msg(&mut stream, &ServerToHostMsg::Query).context("failed to send Query")?;
    match read_msg::<HostToServerMsg, _>(&mut stream).context("failed to read Status")? {
        Some(HostToServerMsg::Status(status)) => Ok(status),
        Some(other) => Err(anyhow!("expected Status, got {other:?}")),
        None => Err(anyhow!("pty-host closed the connection without replying")),
    }
}

/// Relabel on a separate connection so the lifecycle reader cannot consume the ack.
/// The caller must preflight `supports_relabel` before sending this message.
pub fn relabel_via_socket(
    socket: &Path,
    expected_session: &SessionName,
    session_name: &SessionName,
    detached_only: bool,
) -> Result<()> {
    let mut stream = UnixStream::connect(socket)
        .with_context(|| format!("failed to connect to {}", socket.display()))?;
    stream
        .set_read_timeout(Some(HANDSHAKE_TIMEOUT))
        .context("relabel read timeout")?;
    stream
        .set_write_timeout(Some(HANDSHAKE_TIMEOUT))
        .context("relabel write timeout")?;
    write_msg(
        &mut stream,
        &ServerToHostMsg::Relabel {
            expected_session: expected_session.clone(),
            session_name: session_name.clone(),
            detached_only,
        },
    )
    .context("failed to send Relabel")?;
    match read_msg::<HostToServerMsg, _>(&mut stream)
        .context("failed to read relabel acknowledgement; ownership may have changed")?
    {
        Some(HostToServerMsg::Relabelled) => Ok(()),
        Some(HostToServerMsg::RelabelRefused { message }) => Err(anyhow!(message)),
        other => Err(anyhow!(
            "expected relabel acknowledgement, got {other:?}; ownership may have changed"
        )),
    }
}

/// Ask a supervisor to shut down over its named socket. Used by
/// `yinz pty-host kill`.
pub fn terminate_via_socket(socket: &Path) -> Result<super::protocol::HostStatus> {
    let mut stream = UnixStream::connect(socket)
        .with_context(|| format!("failed to connect to {}", socket.display()))?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT)).ok();
    stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT)).ok();
    write_msg(&mut stream, &ServerToHostMsg::Terminate).context("failed to send Terminate")?;
    match read_msg::<HostToServerMsg, _>(&mut stream).context("failed to read Status")? {
        Some(HostToServerMsg::Status(status)) => Ok(status),
        Some(other) => Err(anyhow!("expected Status, got {other:?}")),
        None => Err(anyhow!("pty-host closed the connection without replying")),
    }
}

#[cfg(test)]
#[path = "../unit/pty_host_client_tests.rs"]
mod pty_host_client_tests;
