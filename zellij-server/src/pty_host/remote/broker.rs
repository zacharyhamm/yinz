//! The remote end: a small daemon that turns a pty master descriptor into a
//! byte stream, and back.
//!
//! It is deliberately not a new kind of supervisor. It does exactly what a
//! local server does — `pty_host::client::spawn`, then `recv_fd` — so the
//! supervisor on the far side cannot tell it apart from one, and every existing
//! guarantee about pane lifetime keeps holding. All the broker adds is the one
//! thing ssh cannot carry: it owns the descriptor locally and relays it.
//!
//! One connection per pane. ssh opens a channel per connection to a forwarded
//! socket, so this needs no multiplexing of its own; the accept loop is the
//! whole of it.
//!
//! A pane, though, is *not* one connection. A connection ending is routine over
//! a network and says nothing about the process, so the pane lives in a registry
//! (`session.rs`) and connections attach to and detach from it. A reconnecting
//! server names its pane by token and says how much output it actually took, and
//! the replay picks up exactly there.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};
use std::thread;

use zellij_utils::{
    consts::{VERSION, ZELLIJ_SOCK_DIR},
    errors::prelude::*,
};

use super::protocol::{
    self, BrokerToServerMsg, ChildSignal, Frame, ServerToBrokerMsg, MAX_STREAM_PAYLOAD,
    REMOTE_PROTOCOL_VERSION,
};
use super::session::{orphan_timeout, PaneRegistry, PaneSession, ResumeStep};
use crate::pty_host::client::{self as pty_host_client, SpawnParams};
use crate::pty_host::protocol::{
    check_socket_path_length, new_host_id, SpawnFailureKind, WireWinsize,
};

/// A socket shared by the several things that need to write to it: the output
/// pump, the child-exit callback, and the control pump.
pub type Wire = Arc<Mutex<UnixStream>>;

/// How often to look for panes nobody has come back for.
const REAP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// How often to look, given how long a pane is allowed to sit unclaimed.
///
/// Sweeping every 30 seconds is right for a half-hour timeout and useless for a
/// three-second one, where the sweep interval rather than the timeout would
/// decide when a pane died. Checking a few times per timeout keeps the two in
/// proportion whichever it is.
fn reap_interval(orphan_timeout: std::time::Duration) -> std::time::Duration {
    REAP_INTERVAL
        .min(orphan_timeout / 4)
        .max(std::time::Duration::from_millis(100))
}

pub fn send(wire: &Wire, msg: &BrokerToServerMsg) -> Result<()> {
    let mut sock = wire
        .lock()
        .to_anyhow()
        .context("failed to lock a remote pane's wire")?;
    protocol::write_control(&mut *sock, msg)
}

fn send_output(wire: &Wire, bytes: &[u8]) -> Result<()> {
    let mut sock = wire
        .lock()
        .to_anyhow()
        .context("failed to lock a remote pane's wire")?;
    protocol::write_output(&mut *sock, bytes)
}

/// A `File` view of a descriptor owned by someone else. Never closes it.
fn borrow_fd(fd: std::os::unix::io::RawFd) -> std::mem::ManuallyDrop<std::fs::File> {
    use std::os::unix::io::FromRawFd;
    std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd) })
}

fn read_control(reader: &mut UnixStream) -> Result<Option<ServerToBrokerMsg>> {
    match protocol::read_frame(reader)? {
        None => Ok(None),
        Some(frame @ Frame::Control(_)) => frame.decode_control().map(Some),
        Some(other) => Err(anyhow!("expected a control frame, got {other:?}")),
    }
}

/// Why the control pump stopped, which decides the pane's fate.
enum Outcome {
    /// The server asked for the pane to end.
    Terminated,
    /// The connection went away without saying why.
    Disconnected,
}

/// Serve one connection: either a new pane, or a reconnect to an existing one.
pub fn serve_connection(
    socket: UnixStream,
    host_bin: &std::path::Path,
    registry: &Arc<PaneRegistry>,
) -> Result<()> {
    socket
        .set_nonblocking(false)
        .context("failed to put a broker connection into blocking mode")?;
    let mut reader = socket
        .try_clone()
        .context("failed to clone a broker connection")?;
    let wire: Wire = Arc::new(Mutex::new(socket));

    handshake(&mut reader, &wire)?;

    let request = read_control(&mut reader)?
        .ok_or_else(|| anyhow!("peer closed before asking for a pane"))?;
    let (session, generation) = match request {
        ServerToBrokerMsg::Spawn {
            cmd,
            failover_cmd,
            initial_size,
        } => start_pane(host_bin, registry, &wire, cmd, failover_cmd, initial_size)?,
        ServerToBrokerMsg::Resume {
            pane_token,
            received,
        } => resume_pane(registry, &wire, &pane_token, received)?,
        other => return Err(anyhow!("expected a spawn or a resume, got {other:?}")),
    };

    let outcome = pump_control(&mut reader, session.master_fd(), session.child_pid);
    match &outcome {
        // An explicit terminate is the *only* thing that ends a pane — dropping
        // the supervisor handle deliberately does not. Without this every closed
        // remote pane would leave its supervisor and its child running here.
        Ok(Outcome::Terminated) => {
            session.terminate();
            registry.remove(&session.token);
        },
        // A connection that merely ended is not a teardown. The pane keeps
        // running and keeps buffering, so a reconnect can continue the stream;
        // `reap_orphans` reclaims one that is never picked back up.
        //
        // Only if this connection is still the one holding the pane. A slow
        // notice of a disconnect that a reconnect has already replaced would
        // otherwise silence the pane and start an orphan clock on it.
        Ok(Outcome::Disconnected) | Err(_) => session.detach_if(generation),
    }
    outcome.map(|_| ())
}

fn handshake(reader: &mut UnixStream, wire: &Wire) -> Result<()> {
    let hello = read_control(reader)?.ok_or_else(|| anyhow!("peer closed before saying hello"))?;
    match hello {
        ServerToBrokerMsg::Hello {
            protocol_version, ..
        } if protocol_version == REMOTE_PROTOCOL_VERSION => {},
        ServerToBrokerMsg::Hello {
            protocol_version,
            zellij_version,
        } => {
            // Name both sides. A remote box on a different build is the normal
            // case, and "connection refused" would send the user hunting the
            // network rather than the version.
            return Err(anyhow!(
                "remote pane protocol mismatch: this broker speaks v{REMOTE_PROTOCOL_VERSION} \
                 (zellij {VERSION}), the server speaks v{protocol_version} (zellij \
                 {zellij_version}). Install matching zellij versions on both machines."
            ));
        },
        other => return Err(anyhow!("expected a hello, got {other:?}")),
    }
    send(
        wire,
        &BrokerToServerMsg::HelloAck {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            zellij_version: VERSION.to_owned(),
        },
    )
}

/// What the quit callback talks to, before and after the session exists.
///
/// One lock rather than two slots. With the exit and the session behind separate
/// locks the callback can find no session, the registration can find no exit,
/// and each concludes the other will deal with it.
enum PaneSlot {
    /// No session yet. `Some(status)` if the child got here first.
    Pending(Option<Option<i32>>),
    Live(Arc<PaneSession>),
}

/// Tell the server the child exited, if that can be said yet, and only once.
///
/// Called from both things that might learn it last: the quit callback, and the
/// output pump reaching the end of the pty. Whichever arrives second sends.
fn report_exit_if_complete(session: &PaneSession) {
    let Some(status) = session.exit_is_reportable() else {
        return;
    };
    // Nobody is listening. Deliberately without claiming: the resume is what
    // will tell the next server, and it needs the right to send still going.
    let Some(wire) = session.wire() else {
        return;
    };
    if !session.claim_exit_report() {
        return;
    }
    send(&wire, &BrokerToServerMsg::ChildExited { status })
        .context("failed to report a remote child's exit")
        .non_fatal();
}

/// Where the far machine's idea of "the user's shell" comes from.
///
/// Split out from the resolution below so that the order of preference can be
/// tested without arranging for a second user on the machine running the tests.
#[derive(Debug, Default)]
pub(crate) struct ShellSource {
    pub passwd_shell: Option<std::path::PathBuf>,
    pub passwd_home: Option<std::path::PathBuf>,
    pub env_shell: Option<std::path::PathBuf>,
    pub env_home: Option<std::path::PathBuf>,
}

impl ShellSource {
    fn of_this_machine() -> Self {
        let user = nix::unistd::User::from_uid(nix::unistd::Uid::effective())
            .ok()
            .flatten();
        ShellSource {
            passwd_shell: user.as_ref().map(|u| u.shell.clone()),
            passwd_home: user.as_ref().map(|u| u.dir.clone()),
            env_shell: std::env::var_os("SHELL").map(std::path::PathBuf::from),
            env_home: std::env::var_os("HOME").map(std::path::PathBuf::from),
        }
    }
}

/// Turn "the default shell of whoever I am" into a command, here, and say what
/// `argv[0]` it wants.
///
/// The passwd entry leads because it is the only authority that does not depend
/// on how this broker was started: `$SHELL` in its environment is whatever the
/// first bootstrap ssh session happened to inherit and then froze, and the
/// asking machine's `$SHELL` is a path on the wrong filesystem — it never
/// crossed the wire at all. This is the order sshd effectively lands on, for
/// the same reasons.
pub(crate) fn resolve_default_shell(
    cmd: &mut zellij_utils::input::command::RunCommand,
    source: &ShellSource,
) -> Option<String> {
    if !cmd.is_remote_default_shell() {
        return None;
    }
    let shell = source
        .passwd_shell
        .clone()
        .into_iter()
        .chain(source.env_shell.clone())
        .find(|s| !s.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::PathBuf::from("/bin/sh"));
    if cmd.cwd.is_none() {
        // No cwd asked for means "wherever ssh would have put me".
        cmd.cwd = source
            .passwd_home
            .clone()
            .or_else(|| source.env_home.clone())
            .filter(|d| !d.as_os_str().is_empty());
    }
    // A login shell is a shell whose argv[0] begins with '-', and that single
    // character is the whole mechanism: it is what makes bash read
    // ~/.bash_profile and ash read ~/.profile. It matters more here than it
    // would locally, because nothing about the asking machine's environment is
    // transmitted with a remote pane — sourcing the profile on this side is the
    // only thing that gives the pane a PATH, a prompt and a locale.
    let arg0 = format!(
        "-{}",
        shell
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "sh".to_owned())
    );
    cmd.command = shell;
    cmd.args.clear();
    Some(arg0)
}

fn start_pane(
    host_bin: &std::path::Path,
    registry: &Arc<PaneRegistry>,
    wire: &Wire,
    cmd: zellij_utils::input::command::RunCommand,
    failover_cmd: Option<zellij_utils::input::command::RunCommand>,
    initial_size: Option<WireWinsize>,
) -> Result<(Arc<PaneSession>, u64)> {
    // Before anything looks at the command: a default-shell pane arrives with
    // none, because only this machine can say what it should be.
    let mut cmd = cmd;
    let arg0 = resolve_default_shell(&mut cmd, &ShellSource::of_this_machine());
    let pending = match pty_host_client::spawn(SpawnParams {
        host_bin,
        terminal_id: 0,
        session_name: None,
        cmd: cmd.clone(),
        failover_cmd,
        // Never sent across a machine boundary — the flag words and control
        // character indices are platform constants. See the note on `Spawn`.
        termios: None,
        initial_size,
        arg0,
    }) {
        Ok(pending) => pending,
        Err(e) => {
            // `CommandNotFound` has to survive the trip: eight sites in pty.rs
            // downcast for it, and the local pre-flight `command_exists` check
            // that normally produces it is meaningless for a command that lives
            // on *this* machine. Nothing is lost — the supervisor re-checks
            // before forking and reports `SpawnFailureKind::CommandNotFound`,
            // which `client::spawn` turns back into this error. The check simply
            // happens on the machine that has the command.
            let kind = match e.downcast_ref::<ZellijError>() {
                Some(ZellijError::CommandNotFound { .. }) => SpawnFailureKind::CommandNotFound,
                _ => SpawnFailureKind::Other,
            };
            send(
                wire,
                &BrokerToServerMsg::SpawnFailed {
                    kind,
                    message: format!("{e:#}"),
                },
            )?;
            return Err(e);
        },
    };

    let token = new_host_id();
    // `finish` starts the thread that watches for the child, so the callback is
    // live from here on — before there is a session to record against. A child
    // that exits in that window leaves its status in `Pending`, and the
    // registration below picks it up.
    let session_slot = Arc::new(Mutex::new(PaneSlot::Pending(None)));
    let cb_slot = session_slot.clone();
    let spawned = pending.finish(Box::new(move |_pane_id, status, _cmd| {
        let session = match cb_slot.lock() {
            Ok(mut slot) => match &mut *slot {
                PaneSlot::Live(session) => session.clone(),
                PaneSlot::Pending(early) => {
                    *early = Some(status);
                    return;
                },
            },
            Err(_) => return,
        };
        session.set_child_exit(status);
        // Not sent from here. The child being gone is only half of it; the pty
        // may still have its last words in flight.
        report_exit_if_complete(&session);
    }));

    let session = Arc::new(PaneSession::new(
        token.clone(),
        spawned.handle,
        spawned.master,
        spawned.child_pid,
    ));
    registry.insert(session.clone());

    // Before the wire is attached, so nothing can reach the server ahead of the
    // message that tells it the pane exists.
    if let Err(e) = send(
        wire,
        &BrokerToServerMsg::Spawned {
            pane_token: token.clone(),
        },
    ) {
        // Otherwise the pane sits in the registry attached to nobody, and with
        // no orphan clock running it is never reclaimed either.
        registry.remove(&token);
        session.terminate();
        return Err(e);
    }

    // Attaching under the slot lock, rather than after releasing it, closes the
    // window where the callback sees a `Live` session with no wire to use.
    let (early_exit, generation) = match session_slot.lock() {
        Ok(mut slot) => {
            let early = match &*slot {
                PaneSlot::Pending(early) => *early,
                PaneSlot::Live(_) => None,
            };
            let generation = session.attach(wire.clone());
            *slot = PaneSlot::Live(session.clone());
            (early, generation)
        },
        Err(_) => (None, session.attach(wire.clone())),
    };
    if let Some(status) = early_exit {
        session.set_child_exit(status);
    }

    if let Err(e) =
        start_output_pump(session.clone()).and_then(|()| start_status_pump(session.clone()))
    {
        // A pane nothing is reading is not a pane. Ending it here is better than
        // leaving a live child on this machine that the server can never see.
        registry.remove(&token);
        session.terminate();
        return Err(e);
    }
    // The pump may already have drained a pty whose child was gone before any of
    // this ran, and the callback that would have said so found no session.
    report_exit_if_complete(&session);
    Ok((session, generation))
}

fn resume_pane(
    registry: &Arc<PaneRegistry>,
    wire: &Wire,
    token: &str,
    received: u64,
) -> Result<(Arc<PaneSession>, u64)> {
    let session = match registry.get(token) {
        Some(session) if !session.is_terminated() => session,
        _ => {
            // Refused rather than silently starting a fresh pane: the server
            // believes it still has this one on screen, and giving it a
            // different process under the same pane would be worse than saying
            // no.
            send(
                wire,
                &BrokerToServerMsg::ResumeRefused {
                    reason: format!(
                        "this broker no longer holds pane {token}; it was reclaimed or never \
                         existed here"
                    ),
                },
            )?;
            return Err(anyhow!("refused a resume for unknown pane {token}"));
        },
    };
    if session.is_attached() {
        send(
            wire,
            &BrokerToServerMsg::ResumeRefused {
                reason: format!("pane {token} already has a server attached"),
            },
        )?;
        return Err(anyhow!("refused a duplicate resume for pane {token}"));
    }

    // Claimed before anything is sent, so a second resume is refused from here
    // on — but not yet writable by the pumps, which would let new output
    // overtake the catch-up below.
    let generation = session.begin_attach();
    // From here the pane is spoken for, so nothing may return without either
    // finishing the attach or giving the claim back. A pane abandoned mid-resume
    // would refuse every later reconnect and, having had its orphan clock
    // cleared, never be reclaimed either.
    match catch_up_and_attach(&session, wire, token, received, generation) {
        Ok(()) => Ok((session, generation)),
        Err(e) => {
            session.detach_if(generation);
            Err(e)
        },
    }
}

/// Bring a resuming connection up to date and hand it the pane.
fn catch_up_and_attach(
    session: &Arc<PaneSession>,
    wire: &Wire,
    token: &str,
    received: u64,
    generation: u64,
) -> Result<()> {
    let (replay, dropped) = session.replay_from(received);
    send(wire, &BrokerToServerMsg::Resumed { dropped })?;
    let mut sent_upto = received + dropped;
    if !replay.is_empty() {
        send_output(wire, &replay)?;
        sent_upto += replay.len() as u64;
    }
    // Whatever the pane produced while that was in flight, and so on until it
    // has nothing left to say. Each round is only what the previous round's
    // sending took, so this converges quickly; a pane that outruns it forever is
    // a backpressure problem, not a replay one.
    loop {
        match session.resume_step(sent_upto, wire, generation) {
            ResumeStep::Replay { bytes, dropped } => {
                sent_upto += dropped + bytes.len() as u64;
                send_output(wire, &bytes)?;
            },
            ResumeStep::Attached => break,
            ResumeStep::Superseded => {
                return Err(anyhow!(
                    "another connection claimed pane {token} while this one was catching up"
                ));
            },
        }
    }

    // A child that exited while nobody was attached still has to be reported, or
    // the pane would sit there looking alive. After the replay, so the server
    // has the output before it is told there will be no more, and only once the
    // pty is drained, so there really is no more.
    if let Some(status) = session.exit_is_reportable() {
        session.claim_exit_report();
        send(wire, &BrokerToServerMsg::ChildExited { status })?;
    }
    // A reconnecting server has no cached status at all — it is a fresh
    // `RemoteConn`. The ticker only reports changes, so without this a pane
    // whose cwd and command are steady would look unknown until something about
    // it happened to change.
    let probe = super::status::probe(session.master_fd(), session.child_pid);
    send(wire, &probe.into_report())?;
    // The project a pane belongs to is discovered once, on a cwd change that may
    // have happened long before this reconnect. Replaying it means the pane is
    // attributed straight away rather than at whatever point it next moves.
    if let Some(git) = session.git_probe() {
        send(wire, &git_report(&git))?;
    }
    Ok(())
}

fn git_report(git: &super::status::GitProbe) -> BrokerToServerMsg {
    use crate::pty_host::protocol::encode_buffered;
    BrokerToServerMsg::GitProbe {
        cwd: git.cwd.clone(),
        exit_code: git.exit_code,
        metadata_base64: encode_buffered(&git.metadata),
        config_base64: encode_buffered(&git.config),
    }
}

/// How often to look at the pane's process.
///
/// Matches the server's own `UPDATE_AND_REPORT_CWDS_INTERVAL_MS`, so a remote
/// pane's information is no staler than a local one's.
const STATUS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Watch the pane's process and report what changes.
///
/// Pushed rather than polled: the alternative is the server asking, from
/// `update_and_report_cwds` on the pty thread, over a link that may be dead.
/// Pushing means a disconnected pane reports stale-but-plausible data instead of
/// stalling the thread that drives every other pane.
fn start_status_pump(session: Arc<PaneSession>) -> Result<()> {
    thread::Builder::new()
        .name("remote-pane-status".to_owned())
        .spawn(move || {
            let mut last: Option<super::status::Probe> = None;
            let mut last_git_cwd: Option<String> = None;
            loop {
                thread::sleep(STATUS_INTERVAL);
                if session.is_terminated() || session.child_exit().is_some() {
                    return;
                }
                let probe = super::status::probe(session.master_fd(), session.child_pid);

                // A new working directory may mean a different checkout, which
                // is the only thing that can change a pane's project.
                if let Some(cwd) = probe.cwd.clone() {
                    if last_git_cwd.as_deref() != Some(cwd.as_str()) {
                        let git = super::status::probe_git(&cwd);
                        session.set_git_probe(git.clone());
                        if let Some(wire) = session.wire() {
                            let _ = send(&wire, &git_report(&git));
                        }
                        last_git_cwd = Some(cwd);
                    }
                }

                // Only on change: this is a keystroke-free pane's only traffic,
                // and an idle agent should not keep an ssh channel busy.
                if last.as_ref() == Some(&probe) {
                    continue;
                }
                if let Some((wire, generation)) = session.wire_and_generation() {
                    if send(&wire, &probe.clone().into_report()).is_err() {
                        session.detach_if(generation);
                        continue;
                    }
                }
                last = Some(probe);
            }
        })
        .context("failed to start a remote pane's status pump")?;
    Ok(())
}

/// Read the pty and fan it out: always into the replay buffer, and onto the
/// wire when a server is attached.
fn start_output_pump(session: Arc<PaneSession>) -> Result<()> {
    thread::Builder::new()
        .name("remote-pane-output".to_owned())
        .spawn(move || {
            let mut master = borrow_fd(session.master_fd());
            let mut buf = [0u8; MAX_STREAM_PAYLOAD];
            loop {
                match master.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = &buf[..n];
                        // Buffer first, always. If the send fails or there is
                        // nobody to send to, these are exactly the bytes a
                        // reconnect will need.
                        session.append_output(chunk);
                        if let Some((wire, generation)) = session.wire_and_generation() {
                            if send_output(&wire, chunk).is_err() {
                                // The server went away mid-write. Not a
                                // teardown: keep the pane and keep buffering.
                                session.detach_if(generation);
                            }
                        }
                    },
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    // `EIO` rather than EOF is what a master gives once the last
                    // slave is closed, so this is the ordinary end, not a fault.
                    Err(_) => break,
                }
            }
            // The pty is finished, so anything the child wrote has been read and
            // buffered. Only now can its exit be reported without cutting the
            // last of its output off.
            session.mark_output_drained();
            report_exit_if_complete(&session);
        })
        .context("failed to start a remote pane's output pump")?;
    Ok(())
}

fn pump_control(
    reader: &mut UnixStream,
    master_fd: std::os::unix::io::RawFd,
    child_pid: u32,
) -> Result<Outcome> {
    let mut master_write = borrow_fd(master_fd);
    loop {
        let frame = match protocol::read_frame(reader)? {
            Some(frame) => frame,
            None => return Ok(Outcome::Disconnected),
        };
        match frame {
            Frame::Input(bytes) => {
                // Blocking on purpose: if the pty is full, not reading the
                // socket is precisely the backpressure the far side needs. ssh's
                // window closes, and its outbound queue starts short-counting.
                master_write
                    .write_all(&bytes)
                    .context("failed to write input to a remote pane")?;
            },
            Frame::Output(_) => {
                return Err(anyhow!("a server sent an output frame to a broker"));
            },
            Frame::Control(_) => {
                let msg: ServerToBrokerMsg = frame.decode_control()?;
                match msg {
                    ServerToBrokerMsg::Resize(WireWinsize {
                        cols,
                        rows,
                        width_in_pixels,
                        height_in_pixels,
                    }) => {
                        set_winsize(master_fd, cols, rows, width_in_pixels, height_in_pixels);
                    },
                    ServerToBrokerMsg::SignalChild(signal) => {
                        signal_child(child_pid, signal);
                    },
                    ServerToBrokerMsg::Terminate => return Ok(Outcome::Terminated),
                    // A second hello, spawn or resume on a live pane means the
                    // peer is confused, but the pane is fine; say so and carry on.
                    other => {
                        log::warn!("unexpected message on a live remote pane: {other:?}");
                    },
                }
            },
        }
    }
}

fn set_winsize(fd: std::os::unix::io::RawFd, cols: u16, rows: u16, xpixel: u16, ypixel: u16) {
    let winsize = nix::pty::Winsize {
        ws_col: cols,
        ws_row: rows,
        ws_xpixel: xpixel,
        ws_ypixel: ypixel,
    };
    #[allow(clippy::useless_conversion)]
    unsafe {
        libc::ioctl(fd, libc::TIOCSWINSZ.into(), &winsize)
    };
}

/// Resolve a symbolic signal against the child this broker owns.
///
/// The pid never crosses the wire in either direction, which is the point: a pid
/// from the server's machine would name an unrelated process here, and this
/// child's pid would name an unrelated process there.
fn signal_child(child_pid: u32, signal: ChildSignal) {
    let pid = nix::unistd::Pid::from_raw(child_pid as i32);
    let sig = match signal {
        ChildSignal::Hup => nix::sys::signal::Signal::SIGHUP,
        ChildSignal::Int => nix::sys::signal::Signal::SIGINT,
        ChildSignal::Kill => nix::sys::signal::Signal::SIGKILL,
    };
    let _ = nix::sys::signal::kill(pid, Some(sig));
}

// --- daemon entry points ---

/// The argv flag that turns this binary into a broker, mirroring
/// `PTY_HOST_FLAG`. Recognised only as argv\[1\], so `main` can branch before
/// clap and the logger.
pub const PTY_BROKER_FLAG: &str = "--pty-broker";

pub fn broker_invocation() -> Option<String> {
    broker_invocation_from_args(std::env::args())
}

fn broker_invocation_from_args<I: IntoIterator<Item = String>>(args: I) -> Option<String> {
    let mut args = args.into_iter();
    let _bin = args.next();
    if args.next().as_deref() != Some(PTY_BROKER_FLAG) {
        return None;
    }
    Some(args.next().unwrap_or_default())
}

/// Where this machine's broker listens.
///
/// Computed here and never on the connecting side: `ZELLIJ_SOCK_DIR` resolves
/// differently per platform, and the connecting machine cannot know which
/// applies. `ensure` prints this back over ssh for exactly that reason.
pub fn broker_socket_path() -> std::path::PathBuf {
    ZELLIJ_SOCK_DIR.join("pty-broker.sock")
}

/// `zellij --pty-broker ensure` — make sure a broker is up and say where.
///
/// Prints one JSON line and exits. Output is the contract with `ssh::bootstrap`,
/// so it goes to stdout and nothing else may.
pub fn ensure() -> Result<()> {
    let socket = broker_socket_path();
    check_socket_path_length(&socket)?;

    if UnixStream::connect(&socket).is_err() {
        // Either nothing is listening or the file is a leftover from a crash.
        let _ = std::fs::remove_file(&socket);
        spawn_detached_broker(&socket)?;
        await_socket(&socket)?;
    }

    println!(
        "{}",
        serde_json::json!({
            "socket": socket.to_string_lossy(),
            "protocol_version": REMOTE_PROTOCOL_VERSION,
            "zellij_version": VERSION,
        })
    );
    Ok(())
}

fn spawn_detached_broker(socket: &std::path::Path) -> Result<()> {
    let exe = std::env::current_exe().context("could not resolve our own binary")?;
    let mut command = std::process::Command::new(exe);
    command
        .arg(PTY_BROKER_FLAG)
        .arg("serve")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    unsafe {
        use std::os::unix::process::CommandExt;
        // Its own session, so it outlives the ssh that started it. Without this
        // the broker — and every pane on this machine — would die with the
        // bootstrap connection.
        command.pre_exec(|| {
            let _ = nix::unistd::setsid();
            Ok(())
        });
    }
    command
        .spawn()
        .with_context(|| format!("failed to start a broker listening on {}", socket.display()))?;
    Ok(())
}

fn await_socket(socket: &std::path::Path) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if UnixStream::connect(socket).is_ok() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    Err(anyhow!(
        "a broker was started but never began listening on {}",
        socket.display()
    ))
}

/// `zellij --pty-broker serve` — the daemon itself.
pub fn serve_forever() -> Result<()> {
    let socket = broker_socket_path();
    check_socket_path_length(&socket)?;
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)
        .with_context(|| format!("failed to bind {}", socket.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Owner only: this socket starts processes as us.
        let _ = std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600));
    }
    let host_bin = std::env::current_exe().context("could not resolve our own binary")?;
    serve(listener, host_bin)
}

/// Accept panes until the listener dies.
pub fn serve(listener: UnixListener, host_bin: std::path::PathBuf) -> Result<()> {
    let registry = Arc::new(PaneRegistry::new());

    let reaper_registry = registry.clone();
    thread::Builder::new()
        .name("remote-pane-reaper".to_owned())
        .spawn(move || {
            // Read once: the timeout cannot change under a running broker, and
            // the sweep interval is derived from it.
            let timeout = orphan_timeout();
            let interval = reap_interval(timeout);
            loop {
                thread::sleep(interval);
                reaper_registry.reap_orphans(timeout);
            }
        })
        .context("failed to start the remote pane reaper")?;

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(e) => {
                log::warn!("broker accept failed: {e}");
                continue;
            },
        };
        let host_bin = host_bin.clone();
        let registry = registry.clone();
        thread::Builder::new()
            .name("remote-pane".to_owned())
            .spawn(move || {
                if let Err(e) = serve_connection(stream, &host_bin, &registry) {
                    log::warn!("remote pane connection ended: {e:#}");
                }
            })
            .context("failed to start a remote pane thread")?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../unit/pty_host_remote_broker_tests.rs"]
mod pty_host_remote_broker_tests;
