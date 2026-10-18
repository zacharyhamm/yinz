//! Moving a running pane between sessions.
//!
//! The pane's process belongs to a pty-host supervisor rather than to the
//! server (see `docs/ARCHITECTURE.md`), so handing it over is a matter of
//! releasing the supervisor here and attaching to it there — the child never
//! learns anything happened.
//!
//! Releasing a supervisor is not reversible: it aborts the pane's reader task,
//! which drops this server's dup of the pty master, and there is no second copy
//! to reopen. So every question that could make a release fail is asked before
//! one is attempted — the far session's reachability here, and the pane's own
//! releasability on the pty thread (`Pty::check_terminal_can_be_warped`). What
//! survives that is the one case nothing local can answer: a remote broker that
//! agrees to be asked and then does not confirm. A pane lost that way is held
//! and told so in its own scrollback rather than left looking live.
//!
//! Everything in here returns `Result<String, String>` rather than reporting to
//! a client. Both halves of that pair are prose meant for whoever asked: the CLI
//! path writes them to a cli client, the plugin path writes them into a protobuf
//! response for a model to read. The failure strings already carry their own
//! recovery instructions, so a caller that reworded them would be making things
//! worse.

use std::time::{Duration, Instant};
use yinz_types::internal_ids::SessionName;
use yinz_types::internal_ids::TabId;

use yinz_runtime::{envs, errors::prelude::*};
use {
    yinz_ipc::ipc::ClientToServerMsg, yinz_ipc::ipc::IpcReceiverWithContext,
    yinz_ipc::ipc::IpcSenderWithContext, yinz_ipc::ipc::ServerToClientMsg,
    yinz_types::data::WarpPlacement, yinz_types::data::WarpedPane, yinz_types::data::WarpedTab,
    yinz_types::input::actions::Action,
};

use crate::pty::PtyInstruction;
use crate::screen::ScreenInstruction;
use crate::thread_bus::ThreadSenders;
use crate::ClientId;
use crate::PaneId;

/// How long to wait on the far session before giving up on a warp.
///
/// Generous compared to the other CLI round trips, because this one includes
/// another server's pty thread attaching to a supervisor over a socket.
pub const WARP_TIMEOUT: Duration = Duration::from_secs(10);

/// The budget a plugin's host call spends on a warp.
///
/// Deliberately the same number: what protects the plugin thread is not a
/// shorter timeout but the fact that the reachability check runs before anything
/// is released, so the common failure (a session name that does not exist) costs
/// a `connect(2)` and nothing else. Named separately so the two can diverge
/// later without touching call sites.
pub const PLUGIN_WARP_TIMEOUT: Duration = WARP_TIMEOUT;

/// How long the *asking* side of a warp waits, and deliberately longer than
/// [`WARP_TIMEOUT`] — three of them, plus change.
///
/// `warp_tab_from_session` and `warp_pane_from_session` hand the whole warp to
/// the session that owns the pane and then wait for its answer. That session
/// does not produce its answer under one `WARP_TIMEOUT` clock but under up to
/// four, run one after another, none of them sharing a deadline with ours:
///
/// 1. probing the destination for companion support, bounded by `WARP_TIMEOUT`;
/// 2. releasing the pane or tab, bounded by its own `recv_timeout(WARP_TIMEOUT)`;
/// 3. handing it to *us* and waiting for our reply — our own pty thread
///    adopting it under `adopt_*_here`'s `WARP_TIMEOUT`, behind a socket read
///    the owner bounds by another `WARP_TIMEOUT`;
/// 4. if that failed, taking it back, under a further `WARP_TIMEOUT`.
///
/// With the asker's budget equal to one of them it always expired first — in
/// perfectly ordinary slow cases, not only pathological ones: a release of a
/// couple of remote panes plus an adoption that had to rebuild an ssh forward
/// was enough. The owner's carefully worded answer then lost a race it should
/// never have been in, and worse, the read that gave up was reported by
/// `read_cli_reply` as the other session having *closed the connection*, for a
/// warp that in fact went on to succeed.
///
/// So this is sized to the owner's real worst case rather than to a guess at
/// its usual one. It is a ceiling: a warp that succeeds returns the moment it
/// does, and the full wait is only ever paid for a warp that failed at every
/// step. The extra second covers the round trips between the two servers.
pub const WARP_PULL_TIMEOUT: Duration = Duration::from_secs(4 * WARP_TIMEOUT.as_secs() + 1);

/// A warp that takes longer than this is worth a line in the log.
///
/// When a user reports that their session hung for a moment, this is the only
/// evidence there will be about which leg was slow.
const SLOW_WARP: Duration = Duration::from_secs(1);

/// Keeps the source session open while one of its panes is between sessions.
///
/// Screen creates one when it parks a pane warp, and it travels with the pane
/// through the release, the handoff and any rollback. `PaneWarpFinished` is
/// sent when the last copy drops, which is after the work that leg queued, not
/// when a caller stops waiting. Shared rather than unique only because thread
/// instructions are `Clone`.
#[derive(Clone)]
pub struct PaneWarpHold(
    #[expect(dead_code, reason = "kept only so its drop sends PaneWarpFinished")]
    std::sync::Arc<PaneWarpHoldInner>,
);

struct PaneWarpHoldInner(ThreadSenders);

impl PaneWarpHold {
    pub fn new(senders: ThreadSenders) -> Self {
        Self(std::sync::Arc::new(PaneWarpHoldInner(senders)))
    }
}

impl Drop for PaneWarpHoldInner {
    fn drop(&mut self) {
        self.0
            .send_to_screen(ScreenInstruction::PaneWarpFinished)
            .context("failed to finish pane warp")
            .non_fatal();
    }
}

impl std::fmt::Debug for PaneWarpHold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PaneWarpHold")
    }
}

/// A released pane and the hold that keeps its source session open.
pub type ReleasedPaneWarp = (Box<WarpedPane>, PaneWarpHold);

/// Screen's answer to a parked pane warp.
pub type PaneWarpAnswer = std::result::Result<ReleasedPaneWarp, String>;

/// Where Screen answers a parked pane warp.
///
/// A plain channel cannot tell Screen that its caller has stopped waiting: a
/// send that lands just before the caller drops its receiver succeeds, and the
/// pane goes down with the receiver. Both sides take the same lock, so an answer
/// either reaches a caller that will read it or comes back to Screen.
#[derive(Clone)]
pub struct PaneWarpReply {
    sender: crossbeam::channel::Sender<PaneWarpAnswer>,
    abandoned: std::sync::Arc<std::sync::Mutex<bool>>,
}

/// The caller's end of a [`PaneWarpReply`].
pub struct PaneWarpWaiter {
    receiver: crossbeam::channel::Receiver<PaneWarpAnswer>,
    abandoned: std::sync::Arc<std::sync::Mutex<bool>>,
}

pub fn pane_warp_reply() -> (PaneWarpReply, PaneWarpWaiter) {
    let (sender, receiver) = crossbeam::channel::unbounded();
    let abandoned = std::sync::Arc::default();
    (
        PaneWarpReply {
            sender,
            abandoned: std::sync::Arc::clone(&abandoned),
        },
        PaneWarpWaiter {
            receiver,
            abandoned,
        },
    )
}

impl PaneWarpReply {
    /// Send `answer`, or hand back the released pane if nobody will read it.
    pub fn answer(&self, answer: PaneWarpAnswer) -> Option<ReleasedPaneWarp> {
        let abandoned = self
            .abandoned
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *abandoned {
            return answer.ok();
        }
        match self.sender.send(answer) {
            Ok(()) => None,
            Err(crossbeam::channel::SendError(answer)) => answer.ok(),
        }
    }
}

impl std::fmt::Debug for PaneWarpReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PaneWarpReply")
    }
}

impl PaneWarpWaiter {
    /// Wait for Screen's answer. On a timeout the reply is abandoned first, and
    /// an answer that arrived before that is still returned.
    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> std::result::Result<PaneWarpAnswer, crossbeam::channel::RecvTimeoutError> {
        match self.receiver.recv_timeout(timeout) {
            Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                *self
                    .abandoned
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
                self.receiver
                    .try_recv()
                    .map_err(|_| crossbeam::channel::RecvTimeoutError::Timeout)
            },
            other => other,
        }
    }

    pub fn try_recv(
        &self,
    ) -> std::result::Result<PaneWarpAnswer, crossbeam::channel::TryRecvError> {
        self.receiver.try_recv()
    }
}

/// What Screen checks once an adopted pane's `NewPane` has run.
#[derive(Clone, Debug)]
pub struct AdoptedPaneFinish {
    pub terminal_id: crate::TerminalId,
    /// From [`recovery_command`]; empty for a held terminal.
    pub recovery: String,
    pub held: bool,
    /// Dropped after the check, so the source session stays open until the
    /// pane exists.
    pub hold: Option<PaneWarpHold>,
    pub placed: crossbeam::channel::Sender<Result<()>>,
}

/// Connect to another session the way `yinz action` does, and confirm someone
/// is listening before anything is released.
///
/// Returns the live connection, so the warp itself can reuse it. Reconnecting
/// afterwards would reopen exactly the window this is checking for.
fn connect_to_session(
    session_name: &SessionName,
    reply_timeout: Duration,
) -> Result<IpcSenderWithContext<ClientToServerMsg>> {
    use {yinz_ipc::consts::ipc_connect, yinz_runtime::consts::YINZ_SOCK_DIR};

    let path = YINZ_SOCK_DIR.join(session_name);
    let stream =
        ipc_connect(&path).with_context(|| format!("session '{session_name}' is not running"))?;
    bound_reply_time(&stream, reply_timeout);
    Ok(IpcSenderWithContext::new(stream))
}

fn warp_capabilities(session: &SessionName, timeout: Duration) -> Result<(bool, bool)> {
    let mut probe = connect_to_session(session, timeout)?;
    probe.send_client_msg(ClientToServerMsg::ConnStatus)?;
    let mut receiver: IpcReceiverWithContext<ServerToClientMsg> = probe
        .get_receiver()
        .context("failed to set up warp capability reply")?;
    loop {
        match receiver.recv_server_msg().msg() {
            Some((
                ServerToClientMsg::Connected {
                    warp_companions,
                    warp_held_terminals,
                },
                _,
            )) => return Ok((warp_companions, warp_held_terminals)),
            Some((message, _)) if is_older_server_push(&message) => continue,
            _ => {
                return Err(anyhow!(
                    "destination did not answer the warp capability probe"
                ))
            },
        }
    }
}

/// A push that a session running an older server sends every connection, a warp's included.
/// It does not answer the warp, so a reply reader passes over it. This server sends these to
/// interactive clients only.
fn is_older_server_push(message: &ServerToClientMsg) -> bool {
    matches!(
        message,
        ServerToClientMsg::UnblockInputThread
            | ServerToClientMsg::SetMouseMode { .. }
            | ServerToClientMsg::ConfigFileUpdated
    )
}

/// Put a ceiling on how long we will wait for the far session's reply.
///
/// Without this the read below blocks forever on a session that accepts the
/// connection and then never answers. That is survivable for a throwaway `yinz
/// action` process; it is not survivable on a plugin thread, which would be
/// wedged for the life of the session.
fn bound_reply_time(stream: &interprocess::local_socket::Stream, timeout: Duration) {
    use interprocess::local_socket::traits::Stream as _;

    if let Err(e) = stream.set_recv_timeout(Some(timeout)) {
        log::warn!("could not bound the warp reply time: {e}");
    }
}

/// The far session answered by shutting down.
///
/// Its own error type so that callers can tell it apart from a session that
/// simply said something unexpected: [`warp_tab_from_session`] expects it when
/// asking for a session's last tab, and [`warp_pane_from_session`] reports it.
#[derive(Debug)]
struct SessionEnded;

impl std::fmt::Display for SessionEnded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the other session ended")
    }
}

impl std::error::Error for SessionEnded {}

/// Read the reply a cli-client request gets.
///
/// `Log` is success and `LogError` is the reason it failed — the same protocol
/// `yinz action` reads, which is why both directions of a warp can speak it.
/// Any other message is an error, except an older server's pushes.
fn read_cli_reply(sender: &mut IpcSenderWithContext<ClientToServerMsg>) -> Result<String> {
    let mut receiver: IpcReceiverWithContext<ServerToClientMsg> = sender
        .get_receiver()
        .context("failed to set up warp reply")?;
    let reply = loop {
        match receiver.recv_server_msg().msg() {
            Some((message, _)) if is_older_server_push(&message) => continue,
            reply => break reply,
        }
    };
    match reply {
        Some((ServerToClientMsg::Log { lines }, _)) => Ok(lines.join(" ")),
        Some((ServerToClientMsg::LogError { lines }, _)) => Err(anyhow!("{}", lines.join(" "))),
        Some((ServerToClientMsg::Exit { .. }, _)) => Err(SessionEnded.into()),
        Some((other, _)) => Err(anyhow!(
            "unexpected reply from the other session: {other:?}"
        )),
        None => Err(anyhow!(
            "the other session closed the connection without answering"
        )),
    }
}

/// Hand a tab to another session and wait to hear that it landed.
fn send_tab_to_session(
    sender: &mut IpcSenderWithContext<ClientToServerMsg>,
    payload: Box<WarpedTab>,
) -> Result<()> {
    sender
        .send_client_msg(ClientToServerMsg::Action {
            action: Action::AdoptTab { payload },
            terminal_id: None,
            client_id: None,
            is_cli_client: true,
        })
        .context("failed to send the tab to the other session")?;

    read_cli_reply(sender).map(|_| ())
}

/// Hand a pane to another session and wait to hear that it landed.
fn send_pane_to_session(
    sender: &mut IpcSenderWithContext<ClientToServerMsg>,
    payload: Box<WarpedPane>,
    placement: WarpPlacement,
) -> Result<()> {
    sender
        .send_client_msg(ClientToServerMsg::Action {
            action: Action::AdoptPane { payload, placement },
            terminal_id: None,
            client_id: None,
            is_cli_client: true,
        })
        .context("failed to send the pane to the other session")?;

    read_cli_reply(sender).map(|_| ())
}

/// What this server calls itself, for the two directions that need to tell
/// themselves apart from the session at the other end.
///
/// Read here rather than inside the warp functions so those stay free of
/// process-global state and can be tested without racing every other test that
/// touches `YINZ_SESSION_NAME`.
pub fn current_session_name() -> SessionName {
    envs::get_session_name().unwrap_or_default()
}

/// Move a pane out of this session and into another one.
///
/// The ordering is the whole design: nothing is released until we know the far
/// session is reachable, because between the release and the adoption the pane
/// belongs to nobody, and a failure in that window costs the user a recovery
/// step rather than nothing.
///
/// Also returns the warp's hold, once a pane was released. The caller drops it
/// after writing the result, so a source that empties itself says how the warp
/// went before it shuts down.
pub fn warp_pane_to_session(
    senders: &ThreadSenders,
    client_id: ClientId,
    pane_id: Option<PaneId>,
    current_session: &SessionName,
    to_session: &SessionName,
    placement: WarpPlacement,
    timeout: Duration,
) -> (std::result::Result<String, String>, Option<PaneWarpHold>) {
    let mut reply_hold = None;
    let result = (|| {
        let started_at = std::time::Instant::now();
        if to_session == current_session {
            return Err(format!("this pane is already in session '{to_session}'"));
        }

        // Reachability first, and the connection is kept: everything after this
        // point has released a supervisor.
        let (supports_companions, supports_held) =
            warp_capabilities(to_session, timeout).map_err(|e| format!("{e:#}"))?;
        let mut sender = connect_to_session(to_session, timeout).map_err(|e| format!("{e:#}"))?;

        let (response_sender, response_receiver) = pane_warp_reply();
        // The same instant this side will stop waiting at, handed down so that every
        // leg of the release answers to one clock rather than to a constant of its
        // own. A release that finishes after we have given up has nobody to hand the
        // pane to.
        let deadline = Instant::now() + timeout;
        senders
            .send_to_screen(ScreenInstruction::WarpPane {
                supports_companions,
                supports_held,
                pane_id,
                to_session: to_session.to_owned(),
                placement,
                client_id,
                response_channel: response_sender,
                deadline,
            })
            .map_err(|e| format!("{e:#}"))?;

        let (payload, hold) = match response_receiver.recv_timeout(timeout) {
            Ok(Ok(released)) => released,
            Ok(Err(reason)) => return Err(reason),
            Err(e) => return Err(preparation_failed("pane", e)),
        };
        reply_hold = Some(hold.clone());
        log_if_slow("releasing the pane", started_at);

        // From here the pane belongs to nobody until someone adopts it.
        let recovery = recovery_command(&payload);
        let recovery = if recovery.is_empty() {
            "The held terminal state could not be restored.".to_owned()
        } else {
            format!("The process is still running — recover it with `{recovery}`")
        };
        let handed_off_at = std::time::Instant::now();
        match send_pane_to_session(&mut sender, payload.clone(), placement) {
            Ok(()) => {
                log_if_slow("handing the pane over", handed_off_at);
                Ok(format!("Moved pane to session '{to_session}'"))
            },
            Err(e) => {
                // Take it back. Adopting is the same operation from this side, so
                // the rollback is the feature's own primitive.
                // The hold goes with the pane, so the source stays open until the
                // pane is placed again, even if this wait times out first.
                let rollback =
                    adopt_pane_here(senders, client_id, payload, placement, Some(hold), timeout);
                Err(match rollback {
                    Ok(_) => format!(
                        "Could not move the pane to '{to_session}' ({e:#}); it is back in this session"
                    ),
                    Err(rollback_error) => format!(
                        "Could not move the pane to '{to_session}' ({e:#}), and could not take \
                         it back either ({rollback_error}). {recovery}"
                    ),
                })
            },
        }
    })();
    (result, reply_hold)
}

/// Take a released pane into this session.
///
/// Both the receiving half of a warp and the rollback when the far side of one
/// refuses, which is not a coincidence: taking a pane back is the same operation
/// as taking one in.
pub fn adopt_pane_here(
    senders: &ThreadSenders,
    client_id: ClientId,
    payload: Box<WarpedPane>,
    placement: WarpPlacement,
    hold: Option<PaneWarpHold>,
    timeout: Duration,
) -> std::result::Result<String, String> {
    let started_at = std::time::Instant::now();
    let (response_sender, response_receiver) = crossbeam::channel::unbounded();
    let (placed_sender, placed_receiver) = crossbeam::channel::unbounded();
    senders
        .send_to_pty(PtyInstruction::AdoptTerminal {
            payload,
            placement,
            client_id,
            response_channel: response_sender,
            hold,
            placed: placed_sender,
        })
        .map_err(|e| format!("{e:#}"))?;

    let adopted = match response_receiver.recv_timeout(timeout) {
        // Attached, so this session owns the supervisor and the caller must not
        // roll back. Only a placement that failed and gave the pane back is
        // reported as an error; one still running when time is up is not.
        Ok(Ok(())) => {
            match placed_receiver.recv_timeout(timeout.saturating_sub(started_at.elapsed())) {
                Ok(Err(e)) => Err(format!("{e:#}")),
                Ok(Ok(())) | Err(_) => Ok("Adopted pane".to_owned()),
            }
        },
        Ok(Err(e)) => Err(format!("{e:#}")),
        Err(_) => Err("timed out adopting the pane".to_owned()),
    };
    log_if_slow("adopting the pane", started_at);
    adopted
}

/// Ask another session to send us one of its panes.
///
/// Deliberately the mirror of the push rather than a second implementation of
/// it: this hands the source session a `WarpPane` naming *us* as the
/// destination, so every check, the release, the rollback and the recovery
/// wording all happen on the side that owns the pane — the only side that can
/// decide any of them. The source then connects back into this server to deliver
/// the pane, which our route and pty threads handle, not the caller's.
pub fn warp_pane_from_session(
    current_session: &SessionName,
    from_session: &SessionName,
    pane_id: PaneId,
    placement: WarpPlacement,
    timeout: Duration,
) -> std::result::Result<String, String> {
    let started_at = std::time::Instant::now();
    if current_session.is_empty() {
        return Err("this session has no name to bring the pane to".to_owned());
    }
    if from_session == current_session {
        return Err(format!("'{from_session}' is this session"));
    }

    let mut sender = connect_to_session(from_session, timeout).map_err(|e| format!("{e:#}"))?;
    sender
        .send_client_msg(ClientToServerMsg::Action {
            action: Action::WarpPane {
                pane_id: Some(pane_id),
                to_session: current_session.to_owned(),
                placement,
            },
            terminal_id: None,
            client_id: None,
            is_cli_client: true,
        })
        .map_err(|e| format!("failed to ask '{from_session}' for the pane: {e:#}"))?;

    // The source answers before a warp can end it (see `warp_pane_to_session`),
    // so a goodbye instead of an answer means it ended some other way, and
    // nothing says whether the pane arrived.
    let brought = read_cli_reply(&mut sender).map_err(|e| {
        if e.downcast_ref::<SessionEnded>().is_some() {
            format!(
                "'{from_session}' ended before confirming the pane arrived; check this \
                 session for it"
            )
        } else {
            format!("{e:#}")
        }
    });
    log_if_slow("bringing the pane over", started_at);
    brought.map(|_| format!("Brought pane from session '{from_session}'"))
}

/// Move a tab out of this session and into another one.
///
/// The tab-sized twin of [`warp_pane_to_session`], and the ordering is the same
/// design for the same reason, only with more at stake: between the release and
/// the adoption a whole tab's worth of processes belong to nobody, so nothing is
/// released until the far session has answered.
pub fn warp_tab_to_session(
    senders: &ThreadSenders,
    client_id: ClientId,
    tab_id: Option<TabId>,
    current_session: &SessionName,
    to_session: &SessionName,
    timeout: Duration,
) -> std::result::Result<String, String> {
    let started_at = std::time::Instant::now();
    if to_session == current_session {
        return Err(format!("this tab is already in session '{to_session}'"));
    }

    let (supports_companions, supports_held) =
        warp_capabilities(to_session, timeout).map_err(|e| format!("{e:#}"))?;
    let mut sender = connect_to_session(to_session, timeout).map_err(|e| format!("{e:#}"))?;

    let (response_sender, response_receiver) = crossbeam::channel::unbounded();
    // See `warp_pane_to_session`: one clock, shared all the way down. It matters
    // more here — a tab releases several panes, and before this was plumbed
    // through each of them spent an unrelated budget in turn.
    let deadline = Instant::now() + timeout;
    senders
        .send_to_screen(ScreenInstruction::WarpTab {
            supports_companions,
            supports_held,
            tab_id,
            client_id,
            response_channel: response_sender,
            deadline,
        })
        .map_err(|e| format!("{e:#}"))?;

    let payload = match response_receiver.recv_timeout(timeout) {
        Ok(Ok(payload)) => payload,
        Ok(Err(reason)) => return Err(reason),
        Err(e) => return Err(preparation_failed("tab", e)),
    };
    log_if_slow("releasing the tab", started_at);

    // From here every pane in the tab belongs to nobody until someone adopts it.
    let recoveries: Vec<String> = payload
        .tiled_panes
        .iter()
        .chain(payload.floating_panes.iter())
        .map(recovery_command)
        .filter(|command| !command.is_empty())
        .collect();
    let recovery = if recoveries.is_empty() {
        "The held terminal state could not be restored.".to_owned()
    } else {
        format!(
            "The transferred processes are still running — recover them with: {}",
            recoveries.join("; ")
        )
    };
    let handed_off_at = std::time::Instant::now();
    match send_tab_to_session(&mut sender, payload.clone()) {
        Ok(()) => {
            log_if_slow("handing the tab over", handed_off_at);
            Ok(format!("Moved tab to session '{to_session}'"))
        },
        Err(e) => {
            // Take it back. As with a pane, adopting is the same operation from
            // this side, so the rollback is the feature's own primitive.
            let rollback = adopt_tab_here(senders, client_id, payload, true, timeout);
            Err(match rollback {
                Ok(_) => format!(
                    "Could not move the tab to '{to_session}' ({e:#}); it is back in this session"
                ),
                Err(rollback_error) => format!(
                    "Could not move the tab to '{to_session}' ({e:#}), and could not take it \
                     back either ({rollback_error}). {recovery}"
                ),
            })
        },
    }
}

/// Take a released tab into this session.
///
/// Both the receiving half of a tab warp and the rollback when the far side of
/// one refuses, for the same reason [`adopt_pane_here`] is both.
pub fn adopt_tab_here(
    senders: &ThreadSenders,
    client_id: ClientId,
    payload: Box<WarpedTab>,
    is_rollback: bool,
    timeout: Duration,
) -> std::result::Result<String, String> {
    let started_at = std::time::Instant::now();
    let (response_sender, response_receiver) = crossbeam::channel::unbounded();
    senders
        .send_to_pty(PtyInstruction::AdoptTab {
            payload,
            client_id,
            is_rollback,
            response_channel: response_sender,
        })
        .map_err(|e| format!("{e:#}"))?;

    let adopted = match response_receiver.recv_timeout(timeout) {
        Ok(Ok(())) => Ok("Adopted tab".to_owned()),
        Ok(Err(e)) => Err(format!("{e:#}")),
        Err(_) => Err("timed out adopting the tab".to_owned()),
    };
    log_if_slow("adopting the tab", started_at);
    adopted
}

/// Ask another session to send us one of its tabs.
///
/// The mirror of the push, exactly as [`warp_pane_from_session`] is: this hands
/// the source session a `WarpTab` naming *us*, so every check, the release, the
/// rollback and the recovery wording happen on the side that owns the tab.
pub fn warp_tab_from_session(
    current_session: &SessionName,
    from_session: &SessionName,
    tab_id: TabId,
    timeout: Duration,
) -> std::result::Result<String, String> {
    let started_at = std::time::Instant::now();
    if current_session.is_empty() {
        return Err("this session has no name to bring the tab to".to_owned());
    }
    if from_session == current_session {
        return Err(format!("'{from_session}' is this session"));
    }

    let mut sender = connect_to_session(from_session, timeout).map_err(|e| format!("{e:#}"))?;
    sender
        .send_client_msg(ClientToServerMsg::Action {
            action: Action::WarpTab {
                tab_id: Some(tab_id),
                to_session: current_session.to_owned(),
            },
            terminal_id: None,
            client_id: None,
            is_cli_client: true,
        })
        .map_err(|e| format!("failed to ask '{from_session}' for the tab: {e:#}"))?;

    // A source that has just given away its only tab is a source with nothing
    // left, and it shuts down — so the reply to the last tab a session owns is
    // its orderly goodbye rather than a line of prose. That only happens *after*
    // the tab has been delivered, which is why it reads as success here and not
    // as the missing answer it looks like.
    let brought = match read_cli_reply(&mut sender) {
        Err(e) if e.downcast_ref::<SessionEnded>().is_some() => Ok(String::new()),
        other => other.map_err(|e| format!("{e:#}")),
    };
    log_if_slow("bringing the tab over", started_at);
    brought.map(|_| format!("Brought tab from session '{from_session}'"))
}

/// The command that gets a pane back, when both the handoff and the rollback
/// have failed.
///
/// The wording itself lives with the pty-host client, because the release path
/// has to say the same thing when a pane is stranded without ever reaching a
/// rollback.
pub fn recovery_command(pane: &WarpedPane) -> String {
    if pane.held.is_some() {
        return String::new();
    }
    if let Some(group) = &pane.group {
        return group
            .tiled_panes
            .iter()
            .chain(&group.floating_panes)
            .map(recovery_command)
            .filter(|command| !command.is_empty())
            .collect::<Vec<_>>()
            .join("; ");
    }
    crate::pty_host::client::recovery_command(
        &pane.host_id,
        pane.remote
            .as_ref()
            .map(|remote| (remote.host.as_str(), remote.pane_token.as_str())),
    )
}

/// Why the preparation half of a warp produced nothing.
///
/// `recv_timeout` collapses two genuinely different things into one `Err`, and
/// they call for different reactions from whoever reads this. A timeout means
/// the release is still running somewhere and trying again in a moment may
/// work; a disconnect means the thread that was going to answer is gone, and
/// trying again will not. Saying "timed out" for both sent more than one
/// investigation looking for a slow pane that was never there.
fn preparation_failed(what: &str, e: crossbeam::channel::RecvTimeoutError) -> String {
    match e {
        crossbeam::channel::RecvTimeoutError::Timeout => {
            format!("timed out preparing the {what} to be moved")
        },
        crossbeam::channel::RecvTimeoutError::Disconnected => format!(
            "this session stopped preparing the {what} to be moved before it said whether it \
             could"
        ),
    }
}

fn log_if_slow(what: &str, started_at: std::time::Instant) {
    let elapsed = started_at.elapsed();
    if elapsed >= SLOW_WARP {
        log::warn!("warp: {what} took {}ms", elapsed.as_millis());
    }
}

#[cfg(test)]
#[path = "./unit/warp_tests.rs"]
mod warp_tests;
