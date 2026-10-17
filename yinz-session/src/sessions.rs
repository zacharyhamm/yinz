use anyhow;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};
use std::{fs, io};
use yinz_ipc::probe::SessionProbe;
use yinz_ipc::probe::{classify_probe_connect_error, classify_probe_reply};
use yinz_runtime::session_checkpoint::SessionCheckpoint;
use yinz_types::internal_ids::SessionName;
use {
    yinz_ipc::consts::is_ipc_socket, yinz_ipc::consts::is_session_socket_name,
    yinz_ipc::ipc::ClientToServerMsg, yinz_ipc::ipc::IpcReceiverWithContext,
    yinz_ipc::ipc::IpcSenderWithContext, yinz_ipc::ipc::ServerToClientMsg,
    yinz_runtime::consts::session_info_folder_for_session,
    yinz_runtime::consts::YINZ_SESSION_INFO_CACHE_DIR, yinz_runtime::consts::YINZ_SOCK_DIR,
    yinz_types::data::SessionInfo, yinz_types::input::layout::Layout,
};
use {yinz_runtime::errors::prelude::Context, yinz_runtime::errors::prelude::LoggableError};

/// Probe every candidate socket at once and keep the ones that answered.
///
/// Serially, the cost of enumeration is the number of sessions times
/// `PROBE_TIMEOUT` in the worst case — on a machine with a dozen long-lived
/// sessions that is seconds of latency on every `yinz ls`, and it grows with
/// the number of sessions a user happens to have left running. Probes are
/// independent and spend all their time blocked on a socket, so the whole set
/// costs one timeout instead of N.
///
/// Order is not preserved; both callers below impose their own.
fn probe_sockets<T: Send + 'static>(candidates: Vec<(SessionName, T)>) -> Vec<(SessionName, T)> {
    // Only a server that answers is listed. An unresponsive one is not offered
    // for attaching — attaching to it would hang — but it is not forgotten
    // either: `session_status` still reports it, so nothing creates a new
    // session over its socket, and `probe_sockets_with_status` below still
    // shows it to a person running `yinz ls`.
    probe_sockets_with_status(candidates)
        .into_iter()
        .filter(|(_, _, status)| matches!(status, SessionProbe::Alive))
        .map(|(name, extra, _)| (name, extra))
        .collect()
}

/// The same sweep, keeping what each probe found instead of discarding it.
///
/// `yinz ls` and `yinz kill-all-sessions` want this: listing must show a
/// wedged session, and killing must still attempt it. Other callers are choosing
/// a session to attach to or a name to create, where "answered" is the only
/// useful answer.
fn probe_sockets_with_status<T: Send + 'static>(
    candidates: Vec<(SessionName, T)>,
) -> Vec<(SessionName, T, SessionProbe)> {
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
fn session_socket_candidates() -> Result<Vec<(SessionName, Duration)>, io::ErrorKind> {
    session_socket_candidates_in(&YINZ_SOCK_DIR)
}

fn session_socket_candidates_in(
    sock_dir: &Path,
) -> Result<Vec<(SessionName, Duration)>, io::ErrorKind> {
    match fs::read_dir(sock_dir) {
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
                    if is_ipc_socket(&file.file_type().unwrap())
                        && is_session_socket_name(&file_name)
                    {
                        candidates.push((SessionName::new(file_name), duration));
                    }
                }
            });
            Ok(candidates)
        },
        Err(err) if io::ErrorKind::NotFound != err.kind() => Err(err.kind()),
        Err(_) => Ok(Vec::with_capacity(0)),
    }
}

pub fn get_sessions() -> Result<Vec<(SessionName, Duration)>, io::ErrorKind> {
    Ok(probe_sockets(session_socket_candidates()?))
}

/// Every session with a socket, answering or not.
///
/// Deliberately separate from `get_sessions` rather than a widening of it: the
/// callers of that one are picking a session to attach to or a name to create
/// over, and handing them a wedged session would have them hang on it or build
/// on top of a live server. Listing and kill-all are the callers that need the
/// whole picture.
pub fn get_sessions_with_status(
) -> Result<Vec<(SessionName, Duration, SessionProbe)>, io::ErrorKind> {
    Ok(probe_sockets_with_status(session_socket_candidates()?))
}

pub fn get_resurrectable_sessions() -> Vec<(SessionName, Duration)> {
    match fs::read_dir(&*YINZ_SESSION_INFO_CACHE_DIR) {
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
                        Some((SessionName::new(session_name), elapsed_duration))
                    } else {
                        None
                    }
                })
                .collect()
        },
        Err(e) => {
            log::error!(
                "Failed to read session_info cache folder: \"{:?}\": {:?}",
                *YINZ_SESSION_INFO_CACHE_DIR,
                e
            );
            vec![]
        },
    }
}

pub fn get_resurrectable_session_names() -> Vec<SessionName> {
    match fs::read_dir(&*YINZ_SESSION_INFO_CACHE_DIR) {
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
                            .map(|f| SessionName::new(f.to_string_lossy()))
                    } else {
                        None
                    }
                })
                .collect()
        },
        Err(e) => {
            log::error!(
                "Failed to read session_info cache folder: \"{:?}\": {:?}",
                *YINZ_SESSION_INFO_CACHE_DIR,
                e
            );
            vec![]
        },
    }
}

pub fn get_sessions_sorted_by_mtime() -> anyhow::Result<Vec<SessionName>> {
    match fs::read_dir(&*YINZ_SOCK_DIR) {
        Ok(files) => {
            let mut candidates: Vec<(SessionName, SystemTime)> = Vec::new();
            for file in files {
                let file = file?;
                let file_name = file.file_name().into_string().unwrap();
                let file_modified_at = file.metadata()?.modified()?;
                if is_ipc_socket(&file.file_type()?) && is_session_socket_name(&file_name) {
                    candidates.push((SessionName::new(file_name), file_modified_at));
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
/// enumeration path funnels through it, one wedged session hangs `yinz`,
/// `yinz attach`, `yinz ls` and `yinz action` — for unrelated sessions
/// too, and for a brand new session that has nothing to do with any of them.
///
/// Generous rather than tight: this is a loopback socket answering a constant,
/// so a healthy server replies in well under a millisecond even under load. The
/// margin is for a machine thrashing, not for a slow reply.
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// Probe a session socket to check if a server is alive.
///
/// "Alive" here means *answering*, which is the property callers actually need —
/// a server that cannot reply to `ConnStatus` cannot accept a client either.
#[cfg(unix)]
fn assert_socket(name: &SessionName) -> Result<SessionProbe, io::ErrorKind> {
    use interprocess::local_socket::prelude::*;
    use yinz_ipc::consts::ipc_connect;
    let path = &*YINZ_SOCK_DIR.join(name);
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
            let mut receiver: IpcReceiverWithContext<ServerToClientMsg> =
                sender.get_receiver().map_err(|error| error.kind())?;
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
            classify_probe_connect_error(kind)
        },
    }
}

#[cfg(not(unix))]
fn assert_socket(_name: &SessionName) -> Result<SessionProbe, io::ErrorKind> {
    Ok(SessionProbe::Alive)
}

/// Probe one named session directly, without sweeping the socket directory.
///
/// This is the check every create and attach path should make, because it is
/// the only one that can say `Unresponsive`: `get_sessions` and everything
/// built on it list answering servers only, so through them a wedged session
/// is indistinguishable from no session at all.
pub fn session_status(name: &SessionName) -> Result<SessionProbe, io::ErrorKind> {
    let path = YINZ_SOCK_DIR.join(name);
    match fs::metadata(&path) {
        Ok(metadata) if is_ipc_socket(&metadata.file_type()) => assert_socket(name),
        Ok(_) => Ok(SessionProbe::Absent),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(SessionProbe::Absent),
        Err(e) => Err(e.kind()),
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
pub fn kill_session(name: &SessionName) -> io::Result<()> {
    use yinz_ipc::consts::ipc_connect;
    let path = &*YINZ_SOCK_DIR.join(name);
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
pub fn session_left(name: &SessionName, within: Duration) -> Result<bool, io::ErrorKind> {
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

pub fn delete_session(name: &SessionName, force: bool) -> io::Result<()> {
    let folder = session_info_folder_for_session(name);
    if force {
        delete_session_after_shutdown(&folder, session_status(name), || {
            yinz_ipc::ipc::kill_session_with_completion(&YINZ_SOCK_DIR.join(name))
        })
    } else {
        fs::remove_dir_all(folder)
    }
}

fn delete_session_after_shutdown(
    folder: &Path,
    status: Result<SessionProbe, io::ErrorKind>,
    shutdown: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    match status? {
        SessionProbe::Absent => {},
        SessionProbe::Alive | SessionProbe::Unresponsive => {
            shutdown().map_err(|error| {
                io::Error::other(format!("session shutdown was not confirmed: {error}"))
            })?;
        },
        SessionProbe::Denied => {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "cannot confirm session shutdown: probing is not permitted",
            ));
        },
    }
    fs::remove_dir_all(folder)
}

#[derive(Debug, Clone)]
pub enum SessionNameMatch {
    AmbiguousPrefix(Vec<SessionName>),
    UniquePrefix(SessionName),
    Exact(SessionName),
    None,
}

pub fn match_session_name(prefix: &str) -> Result<SessionNameMatch, io::ErrorKind> {
    let sessions = get_sessions()?;

    let filtered_sessions: Vec<_> = sessions
        .iter()
        .filter(|s| s.0.as_str().starts_with(prefix))
        .collect();

    if filtered_sessions.iter().any(|s| s.0.as_str() == prefix) {
        return Ok(SessionNameMatch::Exact(prefix.into()));
    }

    Ok({
        match &filtered_sessions[..] {
            [] => SessionNameMatch::None,
            [s] => SessionNameMatch::UniquePrefix(s.0.clone()),
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
pub fn session_exists(name: &SessionName) -> Result<bool, io::ErrorKind> {
    session_status(name).map(|status| status == SessionProbe::Alive)
}

// if the session is resurrecable, the returned layout is the one to be used to resurrect it
pub fn resurrection_layout(
    session_name_to_resurrect: &SessionName,
) -> Result<Option<Layout>, String> {
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
    match yinz_config::kdl::layout::from_kdl(
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
            Err(format!(
                "Failed to parse resurrection layout file {}: {}.",
                layout_file_name.display(),
                e
            ))
        },
    }
}

pub fn validate_session_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err(
            "Session name cannot be empty. Please provide a specific session name.".to_string(),
        );
    }
    if name == "." || name == ".." || name == yinz_ipc::consts::SESSION_OWNERSHIP_DIR {
        return Err(format!("Invalid session name: \"{}\".", name));
    }
    if name.contains('/') {
        return Err("Session name cannot contain '/'.".to_string());
    }
    if !is_session_socket_name(name) {
        return Err(format!(
            "\"{name}\" is the pty broker's socket, not a session. It keeps panes alive \
             across server restarts; do not kill or delete it."
        ));
    }
    Ok(())
}

pub fn read_live_session_states(
    current_session_name: &SessionName,
    sock_dir: &Path,
    session_info_cache_dir: &Path,
) -> BTreeMap<SessionName, SessionInfo> {
    let mut other_session_names: Vec<(SessionName, Duration)> = vec![];
    let mut session_infos_on_machine = BTreeMap::new();
    if let Ok(files) = fs::read_dir(sock_dir) {
        files.for_each(|file| {
            if let Ok(file) = file {
                if let Ok(file_name) = file.file_name().into_string() {
                    if is_session_socket_name(&file_name)
                        && file
                            .file_type()
                            .map(|file_type| is_ipc_socket(&file_type))
                            .unwrap_or(false)
                    {
                        let creation_time = std::fs::metadata(file.path())
                            .ok()
                            .and_then(|f| f.created().ok().or_else(|| f.modified().ok()))
                            .and_then(|d| d.elapsed().ok())
                            .unwrap_or_default();
                        other_session_names.push((SessionName::new(file_name), creation_time));
                    }
                }
            }
        });
    }

    // Start every probe before joining so BSD stale-socket retries overlap.
    let probes: Vec<_> = other_session_names
        .into_iter()
        .map(|(session_name, creation_time)| {
            let socket_path = sock_dir.join(&session_name);
            std::thread::spawn(move || {
                if let Err(error) = yinz_ipc::consts::ipc_connect(&socket_path) {
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                    ) {
                        return None;
                    }
                }
                Some((session_name, creation_time))
            })
        })
        .collect();
    for (session_name, creation_time) in probes
        .into_iter()
        .filter_map(|probe| probe.join().ok().flatten())
    {
        let raw_session_info = SessionCheckpoint::read(&session_info_cache_dir.join(&session_name))
            .and_then(|checkpoint| {
                fs::read_to_string(checkpoint.metadata_path())
                    .context("failed to read session checkpoint metadata")
            });
        if let Ok(raw_session_info) = raw_session_info {
            if let Ok(mut session_info) =
                crate::metadata::session_info::from_string(&raw_session_info, current_session_name)
            {
                session_info.creation_time = creation_time;
                session_infos_on_machine.insert(session_name, session_info);
            }
        }
    }
    session_infos_on_machine
}

pub fn read_live_session_states_default_dirs(
    current_session_name: &SessionName,
) -> BTreeMap<SessionName, SessionInfo> {
    read_live_session_states(
        current_session_name,
        &YINZ_SOCK_DIR,
        &YINZ_SESSION_INFO_CACHE_DIR,
    )
}

pub fn generate_unique_session_name() -> Result<Option<SessionName>, io::ErrorKind> {
    use yinz_runtime::logging::startup_stage;
    let sessions = startup_stage("session.probe_existing", get_sessions).map(|sessions| {
        sessions
            .iter()
            .map(|s| s.0.clone())
            .collect::<Vec<SessionName>>()
    });
    let dead_sessions = startup_stage("session.read_checkpoints", get_resurrectable_session_names);
    let sessions = sessions?;

    let name = get_name_generator()
        .take(1000)
        .map(SessionName::new)
        .find(|name| !sessions.contains(name) && !dead_sessions.contains(name));

    {
        Ok(name)
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

#[cfg(all(test, unix))]
#[path = "unit/sessions_tests.rs"]
mod tests;
