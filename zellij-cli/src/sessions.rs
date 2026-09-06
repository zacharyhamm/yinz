//! Session listings and command diagnostics.
use humantime::format_duration;
use std::{collections::HashMap, io, process, time::Duration};
use suggest::Suggest;
use zellij_ipc::probe::SessionProbe;
use zellij_runtime::{consts::ZELLIJ_SOCK_DIR, envs};
use zellij_session::sessions::*;
/// What to tell a user who named a session whose server is not answering.
pub fn unresponsive_session_message(name: &str) -> String {
    format!(
        "Session {name:?} exists but its server is not responding. It may be wedged; \
         use `zellij kill-session {name}` or `zellij delete-session --force {name}` \
         before reusing the name."
    )
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
pub fn delete_session(name: &str, force: bool) {
    if let Err(e) = zellij_session::sessions::delete_session(name, force) {
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
pub fn generate_unique_session_name() -> Option<String> {
    match zellij_session::sessions::generate_unique_session_name() {
        Ok(name) => name,
        Err(kind) => {
            eprintln!("{}", session_listing_error_message(kind));
            None
        },
    }
}
pub enum ActiveSession {
    None,
    One(String),
    Many,
}
