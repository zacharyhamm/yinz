use dialoguer::Confirm;
use std::{
    path::{Path, PathBuf},
    process,
    time::Duration,
};

use zellij_client::{
    os_input_output::get_client_os_input, start_client as start_client_impl, ClientInfo,
};

use zellij_utils::sessions::{
    assert_dead_session, assert_session_ne, delete_session as delete_session_impl,
    generate_unique_session_name, get_active_session, get_resurrectable_sessions, get_sessions,
    get_sessions_sorted_by_mtime, get_sessions_with_status, kill_session as kill_session_impl,
    match_session_name, print_sessions, print_sessions_with_index, resurrection_layout,
    session_left, session_listing_error_message, session_status, unacted_kill_message,
    undeliverable_kill_message, unresponsive_session_message, validate_session_name, ActiveSession,
    SessionListing, SessionNameMatch, SessionProbe, KILL_CONFIRM_TIMEOUT,
};

use zellij_utils::consts::session_layout_cache_file_name;

use miette::{Report, Result};
use zellij_server::{os_input_output::get_server_os_input, start_server as start_server_impl};
use zellij_utils::{
    cli::{CliArgs, Command, SessionCommand, Sessions},
    data::ConnectToSession,
    envs,
    input::{
        actions::Action,
        config::{Config, ConfigError},
        options::Options,
    },
    setup::Setup,
};

pub(crate) use zellij_utils::sessions::list_sessions;

fn sessions_to_kill(
    get_sessions: impl FnOnce() -> Result<Vec<(String, Duration, SessionProbe)>, std::io::ErrorKind>,
) -> Result<Vec<(String, Duration)>, std::io::ErrorKind> {
    Ok(get_sessions()?
        .into_iter()
        .filter_map(|(name, age, status)| {
            (!matches!(status, SessionProbe::Absent)).then_some((name, age))
        })
        .collect())
}

pub(crate) fn firecracker(
    command: zellij_utils::cli::Firecracker,
    config: &Config,
) -> zellij_utils::errors::prelude::Result<()> {
    let provider = zellij_server::firecracker::provider_from_options(
        config.options.firecracker_provider.as_deref(),
        config.options.firecracker_provider_timeout_secs,
    )?;
    run_firecracker(
        command,
        &provider,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    )
}

/// The session a `zellij firecracker` command acts on: `--session` if given, else the
/// session this command runs inside, else the only active one.
pub(crate) fn resolve_firecracker_session(
    explicit: Option<String>,
    env_session: Option<String>,
    active: ActiveSession,
) -> zellij_utils::errors::prelude::Result<String> {
    use zellij_utils::errors::prelude::*;

    explicit
        .or(env_session)
        .or(match active {
            ActiveSession::One(name) => Some(name),
            ActiveSession::None | ActiveSession::Many => None,
        })
        .context("no session selected; pass --session SESSION")
}

/// Run one `zellij firecracker` command against `provider`, writing the listing to
/// `out` and each merge warning to `err`.
pub(crate) fn run_firecracker(
    command: zellij_utils::cli::Firecracker,
    provider: &zellij_server::firecracker::Provider,
    out: &mut impl std::io::Write,
    err: &mut impl std::io::Write,
) -> zellij_utils::errors::prelude::Result<()> {
    use zellij_utils::cli::Firecracker;

    fn selected_session(explicit: Option<String>) -> zellij_utils::errors::prelude::Result<String> {
        // The active-session scan is only worth doing without an explicit choice.
        match explicit {
            Some(session) => Ok(session),
            None => resolve_firecracker_session(
                None,
                envs::get_session_name().ok(),
                get_active_session(),
            ),
        }
    }

    match command {
        Firecracker::List { session, json } => {
            let listing = provider.list(session.as_deref())?;
            if json {
                writeln!(out, "{}", serde_json::to_string(&listing)?)?;
            } else {
                if listing.vms.is_empty() {
                    writeln!(out, "No Firecracker VMs found.")?;
                }
                for entry in &listing.vms {
                    writeln!(out, "{}\t{}\t{}", entry.session, entry.name, entry.state)?;
                }
                if !listing.recoveries.is_empty() {
                    writeln!(out, "Retained disks:")?;
                }
                for recovery in &listing.recoveries {
                    writeln!(
                        out,
                        "{}\t{}\t{}\t{}",
                        recovery.session.as_deref().unwrap_or("-"),
                        recovery.name.as_deref().unwrap_or("-"),
                        recovery.reason,
                        recovery.path.display()
                    )?;
                }
            }
        },
        Firecracker::Stop { name, session } => {
            let session = selected_session(session)?;
            for warning in provider.stop(&session, &name)? {
                writeln!(err, "warning: {warning}")?;
            }
        },
        Firecracker::Sync { name, session } => {
            let session = selected_session(session)?;
            for warning in provider.sync(&session, &name)? {
                writeln!(err, "warning: {warning}")?;
            }
        },
    }
    Ok(())
}

pub(crate) fn kill_all_sessions(yes: bool) {
    match sessions_to_kill(get_sessions_with_status) {
        Ok(sessions) if sessions.is_empty() => {
            eprintln!("No active zellij sessions found.");
            process::exit(1);
        },
        Ok(sessions) => {
            if !yes {
                println!("WARNING: this action will kill all sessions.");
                if !Confirm::new()
                    .with_prompt("Do you want to continue?")
                    .interact()
                    .unwrap()
                {
                    println!("Abort.");
                    process::exit(1);
                }
            }
            let mut all_delivered = true;
            for session in &sessions {
                if let Err(e) = kill_session_impl(&session.0) {
                    // One unreachable server must not stop the rest being
                    // killed, but it must not be reported as killed either.
                    log::error!("failed to kill session {:?}: {e}", session.0);
                    eprintln!("{}", undeliverable_kill_message(&session.0));
                    all_delivered = false;
                }
            }
            process::exit(if all_delivered { 0 } else { 1 });
        },
        Err(e) => {
            eprintln!("Error occurred: {:?}", e);
            process::exit(1);
        },
    }
}

pub(crate) fn delete_all_sessions(yes: bool, force: bool) {
    use std::collections::BTreeMap;
    use zellij_server::background_jobs::scan_session_list_default_dirs;

    let (_live_sessions, resurrectable_map) =
        scan_session_list_default_dirs("", &[], &BTreeMap::new());
    let mut resurrectable_sessions: Vec<(String, Duration)> =
        resurrectable_map.into_iter().collect();
    if force {
        for entry in get_resurrectable_sessions() {
            if !resurrectable_sessions.iter().any(|(n, _)| *n == entry.0) {
                resurrectable_sessions.push(entry);
            }
        }
    }
    let dead_sessions = match sessions_to_delete(force, resurrectable_sessions, get_sessions) {
        Ok(dead_sessions) => dead_sessions,
        Err(kind) => {
            eprintln!("{}", session_listing_error_message(kind));
            process::exit(1);
        },
    };
    if !yes {
        println!("WARNING: this action will delete all resurrectable sessions.");
        if !Confirm::new()
            .with_prompt("Do you want to continue?")
            .interact()
            .unwrap()
        {
            println!("Abort.");
            process::exit(1);
        }
    }
    for session in &dead_sessions {
        delete_session_impl(&session.0, force);
    }
    process::exit(0);
}

fn sessions_to_delete(
    force: bool,
    resurrectable_sessions: Vec<(String, Duration)>,
    get_active_sessions: impl FnOnce() -> Result<Vec<(String, Duration)>, std::io::ErrorKind>,
) -> Result<Vec<(String, Duration)>, std::io::ErrorKind> {
    if force {
        return Ok(resurrectable_sessions);
    }
    let active_sessions: Vec<String> = get_active_sessions()?
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    Ok(resurrectable_sessions
        .into_iter()
        .filter(|(name, _)| !active_sessions.contains(name))
        .collect())
}

/// What `kill-session` found, apart from what it says about it, so that the
/// decision can be tested without a server.
#[derive(Debug)]
enum KillOutcome {
    /// The server is gone, or was answering and took the request.
    Killed,
    Absent,
    /// The server accepted no connection; the request never reached it.
    Undeliverable(std::io::Error),
    /// The request reached a server already known not to be turning, and its
    /// socket is still there: it was queued for a loop that is not draining.
    Unacted,
    ListingError(std::io::ErrorKind),
}

/// Kill a session, and say only what is known to have happened.
///
/// Deliberately not `assert_session`, which refuses an unresponsive session
/// and tells the caller to run `kill-session` — the command they are already
/// running. A wedged server is the one this command most needs to work on, so
/// the request is sent regardless. But delivering it to such a server proves
/// nothing: the router accepts the frame and hands it to the main loop, which
/// is the loop the probe found silent. So for an unresponsive server the
/// socket has to be seen to go, `left`, before the kill is reported done.
fn kill_outcome(
    probe: Result<SessionProbe, std::io::ErrorKind>,
    deliver: impl FnOnce() -> std::io::Result<()>,
    left: impl FnOnce() -> Result<bool, std::io::ErrorKind>,
) -> KillOutcome {
    let probe = match probe {
        Ok(SessionProbe::Absent) => return KillOutcome::Absent,
        Ok(probe) => probe,
        Err(kind) => return KillOutcome::ListingError(kind),
    };
    if let Err(e) = deliver() {
        return KillOutcome::Undeliverable(e);
    }
    if probe == SessionProbe::Alive {
        return KillOutcome::Killed;
    }
    match left() {
        Ok(true) => KillOutcome::Killed,
        Ok(false) => KillOutcome::Unacted,
        Err(kind) => KillOutcome::ListingError(kind),
    }
}

pub(crate) fn kill_session(target_session: &Option<String>) {
    match target_session {
        Some(target_session) => {
            let outcome = kill_outcome(
                session_status(target_session),
                || kill_session_impl(target_session),
                || session_left(target_session, KILL_CONFIRM_TIMEOUT),
            );
            match outcome {
                KillOutcome::Killed => process::exit(0),
                KillOutcome::Absent => {
                    println!("No session named {:?} found.", target_session);
                    process::exit(1);
                },
                KillOutcome::Undeliverable(e) => {
                    log::error!("failed to kill session {target_session:?}: {e}");
                    eprintln!("{}", undeliverable_kill_message(target_session));
                    process::exit(1);
                },
                KillOutcome::Unacted => {
                    log::error!(
                        "session {target_session:?} took the kill request but its socket is still there"
                    );
                    eprintln!("{}", unacted_kill_message(target_session));
                    process::exit(1);
                },
                KillOutcome::ListingError(kind) => {
                    eprintln!("{}", session_listing_error_message(kind));
                    process::exit(1);
                },
            }
        },
        None => {
            println!("Please specify the session name to kill.");
            process::exit(1);
        },
    }
}

pub(crate) fn delete_session(target_session: &Option<String>, force: bool) {
    match target_session {
        Some(target_session) => {
            if let Err(e) = validate_session_name(target_session) {
                eprintln!("{}", e);
                process::exit(1);
            }
            assert_dead_session(target_session, force);
            delete_session_impl(target_session, force);
            process::exit(0);
        },
        None => {
            println!("Please specify the session name to delete.");
            process::exit(1);
        },
    }
}

fn get_os_input<OsInputOutput>(
    fn_get_os_input: fn() -> Result<OsInputOutput, std::io::Error>,
) -> OsInputOutput {
    match fn_get_os_input() {
        Ok(os_input) => os_input,
        Err(e) => {
            eprintln!("failed to open terminal:\n{}", e);
            process::exit(1);
        },
    }
}

pub(crate) fn start_server(path: PathBuf, debug: bool) {
    // Set instance-wide debug mode
    zellij_utils::consts::DEBUG_MODE.set(debug).unwrap();
    let os_input = get_os_input(get_server_os_input);
    start_server_impl(Box::new(os_input), path);
}

fn create_new_client() -> ClientInfo {
    ClientInfo::New(generate_unique_session_name_or_exit(), None, None)
}

fn find_indexed_session(
    sessions: Vec<String>,
    config_options: Options,
    index: usize,
    create: bool,
) -> ClientInfo {
    match sessions.get(index) {
        Some(session) => ClientInfo::Attach(session.clone(), config_options),
        None if create => create_new_client(),
        None => {
            println!(
                "No session indexed by {} found. The following sessions are active:",
                index
            );
            print_sessions_with_index(sessions);
            process::exit(1);
        },
    }
}

/// Client entrypoint for all [`zellij_utils::cli::CliAction`]
///
/// Checks session to send the action to and attaches with client
pub(crate) fn send_action_to_session(
    mut cli_action: zellij_utils::cli::CliAction,
    mut requested_session_name: Option<String>,
    config: Option<Config>,
) {
    if let Err(e) = retarget_implicit_agent_report(
        &mut cli_action,
        &mut requested_session_name,
        std::env::var(zellij_server::pty_host::PANE_REF_ENV_KEY).ok(),
        |pane_ref| {
            zellij_server::pty_host::client::query(&zellij_server::pty_host::protocol::socket_path(
                pane_ref,
            ))
            .map_err(|e| format!("could not resolve pane {pane_ref}: {e:#}"))
        },
    ) {
        eprintln!("{e}");
        std::process::exit(1);
    }
    match get_active_session() {
        ActiveSession::None => {
            eprintln!("There is no active session!");
            std::process::exit(1);
        },
        ActiveSession::One(session_name) => {
            if let Some(requested_session_name) = requested_session_name {
                if requested_session_name != session_name {
                    eprintln!(
                        "Session '{}' not found. The following sessions are active:",
                        requested_session_name
                    );
                    eprintln!("{}", session_name);
                    std::process::exit(1);
                }
            }
            attach_with_cli_client(cli_action, &session_name, config);
        },
        ActiveSession::Many => {
            let existing_sessions: Vec<String> = get_sessions()
                .unwrap_or_default()
                .iter()
                .map(|s| s.0.clone())
                .collect();
            if let Some(session_name) = requested_session_name {
                if existing_sessions.contains(&session_name) {
                    attach_with_cli_client(cli_action, &session_name, config);
                } else {
                    eprintln!(
                        "Session '{}' not found. The following sessions are active:",
                        session_name
                    );
                    list_sessions(false, false, true, false);
                    std::process::exit(1);
                }
            } else if let Ok(session_name) = envs::get_session_name() {
                attach_with_cli_client(cli_action, &session_name, config);
            } else {
                eprintln!("Please specify the session name to send actions to. The following sessions are active:");
                list_sessions(false, false, true, false);
                std::process::exit(1);
            }
        },
    };
}

/// Resolve the stable supervisor reference before choosing which session receives a report.
///
/// A warped process keeps the session name and terminal id it was born with in its environment,
/// while its supervisor is re-keyed to the adopting session and id. Explicit CLI targets still
/// win; this only replaces the implicit environment-based route used by agent hooks.
fn retarget_implicit_agent_report(
    cli_action: &mut zellij_utils::cli::CliAction,
    requested_session_name: &mut Option<String>,
    pane_ref: Option<String>,
    query: impl FnOnce(
        &str,
    )
        -> std::result::Result<zellij_server::pty_host::protocol::HostStatus, String>,
) -> std::result::Result<(), String> {
    let zellij_utils::cli::CliAction::AgentReport {
        pane_id: pane_id @ None,
        ..
    } = cli_action
    else {
        return Ok(());
    };
    if requested_session_name.is_some() {
        return Ok(());
    }
    let Some(pane_ref) = pane_ref else {
        return Ok(());
    };
    if pane_ref.len() != 12 || !pane_ref.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(
            "invalid {}",
            zellij_server::pty_host::PANE_REF_ENV_KEY
        ));
    }

    let status = query(&pane_ref)?;
    if status.host_id != pane_ref {
        return Err(format!(
            "pane reference {pane_ref} resolved to supervisor {}",
            status.host_id
        ));
    }
    if !status.attached {
        return Err(format!("pane {pane_ref} is currently between sessions"));
    }
    let session_name = status
        .session_name
        .ok_or_else(|| format!("pane {pane_ref} has no owning session"))?;
    *pane_id = Some(status.terminal_id.to_string());
    *requested_session_name = Some(session_name);
    Ok(())
}
/// The session a `zellij subscribe` / `zellij events` client attaches to: the one running, the
/// one asked for, or the one this shell is inside. Anything else is an error naming the choices,
/// with `purpose` finishing the sentence ("subscribe to", "stream events from").
fn resolve_session_for_stream(requested_session_name: Option<String>, purpose: &str) -> String {
    match get_active_session() {
        ActiveSession::None => {
            eprintln!("There is no active session!");
            std::process::exit(1);
        },
        ActiveSession::One(session_name) => {
            if let Some(ref requested) = requested_session_name {
                if *requested != session_name {
                    eprintln!(
                        "Session '{}' not found. The following sessions are active:",
                        requested
                    );
                    eprintln!("{}", session_name);
                    std::process::exit(1);
                }
            }
            session_name
        },
        ActiveSession::Many => {
            let existing_sessions: Vec<String> = get_sessions()
                .unwrap_or_default()
                .iter()
                .map(|s| s.0.clone())
                .collect();
            if let Some(session_name) = requested_session_name {
                if existing_sessions.contains(&session_name) {
                    session_name
                } else {
                    eprintln!(
                        "Session '{}' not found. The following sessions are active:",
                        session_name
                    );
                    list_sessions(false, false, true, false);
                    std::process::exit(1);
                }
            } else if let Ok(session_name) = envs::get_session_name() {
                session_name
            } else {
                eprintln!(
                    "Please specify the session name to {purpose}. The following sessions are \
                     active:"
                );
                list_sessions(false, false, true, false);
                std::process::exit(1);
            }
        },
    }
}

pub(crate) fn subscribe_to_session(
    subscribe_cli: zellij_utils::cli::SubscribeCli,
    requested_session_name: Option<String>,
    _config: Option<Config>,
) {
    let session_name = resolve_session_for_stream(requested_session_name, "subscribe to");
    let os_input = get_os_input(zellij_client::os_input_output::get_cli_client_os_input);
    zellij_client::cli_client::start_subscribe_client(
        Box::new(os_input),
        &session_name,
        subscribe_cli,
    );
}

pub(crate) fn stream_session_events(
    events_cli: zellij_utils::cli::EventsCli,
    requested_session_name: Option<String>,
    _config: Option<Config>,
) {
    let session_name = resolve_session_for_stream(requested_session_name, "stream events from");
    let os_input = get_os_input(zellij_client::os_input_output::get_cli_client_os_input);
    zellij_client::cli_client::start_events_client(Box::new(os_input), &session_name, events_cli);
}

fn attach_with_cli_client(
    cli_action: zellij_utils::cli::CliAction,
    session_name: &str,
    config: Option<Config>,
) {
    let os_input = get_os_input(zellij_client::os_input_output::get_cli_client_os_input);
    let get_current_dir = || std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match Action::actions_from_cli(cli_action, Box::new(get_current_dir), config) {
        Ok(actions) => {
            let exit_status = zellij_client::cli_client::start_cli_client(
                Box::new(os_input),
                session_name,
                actions,
            );
            std::process::exit(exit_status);
        },
        Err(e) => {
            eprintln!("{}", e);
            log::error!("Error sending action: {}", e);
            std::process::exit(2);
        },
    }
}

fn attach_with_session_index(config_options: Options, index: usize, create: bool) -> ClientInfo {
    // Ignore the session_name when `--index` is provided
    match get_sessions_sorted_by_mtime() {
        Ok(sessions) if sessions.is_empty() => {
            if create {
                create_new_client()
            } else {
                eprintln!("No active zellij sessions found.");
                process::exit(1);
            }
        },
        Ok(sessions) => find_indexed_session(sessions, config_options, index, create),
        Err(e) => {
            eprintln!("Error occurred: {:?}", e);
            process::exit(1);
        },
    }
}

fn named_session_exists_or_error(
    session_name: &str,
    probe: Result<SessionProbe, std::io::ErrorKind>,
) -> Result<bool, String> {
    match probe {
        Ok(SessionProbe::Alive) => Ok(true),
        Ok(SessionProbe::Absent) => Ok(false),
        Ok(SessionProbe::Unresponsive) => Err(unresponsive_session_message(session_name)),
        Err(kind) => Err(session_listing_error_message(kind)),
    }
}

fn attach_with_session_name(
    session_name: Option<String>,
    config_options: Options,
    create: bool,
) -> ClientInfo {
    match &session_name {
        Some(session) if create => {
            let session_exists = named_session_exists_or_error(session, session_status(session))
                .unwrap_or_else(|error| {
                    eprintln!("{error}");
                    process::exit(1);
                });
            if session_exists {
                ClientInfo::Attach(session_name.unwrap(), config_options)
            } else {
                ClientInfo::New(session_name.unwrap(), None, None)
            }
        },
        Some(prefix) => match match_session_name(prefix) {
            Ok(SessionNameMatch::UniquePrefix(s)) | Ok(SessionNameMatch::Exact(s)) => {
                ClientInfo::Attach(s, config_options)
            },
            Ok(SessionNameMatch::AmbiguousPrefix(sessions)) => {
                println!(
                    "Ambiguous selection: multiple sessions names start with '{}':",
                    prefix
                );
                print_sessions(
                    sessions
                        .iter()
                        .map(|s| (s.clone(), Duration::default(), SessionListing::Running))
                        .collect(),
                    false,
                    false,
                    true,
                );
                process::exit(1);
            },
            Ok(SessionNameMatch::None) => {
                eprintln!("No session with the name '{}' found!", prefix);
                process::exit(1);
            },
            Err(kind) => {
                eprintln!("{}", session_listing_error_message(kind));
                process::exit(1);
            },
        },
        None => match get_active_session() {
            ActiveSession::None if create => create_new_client(),
            ActiveSession::None => {
                eprintln!("No active zellij sessions found.");
                process::exit(1);
            },
            ActiveSession::One(session_name) => ClientInfo::Attach(session_name, config_options),
            ActiveSession::Many => {
                println!("Please specify the session to attach to, either by using the full name or a unique prefix.\nThe following sessions are active:");
                list_sessions(false, false, true, false);
                process::exit(1);
            },
        },
    }
}

pub(crate) fn start_client(opts: CliArgs) {
    let (
        config,
        client_layout_info,
        config_options,
        mut config_without_layout,
        mut config_options_without_layout,
    ) = match Setup::from_cli_args(&opts) {
        Ok(results) => results,
        Err(e) => {
            if let ConfigError::KdlError(error) = e {
                let report: Report = error.into();
                eprintln!("{:?}", report);
            } else {
                eprintln!("{}", e);
            }
            process::exit(1);
        },
    };

    let mut reconnect_to_session: Option<ConnectToSession> = None;
    let os_input = get_os_input(get_client_os_input);
    loop {
        let os_input = os_input.clone();
        let mut config = config.clone();
        let mut config_options = config_options.clone();
        let mut opts = opts.clone();
        let mut is_a_reconnect = false;
        let mut should_create_detached = false;
        let mut layout_info = client_layout_info.clone();
        let mut new_session_cwd = None;

        if let Some(reconnect_to_session) = &reconnect_to_session {
            // this is integration code to make session reconnects work with this existing,
            // untested and pretty involved function
            //
            // ideally, we should write tests for this whole function and refctor it
            reload_config_from_disk(
                &mut config_without_layout,
                &mut config_options_without_layout,
                &opts,
            );
            if reconnect_to_session.name.is_some() {
                opts.command = Some(Command::Sessions(Sessions::Attach {
                    session_name: reconnect_to_session.name.clone(),
                    create: true,
                    create_background: false,
                    force_run_commands: false,
                    index: None,
                    options: None,
                }));
            } else {
                opts.command = None;
                opts.session = None;
                config_options.attach_to_session = None;
            }

            if let Some(reconnect_layout) = &reconnect_to_session.layout {
                layout_info = Some(reconnect_layout.clone());
            }
            if let Some(cwd) = &reconnect_to_session.cwd {
                new_session_cwd = Some(cwd.clone());
            }
            config = config_without_layout.clone();
            config_options = config_options_without_layout.clone();
            is_a_reconnect = true;
        }

        let start_client_plan = |session_name: std::string::String| {
            assert_session_ne(&session_name);
        };

        if let Some(Command::Sessions(Sessions::Attach {
            session_name,
            create,
            create_background,
            force_run_commands,
            index,
            options,
        })) = opts.command.clone()
        {
            let config_options = match options.as_deref() {
                Some(SessionCommand::Options(o)) => config_options.merge_from_cli(o.to_owned()),
                None => config_options,
            };
            should_create_detached = create_background;

            let mut client = if let Some(idx) = index {
                attach_with_session_index(
                    config_options.clone(),
                    idx,
                    create || should_create_detached,
                )
            } else {
                let session_exists = session_name
                    .as_deref()
                    .map(|session_name| {
                        named_session_exists_or_error(session_name, session_status(session_name))
                            .unwrap_or_else(|error| {
                                eprintln!("{error}");
                                process::exit(1);
                            })
                    })
                    .unwrap_or(false);
                let resurrection_layout =
                    session_name
                        .as_ref()
                        .and_then(|s| match resurrection_layout(s) {
                            Ok(layout) => layout,
                            Err(e) => {
                                eprintln!("{}", e);
                                process::exit(2);
                            },
                        });
                if (create || should_create_detached)
                    && !session_exists
                    && resurrection_layout.is_none()
                {
                    session_name.clone().map(start_client_plan);
                }
                match (session_name.as_ref(), resurrection_layout) {
                    (Some(session_name), Some(mut resurrection_layout)) if !session_exists => {
                        if force_run_commands {
                            resurrection_layout.recursively_add_start_suspended(Some(false));
                        }
                        ClientInfo::Resurrect(
                            session_name.clone(),
                            session_layout_cache_file_name(session_name.as_ref()),
                            force_run_commands,
                            new_session_cwd.clone(),
                        )
                    },
                    _ => attach_with_session_name(
                        session_name,
                        config_options.clone(),
                        create || should_create_detached,
                    ),
                }
            };

            if let Ok(val) = std::env::var(envs::SESSION_NAME_ENV_KEY) {
                if val == *client.get_session_name() {
                    panic!("You are trying to attach to the current session (\"{}\"). This is not supported.", val);
                }
            }

            if let Some(layout_info) = layout_info {
                client.set_layout_info(layout_info);
            }

            if let Some(new_session_cwd) = new_session_cwd {
                client.set_cwd(new_session_cwd);
            }

            let tab_position_to_focus = reconnect_to_session.as_ref().and_then(|r| r.tab_position);
            let pane_id_to_focus = reconnect_to_session.as_ref().and_then(|r| r.pane_id);
            reconnect_to_session = start_client_impl(
                Box::new(os_input),
                opts,
                config,
                config_options,
                client,
                tab_position_to_focus,
                pane_id_to_focus,
                is_a_reconnect,
                should_create_detached,
            );
        } else {
            if let Some(session_name) = opts.session.clone() {
                start_client_plan(session_name.clone());
                reconnect_to_session = start_client_impl(
                    Box::new(os_input),
                    opts,
                    config,
                    config_options,
                    ClientInfo::New(session_name, layout_info, new_session_cwd),
                    None,
                    None,
                    is_a_reconnect,
                    should_create_detached,
                );
            } else {
                if let Some(session_name) = config_options.session_name.as_ref() {
                    if let Ok(val) = envs::get_session_name() {
                        // This prevents the same type of recursion as above, only that here we
                        // don't get the command to "attach", but to start a new session instead.
                        // This occurs for example when declaring the session name inside a layout
                        // file and then, from within this session, trying to open a new zellij
                        // session with the same layout. This causes an infinite recursion in the
                        // `zellij_server::terminal_bytes::listen` task, flooding the server and
                        // clients with infinite `Render` requests.
                        if *session_name == val {
                            eprintln!("You are trying to attach to the current session (\"{}\"). Zellij does not support nesting a session in itself.", session_name);
                            process::exit(1);
                        }
                    }
                    match config_options.attach_to_session {
                        Some(true) => {
                            let client = attach_with_session_name(
                                Some(session_name.clone()),
                                config_options.clone(),
                                true,
                            );
                            reconnect_to_session = start_client_impl(
                                Box::new(os_input),
                                opts,
                                config,
                                config_options,
                                client,
                                None,
                                None,
                                is_a_reconnect,
                                should_create_detached,
                            );
                        },
                        _ => {
                            start_client_plan(session_name.clone());
                            reconnect_to_session = start_client_impl(
                                Box::new(os_input),
                                opts,
                                config,
                                config_options.clone(),
                                ClientInfo::New(session_name.clone(), layout_info, new_session_cwd),
                                None,
                                None,
                                is_a_reconnect,
                                should_create_detached,
                            );
                        },
                    }
                    if reconnect_to_session.is_some() {
                        continue;
                    }
                    // after we detach, this happens and so we need to exit before the rest of the
                    // function happens
                    process::exit(0);
                }

                let session_name = generate_unique_session_name_or_exit();
                start_client_plan(session_name.clone());
                reconnect_to_session = start_client_impl(
                    Box::new(os_input),
                    opts,
                    config,
                    config_options,
                    ClientInfo::New(session_name, layout_info, new_session_cwd),
                    None,
                    None,
                    is_a_reconnect,
                    should_create_detached,
                );
            }
        }
        if reconnect_to_session.is_none() {
            break;
        }
    }
}

fn generate_unique_session_name_or_exit() -> String {
    let Some(unique_session_name) = generate_unique_session_name() else {
        eprintln!("Failed to generate a unique session name, giving up");
        process::exit(1);
    };
    unique_session_name
}

pub(crate) fn list_aliases(opts: CliArgs) {
    let (config, _layout, _config_options, _config_without_layout, _config_options_without_layout) =
        match Setup::from_cli_args(&opts) {
            Ok(results) => results,
            Err(e) => {
                if let ConfigError::KdlError(error) = e {
                    let report: Report = error.into();
                    eprintln!("{:?}", report);
                } else {
                    eprintln!("{}", e);
                }
                process::exit(1);
            },
        };
    for alias in config.plugins.list() {
        println!("{}", alias);
    }
    process::exit(0);
}

pub(crate) fn watch_session(session_name: Option<String>, opts: CliArgs) {
    let (config, _, config_options, _, _) = match Setup::from_cli_args(&opts) {
        Ok(results) => results,
        Err(e) => {
            if let ConfigError::KdlError(error) = e {
                let report: Report = error.into();
                eprintln!("{:?}", report);
            } else {
                eprintln!("{}", e);
            }
            process::exit(1);
        },
    };

    // Resolve the session name to watch
    let client_info = match &session_name {
        Some(prefix) => match match_session_name(prefix).unwrap() {
            SessionNameMatch::UniquePrefix(s) | SessionNameMatch::Exact(s) => {
                ClientInfo::Watch(s, config_options.clone())
            },
            SessionNameMatch::AmbiguousPrefix(sessions) => {
                eprintln!(
                    "Ambiguous selection: multiple sessions names start with '{}':",
                    prefix
                );
                print_sessions(
                    sessions
                        .iter()
                        .map(|s| (s.clone(), Duration::default(), SessionListing::Running))
                        .collect(),
                    false,
                    false,
                    true,
                );
                process::exit(1);
            },
            SessionNameMatch::None => {
                eprintln!("No session with the name '{}' found!", prefix);
                process::exit(1);
            },
        },
        None => match get_active_session() {
            ActiveSession::None => {
                eprintln!("No active zellij sessions found.");
                process::exit(1);
            },
            ActiveSession::One(name) => ClientInfo::Watch(name, config_options.clone()),
            ActiveSession::Many => {
                eprintln!("Please specify the session name to watch.");
                process::exit(1);
            },
        },
    };

    let mut opts = opts.clone();
    opts.session = Some(client_info.get_session_name().to_string());

    let os_input = get_os_input(get_client_os_input);

    // Start the watcher client
    start_client_impl(
        Box::new(os_input),
        opts,
        config,
        config_options,
        client_info,
        None,  // tab_position_to_focus
        None,  // pane_id_to_focus
        false, // is_a_reconnect
        false, // should_create_detached
    );
}

fn reload_config_from_disk(
    config_without_layout: &mut Config,
    config_options_without_layout: &mut Options,
    opts: &CliArgs,
) {
    match Setup::from_cli_args(opts) {
        Ok((_, _, _, reloaded_config_without_layout, reloaded_config_options_without_layout)) => {
            *config_without_layout = reloaded_config_without_layout;
            *config_options_without_layout = reloaded_config_options_without_layout;
        },
        Err(e) => {
            log::error!("Failed to reload config: {}", e);
        },
    };
}

#[cfg(unix)]
pub(crate) fn pty_hosts(command: zellij_utils::cli::PtyHosts) -> anyhow::Result<()> {
    use zellij_server::pty_host::cli;
    use zellij_utils::cli::PtyHosts as PtyHostsCli;
    match command {
        PtyHostsCli::List { json, host } => match host {
            Some(host) => cli::remote_list(&host, json),
            None => cli::list(json),
        },
        PtyHostsCli::Kill { host_id } => cli::kill(&host_id),
        PtyHostsCli::KillAll => cli::kill_all(),
        // Handled by `pty_host_adopt`: it needs a session, which the rest of this
        // command deliberately does not.
        PtyHostsCli::Adopt { .. } => Ok(()),
    }
}

/// `zellij pty-host adopt` — put a surviving pane back on screen.
///
/// Resolves the supervisor here, where the failures are legible without a
/// session, then hands the pane to one as an ordinary CLI action.
#[cfg(unix)]
pub(crate) fn pty_host_adopt(
    command: zellij_utils::cli::PtyHosts,
    requested_session_name: Option<String>,
    config: Option<Config>,
) {
    use zellij_server::pty_host::cli;
    use zellij_utils::cli::PtyHosts as PtyHostsCli;
    use zellij_utils::input::actions::warp_placement;

    let PtyHostsCli::Adopt {
        host_id,
        host,
        session,
        focused_tab,
        floating,
    } = command
    else {
        return;
    };

    let session = session.or(requested_session_name);
    // With `--host`, `host_id` is a broker pane token rather than a local
    // supervisor id, and the broker on that machine is the only thing that can
    // resolve it.
    let resolved = match &host {
        Some(host) => cli::remote_adopt_request(host, &host_id, session),
        None => cli::adopt_request(&host_id, session),
    };
    let (session, payload) = match resolved {
        Ok(resolved) => resolved,
        Err(e) => {
            eprintln!("Error: {e:#}");
            std::process::exit(1);
        },
    };

    let os_input = get_os_input(zellij_client::os_input_output::get_cli_client_os_input);
    let exit_status = zellij_client::cli_client::start_cli_client(
        Box::new(os_input),
        &session,
        vec![Action::AdoptPane {
            payload: Box::new(payload),
            placement: warp_placement(focused_tab, floating),
        }],
    );
    let _ = config;
    std::process::exit(exit_status);
}

/// `zellij plugins ...`: the plugin folder is the session's - `user_plugin_dir` in the config,
/// or `<config dir>/plugins` - so what is written here is what `plugin_reload` and a layout's
/// `file:` location find.
pub(crate) fn plugins(command: zellij_utils::cli::Plugins, opts: &CliArgs) -> anyhow::Result<()> {
    use zellij_utils::cli::Plugins as PluginsCli;
    let config = Config::try_from(opts).ok();
    let plugin_dir = zellij_utils::home::user_plugin_dir_or_default(
        config
            .as_ref()
            .and_then(|config| config.options.user_plugin_dir.clone()),
        opts.config_dir.clone(),
    )
    .ok_or_else(|| {
        anyhow::anyhow!(
            "no plugin folder is configured; set `user_plugin_dir` in the config or pass \
             --config-dir"
        )
    })?;
    match command {
        PluginsCli::Dir => {
            println!("{}", plugin_dir.display());
            Ok(())
        },
        PluginsCli::Write {
            name,
            file,
            overwrite,
            no_check,
        } => {
            let source = read_source(file.as_deref())?;
            let written = zellij_server::plugin_authoring::write(
                &plugin_dir,
                &name,
                &source,
                overwrite,
                !no_check,
            )?;
            println!("{}", written.path.display());
            if let Some(replaced) = written.replaced {
                eprintln!("kept the version it replaced as {}", replaced.display());
            }
            // On stderr, and after the path: the path is the answer a script reads, and a
            // warning is for the person watching. Never a refusal - see `plugin_authoring::lint`.
            print_plugin_lint(&name, &source);
            Ok(())
        },
        PluginsCli::Check { name, file } => {
            // Two names, and they are not the same one: `chunk` is what the compiler quotes in
            // an error, and the lint's is the plugin name - `hocus/todo`, not `todo.lua` -
            // because one of its rules turns on where in the folder the file lands.
            let (chunk, lint_name, source) = match (name, file) {
                (_, Some(file)) => (
                    file.file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_else(|| file.display().to_string()),
                    plugin_name_of(&plugin_dir, &file),
                    read_source(Some(&file))?,
                ),
                (Some(name), None) => {
                    let path =
                        zellij_utils::plugin_authoring::resolve_plugin_file(&plugin_dir, &name)
                            .map_err(|e| anyhow::anyhow!(e))?;
                    (
                        path.file_name()
                            .map(|f| f.to_string_lossy().to_string())
                            .unwrap_or_default(),
                        name,
                        std::fs::read_to_string(&path).map_err(|e| {
                            anyhow::anyhow!("failed to read {}: {e}", path.display())
                        })?,
                    )
                },
                (None, None) => anyhow::bail!("give a plugin name, or --file"),
            };
            zellij_server::check_lua_plugin(&source, &chunk).map_err(|e| anyhow::anyhow!(e))?;
            println!("{chunk} compiles");
            print_plugin_lint(&lint_name, &source);
            Ok(())
        },
        PluginsCli::Reload { name } => {
            let path = zellij_utils::plugin_authoring::resolve_plugin_file(&plugin_dir, &name)
                .map_err(|e| anyhow::anyhow!(e))?;
            if !path.is_file() {
                anyhow::bail!("{} is not there; write it first", path.display());
            }
            send_action_to_session(
                zellij_utils::cli::CliAction::StartOrReloadPlugin {
                    url: format!("file:{}", path.display()),
                    configuration: None,
                },
                opts.session.clone(),
                config,
            );
            Ok(())
        },
        PluginsCli::Log { name, lines } => {
            let path = zellij_utils::plugin_authoring::resolve_plugin_file(&plugin_dir, &name)
                .map_err(|e| anyhow::anyhow!(e))?;
            let log_file = zellij_utils::consts::ZELLIJ_TMP_LOG_FILE.clone();
            print!(
                "{}",
                zellij_server::plugin_authoring::log_tail(&log_file, &path, lines)?
            );
            Ok(())
        },
    }
}

/// The plugin name a file would be written under, for a file given by path: its place in the
/// plugin folder when it is in one (`hocus/todo`), and its bare stem when it is not.
///
/// A `--file` check is usually of a file that is not in the folder yet, so the fallback is the
/// common case rather than the odd one; what it must not do is answer `todo.lua` for a file
/// sitting in `<plugin dir>/hocus/`, which is what makes the extension rule invisible.
fn plugin_name_of(plugin_dir: &std::path::Path, file: &std::path::Path) -> String {
    let stem = |path: &std::path::Path| {
        path.file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string())
    };
    let Some(relative) = file.canonicalize().ok().and_then(|file| {
        Some(
            file.strip_prefix(plugin_dir.canonicalize().ok()?)
                .ok()?
                .to_owned(),
        )
    }) else {
        return stem(file);
    };
    match relative
        .parent()
        .map(|parent| parent.to_string_lossy().to_string())
    {
        Some(folder) if !folder.is_empty() => format!("{folder}/{}", stem(&relative)),
        _ => stem(&relative),
    }
}

/// What the source is going to do wrong at run time, on stderr, under whatever the command
/// printed. Compiling is the verdict; these are the traps a compile cannot see.
fn print_plugin_lint(name: &str, source: &str) {
    for finding in zellij_utils::plugin_authoring::lint(name, source) {
        eprintln!("warning: {finding}");
    }
}

fn read_source(file: Option<&Path>) -> anyhow::Result<String> {
    match file {
        Some(file) => std::fs::read_to_string(file)
            .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", file.display())),
        None => {
            use std::io::Read;
            let mut source = String::new();
            std::io::stdin()
                .read_to_string(&mut source)
                .map_err(|e| anyhow::anyhow!("failed to read the plugin source from stdin: {e}"))?;
            Ok(source)
        },
    }
}

pub(crate) fn recordings(
    command: zellij_utils::cli::Recordings,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    use zellij_utils::cli::Recordings as RecordingsCli;
    let data_access = zellij_utils::data_access::DataAccess::sqlite(&data_dir);
    match command {
        RecordingsCli::List { json } => {
            let summaries = data_access.recordings().list()?;
            if json {
                println!("{}", serde_json::to_string(&summaries)?);
            } else {
                print!("{}", format_recordings_list(&summaries));
            }
            Ok(())
        },
        RecordingsCli::Export {
            recording_id,
            output,
        } => {
            if let Some(ref path) = output {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                    .map_err(|e| {
                        if e.kind() == std::io::ErrorKind::AlreadyExists {
                            anyhow::anyhow!("output file {:?} already exists", path)
                        } else {
                            e.into()
                        }
                    })?;
                data_access.recordings().export(recording_id, &mut file)?;
            } else {
                let stdout = std::io::stdout();
                let mut handle = stdout.lock();
                data_access.recordings().export(recording_id, &mut handle)?;
            }
            Ok(())
        },
        RecordingsCli::Search { query, limit, json } => {
            let hits = data_access.recordings().search(&query, limit)?;
            if json {
                println!("{}", serde_json::to_string(&hits)?);
            } else {
                print!("{}", format_search_results(&hits));
            }
            Ok(())
        },
    }
}

pub(crate) fn format_search_results(hits: &[zellij_utils::session_recording::SearchHit]) -> String {
    if hits.is_empty() {
        return "No matches.\n".to_string();
    }
    let mut out = String::from("ID  TIME  SESSION  TAB  PANE  TEXT\n");
    for hit in hits {
        let pane = if hit.pane_title.is_empty() {
            "-"
        } else {
            &hit.pane_title
        };
        out.push_str(&format!(
            "{}  {}  {}  {}  {}  {}\n",
            hit.recording_id,
            format_duration(hit.elapsed_us),
            hit.session_name,
            hit.tab_name,
            pane,
            truncate_chars(&hit.text, 120),
        ));
    }
    out
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    truncated.push('…');
    truncated
}

pub(crate) fn format_recordings_list(
    summaries: &[zellij_utils::session_recording::RecordingSummary],
) -> String {
    // Header as specified in plan
    let mut out = String::new();
    out.push_str("ID  SESSION  TAB  PANE  STARTED  DURATION  INPUT  STATE\n");
    for s in summaries {
        let started = format_started(s.started_at_us);
        let duration = if s.is_running {
            "running".to_string()
        } else {
            format_duration(s.duration_us)
        };
        let input = if s.capture_input { "yes" } else { "no" };
        let state = if s.is_running {
            "running"
        } else if s.error.is_some() {
            "incomplete"
        } else {
            "complete"
        };
        // PANE column: terminal_id plus title
        let pane_col = if s.pane_title.is_empty() {
            format!("{}", s.terminal_id)
        } else {
            format!("{} {}", s.terminal_id, s.pane_title)
        };
        // Use simple space-separated formatting; header checks only contain substrings
        out.push_str(&format!(
            "{}  {}  {}  {}  {}  {}  {}  {}\n",
            s.id, s.session_name, s.tab_name, pane_col, started, duration, input, state
        ));
    }
    out
}

fn format_started(started_at_us: i64) -> String {
    let t = std::time::UNIX_EPOCH + Duration::from_micros(started_at_us.max(0) as u64);
    humantime::format_rfc3339_seconds(t).to_string()
}

fn format_duration(duration_us: i64) -> String {
    let secs = duration_us as f64 / 1_000_000.0;
    if secs < 60.0 {
        format!("{:.1}s", secs)
    } else if secs < 3600.0 {
        format!("{:.1}m", secs / 60.0)
    } else {
        format!("{:.1}h", secs / 3600.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        kill_outcome, named_session_exists_or_error, retarget_implicit_agent_report,
        sessions_to_delete, sessions_to_kill, KillOutcome,
    };
    use std::{cell::Cell, io, time::Duration};
    use zellij_server::pty_host::protocol::HostStatus;
    use zellij_utils::cli::CliAction;
    use zellij_utils::sessions::{
        session_listing_error_message, unresponsive_session_message, SessionProbe,
    };

    fn session(name: &str) -> (String, Duration) {
        (name.to_owned(), Duration::default())
    }

    fn agent_report(pane_id: Option<&str>) -> CliAction {
        CliAction::AgentReport {
            pane_id: pane_id.map(str::to_owned),
            kind: Some("codex".to_owned()),
            state: Some("busy".to_owned()),
            detail: None,
            cwd: None,
            hook_json: false,
            clear: false,
            run_id: None,
        }
    }

    fn pane_status(attached: bool) -> HostStatus {
        HostStatus {
            host_id: "abcdef012345".to_owned(),
            session_name: Some("after-warp".to_owned()),
            terminal_id: 42,
            command: "codex".to_owned(),
            cwd: Some("/work".into()),
            child_pid: Some(10),
            host_pid: 11,
            started_at_unix: 0,
            attached,
            child_exit_status: None,
            buffered_bytes: 0,
        }
    }

    #[test]
    fn implicit_agent_reports_follow_the_supervisor_after_a_warp() {
        let mut action = agent_report(None);
        let mut session = None;
        retarget_implicit_agent_report(
            &mut action,
            &mut session,
            Some("abcdef012345".to_owned()),
            |_| Ok(pane_status(true)),
        )
        .unwrap();

        assert_eq!(session.as_deref(), Some("after-warp"));
        assert!(matches!(
            action,
            CliAction::AgentReport {
                pane_id: Some(ref pane_id),
                ..
            } if pane_id == "42"
        ));
    }

    #[test]
    fn stable_agent_reports_never_fall_back_to_a_detached_or_explicit_target() {
        let mut detached = agent_report(None);
        assert!(retarget_implicit_agent_report(
            &mut detached,
            &mut None,
            Some("abcdef012345".to_owned()),
            |_| Ok(pane_status(false)),
        )
        .unwrap_err()
        .contains("between sessions"));

        let mut explicit = agent_report(Some("7"));
        retarget_implicit_agent_report(
            &mut explicit,
            &mut None,
            Some("abcdef012345".to_owned()),
            |_| panic!("an explicit pane must not query the supervisor"),
        )
        .unwrap();
        assert!(matches!(
            explicit,
            CliAction::AgentReport {
                pane_id: Some(ref pane_id),
                ..
            } if pane_id == "7"
        ));
    }

    #[test]
    fn session_deletion_fails_when_live_session_discovery_fails() {
        let result = sessions_to_delete(false, vec![session("dead")], || {
            Err(io::ErrorKind::PermissionDenied)
        });

        assert_eq!(result.unwrap_err(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn session_deletion_excludes_live_sessions() {
        let sessions = sessions_to_delete(false, vec![session("live"), session("dead")], || {
            Ok(vec![session("live")])
        })
        .unwrap();

        assert_eq!(sessions, vec![session("dead")]);
    }

    #[test]
    fn forced_session_deletion_skips_live_session_discovery() {
        let discovery_called = Cell::new(false);
        let eligible = vec![session("live"), session("dead")];
        let sessions = sessions_to_delete(true, eligible.clone(), || {
            discovery_called.set(true);
            Ok(vec![session("live")])
        })
        .unwrap();

        assert_eq!(sessions, eligible);
        assert!(!discovery_called.get());
    }

    #[test]
    fn session_killing_includes_every_non_absent_socket() {
        let sessions = sessions_to_kill(|| {
            Ok(vec![
                ("alive".to_owned(), Duration::default(), SessionProbe::Alive),
                (
                    "wedged".to_owned(),
                    Duration::default(),
                    SessionProbe::Unresponsive,
                ),
                (
                    "absent".to_owned(),
                    Duration::default(),
                    SessionProbe::Absent,
                ),
            ])
        })
        .unwrap();

        assert_eq!(sessions, vec![session("alive"), session("wedged")]);
    }

    /// An answering server takes the request and that is the end of it: no
    /// wait, and no doubt. A server that did not answer the probe is only
    /// killed once its socket has gone, because the request went to the very
    /// loop that was not answering.
    #[test]
    fn a_kill_is_only_reported_done_when_the_server_is_known_to_have_gone() {
        let delivered = || Ok(());
        let never_asked = || panic!("an answering server is not waited for");

        assert!(matches!(
            kill_outcome(Ok(SessionProbe::Alive), delivered, never_asked),
            KillOutcome::Killed
        ));
        assert!(matches!(
            kill_outcome(Ok(SessionProbe::Unresponsive), delivered, || Ok(true)),
            KillOutcome::Killed
        ));
        assert!(matches!(
            kill_outcome(Ok(SessionProbe::Unresponsive), delivered, || Ok(false)),
            KillOutcome::Unacted
        ));
        assert!(matches!(
            kill_outcome(Ok(SessionProbe::Unresponsive), delivered, || Err(
                io::ErrorKind::PermissionDenied
            )),
            KillOutcome::ListingError(io::ErrorKind::PermissionDenied)
        ));
    }

    /// Nothing is sent to a session that is not there, or that could not be
    /// looked for; and a request that could not be delivered is reported as
    /// that, whatever the probe said.
    #[test]
    fn a_kill_that_never_reached_a_server_says_so() {
        let never_sent = || panic!("nothing is sent to an absent session");
        let never_asked = || panic!("nothing is waited for");
        let refused = || Err(io::Error::from(io::ErrorKind::TimedOut));

        assert!(matches!(
            kill_outcome(Ok(SessionProbe::Absent), never_sent, never_asked),
            KillOutcome::Absent
        ));
        assert!(matches!(
            kill_outcome(
                Err(io::ErrorKind::PermissionDenied),
                never_sent,
                never_asked
            ),
            KillOutcome::ListingError(io::ErrorKind::PermissionDenied)
        ));
        assert!(matches!(
            kill_outcome(Ok(SessionProbe::Unresponsive), refused, never_asked),
            KillOutcome::Undeliverable(e) if e.kind() == io::ErrorKind::TimedOut
        ));
        assert!(matches!(
            kill_outcome(Ok(SessionProbe::Alive), refused, never_asked),
            KillOutcome::Undeliverable(_)
        ));
    }

    #[test]
    fn only_an_absent_named_session_allows_creation() {
        let creation_allowed = |probe| {
            named_session_exists_or_error("named", probe).map(|session_exists| !session_exists)
        };

        assert_eq!(creation_allowed(Ok(SessionProbe::Alive)), Ok(false));
        assert_eq!(creation_allowed(Ok(SessionProbe::Absent)), Ok(true));
        assert_eq!(
            creation_allowed(Ok(SessionProbe::Unresponsive)),
            Err(unresponsive_session_message("named"))
        );
        assert_eq!(
            creation_allowed(Err(io::ErrorKind::PermissionDenied)),
            Err(session_listing_error_message(
                io::ErrorKind::PermissionDenied
            ))
        );
    }
}

#[cfg(test)]
mod firecracker_command_tests {
    use super::{resolve_firecracker_session, run_firecracker};
    use serde_json::json;
    use std::{fs, time::Duration};
    use tempfile::TempDir;
    use zellij_server::firecracker::Provider;
    use zellij_test_support::write_script;
    use zellij_utils::cli::Firecracker;
    use zellij_utils::sessions::ActiveSession;

    /// A provider that logs each request to `requests` beside itself and answers
    /// every one with `response`.
    fn stub_provider(response: &str) -> (TempDir, Provider) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("provider");
        write_script(
            &path,
            &format!(
                "#!/bin/sh\ncat >> \"$(dirname \"$0\")/requests\"\nprintf '%s' '{response}'\n"
            ),
        );
        let provider = Provider::new(path, Duration::from_secs(5)).unwrap();
        (dir, provider)
    }

    fn requests(dir: &TempDir) -> Vec<serde_json::Value> {
        fs::read_to_string(dir.path().join("requests"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn run(command: Firecracker, provider: &Provider) -> (String, String) {
        let mut out = Vec::new();
        let mut err = Vec::new();
        run_firecracker(command, provider, &mut out, &mut err).unwrap();
        (
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn list(session: Option<&str>, json: bool) -> Firecracker {
        Firecracker::List {
            session: session.map(str::to_owned),
            json,
        }
    }

    #[test]
    fn an_explicit_session_wins_over_the_environment_and_the_active_one() {
        assert_eq!(
            resolve_firecracker_session(
                Some("explicit".to_owned()),
                Some("env".to_owned()),
                ActiveSession::One("active".to_owned()),
            )
            .unwrap(),
            "explicit"
        );
    }

    #[test]
    fn the_environment_session_wins_over_the_active_one() {
        assert_eq!(
            resolve_firecracker_session(
                None,
                Some("env".to_owned()),
                ActiveSession::One("active".to_owned()),
            )
            .unwrap(),
            "env"
        );
    }

    #[test]
    fn a_single_active_session_is_used_when_nothing_else_selects_one() {
        assert_eq!(
            resolve_firecracker_session(None, None, ActiveSession::One("active".to_owned()))
                .unwrap(),
            "active"
        );
    }

    #[test]
    fn no_session_and_an_ambiguous_one_are_both_refused() {
        for active in [ActiveSession::None, ActiveSession::Many] {
            let error = resolve_firecracker_session(None, None, active)
                .unwrap_err()
                .to_string();
            assert_eq!(error, "no session selected; pass --session SESSION");
        }
    }

    #[test]
    fn list_says_so_when_there_are_no_vms() {
        let (dir, provider) = stub_provider(r#"{"version":2,"vms":[]}"#);
        let (out, err) = run(list(None, false), &provider);
        assert_eq!(out, "No Firecracker VMs found.\n");
        assert_eq!(err, "");
        assert_eq!(
            requests(&dir),
            vec![json!({"version": 2, "action": "list", "session": null})]
        );
    }

    #[test]
    fn list_prints_one_tab_separated_line_per_vm() {
        let (dir, provider) = stub_provider(
            r#"{"version":2,"vms":[{"session":"work","name":"review","state":"running"},{"session":"other","name":"build","state":"provisioning"}]}"#,
        );
        let (out, err) = run(list(Some("work"), false), &provider);
        assert_eq!(out, "work\treview\trunning\nother\tbuild\tprovisioning\n");
        assert_eq!(err, "");
        assert_eq!(
            requests(&dir),
            vec![json!({"version": 2, "action": "list", "session": "work"})]
        );
    }

    #[test]
    fn list_prints_retained_disks_after_the_vms() {
        let (_dir, provider) = stub_provider(
            r#"{"version":2,"vms":[{"session":"work","name":"review","state":"running"}],"recoveries":[{"path":"/state/recovery/abc-1-sync-failed","reason":"sync-failed","retained_at":1,"session":"work","name":"review","worktree":"/repo"},{"path":"/state/recovery/def-2-crashed","reason":"crashed","retained_at":2}]}"#,
        );
        let (out, _) = run(list(None, false), &provider);
        assert_eq!(
            out,
            "work\treview\trunning\n\
             Retained disks:\n\
             work\treview\tsync-failed\t/state/recovery/abc-1-sync-failed\n\
             -\t-\tcrashed\t/state/recovery/def-2-crashed\n"
        );
    }

    #[test]
    fn list_json_is_one_object_with_vms_and_recoveries() {
        let (_dir, provider) = stub_provider(
            r#"{"version":2,"vms":[{"session":"work","name":"review","state":"running"}],"recoveries":[{"path":"/state/recovery/def-2-crashed","reason":"crashed","retained_at":2}]}"#,
        );
        let (out, err) = run(list(None, true), &provider);
        assert_eq!(err, "");
        assert_eq!(out.matches('\n').count(), 1, "{out:?}");
        let printed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            printed,
            json!({
                "vms": [{"session": "work", "name": "review", "state": "running"}],
                "recoveries": [{
                    "path": "/state/recovery/def-2-crashed",
                    "reason": "crashed",
                    "retained_at": 2,
                    "session": null,
                    "name": null,
                    "worktree": null,
                }],
            })
        );

        let (_dir, provider) = stub_provider(r#"{"version":2,"vms":[]}"#);
        let (out, _) = run(list(None, true), &provider);
        assert_eq!(out, "{\"vms\":[],\"recoveries\":[]}\n");
    }

    #[test]
    fn stop_prints_each_merge_warning_to_stderr() {
        let (dir, provider) = stub_provider(
            r#"{"version":2,"warnings":["/repo/a.rs: changed on both sides; kept the host copy, the guest copy is /repo/a.rs.guest","/repo/b.rs: changed on both sides; kept the host copy, the guest copy is /repo/b.rs.guest"]}"#,
        );
        let (out, err) = run(
            Firecracker::Stop {
                name: "review".to_owned(),
                session: Some("work".to_owned()),
            },
            &provider,
        );
        assert_eq!(out, "");
        assert_eq!(
            err,
            "warning: /repo/a.rs: changed on both sides; kept the host copy, the guest copy is /repo/a.rs.guest\n\
             warning: /repo/b.rs: changed on both sides; kept the host copy, the guest copy is /repo/b.rs.guest\n"
        );
        assert_eq!(
            requests(&dir),
            vec![json!({"version": 2, "action": "stop", "session": "work", "name": "review"})]
        );
    }

    #[test]
    fn sync_prints_each_merge_warning_to_stderr() {
        let (dir, provider) = stub_provider(
            r#"{"version":2,"warnings":["/repo/a.rs: changed on both sides; kept the host copy, the guest copy is /repo/a.rs.guest"]}"#,
        );
        let (out, err) = run(
            Firecracker::Sync {
                name: "review".to_owned(),
                session: Some("work".to_owned()),
            },
            &provider,
        );
        assert_eq!(out, "");
        assert_eq!(
            err,
            "warning: /repo/a.rs: changed on both sides; kept the host copy, the guest copy is /repo/a.rs.guest\n"
        );
        assert_eq!(
            requests(&dir),
            vec![json!({"version": 2, "action": "sync", "session": "work", "name": "review"})]
        );
    }

    #[test]
    fn a_clean_stop_or_sync_prints_nothing() {
        let (_dir, provider) = stub_provider(r#"{"version":2}"#);
        for command in [
            Firecracker::Stop {
                name: "review".to_owned(),
                session: Some("work".to_owned()),
            },
            Firecracker::Sync {
                name: "review".to_owned(),
                session: Some("work".to_owned()),
            },
        ] {
            assert_eq!(run(command, &provider), (String::new(), String::new()));
        }
    }
}
