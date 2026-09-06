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

use zellij_runtime::{envs, errors::prelude::*};
use zellij_utils::{
    data::{WarpPlacement, WarpedPane, WarpedTab},
    input::actions::Action,
    ipc::{ClientToServerMsg, IpcReceiverWithContext, IpcSenderWithContext, ServerToClientMsg},
};

use crate::panes::PaneId;
use crate::pty::PtyInstruction;
use crate::screen::ScreenInstruction;
use crate::thread_bus::ThreadSenders;
use crate::ClientId;

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
/// three, run one after another, none of them sharing a deadline with ours:
///
/// 1. releasing the pane or tab, bounded by its own `recv_timeout(WARP_TIMEOUT)`;
/// 2. handing it to *us* and waiting for our reply — our own pty thread
///    adopting it under `adopt_*_here`'s `WARP_TIMEOUT`, behind a socket read
///    the owner bounds by another `WARP_TIMEOUT`;
/// 3. if that failed, taking it back, under a further `WARP_TIMEOUT`.
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
pub const WARP_PULL_TIMEOUT: Duration = Duration::from_secs(3 * WARP_TIMEOUT.as_secs() + 1);

/// A warp that takes longer than this is worth a line in the log.
///
/// When a user reports that their session hung for a moment, this is the only
/// evidence there will be about which leg was slow.
const SLOW_WARP: Duration = Duration::from_secs(1);

/// Connect to another session the way `zellij action` does, and confirm someone
/// is listening before anything is released.
///
/// Returns the live connection, so the warp itself can reuse it. Reconnecting
/// afterwards would reopen exactly the window this is checking for.
fn connect_to_session(
    session_name: &str,
    reply_timeout: Duration,
) -> Result<IpcSenderWithContext<ClientToServerMsg>> {
    use zellij_utils::consts::{ipc_connect, ZELLIJ_SOCK_DIR};

    let path = ZELLIJ_SOCK_DIR.join(session_name);
    let stream =
        ipc_connect(&path).with_context(|| format!("session '{session_name}' is not running"))?;
    bound_reply_time(&stream, reply_timeout);
    Ok(IpcSenderWithContext::new(stream))
}

/// Put a ceiling on how long we will wait for the far session's reply.
///
/// Without this the read below blocks forever on a session that accepts the
/// connection and then never answers. That is survivable for a throwaway `zellij
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
/// Its own error type so that the one caller which can expect it —
/// [`warp_tab_from_session`], asking for a session's last tab — can tell it apart
/// from a session that simply said something unexpected.
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
/// `zellij action` reads, which is why both directions of a warp can speak it.
/// A raw session connection is also registered as a regular client, so it can
/// receive an unrelated input-unblock broadcast before its action-specific
/// reply. That control message says nothing about the warp and is skipped.
fn read_cli_reply(sender: &mut IpcSenderWithContext<ClientToServerMsg>) -> Result<String> {
    let mut receiver: IpcReceiverWithContext<ServerToClientMsg> = sender.get_receiver();
    loop {
        match receiver.recv_server_msg().msg() {
            Some((ServerToClientMsg::UnblockInputThread, _)) => continue,
            Some((ServerToClientMsg::Log { lines }, _)) => return Ok(lines.join(" ")),
            Some((ServerToClientMsg::LogError { lines }, _)) => {
                return Err(anyhow!("{}", lines.join(" ")))
            },
            Some((ServerToClientMsg::Exit { .. }, _)) => return Err(SessionEnded.into()),
            Some((other, _)) => {
                return Err(anyhow!(
                    "unexpected reply from the other session: {other:?}"
                ))
            },
            None => {
                return Err(anyhow!(
                    "the other session closed the connection without answering"
                ))
            },
        }
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
/// touches `ZELLIJ_SESSION_NAME`.
pub fn current_session_name() -> String {
    envs::get_session_name().unwrap_or_default()
}

/// Move a pane out of this session and into another one.
///
/// The ordering is the whole design: nothing is released until we know the far
/// session is reachable, because between the release and the adoption the pane
/// belongs to nobody, and a failure in that window costs the user a recovery
/// step rather than nothing.
pub fn warp_pane_to_session(
    senders: &ThreadSenders,
    client_id: ClientId,
    pane_id: Option<PaneId>,
    current_session: &str,
    to_session: &str,
    placement: WarpPlacement,
    timeout: Duration,
) -> std::result::Result<String, String> {
    let started_at = std::time::Instant::now();
    if to_session == current_session {
        return Err(format!("this pane is already in session '{to_session}'"));
    }

    // Reachability first, and the connection is kept: everything after this
    // point has released a supervisor.
    let mut sender = connect_to_session(to_session, timeout).map_err(|e| format!("{e:#}"))?;

    let (response_sender, response_receiver) = crossbeam::channel::unbounded();
    // The same instant this side will stop waiting at, handed down so that every
    // leg of the release answers to one clock rather than to a constant of its
    // own. A release that finishes after we have given up has nobody to hand the
    // pane to.
    let deadline = Instant::now() + timeout;
    senders
        .send_to_screen(ScreenInstruction::WarpPane {
            pane_id,
            to_session: to_session.to_owned(),
            placement,
            client_id,
            response_channel: response_sender,
            deadline,
        })
        .map_err(|e| format!("{e:#}"))?;

    let payload = match response_receiver.recv_timeout(timeout) {
        Ok(Ok(payload)) => payload,
        Ok(Err(reason)) => return Err(reason),
        Err(e) => return Err(preparation_failed("pane", e)),
    };
    log_if_slow("releasing the pane", started_at);

    // From here the pane belongs to nobody until someone adopts it.
    let recovery = recovery_command(&payload);
    let handed_off_at = std::time::Instant::now();
    match send_pane_to_session(&mut sender, payload.clone(), placement) {
        Ok(()) => {
            log_if_slow("handing the pane over", handed_off_at);
            Ok(format!("Moved pane to session '{to_session}'"))
        },
        Err(e) => {
            // Take it back. Adopting is the same operation from this side, so
            // the rollback is the feature's own primitive.
            let rollback = adopt_pane_here(senders, client_id, payload, placement, timeout);
            Err(match rollback {
                Ok(_) => format!(
                    "Could not move the pane to '{to_session}' ({e:#}); it is back in this session"
                ),
                Err(rollback_error) => format!(
                    "Could not move the pane to '{to_session}' ({e:#}), and could not take \
                     it back either ({rollback_error}). The process is still running — \
                     recover it with `{recovery}`"
                ),
            })
        },
    }
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
    timeout: Duration,
) -> std::result::Result<String, String> {
    let started_at = std::time::Instant::now();
    let (response_sender, response_receiver) = crossbeam::channel::unbounded();
    senders
        .send_to_pty(PtyInstruction::AdoptTerminal {
            payload,
            placement,
            client_id,
            response_channel: response_sender,
        })
        .map_err(|e| format!("{e:#}"))?;

    let adopted = match response_receiver.recv_timeout(timeout) {
        Ok(Ok(())) => Ok("Adopted pane".to_owned()),
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
    current_session: &str,
    from_session: &str,
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

    let brought = read_cli_reply(&mut sender).map_err(|e| format!("{e:#}"));
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
    tab_index: Option<usize>,
    current_session: &str,
    to_session: &str,
    timeout: Duration,
) -> std::result::Result<String, String> {
    let started_at = std::time::Instant::now();
    if to_session == current_session {
        return Err(format!("this tab is already in session '{to_session}'"));
    }

    let mut sender = connect_to_session(to_session, timeout).map_err(|e| format!("{e:#}"))?;

    let (response_sender, response_receiver) = crossbeam::channel::unbounded();
    // See `warp_pane_to_session`: one clock, shared all the way down. It matters
    // more here — a tab releases several panes, and before this was plumbed
    // through each of them spent an unrelated budget in turn.
    let deadline = Instant::now() + timeout;
    senders
        .send_to_screen(ScreenInstruction::WarpTab {
            tab_index,
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
        .collect();
    let handed_off_at = std::time::Instant::now();
    match send_tab_to_session(&mut sender, payload.clone()) {
        Ok(()) => {
            log_if_slow("handing the tab over", handed_off_at);
            Ok(format!("Moved tab to session '{to_session}'"))
        },
        Err(e) => {
            // Take it back. As with a pane, adopting is the same operation from
            // this side, so the rollback is the feature's own primitive.
            let rollback = adopt_tab_here(senders, client_id, payload, timeout);
            Err(match rollback {
                Ok(_) => format!(
                    "Could not move the tab to '{to_session}' ({e:#}); it is back in this session"
                ),
                Err(rollback_error) => format!(
                    "Could not move the tab to '{to_session}' ({e:#}), and could not take it \
                     back either ({rollback_error}). The processes are still running — recover \
                     them with: {}",
                    recoveries.join("; ")
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
    timeout: Duration,
) -> std::result::Result<String, String> {
    let started_at = std::time::Instant::now();
    let (response_sender, response_receiver) = crossbeam::channel::unbounded();
    senders
        .send_to_pty(PtyInstruction::AdoptTab {
            payload,
            client_id,
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
    current_session: &str,
    from_session: &str,
    tab_index: usize,
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
                tab_index: Some(tab_index),
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
