#![allow(clippy::needless_return)]
#![allow(clippy::useless_borrows_in_formatting)]
#![allow(clippy::explicit_auto_deref)]
use crate::errors::prelude::{Context, LoggableError};
use crate::session_checkpoint::SessionCheckpoint;
use crate::{
    consts::{
        is_ipc_socket, session_info_folder_for_session, ZELLIJ_SESSION_INFO_CACHE_DIR,
        ZELLIJ_SOCK_DIR,
    },
    data::SessionInfo,
    envs,
    input::layout::Layout,
    ipc::{
        ClientToServerMsg, IpcReceiverWithContext, IpcRecv, IpcSenderWithContext, ServerToClientMsg,
    },
};
use anyhow;
use humantime::format_duration;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};
use std::{fs, io, process};
use suggest::Suggest;

/// Probe every candidate socket at once and keep the ones that answered.
///
/// Serially, the cost of enumeration is the number of sessions times
/// `PROBE_TIMEOUT` in the worst case — on a machine with a dozen long-lived
/// sessions that is seconds of latency on every `zellij ls`, and it grows with
/// the number of sessions a user happens to have left running. Probes are
/// independent and spend all their time blocked on a socket, so the whole set
/// costs one timeout instead of N.
///
/// Order is not preserved; both callers below impose their own.
fn probe_sockets<T: Send + 'static>(candidates: Vec<(String, T)>) -> Vec<(String, T)> {
    // Only a server that answers is listed. An unresponsive one is not offered
    // for attaching — attaching to it would hang — but it is not forgotten
    // either: `session_status` still reports it, so nothing creates a new
    // session over its socket, and `probe_sockets_with_status` below still
    // shows it to a person running `zellij ls`.
    probe_sockets_with_status(candidates)
        .into_iter()
        .filter(|(_, _, status)| matches!(status, SessionProbe::Alive))
        .map(|(name, extra, _)| (name, extra))
        .collect()
}

/// The same sweep, keeping what each probe found instead of discarding it.
///
/// `zellij ls` and `zellij kill-all-sessions` want this: listing must show a
/// wedged session, and killing must still attempt it. Other callers are choosing
/// a session to attach to or a name to create, where "answered" is the only
/// useful answer.
fn probe_sockets_with_status<T: Send + 'static>(
    candidates: Vec<(String, T)>,
) -> Vec<(String, T, SessionProbe)> {
    let probes: Vec<_> = candidates
        .into_iter()
        .map(|(name, extra)| {
            std::thread::spawn(move || {
                let status = assert_socket(&name).unwrap_or(SessionProbe::Unresponsive);
                (name, extra, status)
            })
        })
        .collect();
    probes
        .into_iter()
        // A panicking probe is treated as a session that did not answer, which
        // is the same conclusion by a worse route: never let it poison the rest.
        .filter_map(|probe| probe.join().ok())
        .collect()
}

/// Every socket in the session directory, paired with its session's age.
///
/// Unprobed: this is the candidate list the probe sweeps, shared by the two
/// entry points below so that they cannot disagree about what a session is.
fn session_socket_candidates() -> Result<Vec<(String, Duration)>, io::ErrorKind> {
    match fs::read_dir(&*ZELLIJ_SOCK_DIR) {
        Ok(files) => {
            let mut candidates = Vec::new();
            files.for_each(|file| {
                if let Ok(file) = file {
                    let file_name = file.file_name().into_string().unwrap();
                    // try to get creation time, fall back to modification time on platforms where it's not supported (e.g., musl)
                    // for session creation time these are almost always identical (notable
                    // exceptions are session name changes)
                    let ctime = std::fs::metadata(file.path())
                        .ok()
                        .and_then(|f| f.created().ok().or_else(|| f.modified().ok()))
                        .and_then(|d| d.elapsed().ok())
                        .unwrap_or_default();
                    let duration = Duration::from_secs(ctime.as_secs());
                    if is_ipc_socket(&file.file_type().unwrap()) {
                        candidates.push((file_name, duration));
                    }
                }
            });
            Ok(candidates)
        },
        Err(err) if io::ErrorKind::NotFound != err.kind() => Err(err.kind()),
        Err(_) => Ok(Vec::with_capacity(0)),
    }
}

pub fn get_sessions() -> Result<Vec<(String, Duration)>, io::ErrorKind> {
    Ok(probe_sockets(session_socket_candidates()?))
}

/// Every session with a socket, answering or not.
///
/// Deliberately separate from `get_sessions` rather than a widening of it: the
/// callers of that one are picking a session to attach to or a name to create
/// over, and handing them a wedged session would have them hang on it or build
/// on top of a live server. Listing and kill-all are the callers that need the
/// whole picture.
pub fn get_sessions_with_status() -> Result<Vec<(String, Duration, SessionProbe)>, io::ErrorKind> {
    Ok(probe_sockets_with_status(session_socket_candidates()?))
}

pub fn get_resurrectable_sessions() -> Vec<(String, Duration)> {
    match fs::read_dir(&*ZELLIJ_SESSION_INFO_CACHE_DIR) {
        Ok(files_in_session_info_folder) => {
            let files_that_are_folders = files_in_session_info_folder
                .filter_map(|f| f.ok().map(|f| f.path()))
                .filter(|f| f.is_dir());
            files_that_are_folders
                .filter_map(|folder_name| {
                    let checkpoint = SessionCheckpoint::read(&folder_name).to_log().ok()?;
                    let layout_file_name = checkpoint.layout_path();
                    // Try to get creation time, fall back to modification time on platforms where it's not supported (e.g., musl)
                    let ctime = std::fs::metadata(&layout_file_name)
                        .ok()
                        .and_then(|metadata| {
                            metadata.created().ok().or_else(|| metadata.modified().ok())
                        });
                    let elapsed_duration = ctime
                        .map(|ctime| {
                            Duration::from_secs(ctime.elapsed().ok().unwrap_or_default().as_secs())
                        })
                        .unwrap_or_default();
                    let session_name = folder_name
                        .file_name()
                        .map(|f| std::path::PathBuf::from(f).display().to_string())?;
                    if std::path::Path::new(&layout_file_name).exists() {
                        Some((session_name, elapsed_duration))
                    } else {
                        None
                    }
                })
                .collect()
        },
        Err(e) => {
            log::error!(
                "Failed to read session_info cache folder: \"{:?}\": {:?}",
                &*ZELLIJ_SESSION_INFO_CACHE_DIR,
                e
            );
            vec![]
        },
    }
}

pub fn get_resurrectable_session_names() -> Vec<String> {
    match fs::read_dir(&*ZELLIJ_SESSION_INFO_CACHE_DIR) {
        Ok(files_in_session_info_folder) => {
            let files_that_are_folders = files_in_session_info_folder
                .filter_map(|f| f.ok().map(|f| f.path()))
                .filter(|f| f.is_dir());
            files_that_are_folders
                .filter_map(|folder_name| {
                    let checkpoint = SessionCheckpoint::read(&folder_name).to_log().ok()?;
                    let resurrection_layout_file = checkpoint.layout_path();
                    if std::path::Path::new(&resurrection_layout_file).exists() {
                        folder_name
                            .file_name()
                            .map(|f| format!("{}", f.to_string_lossy()))
                    } else {
                        None
                    }
                })
                .collect()
        },
        Err(e) => {
            log::error!(
                "Failed to read session_info cache folder: \"{:?}\": {:?}",
                &*ZELLIJ_SESSION_INFO_CACHE_DIR,
                e
            );
            vec![]
        },
    }
}

pub fn get_sessions_sorted_by_mtime() -> anyhow::Result<Vec<String>> {
    match fs::read_dir(&*ZELLIJ_SOCK_DIR) {
        Ok(files) => {
            let mut candidates: Vec<(String, SystemTime)> = Vec::new();
            for file in files {
                let file = file?;
                let file_name = file.file_name().into_string().unwrap();
                let file_modified_at = file.metadata()?.modified()?;
                if is_ipc_socket(&file.file_type()?) {
                    candidates.push((file_name, file_modified_at));
                }
            }
            let mut sessions_with_mtime = probe_sockets(candidates);
            sessions_with_mtime.sort_by_key(|x| x.1); // the oldest one will be the first

            let sessions = sessions_with_mtime.iter().map(|x| x.0.clone()).collect();
            Ok(sessions)
        },
        Err(err) if io::ErrorKind::NotFound != err.kind() => Err(err.into()),
        Err(_) => Ok(Vec::with_capacity(0)),
    }
}

/// How long to wait for a server to answer a liveness probe.
///
/// A wedged server is not a dead one: it accepts the connection and then never
/// replies, because the reply comes from a main loop that has stopped turning.
/// Without a deadline the probe below blocks forever, and since every
/// enumeration path funnels through it, one wedged session hangs `zellij`,
/// `zellij attach`, `zellij ls` and `zellij action` — for unrelated sessions
/// too, and for a brand new session that has nothing to do with any of them.
///
/// Generous rather than tight: this is a loopback socket answering a constant,
/// so a healthy server replies in well under a millisecond even under load. The
/// margin is for a machine thrashing, not for a slow reply.
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// What a liveness probe found behind a session socket.
///
/// Three answers, not two, because "did not answer" and "nobody there" call
/// for opposite reactions: an absent server's name is free to reuse, while an
/// unresponsive one is still running and still owns its socket — creating a
/// session over it would unlink that socket and strand the live server with
/// no way for any client to reach it again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionProbe {
    /// The server answered the probe; it can take a client.
    Alive,
    /// A server accepted the connection but did not answer within
    /// `PROBE_TIMEOUT` — it is running, but its main loop is not turning.
    Unresponsive,
    /// Nothing is listening on the socket.
    Absent,
}

/// Probe a session socket to check if a server is alive.
///
/// "Alive" here means *answering*, which is the property callers actually need —
/// a server that cannot reply to `ConnStatus` cannot accept a client either.
#[cfg(unix)]
fn assert_socket(name: &str) -> Result<SessionProbe, io::ErrorKind> {
    use crate::consts::ipc_connect;
    use interprocess::local_socket::prelude::*;
    let path = &*ZELLIJ_SOCK_DIR.join(name);
    let started_at = Instant::now();
    log::info!("attach trace: probe {name:?} connect begin");
    match ipc_connect(path) {
        Ok(stream) => {
            log::info!(
                "attach trace: probe {name:?} connected after {:?}; sending ConnStatus",
                started_at.elapsed()
            );
            // Best effort: a platform that will not take a timeout still gets
            // probed, it just gets the old unbounded behaviour.
            let _ = stream.set_recv_timeout(Some(PROBE_TIMEOUT));
            let _ = stream.set_send_timeout(Some(PROBE_TIMEOUT));
            let mut sender: IpcSenderWithContext<ClientToServerMsg> =
                IpcSenderWithContext::new(stream);
            let send_result = sender.send_client_msg(ClientToServerMsg::ConnStatus);
            log::info!(
                "attach trace: probe {name:?} ConnStatus send finished after {:?} ({}); waiting for reply",
                started_at.elapsed(),
                if send_result.is_ok() { "ok" } else { "error" }
            );
            let mut receiver: IpcReceiverWithContext<ServerToClientMsg> = sender.get_receiver();
            let status = classify_probe_reply(receiver.recv_server_msg());
            log::info!(
                "attach trace: probe {name:?} reply {status:?} after {:?}",
                started_at.elapsed()
            );
            Ok(status)
        },
        Err(e) => {
            let kind = e.kind();
            log::info!(
                "attach trace: probe {name:?} connect failed after {:?}: {e}",
                started_at.elapsed(),
            );
            if kind == io::ErrorKind::ConnectionRefused {
                // Nobody is listening, so the socket outlived its server: clean it up.
                // Deliberately not reachable from the timeout above — a wedged server
                // is still running, and deleting its socket would strand it with no
                // way for any client to ever find it again.
                drop(fs::remove_file(path));
            }
            classify_probe_connect_error(kind)
        },
    }
}

/// The connect itself failed; this is what that means about the session.
///
/// The distinction that carries weight is timeout versus refusal. Nothing is
/// listening behind a refusal, so the socket is stale and its name is free. A
/// timeout is the opposite: a server whose listen backlog is full is *running*
/// — it is simply too far gone to accept anyone — and treating it as absent
/// would unlink the socket of a live process, stranding it where no client
/// could ever reach it again.
pub(crate) fn classify_probe_connect_error(
    kind: io::ErrorKind,
) -> Result<SessionProbe, io::ErrorKind> {
    match kind {
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => Ok(SessionProbe::Absent),
        // `IPC_CONNECT_TIMEOUT` elapsed with the connection still unaccepted.
        // `WouldBlock` is the same condition reported under its other name: the
        // wait-mode docs name `TimedOut` in prose but link `WouldBlock`, and the
        // unix path returns `TimedOut`, so accept both rather than depend on it.
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => Ok(SessionProbe::Unresponsive),
        kind => Err(kind),
    }
}

/// The connection was made; this is what came back over it.
///
/// Everything short of the expected answer counts as unresponsive rather than
/// absent: the connect succeeded, so something is there. A timeout arrives as
/// `IpcRecv::Idle`; a hangup mid-probe (`Eof`) or a garbage frame is a server
/// in some trouble, and the safe reading of trouble is "do not build over it".
pub(crate) fn classify_probe_reply(reply: IpcRecv<ServerToClientMsg>) -> SessionProbe {
    match reply {
        IpcRecv::Msg(ServerToClientMsg::Connected, _) => SessionProbe::Alive,
        IpcRecv::Msg(..) | IpcRecv::Idle | IpcRecv::Eof | IpcRecv::Malformed => {
            SessionProbe::Unresponsive
        },
    }
}

#[cfg(not(unix))]
fn assert_socket(_name: &str) -> Result<SessionProbe, io::ErrorKind> {
    Ok(SessionProbe::Alive)
}

/// Probe one named session directly, without sweeping the socket directory.
///
/// This is the check every create and attach path should make, because it is
/// the only one that can say `Unresponsive`: `get_sessions` and everything
/// built on it list answering servers only, so through them a wedged session
/// is indistinguishable from no session at all.
pub fn session_status(name: &str) -> Result<SessionProbe, io::ErrorKind> {
    let path = ZELLIJ_SOCK_DIR.join(name);
    match fs::metadata(&path) {
        Ok(metadata) if is_ipc_socket(&metadata.file_type()) => assert_socket(name),
        Ok(_) => Ok(SessionProbe::Absent),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(SessionProbe::Absent),
        Err(e) => Err(e.kind()),
    }
}

/// What to tell a user who named a session whose server is not answering.
pub fn unresponsive_session_message(name: &str) -> String {
    format!(
        "Session {name:?} exists but its server is not responding. It may be wedged; \
         use `zellij kill-session {name}` or `zellij delete-session --force {name}` \
         before reusing the name."
    )
}

/// How a session should be described in a listing.
///
/// Three states rather than the running/exited pair this used to be, because
/// `Unresponsive` is the one a person can do nothing about until they are told
/// it exists: a wedged server holds its name and its socket but answers
/// nothing, and listing it as absent is what let one of them sit for days
/// hanging every other zellij command on the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionListing {
    /// A server that answered.
    Running,
    /// No server, but a layout on disk to resurrect from.
    Exited,
    /// A server that holds the socket but does not answer.
    Unresponsive,
}

pub fn print_sessions(
    mut sessions: Vec<(String, Duration, SessionListing)>,
    no_formatting: bool,
    short: bool,
    reverse: bool,
) {
    let curr_session = envs::get_session_name().unwrap_or_else(|_| "".into());
    sessions.sort_by(|a, b| {
        if reverse {
            // sort by `Duration` ascending (newest would be first)
            a.1.cmp(&b.1)
        } else {
            b.1.cmp(&a.1)
        }
    });
    sessions.iter().for_each(|(session_name, timestamp, kind)| {
        if short {
            println!("{}", session_name);
            return;
        }
        if no_formatting {
            let suffix = if curr_session == *session_name {
                "(current)".to_string()
            } else {
                match kind {
                    SessionListing::Exited => "(EXITED - attach to resurrect)".to_string(),
                    SessionListing::Unresponsive => format!(
                        "(UNRESPONSIVE - run `zellij kill-session {}`)",
                        session_name
                    ),
                    SessionListing::Running => String::new(),
                }
            };
            let timestamp = format!("[Created {} ago]", format_duration(*timestamp));
            println!("{} {} {}", session_name, timestamp, suffix);
        } else {
            let formatted_session_name = format!("\u{1b}[32;1m{}\u{1b}[m", session_name);
            let suffix = if curr_session == *session_name {
                "(current)".to_string()
            } else {
                match kind {
                    SessionListing::Exited => {
                        "(\u{1b}[31;1mEXITED\u{1b}[m - attach to resurrect)".to_string()
                    },
                    SessionListing::Unresponsive => format!(
                        "(\u{1b}[31;1mUNRESPONSIVE\u{1b}[m - run `zellij kill-session {}`)",
                        session_name
                    ),
                    SessionListing::Running => String::new(),
                }
            };
            let timestamp = format!(
                "[Created \u{1b}[35;1m{}\u{1b}[m ago]",
                format_duration(*timestamp)
            );
            println!("{} {} {}", formatted_session_name, timestamp, suffix);
        }
    })
}

pub fn print_sessions_with_index(sessions: Vec<String>) {
    let curr_session = envs::get_session_name().unwrap_or_else(|_| "".into());
    for (i, session) in sessions.iter().enumerate() {
        let suffix = if curr_session == *session {
            " (current)"
        } else {
            ""
        };
        println!("{}: {}{}", i, session, suffix);
    }
}

pub enum ActiveSession {
    None,
    One(String),
    Many,
}

pub fn get_active_session() -> ActiveSession {
    match get_sessions() {
        Ok(sessions) if sessions.is_empty() => ActiveSession::None,
        Ok(mut sessions) if sessions.len() == 1 => ActiveSession::One(sessions.pop().unwrap().0),
        Ok(_) => ActiveSession::Many,
        Err(e) => {
            eprintln!("Error occurred: {:?}", e);
            process::exit(1);
        },
    }
}

/// Ask a session's server to shut itself down.
///
/// Returns whether the request was delivered. It can fail for the reason that
/// makes killing worth doing in the first place: a server too wedged to accept
/// a connection cannot be told anything, and `ipc_connect` gives up on it after
/// `IPC_CONNECT_TIMEOUT`. Reporting that rather than exiting here lets the
/// caller say something useful about a server that has to be killed from
/// outside.
pub fn kill_session(name: &str) -> io::Result<()> {
    use crate::consts::ipc_connect;
    let path = &*ZELLIJ_SOCK_DIR.join(name);
    let stream = ipc_connect(path)?;
    IpcSenderWithContext::<ClientToServerMsg>::new(stream)
        .send_client_msg(ClientToServerMsg::KillSession)
        .map_err(|e| io::Error::other(format!("{e:?}")))
}

/// How long `session_left` gives a server that was told to shut down.
///
/// A few probes' worth. A server that is turning unlinks its socket within
/// milliseconds of the request, and one that is not will not do so however
/// long this waits; the margin is for a thrashing machine, as `PROBE_TIMEOUT`'s
/// is.
pub const KILL_CONFIRM_TIMEOUT: Duration = Duration::from_secs(3);

/// Whether a session's socket went away, polled for up to `within`.
///
/// For after a `KillSession` was delivered to a server a probe had already
/// found unresponsive. Delivery proves nothing there: the router that accepted
/// the frame only hands it on to the main loop, which is the loop whose
/// silence made the probe say so. The server unlinks its socket on the way
/// out, so the socket going is the shutdown having happened, and its staying
/// is the request sitting in a queue nobody is draining.
pub fn session_left(name: &str, within: Duration) -> Result<bool, io::ErrorKind> {
    let deadline = Instant::now() + within;
    loop {
        if session_status(name)? == SessionProbe::Absent {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// What to tell a person whose `kill-session` could not reach the server.
pub fn undeliverable_kill_message(name: &str) -> String {
    format!(
        "Could not reach the server for session {name:?} to ask it to shut down: \
         it is not accepting connections.\n\n{}",
        kill_from_outside_remedy(name)
    )
}

/// What to tell a person whose `kill-session` reached the server and changed
/// nothing: the request was accepted and then never acted on.
pub fn unacted_kill_message(name: &str) -> String {
    format!(
        "Session {name:?} took the request to shut down but has not acted on it: \
         its server accepts connections, but its main loop is not turning.\n\n{}",
        kill_from_outside_remedy(name)
    )
}

/// The one thing left to do about a server that cannot be told to stop.
fn kill_from_outside_remedy(name: &str) -> String {
    format!(
        "It has to be stopped from outside. Find it with:\n  \
         pgrep -af 'zellij --server .*{name}'\n\
         and kill that process id, then `zellij delete-session --force {name}` \
         to clear the name."
    )
}

pub fn delete_session(name: &str, force: bool) {
    if force {
        use crate::consts::ipc_connect;
        let path = &*ZELLIJ_SOCK_DIR.join(name);
        let _ = ipc_connect(path).ok().map(|stream| {
            IpcSenderWithContext::<ClientToServerMsg>::new(stream)
                .send_client_msg(ClientToServerMsg::KillSession)
                .ok();
        });
    }
    if let Err(e) = std::fs::remove_dir_all(session_info_folder_for_session(name)) {
        if e.kind() == std::io::ErrorKind::NotFound {
            eprintln!("Session: {:?} not found.", name);
            process::exit(2);
        } else {
            log::error!("Failed to remove session {:?}: {:?}", name, e);
        }
    } else {
        println!("Session: {:?} successfully deleted.", name);
    }
}

pub fn list_sessions(no_formatting: bool, short: bool, reverse: bool, exited: bool) {
    let exit_code = match get_sessions_with_status() {
        Ok(sessions_with_status) => {
            let mut all_sessions: HashMap<String, (Duration, SessionListing)> = if exited {
                get_resurrectable_sessions()
                    .into_iter()
                    .map(|(name, timestamp)| (name, (timestamp, SessionListing::Exited)))
                    .collect()
            } else {
                HashMap::new()
            };
            // A session with a socket overrides a resurrectable one of the same
            // name either way: it is the live thing, answering or not.
            for (session_name, duration, status) in sessions_with_status {
                let listing = match status {
                    SessionProbe::Alive => SessionListing::Running,
                    SessionProbe::Unresponsive => SessionListing::Unresponsive,
                    // Nothing behind the socket by the time it was probed —
                    // `assert_socket` has already unlinked it, so there is no
                    // session here to list.
                    SessionProbe::Absent => continue,
                };
                all_sessions.insert(session_name.clone(), (duration, listing));
            }
            if all_sessions.is_empty() {
                eprintln!("No active zellij sessions found.");
                1
            } else {
                print_sessions(
                    all_sessions
                        .iter()
                        .map(|(name, (timestamp, listing))| (name.clone(), *timestamp, *listing))
                        .collect(),
                    no_formatting,
                    short,
                    reverse,
                );
                0
            }
        },
        Err(e) => {
            eprintln!("Error occurred: {:?}", e);
            1
        },
    };
    process::exit(exit_code);
}

#[derive(Debug, Clone)]
pub enum SessionNameMatch {
    AmbiguousPrefix(Vec<String>),
    UniquePrefix(String),
    Exact(String),
    None,
}

pub fn match_session_name(prefix: &str) -> Result<SessionNameMatch, io::ErrorKind> {
    let sessions = get_sessions()?;

    let filtered_sessions: Vec<_> = sessions
        .iter()
        .filter(|s| s.0.starts_with(prefix))
        .collect();

    if filtered_sessions.iter().any(|s| s.0 == prefix) {
        return Ok(SessionNameMatch::Exact(prefix.to_string()));
    }

    Ok({
        match &filtered_sessions[..] {
            [] => SessionNameMatch::None,
            [s] => SessionNameMatch::UniquePrefix(s.0.to_string()),
            _ => SessionNameMatch::AmbiguousPrefix(
                filtered_sessions.into_iter().map(|s| s.0.clone()).collect(),
            ),
        }
    })
}

/// Whether a server by this name is up and answering.
///
/// `false` covers both an absent server and an unresponsive one; a caller about
/// to create or resurrect a session must use `session_status` and refuse on
/// `Unresponsive`, or it will spawn a second server over a live socket.
pub fn session_exists(name: &str) -> Result<bool, io::ErrorKind> {
    session_status(name).map(|status| status == SessionProbe::Alive)
}

// if the session is resurrecable, the returned layout is the one to be used to resurrect it
pub fn resurrection_layout(session_name_to_resurrect: &str) -> Result<Option<Layout>, String> {
    let checkpoint =
        SessionCheckpoint::read(&session_info_folder_for_session(session_name_to_resurrect))
            .map_err(|error| format!("{error:#}"))?;
    let layout_file_name = checkpoint.layout_path();
    let raw_layout = match std::fs::read_to_string(&layout_file_name) {
        Ok(raw_layout) => raw_layout,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!(
                "failed to read resurrection layout {}: {e}",
                layout_file_name.display()
            ))
        },
    };
    match Layout::from_kdl(
        &raw_layout,
        Some(layout_file_name.display().to_string()),
        None,
        None,
    ) {
        Ok(layout) => Ok(Some(layout)),
        Err(e) => {
            log::error!(
                "Failed to parse resurrection layout file {}: {}",
                layout_file_name.display(),
                e
            );
            return Err(format!(
                "Failed to parse resurrection layout file {}: {}.",
                layout_file_name.display(),
                e
            ));
        },
    }
}

pub fn assert_session(name: &str) {
    match session_status(name) {
        Ok(SessionProbe::Alive) => return,
        // Attaching would connect and then wait forever for a reply that is
        // not coming; say so instead.
        Ok(SessionProbe::Unresponsive) => eprintln!("{}", unresponsive_session_message(name)),
        Ok(SessionProbe::Absent) => {
            println!("No session named {:?} found.", name);
            if let Some(sugg) = get_sessions()
                .unwrap()
                .iter()
                .map(|s| s.0.clone())
                .collect::<Vec<_>>()
                .suggest(name)
            {
                println!("  help: Did you mean `{}`?", sugg);
            }
        },
        Err(e) => {
            eprintln!("Error occurred: {:?}", e);
        },
    };
    process::exit(1);
}

pub fn assert_dead_session(name: &str, force: bool) {
    // An unresponsive server is still a running one: deleting under it needs
    // `--force` just like deleting under a healthy one.
    match session_status(name).map(|status| status != SessionProbe::Absent) {
        Ok(exists) => {
            if exists && !force {
                println!(
                    "A session by the name {:?} exists and is active, use --force to delete it.",
                    name
                )
            } else if exists && force {
                println!("A session by the name {:?} exists and is active, but will be force killed and deleted.", name);
                return;
            } else {
                return;
            }
        },
        Err(e) => {
            eprintln!("Error occurred: {:?}", e);
        },
    };
    process::exit(1);
}

pub fn validate_session_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err(
            "Session name cannot be empty. Please provide a specific session name.".to_string(),
        );
    }
    if name == "." || name == ".." {
        return Err(format!("Invalid session name: \"{}\".", name));
    }
    if name.contains('/') {
        return Err("Session name cannot contain '/'.".to_string());
    }
    Ok(())
}

pub fn assert_session_ne(name: &str) {
    if let Err(e) = validate_session_name(name) {
        eprintln!("{}", e);
        process::exit(1);
    }

    match session_status(name) {
        Ok(SessionProbe::Absent) => {
            let resurrectable_sessions = get_resurrectable_session_names();
            if resurrectable_sessions.iter().find(|s| s == &name).is_some() {
                println!("Session with name {:?} already exists, but is dead. Use the attach command to resurrect it or, the delete-session command to kill it or specify a different name.", name);
            } else {
                return
            }
        }
        // Creating here would spawn a second server on the same socket path,
        // which unlinks the live server's socket out from under it.
        Ok(SessionProbe::Unresponsive) => eprintln!("{}", unresponsive_session_message(name)),
        Ok(SessionProbe::Alive) => println!("Session with name {:?} already exists. Use attach command to connect to it or specify a different name.", name),
        Err(e) => eprintln!("Error occurred: {:?}", e),
    };
    process::exit(1);
}

pub fn session_listing_error_message(kind: io::ErrorKind) -> String {
    format!(
        "Failed to list existing Zellij sessions in the socket directory:\n  {}\n\n\
         Reason: {}\n\n\
         This usually means the directory (or one of its parents) is not readable \
         by the current user - for example if $ZELLIJ_SOCKET_DIR or $XDG_RUNTIME_DIR \
         points to a directory you do not own.\n\
         To fix this, set a readable and writable socket directory, eg.:\n  \
         ZELLIJ_SOCKET_DIR=/tmp/zellij-$USER zellij",
        ZELLIJ_SOCK_DIR.display(),
        io::Error::from(kind)
    )
}

pub fn read_live_session_states(
    current_session_name: &str,
    sock_dir: &Path,
    session_info_cache_dir: &Path,
) -> BTreeMap<String, SessionInfo> {
    let mut other_session_names: Vec<(String, Duration)> = vec![];
    let mut session_infos_on_machine = BTreeMap::new();
    if let Ok(files) = fs::read_dir(sock_dir) {
        files.for_each(|file| {
            if let Ok(file) = file {
                if let Ok(file_name) = file.file_name().into_string() {
                    if file
                        .file_type()
                        .map(|file_type| is_ipc_socket(&file_type))
                        .unwrap_or(false)
                    {
                        let creation_time = std::fs::metadata(file.path())
                            .ok()
                            .and_then(|f| f.created().ok().or_else(|| f.modified().ok()))
                            .and_then(|d| d.elapsed().ok())
                            .unwrap_or_default();
                        other_session_names.push((file_name, creation_time));
                    }
                }
            }
        });
    }

    for (session_name, creation_time) in other_session_names {
        let raw_session_info = SessionCheckpoint::read(&session_info_cache_dir.join(&session_name))
            .and_then(|checkpoint| {
                fs::read_to_string(checkpoint.metadata_path())
                    .context("failed to read session checkpoint metadata")
            });
        if let Ok(raw_session_info) = raw_session_info {
            if let Ok(mut session_info) =
                SessionInfo::from_string(&raw_session_info, current_session_name)
            {
                session_info.creation_time = creation_time;
                session_infos_on_machine.insert(session_name, session_info);
            }
        }
    }
    session_infos_on_machine
}

pub fn read_live_session_states_default_dirs(
    current_session_name: &str,
) -> BTreeMap<String, SessionInfo> {
    read_live_session_states(
        current_session_name,
        &*ZELLIJ_SOCK_DIR,
        &*ZELLIJ_SESSION_INFO_CACHE_DIR,
    )
}

pub fn generate_unique_session_name() -> Option<String> {
    let sessions = get_sessions().map(|sessions| {
        sessions
            .iter()
            .map(|s| s.0.clone())
            .collect::<Vec<String>>()
    });
    let dead_sessions = get_resurrectable_session_names();
    let sessions = match sessions {
        Ok(sessions) => sessions,
        Err(kind) => {
            eprintln!("{}", session_listing_error_message(kind));
            return None;
        },
    };

    let name = get_name_generator()
        .take(1000)
        .find(|name| !sessions.contains(name) && !dead_sessions.contains(name));

    {
        let name = name?;
        return Some(name);
    }
}

/// Create a new random name generator
///
/// Used to provide a memorable handle for a session when users don't specify a session name when the session is
/// created.
///
/// Uses the list of adjectives and nouns defined below, with the intention of avoiding unfortunate
/// and offensive combinations. Care should be taken when adding or removing to either list due to the birthday paradox/
/// hash collisions, e.g. with 4096 unique names, the likelihood of a collision in 10 session names is 1%.
pub fn get_name_generator() -> impl Iterator<Item = String> {
    names::Generator::new(ADJECTIVES, NOUNS, names::Name::Plain)
}

/// Generates a random human-readable name using curated adjectives and nouns.
/// Returns a single name in the format: AdjectiveNoun (e.g., "BraveRustacean")
pub fn generate_random_name() -> String {
    get_name_generator().next().unwrap()
}

const ADJECTIVES: &[&str] = &[
    "adamant",
    "adept",
    "adventurous",
    "arcadian",
    "auspicious",
    "awesome",
    "blossoming",
    "brave",
    "charming",
    "chatty",
    "circular",
    "considerate",
    "cubic",
    "curious",
    "delighted",
    "didactic",
    "diligent",
    "effulgent",
    "erudite",
    "excellent",
    "exquisite",
    "fabulous",
    "fascinating",
    "friendly",
    "glowing",
    "gracious",
    "gregarious",
    "hopeful",
    "implacable",
    "inventive",
    "joyous",
    "judicious",
    "jumping",
    "kind",
    "likable",
    "loyal",
    "lucky",
    "marvellous",
    "mellifluous",
    "nautical",
    "oblong",
    "outstanding",
    "polished",
    "polite",
    "profound",
    "quadratic",
    "quiet",
    "rectangular",
    "remarkable",
    "rusty",
    "sensible",
    "sincere",
    "sparkling",
    "splendid",
    "stellar",
    "tenacious",
    "tremendous",
    "triangular",
    "undulating",
    "unflappable",
    "unique",
    "verdant",
    "vitreous",
    "wise",
    "zippy",
];

const NOUNS: &[&str] = &[
    "aardvark",
    "accordion",
    "apple",
    "apricot",
    "bee",
    "brachiosaur",
    "cactus",
    "capsicum",
    "clarinet",
    "cowbell",
    "crab",
    "cuckoo",
    "cymbal",
    "diplodocus",
    "donkey",
    "drum",
    "duck",
    "echidna",
    "elephant",
    "foxglove",
    "galaxy",
    "glockenspiel",
    "goose",
    "hill",
    "horse",
    "iguanodon",
    "jellyfish",
    "kangaroo",
    "lake",
    "lemon",
    "lemur",
    "magpie",
    "megalodon",
    "mountain",
    "mouse",
    "muskrat",
    "newt",
    "oboe",
    "ocelot",
    "orange",
    "panda",
    "peach",
    "pepper",
    "petunia",
    "pheasant",
    "piano",
    "pigeon",
    "platypus",
    "quasar",
    "rhinoceros",
    "river",
    "rustacean",
    "salamander",
    "sitar",
    "stegosaurus",
    "tambourine",
    "tiger",
    "tomato",
    "triceratops",
    "ukulele",
    "viola",
    "weasel",
    "xylophone",
    "yak",
    "zebra",
];
