//! Moving a running pane between sessions.
//!
//! The pane's process belongs to a pty-host supervisor rather than to the
//! server (see `docs/ARCHITECTURE.md`), so handing it over is a matter of
//! releasing the supervisor here and attaching to it there — the child never
//! learns anything happened.
//!
//! Everything in here returns `Result<String, String>` rather than reporting to
//! a client. Both halves of that pair are prose meant for whoever asked: the CLI
//! path writes them to a cli client, the plugin path writes them into a protobuf
//! response for a model to read. The failure strings already carry their own
//! recovery instructions, so a caller that reworded them would be making things
//! worse.

use std::time::Duration;

use zellij_utils::{
    data::{WarpPlacement, WarpedPane},
    envs,
    errors::prelude::*,
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

/// Read the one reply a cli-client request gets.
///
/// `Log` is success and `LogError` is the reason it failed — the same protocol
/// `zellij action` reads, which is why both directions of a warp can speak it.
fn read_cli_reply(sender: &mut IpcSenderWithContext<ClientToServerMsg>) -> Result<String> {
    let mut receiver: IpcReceiverWithContext<ServerToClientMsg> = sender.get_receiver();
    match receiver.recv_server_msg() {
        Some((ServerToClientMsg::Log { lines }, _)) => Ok(lines.join(" ")),
        Some((ServerToClientMsg::LogError { lines }, _)) => Err(anyhow!("{}", lines.join(" "))),
        Some((other, _)) => Err(anyhow!(
            "unexpected reply from the other session: {other:?}"
        )),
        None => Err(anyhow!(
            "the other session closed the connection without answering"
        )),
    }
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
    senders
        .send_to_screen(ScreenInstruction::WarpPane {
            pane_id,
            to_session: to_session.to_owned(),
            placement,
            client_id,
            response_channel: response_sender,
        })
        .map_err(|e| format!("{e:#}"))?;

    let payload = match response_receiver.recv_timeout(timeout) {
        Ok(Ok(payload)) => payload,
        Ok(Err(reason)) => return Err(reason),
        Err(_) => return Err("timed out preparing the pane to be moved".to_owned()),
    };
    log_if_slow("releasing the pane", started_at);

    // From here the pane belongs to nobody until someone adopts it.
    let host_id = payload.host_id.clone();
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
                     recover it with `zellij pty-host adopt {host_id}`"
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
                pane_id: Some(pane_id.into()),
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

fn log_if_slow(what: &str, started_at: std::time::Instant) {
    let elapsed = started_at.elapsed();
    if elapsed >= SLOW_WARP {
        log::warn!("warp: {what} took {}ms", elapsed.as_millis());
    }
}

#[cfg(test)]
#[path = "./unit/warp_tests.rs"]
mod warp_tests;
