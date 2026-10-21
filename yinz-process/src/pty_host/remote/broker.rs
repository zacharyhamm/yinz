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
use yinz_ipc::session_ownership::SessionOwnership;
use yinz_types::internal_ids::ProcessId;
use yinz_types::internal_ids::TerminalId;

use yinz_runtime::errors::prelude::*;
use yinz_runtime::shared::ensure_secure_dir;
use {yinz_runtime::consts::VERSION, yinz_runtime::consts::YINZ_SOCK_DIR};

use super::protocol::{
    self, BrokerToServerMsg, ChildSignal, Frame, ResumeRefusal, ServerToBrokerMsg,
    MAX_STREAM_PAYLOAD, REMOTE_PROTOCOL_VERSION,
};
use super::session::{orphan_timeout, PaneRegistry, PaneSession, ResumeStep};
use crate::pty_host::client::{self as pty_host_client, SpawnParams};
use crate::pty_host::protocol::{check_socket_path_length, SpawnFailureKind, WireWinsize};
use crate::pty_host::secure_socket;

/// A socket shared by the several things that need to write to it: the output
/// pump, the child-exit callback, and the control pump.
pub type Wire = Arc<Mutex<UnixStream>>;

/// How often to look for panes nobody has come back for.
const REAP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
const TERMINATION_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// How long a hook reporting from inside a remote pane will wait on this machine's broker.
///
/// Under both agents' own hook budgets (`CLAUDE_HOOK_TIMEOUT_SECONDS` is 5,
/// `CODEX_HOOK_TIMEOUT_SECONDS` 3) on purpose: this is the route *every* hook event in a
/// remote pane takes, so a wedged broker must cost a bounded wait and let the script exit 0
/// on its own terms, rather than stalling every tool call until the agent kills the hook.
const AGENT_REPORT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

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
            TerminalId::new(terminal_id),
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
        // From a hook inside one of this broker's panes, on a connection of its
        // own: relayed to whichever server is attached to that pane, and done.
        ServerToBrokerMsg::AgentReport {
            pane_token,
            kind,
            json,
        } => {
            let delivered = relay_agent_report(registry, &pane_token, kind, json);
            send(&wire, &BrokerToServerMsg::AgentReported { delivered })?;
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
            terminate_session(&session, registry, &wire)?;
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

/// End a pane the server asked to end, and tell it so.
///
/// The exit is reported once: the output pump reports it too, the moment the
/// pty drains after the child is gone, and whichever of the two gets there
/// first claims it. A pane whose descendants keep the pty open never drains;
/// that is an ordinary close (a backgrounded job holding the terminal), noted
/// and finished, not an error on the broker.
fn terminate_session(
    session: &Arc<PaneSession>,
    registry: &Arc<PaneRegistry>,
    wire: &Wire,
) -> Result<()> {
    terminate_session_with_drain_timeout(session, registry, wire, TERMINATION_DRAIN_TIMEOUT)
}

fn terminate_session_with_drain_timeout(
    session: &Arc<PaneSession>,
    registry: &Arc<PaneRegistry>,
    wire: &Wire,
    drain_timeout: std::time::Duration,
) -> Result<()> {
    session.terminate();
    let drained = session.wait_for_output_drained(drain_timeout);
    registry.remove(&session.token);
    if !drained {
        log::warn!(
            "remote pane {} did not drain within {drain_timeout:?} after termination; \
             something still holds its terminal open",
            session.token
        );
    }
    if !session.claim_exit_report() {
        return Ok(());
    }
    send(
        wire,
        &BrokerToServerMsg::ChildExited {
            status: session.child_exit().unwrap_or(None),
        },
    )
}

fn handshake(reader: &mut UnixStream, wire: &Wire) -> Result<()> {
    let hello = read_control(reader)?.ok_or_else(|| anyhow!("peer closed before saying hello"))?;
    let ServerToBrokerMsg::Hello {
        protocol_version,
        yinz_version,
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
            yinz_version: VERSION.to_owned(),
        },
    )?;

    if protocol_version != REMOTE_PROTOCOL_VERSION {
        // Name both sides. A remote box on a different build is the normal
        // case, and "connection refused" would send the user hunting the
        // network rather than the version.
        return Err(anyhow!(
            "remote pane protocol mismatch: this broker speaks v{REMOTE_PROTOCOL_VERSION} \
             (yinz {VERSION}), the server speaks v{protocol_version} (yinz \
             {yinz_version}). Install matching yinz versions on both machines."
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
    cmd: &mut yinz_types::input::command::RunCommand,
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
    cmd: yinz_types::input::command::RunCommand,
    failover_cmd: Option<yinz_types::input::command::RunCommand>,
    initial_size: Option<WireWinsize>,
    carried_env: Vec<(String, String)>,
    terminal_id: TerminalId,
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
    crate::command::resolve_agent_launcher(host_bin, &mut cmd.command, &mut cmd.args)?;

    // Recomputed here, per pane, from *this* machine's passwd entry. Nothing is
    // inherited from the broker: its environment is a frozen copy of whichever
    // ssh session happened to start it, possibly another login, hours ago, and
    // shared by every pane and every session that reaches this machine.
    let mut env = remote_env::compose(&remote_env::LoginSource::from_user(&user), carried_env);
    // The pane's token, so a hook running inside it can name it to this broker
    // (`report_agent_hook`). Minted before the spawn for exactly that reason.
    let token = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
    env.push((PANE_TOKEN_ENV.to_owned(), token.clone()));

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
        // Deliberately off: nothing reports a remote pane's suspension back to
        // the asking server, so a `^Z` that really stopped the child would leave
        // a pane nobody could wake. Without job control the signal is discarded,
        // which is what remote panes have always done.
        job_control: false,
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
            let kind = match e.downcast_ref::<YinzError>() {
                Some(YinzError::CommandNotFound { .. }) => SpawnFailureKind::CommandNotFound,
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

    // `finish` starts the thread that watches for the child, so the callback is
    // live from here on — before there is a session to record against. A child
    // that exits in that window leaves its status in `Pending`, and the
    // registration below picks it up.
    let session_slot = Arc::new(Mutex::new(PaneSlot::Pending(None)));
    let cb_slot = session_slot.clone();
    let spawned = pending.finish(
        Box::new(move |_pane_id, status, _cmd| {
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
        }),
        // Remote panes do not report suspension: the asking server is on another
        // machine and has no way to resume the child, so there would be nothing to
        // do with the news. See `docs/` and the plan for what covering them entails.
        None,
    );

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
    let generation = match session.try_begin_attach() {
        Ok(generation) => generation,
        Err(kind) => {
            let reason = match kind {
                ResumeRefusal::AlreadyAttached => {
                    format!("pane {token} already has a server attached")
                },
                ResumeRefusal::Unknown => {
                    format!("pane {token} was reclaimed and is being terminated")
                },
            };
            send(wire, &BrokerToServerMsg::ResumeRefused { kind, reason })?;
            return Err(anyhow!("refused a resume for pane {token}: {kind:?}"));
        },
    };

    // Claimed before anything is sent, so a second resume is refused from here
    // on — but not yet writable by the pumps, which would let new output
    // overtake the catch-up below.
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
                if dropped > 0 {
                    send(wire, &BrokerToServerMsg::OutputGap { dropped })?;
                }
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
    if let Some((git, refresh_token)) = session.git_probe() {
        send(wire, &git_report(&git, refresh_token))?;
    }
    Ok(())
}

fn git_report(git: &super::status::GitProbe, refresh_token: Option<String>) -> BrokerToServerMsg {
    use crate::pty_host::protocol::encode_buffered;
    BrokerToServerMsg::GitProbe {
        refresh_token,
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
/// `agent-service`'s own `GIT_REFRESH_SECONDS`, which is the cadence the local
/// half of the same job already runs at.
///
/// Not shorter: each pane has its own pump, so this is two `git` processes per
/// remote pane per interval, duplicated between panes that share a folder.
const GIT_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// Overrides `GIT_REFRESH_INTERVAL`, so a test can watch a branch change be
/// picked up without waiting a minute for it. Same shape, and the same reason,
/// as `ORPHAN_TIMEOUT_ENV`.
pub const GIT_REFRESH_ENV: &str = "YINZ_REMOTE_GIT_REFRESH_SECS";

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
    refresh_token: Option<String>,
    last: Option<super::status::GitProbe>,
    since_refresh: std::time::Duration,
    interval: std::time::Duration,
}

impl GitProbePump {
    pub fn new(interval: std::time::Duration) -> Self {
        GitProbePump {
            refresh_token: None,
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
        self.tick_with_refresh(cwd, None, elapsed, probe)
    }

    pub fn tick_with_refresh(
        &mut self,
        cwd: &str,
        refresh_token: Option<&str>,
        elapsed: std::time::Duration,
        probe: impl Fn(&str) -> super::status::GitProbe,
    ) -> Option<Probed> {
        let refresh = refresh_token != self.refresh_token.as_deref();
        self.since_refresh += elapsed;
        let moved = self.last.as_ref().map(|last| last.cwd.as_str()) != Some(cwd);
        let due = self.since_refresh >= self.interval;
        if !moved && !due && !refresh {
            return None;
        }
        self.since_refresh = std::time::Duration::ZERO;
        let git = probe(cwd);
        let changed = refresh || self.last.as_ref() != Some(&git);
        self.refresh_token = refresh_token.map(str::to_owned);
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
                if session.is_terminated()
                    || (session.child_exit().is_some()
                        && last.as_ref().is_some_and(|p| p.child_group_terminated))
                {
                    return;
                }
                let probe = super::status::probe(session.master_fd(), session.child_pid);

                if let Some((cwd, refresh_token)) = session.git_probe_target()
                    && let Some(ran) = git.tick_with_refresh(
                        &cwd,
                        refresh_token.as_deref(),
                        STATUS_INTERVAL,
                        super::status::probe_git,
                    )
                {
                    // Cached whether or not it is sent: this cache is what
                    // `catch_up_and_attach` replays to a server that has
                    // just reconnected and has no probe of its own.
                    session.set_git_probe(ran.probe.clone(), refresh_token.clone());
                    if ran.changed
                        && let Some(wire) = session.wire()
                    {
                        let _ = send(&wire, &git_report(&ran.probe, refresh_token));
                    }
                }

                // Only on change: this is a keystroke-free pane's only traffic,
                // and an idle agent should not keep an ssh channel busy.
                if last.as_ref() == Some(&probe) {
                    continue;
                }
                if let Some((wire, generation)) = session.wire_and_generation()
                    && send(&wire, &probe.clone().into_report()).is_err()
                {
                    session.detach_if(generation);
                    continue;
                }
                if probe.child_group_terminated && session.child_exit().is_some() {
                    return;
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
                        if let Some((wire, generation)) = session.append_output(chunk)
                            && send_output(&wire, chunk).is_err()
                        {
                            // The server went away mid-write. Not a
                            // teardown: keep the pane and keep buffering.
                            session.detach_if(generation);
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
                    ServerToBrokerMsg::GetGitProbe { cwd, refresh_token } => {
                        if cwd.is_empty() {
                            return Err(anyhow!("git probe directory is empty"));
                        }
                        session.request_git_probe(cwd, refresh_token);
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
    unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &winsize) };
}

/// Resolve a symbolic signal against the child this broker owns.
///
/// The pid never crosses the wire in either direction, which is the point: a pid
/// from the server's machine would name an unrelated process here, and this
/// child's pid would name an unrelated process there.
fn signal_child(child_pid: ProcessId, signal: ChildSignal) {
    let pid = super::super::kernel_pid(child_pid);
    let sig = match signal {
        ChildSignal::Hup => nix::sys::signal::Signal::SIGHUP,
        ChildSignal::Int => nix::sys::signal::Signal::SIGINT,
        ChildSignal::Kill => nix::sys::signal::Signal::SIGKILL,
        // Aimed at the process group, like the local `send_sigcont` — see there
        // for why the leader alone is not enough. Unreachable today: no remote
        // pane ever reports a suspend, so nothing asks for this.
        ChildSignal::Cont => nix::sys::signal::Signal::SIGCONT,
    };
    let group_signalled = super::super::process_group(pid)
        .is_ok_and(|group| nix::sys::signal::kill(group, Some(sig)).is_ok());
    if !group_signalled {
        let _ = nix::sys::signal::kill(pid, Some(sig));
    }
}

// --- daemon entry points ---

/// The argv flag that turns this binary into a broker, mirroring
/// `PTY_HOST_FLAG`. Recognised only as argv\[1\], so `main` can branch before
/// clap and the logger.
pub const PTY_BROKER_FLAG: &str = "--pty-broker";

pub fn broker_invocation() -> Option<String> {
    broker_invocation_from_args(std::env::args())
}

/// The environment variable a remote pane's process finds its broker token in.
///
/// Read by `report_agent_hook` below, and by the hook script `yinz hooks install`
/// writes: it is how a hook running on the far side of the transport knows there is no
/// session on this machine to report to, and which pane the broker should relay it to.
pub const PANE_TOKEN_ENV: &str = "YINZ_REMOTE_PANE_TOKEN";

/// Send a hook's event on the named pane's own channel, if a server is attached
/// to it. A pane nobody is attached to has no one to tell; the event is
/// dropped rather than queued, as a local hook's would be with no session.
fn relay_agent_report(
    registry: &Arc<PaneRegistry>,
    pane_token: &str,
    kind: String,
    json: String,
) -> bool {
    let mut trace = yinz_runtime::agent::report::ReportTrace::new(std::time::Instant::now());
    trace.kind = kind.clone();
    trace.payload_bytes = json.len();
    if let Ok(event) = serde_json::from_str::<yinz_types::agent::HookEvent>(&json) {
        trace.event = event.hook_event_name;
    }
    trace.enter("relay_resolve");
    let result = (|| {
        let session = registry
            .get(pane_token)
            .context("no pane for remote report")?;
        let wire = session.wire().context("no owner for remote report")?;
        trace.enter("relay_lock");
        let mut socket = wire.lock().map_err(|_| anyhow!("remote wire poisoned"))?;
        trace.enter("relay_write");
        protocol::write_control(&mut *socket, &BrokerToServerMsg::AgentReport { kind, json })
    })();
    trace.finish(result.is_ok());
    result.is_ok()
}

fn valid_agent_kind(kind: &str) -> bool {
    !kind.is_empty()
        && kind.len() <= 32
        && kind
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

/// `yinz --pty-broker report <kind>`: the hook command a coding agent runs
/// inside a remote pane. Reads the event from stdin and hands it to this
/// machine's broker, which relays it to the pane's server on the pane's own
/// channel, where it is trusted because it came over the transport rather
/// than out of the pane's bytes.
/// One native hook payload off the intake, bounded by what a provider sends.
///
/// The broadcast cap bounds the *normalized* event, not these bytes: a prompt
/// `agent prompt` accepted arrives whole inside `UserPromptSubmit`, so this reads
/// up to [`NATIVE_HOOK_MAX_BYTES`](yinz_types::agent::NATIVE_HOOK_MAX_BYTES) and
/// lets the report's own parse and prune cut it to broadcast size. Rejecting here
/// would discard the acknowledgement before the prune could preserve its
/// fingerprint, and a large final message before it could complete the turn.
fn read_native_hook(reader: &mut impl Read) -> Result<Vec<u8>> {
    let mut json = Vec::new();
    reader
        .take(yinz_types::agent::NATIVE_HOOK_MAX_BYTES as u64 + 1)
        .read_to_end(&mut json)?;
    if json.len() > yinz_types::agent::NATIVE_HOOK_MAX_BYTES {
        return Err(anyhow!("agent hook event exceeds the native payload limit"));
    }
    Ok(json)
}

pub fn report_agent_hook(started: std::time::Instant) -> Result<()> {
    let mut trace = yinz_runtime::agent::report::ReportTrace::new(started);
    let result = (|| {
        let kind = std::env::args()
            .nth(3)
            .unwrap_or_else(|| "claude".to_owned());
        if !valid_agent_kind(&kind) {
            return Err(anyhow!("invalid remote agent kind"));
        }
        trace.kind = kind.clone();
        trace.enter("stdin_parse");
        let pane_token = std::env::var(PANE_TOKEN_ENV).with_context(|| {
            format!("{PANE_TOKEN_ENV} is not set; this is not a pane of a Yinz remote broker")
        })?;
        let json = String::from_utf8(read_native_hook(&mut std::io::stdin())?)
            .context("agent hook JSON is not UTF-8")?;
        // Validated through the same reading the report itself uses, so an opencode event
        // written in opencode's own shape is accepted here rather than only further on.
        let (event, _) = yinz_runtime::agent::hooks::parse_hook_event(&kind, json.clone())
            .context("invalid agent hook JSON")?;
        trace.event = event.hook_event_name.clone();
        trace.payload_bytes = json.len();
        let socket = broker_socket_path();
        report_agent_hook_to_traced(&socket, &pane_token, &kind, &json, &mut trace)?;
        crate::claude_turn::start(
            &kind,
            &event,
            crate::claude_turn::Route::Remote { socket, pane_token },
        )
    })();
    trace.finish(result.is_ok());
    result
}

/// The report itself, against a given broker socket: a fresh connection, the
/// handshake every connection starts with, the report, and the broker's word
/// on whether anyone was there to get it.
///
/// Every step is bounded by [`AGENT_REPORT_TIMEOUT`]: an agent is waiting on this, and a
/// broker that has stopped answering must not be able to hold up the pane's work.
pub fn report_agent_hook_to(
    socket: &std::path::Path,
    pane_token: &str,
    kind: &str,
    json: &str,
) -> Result<()> {
    let mut trace = yinz_runtime::agent::report::ReportTrace::new(std::time::Instant::now());
    trace.kind = kind.to_owned();
    trace.payload_bytes = json.len();
    let result = report_agent_hook_to_traced(socket, pane_token, kind, json, &mut trace);
    trace.finish(result.is_ok());
    result
}

fn report_agent_hook_to_traced(
    socket: &std::path::Path,
    pane_token: &str,
    kind: &str,
    json: &str,
    trace: &mut yinz_runtime::agent::report::ReportTrace,
) -> Result<()> {
    let (event, json) = yinz_runtime::agent::hooks::parse_hook_event(kind, json.to_owned())
        .context("invalid agent hook JSON")?;
    let json = event.broadcast_json(json);
    trace.enter("connect");
    let mut stream = UnixStream::connect(socket)
        .with_context(|| format!("no remote pane broker at {}", socket.display()))?;
    // Set before the clone below, which shares this socket and so inherits both.
    stream
        .set_read_timeout(Some(AGENT_REPORT_TIMEOUT))
        .context("failed to bound an agent report")?;
    stream
        .set_write_timeout(Some(AGENT_REPORT_TIMEOUT))
        .context("failed to bound an agent report")?;
    trace.enter("handshake_write");
    protocol::write_control(
        &mut stream,
        &ServerToBrokerMsg::Hello {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            yinz_version: VERSION.to_owned(),
        },
    )?;
    let mut reader = stream
        .try_clone()
        .context("failed to clone the broker connection")?;
    trace.enter("handshake_reply");
    match read_broker_control(&mut reader)? {
        Some(BrokerToServerMsg::HelloAck {
            protocol_version, ..
        }) if protocol_version == REMOTE_PROTOCOL_VERSION => {},
        other => return Err(anyhow!("unexpected broker handshake: {other:?}")),
    }
    trace.enter("write");
    protocol::write_control(
        &mut stream,
        &ServerToBrokerMsg::AgentReport {
            pane_token: pane_token.to_owned(),
            kind: kind.to_owned(),
            json: json.to_owned(),
        },
    )?;
    trace.enter("reply");
    match read_broker_control(&mut reader)? {
        Some(BrokerToServerMsg::AgentReported { delivered: true }) => Ok(()),
        Some(BrokerToServerMsg::AgentReported { delivered: false }) => Err(anyhow!(
            "no session is attached to this pane, so the report had nowhere to go"
        )),
        other => Err(anyhow!("unexpected answer to an agent report: {other:?}")),
    }
}

fn read_broker_control(reader: &mut UnixStream) -> Result<Option<BrokerToServerMsg>> {
    match protocol::read_frame(reader)? {
        None => Ok(None),
        Some(frame @ Frame::Control(_)) => frame.decode_control().map(Some),
        Some(_) => Err(anyhow!("expected a control frame from the broker")),
    }
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
/// Computed here and never on the connecting side: `YINZ_SOCK_DIR` resolves
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
    YINZ_SOCK_DIR.join(yinz_ipc::consts::pty_broker_socket_name(
        REMOTE_PROTOCOL_VERSION,
    ))
}

/// `yinz --pty-broker ensure` — make sure a broker is up and say where.
///
/// Prints one JSON line and exits. Output is the contract with `ssh::bootstrap`,
/// so it goes to stdout and nothing else may.
pub fn ensure() -> Result<()> {
    let socket = broker_socket_path();
    ensure_socket(&socket, spawn_detached_broker)?;

    println!(
        "{}",
        serde_json::json!({
            "socket": socket.to_string_lossy(),
            "protocol_version": REMOTE_PROTOCOL_VERSION,
            "yinz_version": VERSION,
        })
    );
    Ok(())
}

fn ensure_socket(
    socket: &std::path::Path,
    start: impl FnOnce(&std::path::Path) -> Result<()>,
) -> Result<()> {
    check_socket_path_length(socket, 0)?;
    if !broker_is_reachable(socket)? {
        start(socket)?;
        await_socket(socket)?;
    }
    Ok(())
}

fn broker_is_reachable(socket: &std::path::Path) -> Result<bool> {
    match yinz_ipc::consts::ipc_connect(socket) {
        Ok(_) => Ok(true),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            Ok(false)
        },
        Err(error) => Err(error)
            .with_context(|| format!("failed to connect to broker at {}", socket.display())),
    }
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
        if broker_is_reachable(socket)? {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    Err(anyhow!(
        "a broker was started but never began listening on {}",
        socket.display()
    ))
}

/// `yinz --pty-broker serve` — the daemon itself.
pub fn serve_forever() -> Result<()> {
    let socket = broker_socket_path();
    let Some((_ownership, listener)) = bind_broker(&socket)? else {
        return Ok(());
    };
    let host_bin = std::env::current_exe().context("could not resolve our own binary")?;
    serve(listener, host_bin)
}

fn bind_broker(socket: &std::path::Path) -> Result<Option<(SessionOwnership, UnixListener)>> {
    check_socket_path_length(socket, 0)?;
    if let Some(parent) = socket.parent() {
        ensure_secure_dir(parent)?;
    }
    let Some(ownership) = SessionOwnership::acquire(socket)
        .with_context(|| format!("failed to claim broker socket {}", socket.display()))?
    else {
        return Ok(None);
    };
    let listener = ownership
        .bind()
        .with_context(|| format!("failed to bind {}", socket.display()))?;
    let interprocess::local_socket::Listener::UdSocket(listener) = listener;
    // Owner only: this socket starts processes as us.
    secure_socket(socket, 0o600)?;
    Ok(Some((ownership, listener.into())))
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
