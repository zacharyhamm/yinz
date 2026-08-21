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

use super::env as remote_env;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};
use std::thread;

use zellij_utils::{
    consts::{VERSION, ZELLIJ_SOCK_DIR},
    errors::prelude::*,
};

use super::protocol::{
    self, BrokerToServerMsg, ChildSignal, Frame, ResumeRefusal, ServerToBrokerMsg,
    MAX_STREAM_PAYLOAD, REMOTE_PROTOCOL_VERSION,
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
    /// The server handed the pane over: it is detached, still running, and
    /// waiting for whoever was given its token to `Resume` it.
    ///
    /// Distinct from `Disconnected` in exactly one way that matters — the
    /// detach has already happened by the time this is returned, rather than
    /// being this function's caller's job. See `release_pane`.
    Released,
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
            env,
            terminal_id,
        } => start_pane(
            host_bin,
            registry,
            &wire,
            cmd,
            failover_cmd,
            initial_size,
            env,
            terminal_id,
        )?,
        ServerToBrokerMsg::Resume {
            pane_token,
            received,
        } => resume_pane(registry, &wire, &pane_token, received)?,
        // Answered without taking a pane, and then done. The panes worth asking
        // about are the ones no connection is speaking for, so requiring a pane
        // first would exclude exactly the case this serves.
        ServerToBrokerMsg::ListPanes => {
            send(
                &wire,
                &BrokerToServerMsg::PaneList {
                    entries: registry.entries(),
                },
            )?;
            return Ok(());
        },
        other => return Err(anyhow!("expected a spawn or a resume, got {other:?}")),
    };

    let outcome = pump_control(&mut reader, &session, generation, &wire, registry);
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
        // Already detached, by `release_pane`, before the server was told. Doing
        // it again here would be a no-op in the ordinary case and actively wrong
        // in the one that matters: the resume this handoff exists to allow may
        // already have landed, and `detach_if` only spares it because the
        // generation has moved on. Not relying on that is cheaper than
        // explaining it.
        Ok(Outcome::Released) => {},
        Ok(Outcome::Disconnected) | Err(_) => session.detach_if(generation),
    }
    outcome.map(|_| ())
}

fn handshake(reader: &mut UnixStream, wire: &Wire) -> Result<()> {
    let hello = read_control(reader)?.ok_or_else(|| anyhow!("peer closed before saying hello"))?;
    let ServerToBrokerMsg::Hello {
        protocol_version,
        zellij_version,
    } = hello
    else {
        return Err(anyhow!("expected a hello, got {hello:?}"));
    };

    // Acknowledged before the version is judged, and deliberately so: this ack is
    // the only way the far side can learn what this broker speaks. Closing on a
    // mismatch instead leaves it reading EOF, which it can only report as "the
    // broker closed the connection" — the version gap, the one thing worth
    // saying, is exactly what goes missing.
    send(
        wire,
        &BrokerToServerMsg::HelloAck {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            zellij_version: VERSION.to_owned(),
        },
    )?;

    if protocol_version != REMOTE_PROTOCOL_VERSION {
        // Name both sides. A remote box on a different build is the normal
        // case, and "connection refused" would send the user hunting the
        // network rather than the version.
        return Err(anyhow!(
            "remote pane protocol mismatch: this broker speaks v{REMOTE_PROTOCOL_VERSION} \
             (zellij {VERSION}), the server speaks v{protocol_version} (zellij \
             {zellij_version}). Install matching zellij versions on both machines."
        ));
    }
    Ok(())
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
    /// Built from a passwd lookup the caller already did: `start_pane` needs one
    /// for the login environment as well, and there is no reason to ask twice.
    fn from_user(user: &Option<nix::unistd::User>) -> Self {
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
    // would locally: `remote::env` composes the environment ssh would have
    // handed a login, and the profile is what the far machine gets to say on top
    // of it — a prompt, and whatever PATH it would rather have.
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

#[allow(clippy::too_many_arguments)]
fn start_pane(
    host_bin: &std::path::Path,
    registry: &Arc<PaneRegistry>,
    wire: &Wire,
    cmd: zellij_utils::input::command::RunCommand,
    failover_cmd: Option<zellij_utils::input::command::RunCommand>,
    initial_size: Option<WireWinsize>,
    carried_env: Vec<(String, String)>,
    terminal_id: u32,
) -> Result<(Arc<PaneSession>, u64)> {
    // One passwd lookup answers two different questions: which shell this user
    // has, and what a login on this machine is handed for an environment. They
    // have different fallback rules - see `remote::env::LoginSource` - but there
    // is no reason to ask the system twice.
    let user = nix::unistd::User::from_uid(nix::unistd::Uid::effective())
        .ok()
        .flatten();

    // Before anything looks at the command: a default-shell pane arrives with
    // none, because only this machine can say what it should be.
    let mut cmd = cmd;
    let arg0 = resolve_default_shell(&mut cmd, &ShellSource::from_user(&user));

    // Recomputed here, per pane, from *this* machine's passwd entry. Nothing is
    // inherited from the broker: its environment is a frozen copy of whichever
    // ssh session happened to start it, possibly another login, hours ago, and
    // shared by every pane and every session that reaches this machine.
    let env = remote_env::compose(&remote_env::LoginSource::from_user(&user), carried_env);

    let pending = match pty_host_client::spawn(SpawnParams {
        host_bin,
        terminal_id,
        session_name: None,
        cmd: cmd.clone(),
        failover_cmd,
        // Never sent across a machine boundary — the flag words and control
        // character indices are platform constants. See the note on `Spawn`.
        termios: None,
        initial_size,
        arg0,
        env,
        // Every remote pane, not just a default-shell one. An explicit command
        // gets no login shell and so no profile, which makes it the case that
        // needs a composed environment most - `ssh host cmd` is handed one too.
        login_session: true,
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

    // Built from the resolved command rather than `cmd.to_string()`: a
    // default-shell pane still carries the flag that makes `Display` answer
    // "login shell on <host>", and the whole point of listing panes on a host is
    // to say which shell that turned out to be.
    let command = std::iter::once(cmd.command.to_string_lossy().into_owned())
        .chain(cmd.args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    let session = Arc::new(PaneSession::new(
        token.clone(),
        command,
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
                    kind: ResumeRefusal::Unknown,
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
                kind: ResumeRefusal::AlreadyAttached,
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

/// Let a pane go without ending it, and say so once it is really gone.
///
/// The ordering is the entire point, and it is the opposite of the disconnect
/// path's. There, the detach happens in `serve_connection` *after* this
/// connection's read has returned, which is the earliest the broker can know the
/// connection is over. A server handing a pane to another session cannot wait on
/// "the earliest the broker notices": it has to be able to give the token away
/// and know the next `Resume` will not be refused as a double attach.
///
/// So the detach happens here, before the ack, and `Outcome::Released` tells
/// `serve_connection` not to do it again — `detach_if` is generation-guarded, so
/// a second call would be harmless, but a resume that has already landed in
/// between would have bumped the generation and the guard is the only thing
/// stopping us from silencing it.
///
/// The pane itself is untouched: still in the registry, still running, still
/// buffering into its ring. The orphan clock starts, which is correct — a
/// handoff nobody completes is exactly an abandoned pane.
fn release_pane(session: &Arc<PaneSession>, generation: u64, wire: &Wire) -> Result<()> {
    session.detach_if(generation);
    // Sent on the wire this connection still owns, even though the pane no
    // longer points at it: `send` writes to the socket, and detaching only means
    // the pumps will not.
    send(wire, &BrokerToServerMsg::Released).context("failed to acknowledge a released remote pane")
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

/// How often to ask Git about the pane's folder again, on top of asking whenever
/// the folder changes.
///
/// A checkout's *identity* cannot change under it, but its branch can, and
/// nothing about the pane says when: `git checkout` in place moves the branch
/// without moving the folder, so a cwd-triggered probe alone would report a
/// branch that is wrong for as long as the pane sits there. Matches
/// `hocus-agentd`'s own `GIT_REFRESH_SECONDS`, which is the cadence the local
/// half of the same job already runs at.
///
/// Not shorter: each pane has its own pump, so this is two `git` processes per
/// remote pane per interval, duplicated between panes that share a folder.
const GIT_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// Overrides `GIT_REFRESH_INTERVAL`, so a test can watch a branch change be
/// picked up without waiting a minute for it. Same shape, and the same reason,
/// as `ORPHAN_TIMEOUT_ENV`.
pub const GIT_REFRESH_ENV: &str = "ZELLIJ_REMOTE_GIT_REFRESH_SECS";

fn git_refresh_interval() -> std::time::Duration {
    match std::env::var(GIT_REFRESH_ENV) {
        Ok(secs) => match secs.parse::<u64>() {
            Ok(secs) if secs > 0 => std::time::Duration::from_secs(secs),
            _ => {
                log::warn!(
                    "ignoring {GIT_REFRESH_ENV}={secs:?}: expected a positive number of seconds"
                );
                GIT_REFRESH_INTERVAL
            },
        },
        Err(_) => GIT_REFRESH_INTERVAL,
    }
}

/// When to run the Git probe again, and whether the answer is worth sending.
///
/// Pulled out of the pump's loop so it can be tested at all: inside a
/// `loop { sleep }` the refresh interval is only reachable by waiting for it.
pub struct GitProbePump {
    last: Option<super::status::GitProbe>,
    since_refresh: std::time::Duration,
    interval: std::time::Duration,
}

impl GitProbePump {
    pub fn new(interval: std::time::Duration) -> Self {
        GitProbePump {
            last: None,
            since_refresh: std::time::Duration::ZERO,
            interval,
        }
    }

    /// Advance by `elapsed`, and return the probe if this tick ran one.
    ///
    /// Two triggers. A folder change probes at once — it is the only thing that
    /// can change which *checkout* a pane is in — and the interval probes
    /// whatever else may have moved under an unchanging folder.
    ///
    /// `changed` is reported separately from having run, because the two have
    /// different consumers: every probe is worth *caching*, since that cache is
    /// what a reconnecting server is handed, but only a changed one is worth
    /// *sending*. An idle pane in an unchanging checkout therefore stays silent,
    /// which is the property this pane's only traffic is built around.
    ///
    /// The whole `GitProbe` is compared rather than any parsed part of it: a
    /// branch change shows up in `metadata` and a fetch that rewrites a remote
    /// url shows up in `config`, and comparing the raw struct catches both
    /// without knowing which is which.
    pub fn tick(
        &mut self,
        cwd: &str,
        elapsed: std::time::Duration,
        probe: impl Fn(&str) -> super::status::GitProbe,
    ) -> Option<Probed> {
        self.since_refresh += elapsed;
        let moved = self.last.as_ref().map(|last| last.cwd.as_str()) != Some(cwd);
        let due = self.since_refresh >= self.interval;
        if !moved && !due {
            return None;
        }
        self.since_refresh = std::time::Duration::ZERO;
        let git = probe(cwd);
        let changed = self.last.as_ref() != Some(&git);
        self.last = Some(git.clone());
        Some(Probed {
            probe: git,
            changed,
        })
    }
}

/// One run of the Git probe: what it said, and whether that is news.
pub struct Probed {
    pub probe: super::status::GitProbe,
    pub changed: bool,
}

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
            let mut git = GitProbePump::new(git_refresh_interval());
            loop {
                thread::sleep(STATUS_INTERVAL);
                if session.is_terminated() || session.child_exit().is_some() {
                    return;
                }
                let probe = super::status::probe(session.master_fd(), session.child_pid);

                if let Some(cwd) = probe.cwd.as_deref() {
                    if let Some(ran) = git.tick(cwd, STATUS_INTERVAL, super::status::probe_git) {
                        // Cached whether or not it is sent: this cache is what
                        // `catch_up_and_attach` replays to a server that has
                        // just reconnected and has no probe of its own.
                        session.set_git_probe(ran.probe.clone());
                        if ran.changed {
                            if let Some(wire) = session.wire() {
                                let _ = send(&wire, &git_report(&ran.probe));
                            }
                        }
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
    session: &Arc<PaneSession>,
    generation: u64,
    wire: &Wire,
    registry: &Arc<PaneRegistry>,
) -> Result<Outcome> {
    let master_fd = session.master_fd();
    let child_pid = session.child_pid;
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
                    ServerToBrokerMsg::Release => {
                        release_pane(session, generation, wire)?;
                        return Ok(Outcome::Released);
                    },
                    ServerToBrokerMsg::ListPanes => {
                        send(
                            wire,
                            &BrokerToServerMsg::PaneList {
                                entries: registry.entries(),
                            },
                        )?;
                    },
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
///
/// The protocol version is in the name because `ensure` cannot ask a running
/// broker what it speaks — it prints *its own* constant, so a daemon left over
/// from a build before a version bump would be adopted by a newer binary and
/// answer a handshake it does not understand. A version in the path means a new
/// binary simply starts its own broker alongside the old one, which keeps
/// serving the panes it already has until they exit.
pub fn broker_socket_path() -> std::path::PathBuf {
    ZELLIJ_SOCK_DIR.join(format!("pty-broker-v{REMOTE_PROTOCOL_VERSION}.sock"))
}

/// `zellij --pty-broker ensure` — make sure a broker is up and say where.
///
/// Prints one JSON line and exits. Output is the contract with `ssh::bootstrap`,
/// so it goes to stdout and nothing else may.
pub fn ensure() -> Result<()> {
    let socket = broker_socket_path();
    check_socket_path_length(&socket, 0)?;

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
    check_socket_path_length(&socket, 0)?;
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
