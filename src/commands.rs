use dialoguer::Confirm;
use std::{
    path::{Path, PathBuf},
    process,
    time::Duration,
};
use yinz_runtime::logging::startup_stage;
use yinz_types::internal_ids::SessionName;
use yinz_types::internal_ids::TerminalId;

use yinz_client::{
    os_input_output::get_client_os_input, start_client as start_client_impl, ClientInfo,
};

use {
    yinz_cli::sessions::assert_dead_session, yinz_cli::sessions::assert_session_ne,
    yinz_cli::sessions::delete_session as delete_session_impl,
    yinz_cli::sessions::denied_session_message, yinz_cli::sessions::generate_unique_session_name,
    yinz_cli::sessions::get_active_session, yinz_cli::sessions::no_active_session_message,
    yinz_cli::sessions::print_sessions, yinz_cli::sessions::print_sessions_with_index,
    yinz_cli::sessions::session_listing_error_message,
    yinz_cli::sessions::undeliverable_kill_message,
    yinz_cli::sessions::unresponsive_session_message, yinz_cli::sessions::ActiveSession,
    yinz_cli::sessions::SessionListing, yinz_ipc::probe::SessionProbe,
    yinz_session::sessions::get_resurrectable_sessions, yinz_session::sessions::get_sessions,
    yinz_session::sessions::get_sessions_sorted_by_mtime,
    yinz_session::sessions::get_sessions_with_status,
    yinz_session::sessions::kill_session as kill_session_impl,
    yinz_session::sessions::match_session_name, yinz_session::sessions::resurrection_layout,
    yinz_session::sessions::session_status, yinz_session::sessions::validate_session_name,
    yinz_session::sessions::SessionNameMatch,
};

use yinz_runtime::consts::session_layout_cache_file_name;

use miette::{Report, Result};
use yinz_server::start_server as start_server_impl;
use yinz_server_os::get_server_os_input_from_termios_arg;
use {
    yinz_cli::args::CliArgs, yinz_cli::args::Command, yinz_cli::args::SessionCommand,
    yinz_cli::args::Sessions, yinz_cli::setup::Setup, yinz_config::config::Config,
    yinz_config::config::ConfigError, yinz_types::data::SessionConnection,
    yinz_types::input::actions::Action, yinz_types::input::options::Options,
};

pub(crate) fn list_sessions(no_formatting: bool, short: bool, reverse: bool, exited: bool) {
    yinz_cli::sessions::list_sessions(
        no_formatting,
        short,
        reverse,
        exited,
        current_session().as_ref(),
    );
}

/// Whether a nameless `yinz attach` may ask which session to attach to: only
/// a person at a terminal can answer.
fn can_pick_a_session() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && std::io::stderr().is_terminal()
}

/// A nameless `yinz attach` with several sessions running: a menu that
/// attaches to the session picked, or switches to it when run from inside a
/// session.
fn pick_session_to_attach(config_options: Options, config: &Config) -> ClientInfo {
    use yinz_cli::session_picker::{Outcome, PickerRow};
    let mut sessions = yinz_cli::sessions::collect_sessions(false).unwrap_or_else(|e| {
        eprintln!("Error occurred: {:?}", e);
        process::exit(1);
    });
    if sessions.is_empty() {
        eprintln!("No active yinz sessions found.");
        process::exit(1);
    }
    // Newest first, as the listing this replaces printed them.
    yinz_cli::sessions::sort_sessions(&mut sessions, true);
    let current = current_session();
    let rows = sessions
        .into_iter()
        .map(|(name, age, listing)| PickerRow {
            current: current.as_ref() == Some(&name),
            name,
            age,
            listing,
        })
        .collect();
    let verb = if current.is_some() {
        "switch"
    } else {
        "attach"
    };
    match crate::session_menu::run(rows, verb) {
        Ok(Outcome::Picked(name)) if current.is_some() => {
            let switch = yinz_cli::args::CliAction::SwitchSession {
                name: name.into_string(),
                tab_position: None,
                pane_id: None,
                layout: None,
                layout_string: None,
                layout_dir: None,
                cwd: None,
            };
            send_action_to_session(switch, None, Some(config.clone()));
            process::exit(0);
        },
        Ok(Outcome::Picked(name)) => ClientInfo::Attach(name, config_options),
        Ok(Outcome::Cancelled(message)) => {
            if let Some(message) = message {
                eprintln!("{message}");
            }
            process::exit(0);
        },
        Ok(Outcome::Interrupted) => process::exit(130),
        Ok(Outcome::Continue) => process::exit(0),
        Err(e) => {
            eprintln!("Error occurred: {e}");
            process::exit(1);
        },
    }
}

fn sessions_to_kill(
    get_sessions: impl FnOnce()
        -> Result<Vec<(SessionName, Duration, SessionProbe)>, std::io::ErrorKind>,
) -> Result<Vec<(SessionName, Duration)>, std::io::ErrorKind> {
    Ok(get_sessions()?
        .into_iter()
        .filter_map(|(name, age, status)| {
            (!matches!(status, SessionProbe::Absent)).then_some((name, age))
        })
        .collect())
}

pub(crate) fn kill_all_sessions(yes: bool) {
    match sessions_to_kill(get_sessions_with_status) {
        Ok(sessions) if sessions.is_empty() => {
            eprintln!("No active yinz sessions found.");
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
                    eprintln!("{}", undeliverable_kill_message(session.0.as_str()));
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
    use yinz_server::background_jobs::scan_session_list_default_dirs;

    let (_live_sessions, resurrectable_map) =
        scan_session_list_default_dirs(&SessionName::default(), &[], &BTreeMap::new());
    let mut resurrectable_sessions: Vec<(SessionName, Duration)> =
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
        delete_session_impl(session.0.as_str(), force);
    }
    process::exit(0);
}

fn sessions_to_delete(
    force: bool,
    resurrectable_sessions: Vec<(SessionName, Duration)>,
    get_active_sessions: impl FnOnce() -> Result<Vec<(SessionName, Duration)>, std::io::ErrorKind>,
) -> Result<Vec<(SessionName, Duration)>, std::io::ErrorKind> {
    if force {
        return Ok(resurrectable_sessions);
    }
    let active_sessions: Vec<SessionName> = get_active_sessions()?
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    Ok(resurrectable_sessions
        .into_iter()
        .filter(|(name, _)| !active_sessions.contains(name))
        .collect())
}

/// Success requires a completion reply, or an absent session with reclaimed orphans.
fn kill_outcome(
    probe: Result<SessionProbe, std::io::ErrorKind>,
    complete: impl FnOnce() -> std::io::Result<()>,
    cleanup: impl FnOnce() -> std::result::Result<usize, String>,
) -> std::result::Result<(), String> {
    let shutdown = match probe {
        Ok(SessionProbe::Absent) => None,
        Ok(_) => Some(complete().map_err(|e| format!("server shutdown unconfirmed: {e}"))),
        Err(kind) => Some(Err(session_listing_error_message(kind))),
    };
    if matches!(shutdown, Some(Ok(()))) {
        return Ok(());
    }
    let cleaned = cleanup();
    match (shutdown, cleaned) {
        (None, Ok(count)) if count > 0 => Ok(()),
        (None, Ok(_)) => Err("No session or orphaned pane supervisors found.".to_owned()),
        (None, Err(e)) => Err(format!("orphan cleanup incomplete: {e}")),
        (Some(Err(e)), Ok(_)) => Err(e),
        (Some(Err(e)), Err(cleanup)) => Err(format!("{e}; orphan cleanup incomplete: {cleanup}")),
        (Some(Ok(())), _) => unreachable!(),
    }
}

pub(crate) fn kill_session(target_session: &Option<String>) {
    let Some(name) = target_session else {
        eprintln!("Please specify the session name to kill.");
        process::exit(1);
    };
    if let Err(e) = validate_session_name(name) {
        eprintln!("{e}");
        process::exit(1);
    }
    let outcome = kill_outcome(
        session_status(&SessionName::new(name.clone())),
        || {
            yinz_ipc::ipc::kill_session_with_completion(
                &yinz_runtime::consts::YINZ_SOCK_DIR.join(name),
            )
        },
        || {
            yinz_process::pty_host::cli::terminate_orphans_of_session(name)
                .map(|terminated| terminated.len())
                .map_err(|e| format!("{e:#}"))
        },
    );
    match outcome {
        Ok(()) => process::exit(0),
        Err(e) => {
            eprintln!("Failed to kill session {name:?}: {e}");
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
    fn_get_os_input: impl FnOnce() -> Result<OsInputOutput, std::io::Error>,
) -> OsInputOutput {
    match fn_get_os_input() {
        Ok(os_input) => os_input,
        Err(e) => {
            eprintln!("failed to open terminal:\n{}", e);
            process::exit(1);
        },
    }
}

pub(crate) fn start_server(path: PathBuf, debug: bool, termios: Option<String>) {
    // Set instance-wide debug mode
    yinz_runtime::consts::DEBUG_MODE.set(debug).unwrap();
    let os_input = startup_stage("server.os_backend", || {
        get_os_input(|| get_server_os_input_from_termios_arg(termios.as_deref()))
    });
    start_server_impl(Box::new(os_input), path);
}

fn create_new_client() -> ClientInfo {
    ClientInfo::New(generate_unique_session_name_or_exit(), None, None)
}

fn find_indexed_session(
    sessions: Vec<SessionName>,
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
            print_sessions_with_index(sessions, current_session().as_ref());
            process::exit(1);
        },
    }
}

/// Client entrypoint for all [`yinz_cli::args::CliAction`]
///
/// Checks session to send the action to and attaches with client
pub(crate) fn send_action_to_session(
    mut cli_action: yinz_cli::args::CliAction,
    requested_session_name: Option<String>,
    config: Option<Config>,
) {
    let (requested_session_name, caller) = command_target(requested_session_name);
    fill_caller_pane(&mut cli_action, caller);
    if let Some(session_name) = requested_session_name {
        wait_for_command_session(&session_name);
        attach_with_cli_client(cli_action, &session_name, config, caller);
        return;
    }
    let named_store_action = matches!(
        cli_action,
        yinz_cli::args::CliAction::NotesTodos(_)
            | yinz_cli::args::CliAction::Operations(_)
            | yinz_cli::args::CliAction::Wait(_)
    );
    match get_active_session() {
        ActiveSession::None => {
            eprintln!("{}", no_active_session_message());
            std::process::exit(1);
        },
        ActiveSession::One(session_name) => {
            attach_with_cli_client(cli_action, &session_name, config, caller);
        },
        ActiveSession::Many => {
            let existing_sessions: Vec<SessionName> = get_sessions()
                .unwrap_or_default()
                .iter()
                .map(|s| s.0.clone())
                .collect();
            eprintln!("Please specify the session name to send actions to. The following sessions are active:");
            if named_store_action {
                for name in &existing_sessions {
                    eprintln!("{name}");
                }
            } else {
                list_sessions(false, false, true, false);
            }
            std::process::exit(1);
        },
    };
}

pub(crate) fn command_target(
    explicit: Option<String>,
) -> (Option<SessionName>, Option<TerminalId>) {
    yinz_process::pane_context::target(explicit.map(SessionName::new)).unwrap_or_else(|error| {
        eprintln!("{error:#}");
        process::exit(1);
    })
}

pub(crate) fn fill_caller_pane(action: &mut yinz_cli::args::CliAction, caller: Option<TerminalId>) {
    match action {
        yinz_cli::args::CliAction::AgentReport { pane_id, .. }
        | yinz_cli::args::CliAction::SetPaneColor { pane_id, .. }
            if pane_id.is_none() =>
        {
            *pane_id = caller.map(|id| id.to_string());
        },
        _ => {},
    }
}

pub(crate) fn current_session() -> Option<SessionName> {
    yinz_process::pane_context::current_pane()
        .ok()
        .flatten()
        .map(|(session, _)| session)
}

/// The session a `yinz subscribe` / `yinz events` client attaches to: the one running, the
/// one asked for, or the one this shell is inside. Anything else is an error naming the choices,
/// with `purpose` finishing the sentence ("subscribe to", "stream events from").
pub(crate) fn resolve_session_for_stream(
    requested_session_name: Option<String>,
    purpose: &str,
) -> SessionName {
    resolve_session_for_command(requested_session_name, purpose).0
}

pub(crate) fn resolve_session_for_command(
    requested_session_name: Option<String>,
    purpose: &str,
) -> (SessionName, Option<TerminalId>) {
    let (requested_session_name, caller) = command_target(requested_session_name);
    if let Some(session_name) = requested_session_name {
        wait_for_command_session(&session_name);
        return (session_name, caller);
    }
    let session = match get_active_session() {
        ActiveSession::None => {
            eprintln!("{}", no_active_session_message());
            std::process::exit(1);
        },
        ActiveSession::One(session_name) => session_name,
        ActiveSession::Many => {
            eprintln!(
                "Please specify the session name to {purpose}. The following sessions are active:"
            );
            match yinz_cli::sessions::collect_sessions(false) {
                Ok(sessions) => print_sessions(sessions, false, false, true, None),
                Err(error) => eprintln!("Error occurred: {error:?}"),
            }
            std::process::exit(1);
        },
    };
    (session, caller)
}

fn wait_for_command_session(session_name: &SessionName) {
    command_session_ready(session_name, Duration::from_secs(30), session_status).unwrap_or_else(
        |error| {
            eprintln!("{error}");
            process::exit(1);
        },
    );
}

fn command_session_ready(
    session_name: &SessionName,
    within: Duration,
    mut probe: impl FnMut(&SessionName) -> Result<SessionProbe, std::io::ErrorKind>,
) -> Result<(), String> {
    validate_session_name(session_name.as_str())?;
    let deadline = std::time::Instant::now() + within;
    loop {
        let status = probe(session_name);
        if status != Ok(SessionProbe::Unresponsive) {
            return match named_session_exists_or_error(session_name, status)? {
                true => Ok(()),
                false => Err(format!("Session '{session_name}' not found.")),
            };
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(unresponsive_session_message(session_name.as_str()));
        }
        std::thread::sleep(remaining.min(Duration::from_millis(100)));
        if std::time::Instant::now() >= deadline {
            return Err(unresponsive_session_message(session_name.as_str()));
        }
    }
}

#[cfg(test)]
#[path = "unit/command_session.rs"]
mod command_session_tests;

pub(crate) fn subscribe_to_session(
    subscribe_cli: yinz_cli::args::SubscribeCli,
    requested_session_name: Option<String>,
    _config: Option<Config>,
) {
    let session_name = resolve_session_for_stream(requested_session_name, "subscribe to");
    let os_input = get_os_input(yinz_client::os_input_output::get_cli_client_os_input);
    yinz_client::cli_client::start_subscribe_client(
        Box::new(os_input),
        &session_name,
        subscribe_cli,
    );
}

pub(crate) fn stream_session_events(
    events_cli: yinz_cli::args::EventsCli,
    requested_session_name: Option<String>,
    _config: Option<Config>,
) {
    let session_name = resolve_session_for_stream(requested_session_name, "stream events from");
    let os_input = get_os_input(yinz_client::os_input_output::get_cli_client_os_input);
    yinz_client::cli_client::start_events_client(Box::new(os_input), &session_name, events_cli);
}

fn attach_with_cli_client(
    cli_action: yinz_cli::args::CliAction,
    session_name: &SessionName,
    config: Option<Config>,
    caller: Option<TerminalId>,
) {
    if let yinz_cli::args::CliAction::Wait(request) = cli_action {
        match yinz_client::wait_client::wait(session_name, request) {
            Ok(result) => {
                println!("{result}");
                std::process::exit(0);
            },
            Err(error) => {
                eprintln!("{error:#}");
                std::process::exit(1);
            },
        }
    }
    let service_action = match &cli_action {
        yinz_cli::args::CliAction::NotesTodos(command) => {
            Some(command.action(&mut std::io::stdin()))
        },
        yinz_cli::args::CliAction::Operations(command) => {
            Some(command.action(&mut std::io::stdin()))
        },
        _ => None,
    };
    if let Some(action) = service_action {
        let result = action
            .map_err(anyhow::Error::msg)
            .and_then(|action| yinz_client::store_client::request(session_name, action, caller));
        match result {
            Ok(data) => {
                println!("{data}");
                std::process::exit(0);
            },
            Err(error) => {
                eprintln!("{error:#}");
                std::process::exit(1);
            },
        }
    }
    let os_input = get_os_input(yinz_client::os_input_output::get_cli_client_os_input);
    let get_current_dir = || std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match yinz_cli::actions::actions_from_cli(cli_action, Box::new(get_current_dir), config) {
        Ok(actions) => {
            let exit_status = yinz_client::cli_client::start_cli_client(
                Box::new(os_input),
                session_name,
                actions,
                caller,
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
                eprintln!("No active yinz sessions found.");
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
    session_name: &SessionName,
    probe: Result<SessionProbe, std::io::ErrorKind>,
) -> Result<bool, String> {
    match probe {
        Ok(SessionProbe::Alive) => Ok(true),
        Ok(SessionProbe::Absent) => Ok(false),
        Ok(SessionProbe::Unresponsive) => Err(unresponsive_session_message(session_name.as_str())),
        Ok(SessionProbe::Denied) => Err(denied_session_message(session_name.as_str())),
        Err(kind) => Err(session_listing_error_message(kind)),
    }
}

/// `picker` is the config to switch sessions with when no name is given and
/// the choice is put to the person; `None` lists the sessions and exits instead.
fn attach_with_session_name(
    session_name: Option<SessionName>,
    config_options: Options,
    create: bool,
    picker: Option<&Config>,
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
        Some(prefix) => match match_session_name(prefix.as_str()) {
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
                    current_session().as_ref(),
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
                eprintln!("No active yinz sessions found.");
                process::exit(1);
            },
            ActiveSession::One(session_name) => ClientInfo::Attach(session_name, config_options),
            ActiveSession::Many => match picker {
                Some(config) => pick_session_to_attach(config_options, config),
                None => {
                    println!("Please specify the session to attach to, either by using the full name or a unique prefix.\nThe following sessions are active:");
                    list_sessions(false, false, true, false);
                    process::exit(1);
                },
            },
        },
    }
}

fn handle_client_outcome(outcome: yinz_client::ClientOutcome) -> Option<SessionConnection> {
    if outcome.exit_status != 0 {
        std::process::exit(outcome.exit_status);
    }
    outcome.reconnect_to_session
}

pub(crate) fn start_client(opts: CliArgs) {
    let (
        config,
        client_layout_info,
        config_options,
        mut config_without_layout,
        mut config_options_without_layout,
    ) = match startup_stage("cli.setup", || Setup::from_cli_args(&opts)) {
        Ok(results) => results,
        Err(e) => {
            log::error!("startup trace: client configuration failed");
            if let ConfigError::KdlError(error) = e {
                let report: Report = error.into();
                eprintln!("{:?}", report);
            } else {
                eprintln!("{}", e);
            }
            process::exit(1);
        },
    };

    let mut reconnect_to_session: Option<SessionConnection> = None;
    let os_input = startup_stage("cli.open_terminal", || get_os_input(get_client_os_input));
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
                    session_name: reconnect_to_session
                        .name
                        .clone()
                        .map(SessionName::into_string),
                    create: true,
                    remote: None,
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

        let start_client_plan = |session_name: SessionName| {
            startup_stage("cli.validate_session_name", || {
                assert_session_ne(session_name.as_str());
            });
        };

        if let Some(Command::Sessions(Sessions::Attach {
            session_name,
            create,
            create_background,
            force_run_commands,
            index,
            options,
            remote: _,
        })) = opts.command.clone()
        {
            let config_options = match options.as_deref() {
                Some(SessionCommand::Options(o)) => {
                    yinz_cli::options::merge_from_cli(&config_options, o.to_owned().into())
                },
                None => config_options,
            };
            let session_name = session_name.map(SessionName::new);
            should_create_detached = create_background;

            let mut client = if let Some(idx) = index {
                attach_with_session_index(
                    config_options.clone(),
                    idx,
                    create || should_create_detached,
                )
            } else {
                let session_exists = session_name
                    .as_ref()
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
                            session_layout_cache_file_name(session_name),
                            force_run_commands,
                            new_session_cwd.clone(),
                        )
                    },
                    _ => attach_with_session_name(
                        session_name,
                        config_options.clone(),
                        create || should_create_detached,
                        (!should_create_detached && can_pick_a_session()).then_some(&config),
                    ),
                }
            };

            if let Some(val) = current_session()
                && &val == client.get_session_name()
            {
                panic!("You are trying to attach to the current session (\"{}\"). This is not supported.", val);
            }

            if let Some(layout_info) = layout_info {
                client.set_layout_info(layout_info);
            }

            if let Some(new_session_cwd) = new_session_cwd {
                client.set_cwd(new_session_cwd);
            }

            let tab_number_to_focus = reconnect_to_session.as_ref().and_then(|r| r.tab_position);
            let pane_id_to_focus = reconnect_to_session.as_ref().and_then(|r| r.pane_id);
            reconnect_to_session = handle_client_outcome(start_client_impl(
                Box::new(os_input),
                opts,
                config,
                config_options,
                client,
                tab_number_to_focus,
                pane_id_to_focus,
                is_a_reconnect,
                should_create_detached,
            ));
        } else {
            if let Some(session_name) = opts.session.clone().map(SessionName::new) {
                start_client_plan(session_name.clone());
                reconnect_to_session = handle_client_outcome(start_client_impl(
                    Box::new(os_input),
                    opts,
                    config,
                    config_options,
                    ClientInfo::New(session_name, layout_info, new_session_cwd),
                    None,
                    None,
                    is_a_reconnect,
                    should_create_detached,
                ));
            } else {
                if let Some(session_name) = config_options.session_name.as_ref() {
                    if let Some(val) = current_session() {
                        // This prevents the same type of recursion as above, only that here we
                        // don't get the command to "attach", but to start a new session instead.
                        // This occurs for example when declaring the session name inside a layout
                        // file and then, from within this session, trying to open a new yinz
                        // session with the same layout. This causes an infinite recursion in the
                        // `yinz_pty::terminal_bytes::listen` task, flooding the server and
                        // clients with infinite `Render` requests.
                        if *session_name == val {
                            eprintln!("You are trying to attach to the current session (\"{}\"). Yinz does not support nesting a session in itself.", session_name);
                            process::exit(1);
                        }
                    }
                    match config_options.attach_to_session {
                        Some(true) => {
                            let client = attach_with_session_name(
                                Some(session_name.clone()),
                                config_options.clone(),
                                true,
                                None,
                            );
                            reconnect_to_session = handle_client_outcome(start_client_impl(
                                Box::new(os_input),
                                opts,
                                config,
                                config_options,
                                client,
                                None,
                                None,
                                is_a_reconnect,
                                should_create_detached,
                            ));
                        },
                        _ => {
                            start_client_plan(session_name.clone());
                            reconnect_to_session = handle_client_outcome(start_client_impl(
                                Box::new(os_input),
                                opts,
                                config,
                                config_options.clone(),
                                ClientInfo::New(session_name.clone(), layout_info, new_session_cwd),
                                None,
                                None,
                                is_a_reconnect,
                                should_create_detached,
                            ));
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
                reconnect_to_session = handle_client_outcome(start_client_impl(
                    Box::new(os_input),
                    opts,
                    config,
                    config_options,
                    ClientInfo::New(session_name, layout_info, new_session_cwd),
                    None,
                    None,
                    is_a_reconnect,
                    should_create_detached,
                ));
            }
        }
        if reconnect_to_session.is_none() {
            break;
        }
    }
}

fn generate_unique_session_name_or_exit() -> SessionName {
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

    let session_name = session_name.or(opts.session.clone());
    // Resolve the session name to watch
    let client_info = match &session_name {
        Some(prefix) => match match_session_name(prefix.as_str()).unwrap() {
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
                    current_session().as_ref(),
                );
                process::exit(1);
            },
            SessionNameMatch::None => {
                eprintln!("No session with the name '{}' found!", prefix);
                process::exit(1);
            },
        },
        None if yinz_process::pane_context::inside_pane() => ClientInfo::Watch(
            resolve_session_for_stream(None, "watch"),
            config_options.clone(),
        ),
        None => match get_active_session() {
            ActiveSession::None => {
                eprintln!("No active yinz sessions found.");
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
    handle_client_outcome(start_client_impl(
        Box::new(os_input),
        opts,
        config,
        config_options,
        client_info,
        None,  // tab_number_to_focus
        None,  // pane_id_to_focus
        false, // is_a_reconnect
        false, // should_create_detached
    ));
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
pub(crate) fn pty_hosts(command: yinz_cli::args::PtyHosts) -> anyhow::Result<()> {
    use yinz_cli::args::PtyHosts as PtyHostsCli;
    use yinz_server::pty_host::cli;
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

/// `yinz pty-host adopt` — put a surviving pane back on screen.
///
/// Resolves the supervisor here, where the failures are legible without a
/// session, then hands the pane to one as an ordinary CLI action.
#[cfg(unix)]
pub(crate) fn pty_host_adopt(
    command: yinz_cli::args::PtyHosts,
    requested_session_name: Option<String>,
    config: Option<Config>,
) {
    use yinz_cli::args::PtyHosts as PtyHostsCli;
    use yinz_server::pty_host::cli;
    use yinz_types::input::actions::warp_placement;

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

    let (session, caller) = command_target(session.or(requested_session_name));
    let session = session.map(SessionName::into_string);
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

    let os_input = get_os_input(yinz_client::os_input_output::get_cli_client_os_input);
    let exit_status = yinz_client::cli_client::start_cli_client(
        Box::new(os_input),
        &SessionName::new(session),
        vec![Action::AdoptPane {
            payload: Box::new(payload),
            placement: warp_placement(focused_tab, floating),
        }],
        caller,
    );
    let _ = config;
    std::process::exit(exit_status);
}

/// `yinz plugins ...`: the plugin folder is the session's - `user_plugin_dir` in the config,
/// or `<config dir>/plugins` - so what is written here is what `plugin_reload` and a layout's
/// `file:` location find.
pub(crate) fn plugins(command: yinz_cli::args::Plugins, opts: &CliArgs) -> anyhow::Result<()> {
    use anyhow::Context;
    use yinz_cli::args::Plugins as PluginsCli;
    let config = if matches!(
        command,
        PluginsCli::Install { .. }
            | PluginsCli::Update { .. }
            | PluginsCli::Status { .. }
            | PluginsCli::Reload { .. }
            | PluginsCli::Start { .. }
            | PluginsCli::Stop { .. }
            | PluginsCli::Check { .. }
    ) {
        yinz_config::config::Config::validate_plugin_overlays(
            opts.config.as_deref(),
            opts.config_dir.as_deref(),
        )
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        Some(
            yinz_cli::setup::load_config(opts)
                .map_err(|error| anyhow::anyhow!(error.to_string()))?,
        )
    } else {
        yinz_cli::setup::load_config(opts).ok()
    };
    let plugin_dir = || {
        yinz_runtime::home::user_plugin_dir_or_default(
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
        })
    };
    if let PluginsCli::Test {
        name,
        file,
        kind,
        filter,
        timeout_ms,
        json,
    } = command
    {
        let options = yinz_types::authoring::PluginTestOptions {
            kind: serde_json::from_value(serde_json::Value::String(kind))?,
            filter,
            timeout_ms,
        };
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let signals = [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM]
            .map(|signal| signal_hook::flag::register(signal, cancel.clone()))
            .into_iter()
            .collect::<std::io::Result<Vec<_>>>()?;
        let report = match (name, file) {
            (_, Some(path)) => yinz_server::plugin_testing::run(
                &opts
                    .data_dir
                    .clone()
                    .unwrap_or_else(yinz_runtime::home::get_default_data_dir),
                &path,
                &options,
                None,
                &cancel,
            )?,
            (Some(name), None) => {
                let directory = plugin_dir().ok();
                let directory = directory.as_deref().unwrap_or(std::path::Path::new(""));
                yinz_server::plugin_testing::run_named(
                    &opts
                        .data_dir
                        .clone()
                        .unwrap_or_else(yinz_runtime::home::get_default_data_dir),
                    directory,
                    &name,
                    &options,
                    None,
                    &cancel,
                )?
            },
            _ => anyhow::bail!("give a plugin name or --file"),
        };
        for signal in signals {
            signal_hook::low_level::unregister(signal);
        }
        if json {
            println!("{}", serde_json::to_string(&report)?);
        } else {
            for test in &report.tests {
                println!("{} {} ({}ms)", test.status, test.name, test.duration_ms);
                if let Some(error) = &test.error {
                    eprintln!("{error}\n{}", test.output);
                }
            }
        }
        if !report.passed() {
            anyhow::bail!("plugin tests failed");
        }
        return Ok(());
    }
    use yinz_types::cli::PluginOperation;
    let lifecycle = match &command {
        PluginsCli::Start {
            name,
            configuration_json,
            json,
        } => Some((
            PluginOperation::Start,
            Some(name.as_str()),
            configuration_json.as_deref(),
            *json,
        )),
        PluginsCli::Reload {
            name,
            configuration_json,
            json,
            ..
        } => Some((
            PluginOperation::Reload,
            Some(name.as_str()),
            configuration_json.as_deref(),
            *json,
        )),
        PluginsCli::Stop {
            name,
            configuration_json,
            json,
        } => Some((
            PluginOperation::Stop,
            Some(name.as_str()),
            configuration_json.as_deref(),
            *json,
        )),
        PluginsCli::List {
            running: true,
            json,
        } => Some((PluginOperation::List, None, None, *json)),
        _ => None,
    };
    if let Some((operation, target, configuration, json)) = lifecycle {
        let (session, caller) =
            resolve_session_for_command(opts.session.clone(), "manage plugins in");
        let result = plugin_lifecycle_request(
            operation,
            target,
            configuration,
            config.as_ref(),
            plugin_dir().ok().as_deref(),
            caller,
        )
        .and_then(|request| yinz_client::store_client::plugin_request(&session, request));
        if json {
            match &result {
                Ok(response) => println!("{}", serde_json::to_string(response)?),
                Err(error) => println!("{}", serde_json::json!({"error": format!("{error:#}")})),
            }
        } else if let Ok(response) = &result {
            for plugin in &response.plugins {
                println!(
                    "plugin_{} {} {} {}",
                    plugin.id,
                    serde_json::to_value(plugin.state)?
                        .as_str()
                        .unwrap_or_default(),
                    plugin.location,
                    serde_json::to_string(&plugin.configuration)?
                );
            }
        }
        result?;
        return Ok(());
    }
    let update_repositories = matches!(command, PluginsCli::Update { .. });
    let repository_status = matches!(command, PluginsCli::Status { .. });
    match command {
        PluginsCli::Install { aliases }
        | PluginsCli::Update { aliases }
        | PluginsCli::Status { aliases } => {
            use yinz_types::input::layout::RunPluginLocation;
            let config = config
                .as_ref()
                .context("repository commands require configuration")?;
            let repositories = config
                .plugins
                .aliases
                .iter()
                .filter_map(|(name, plugin)| match &plugin.location {
                    RunPluginLocation::Git(source) => Some((name, source)),
                    _ => None,
                })
                .collect::<std::collections::BTreeMap<_, _>>();
            let selected = if aliases.is_empty() {
                repositories.keys().map(|name| (*name).clone()).collect()
            } else {
                aliases
            };
            for name in &selected {
                if !repositories.contains_key(name) {
                    anyhow::bail!("{name} is not a configured repository plugin");
                }
            }
            let data_dir = opts
                .data_dir
                .clone()
                .unwrap_or_else(yinz_runtime::home::get_default_data_dir);
            let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let signals = [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM]
                .map(|signal| signal_hook::flag::register(signal, cancel.clone()))
                .into_iter()
                .collect::<std::io::Result<Vec<_>>>()?;
            let result = (|| -> anyhow::Result<()> {
                let mut completed = std::collections::BTreeSet::new();
                for name in selected {
                    let source = repositories[&name];
                    if !repository_status
                        && completed.insert((source.repo.clone(), source.revision.clone()))
                    {
                        let entries = repositories
                            .values()
                            .filter(|other| {
                                other.repo == source.repo && other.revision == source.revision
                            })
                            .map(|other| (*other).clone())
                            .collect::<Vec<_>>();
                        yinz_server::plugin_authoring::git::install(
                            &data_dir,
                            source,
                            &entries,
                            update_repositories,
                            &cancel,
                        )?;
                    }
                    let installed = yinz_server::plugin_authoring::git::status(&data_dir, source)?;
                    let selector = source
                        .revision
                        .as_ref()
                        .map(|value| {
                            let (key, value) = value.property();
                            format!("{key}={value}")
                        })
                        .unwrap_or_else(|| {
                            if source.is_local() {
                                "local".into()
                            } else {
                                "default branch".into()
                            }
                        });
                    let revision = installed
                        .as_ref()
                        .map(|value| value.commit.as_deref().unwrap_or("user-managed"))
                        .unwrap_or("not installed");
                    println!(
                        "{name}: {} ({selector}), entrypoint={}, revision={revision}",
                        source.repo,
                        source.entrypoint.display()
                    );
                }
                Ok(())
            })();
            for signal in signals {
                signal_hook::low_level::unregister(signal);
            }
            result
        },
        PluginsCli::Test { .. } => unreachable!("tests return above"),
        PluginsCli::Docs { section } => {
            let content = match section.as_str() {
                "index" => "guide: plugin authoring guide\nsdk: Steel Scheme SDK reference\nprotocol: command and event shapes\ntesting: plugin test API\nrepositories: Git repository plugins",
                "guide" => concat!(include_str!("../docs/PLUGINS.md"), "\n\nClock example:\n\n", include_str!("../example-plugins/clock.scm"), "\n\nPermission and command example:\n\n", include_str!("../example-plugins/capabilities.scm")),
                "repositories" => include_str!("../docs/GIT_PLUGINS.md"),
                "sdk" => include_str!("../docs/PLUGIN_SDK.md"),
                "protocol" => include_str!("../docs/PLUGIN_PROTOCOL.md"),
                "testing" => include_str!("../docs/PLUGIN_TESTING.md"),
                _ => anyhow::bail!("unknown docs section; use index, guide, sdk, protocol, testing or repositories"),
            };
            println!(
                "{}",
                serde_json::json!({"section":section,"content":content})
            );
            Ok(())
        },
        PluginsCli::List { .. } => {
            fn sources(root: &Path, paths: &mut Vec<PathBuf>) -> anyhow::Result<()> {
                if !root.exists() {
                    return Ok(());
                }
                for entry in std::fs::read_dir(root)? {
                    let entry = entry?;
                    let kind = entry.file_type()?;
                    if kind.is_dir() {
                        sources(&entry.path(), paths)?;
                    } else if kind.is_file()
                        && yinz_plugin_api::authoring::is_script_plugin(&entry.path())
                    {
                        paths.push(entry.path());
                    }
                }
                Ok(())
            }
            let directory = plugin_dir()?;
            let mut files = Vec::new();
            sources(&directory, &mut files)?;
            files.sort();
            println!(
                "{}",
                serde_json::json!({"directory":directory,"files":files})
            );
            Ok(())
        },
        PluginsCli::Edit { name, file } => {
            let edits: Vec<yinz_plugin_api::authoring::edits::EditPair> =
                serde_json::from_str(&read_source(Some(&file))?)?;
            let written = yinz_server::plugin_authoring::edit(&plugin_dir()?, &name, &edits)?;
            println!(
                "{}",
                serde_json::json!({"path":written.path,"backup":written.replaced})
            );
            Ok(())
        },
        PluginsCli::Dir => {
            println!("{}", plugin_dir()?.display());
            Ok(())
        },
        PluginsCli::Write {
            name,
            file,
            overwrite,
            no_check,
        } => {
            let source = read_source(file.as_deref())?;
            let written = yinz_server::plugin_authoring::write(
                &plugin_dir()?,
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
            // The write already checked syntax, so only the warnings are wanted here.
            print_plugin_findings(
                &yinz_server::plugin_authoring::check_source(
                    &written.path,
                    &source,
                    yinz_types::authoring::PluginCheckMode::Syntax,
                )
                .warnings,
            );
            Ok(())
        },
        PluginsCli::Check { name, file, json } => {
            if file.is_none()
                && let Some(yinz_types::input::layout::RunPluginLocation::Git(source)) = name
                    .as_ref()
                    .and_then(|name| config.as_ref()?.plugins.aliases.get(name))
                    .map(|plugin| &plugin.location)
            {
                let data_dir = opts
                    .data_dir
                    .clone()
                    .unwrap_or_else(yinz_runtime::home::get_default_data_dir);
                let installed = yinz_server::plugin_authoring::git::status(&data_dir, source)?
                    .context("repository is not installed; use plugins install")?;
                let report = yinz_server::plugin_authoring::check_repository(&installed)?;
                if json {
                    println!("{}", serde_json::to_string(&report)?);
                }
                if !report.diagnostics.is_empty() {
                    anyhow::bail!("{}", report.diagnostics.join("\n"));
                }
                if !json {
                    println!("{} checks successfully", installed.entrypoint.display());
                    print_plugin_findings(&report.warnings);
                }
                return Ok(());
            }

            let path = match (name, file) {
                (_, Some(file)) => file,
                (Some(name), None) => {
                    if let Some(plugin) = config
                        .as_ref()
                        .and_then(|config| config.plugins.aliases.get(&name))
                    {
                        if !matches!(
                            plugin.location,
                            yinz_types::input::layout::RunPluginLocation::File(_)
                        ) {
                            anyhow::bail!(
                                "source checking is unsupported for builtin alias {name:?}"
                            );
                        }
                        let plugin = yinz_config::plugins::PluginConfig::from_run_plugin(plugin)
                            .with_context(|| {
                                format!("source checking is unsupported for alias {name:?}; expected a Scheme file")
                            })?;
                        plugin.resolve_source(plugin_dir().ok().as_deref())?.0
                    } else if let Some(path) =
                        yinz_server::plugin_authoring::resolve_explicit_plugin_path(
                            &name,
                            &std::env::current_dir().context("resolve plugin working directory")?,
                        )?
                    {
                        path
                    } else {
                        yinz_plugin_api::authoring::resolve_plugin_file(&plugin_dir()?, &name)
                            .map_err(|e| anyhow::anyhow!(e))?
                    }
                },
                (None, None) => anyhow::bail!("give a plugin name, or --file"),
            };
            if json {
                let source = read_source(Some(&path))?;
                let report = yinz_server::plugin_authoring::check_source(
                    &path,
                    &source,
                    yinz_types::authoring::PluginCheckMode::Semantic,
                );
                println!("{}", serde_json::to_string(&report)?);
                if !report.diagnostics.is_empty() {
                    anyhow::bail!("plugin checks failed");
                }
                return Ok(());
            }
            let report = yinz_server::plugin_authoring::check_plugin_report(&path)?;
            println!("{} checks successfully", path.display());
            print_plugin_findings(&report.warnings);
            Ok(())
        },
        PluginsCli::Format { name, file, check } => {
            let path = match (name, file) {
                (_, Some(file)) => file,
                (Some(name), None) => {
                    yinz_plugin_api::authoring::resolve_plugin_file(&plugin_dir()?, &name)
                        .map_err(|e| anyhow::anyhow!(e))?
                },
                (None, None) => anyhow::bail!("give a plugin name, or --file"),
            };
            yinz_server::plugin_authoring::format_file(&path, check)?;
            println!("{} is formatted", path.display());
            Ok(())
        },
        PluginsCli::Start { .. } | PluginsCli::Reload { .. } | PluginsCli::Stop { .. } => {
            unreachable!("lifecycle commands return above")
        },
        PluginsCli::Log {
            name,
            lines,
            configuration_json,
            json,
        } => {
            let (requested, caller, target_error) = match yinz_process::pane_context::target(
                opts.session.clone().map(SessionName::new),
            ) {
                Ok((requested, caller)) => (requested, caller, None),
                Err(error) => (None, None, Some(format!("{error:#}"))),
            };
            let session = requested.or_else(|| {
                if target_error.is_none() {
                    match get_active_session() {
                        ActiveSession::One(session) => Some(session),
                        _ => None,
                    }
                } else {
                    None
                }
            });
            let status = if let Some(session) = session {
                let request = plugin_lifecycle_request(
                    PluginOperation::Inspect,
                    Some(&name),
                    configuration_json.as_deref(),
                    config.as_ref(),
                    plugin_dir().ok().as_deref(),
                    caller,
                )?;
                match yinz_client::store_client::plugin_request(&session, request) {
                    Err(error)
                        if error.is::<yinz_client::store_client::PluginRequestRejected>() =>
                    {
                        if json {
                            println!("{}", serde_json::json!({"error": format!("{error:#}")}));
                        }
                        return Err(error);
                    },
                    response => Some(response),
                }
            } else {
                None
            };
            let runtime = match &status {
                Some(Ok(response)) if response.plugins.is_empty() => "not loaded".to_owned(),
                Some(Ok(response)) => response
                    .plugins
                    .iter()
                    .map(|plugin| {
                        format!(
                            "plugin_{}: {}",
                            plugin.id,
                            serde_json::to_value(plugin.state)
                                .unwrap_or_default()
                                .as_str()
                                .unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
                Some(Err(error)) => format!("runtime status unknown: {error:#}"),
                None => format!(
                    "runtime status unknown: {}",
                    target_error.as_deref().unwrap_or("no available session")
                ),
            };
            let log_name = status
                .as_ref()
                .and_then(|status| status.as_ref().ok())
                .and_then(|response| response.plugins.first())
                .map(|plugin| plugin.location.as_str())
                .unwrap_or(&name);

            let target = yinz_server::plugin_authoring::resolve_log_target(
                log_name,
                plugin_dir().ok().as_deref(),
                &config
                    .as_ref()
                    .map(|config| config.plugins.clone())
                    .unwrap_or_default(),
                &std::env::current_dir().context("resolve log working directory")?,
            )?;
            let log_file = yinz_runtime::consts::YINZ_TMP_LOG_FILE.clone();
            let output = if log_file.exists() {
                yinz_server::plugin_authoring::log_target_tail(&log_file, &target, lines)?
            } else {
                String::new()
            };
            if json {
                println!(
                    "{}",
                    serde_json::json!({"runtime":runtime,"historical":true,"output":output})
                );
            } else {
                println!("{runtime}");
                if output.is_empty() {
                    println!("no log lines");
                } else {
                    println!("Historical log output:");
                    print!("{output}");
                }
            }
            Ok(())
        },
    }
}

fn plugin_lifecycle_request(
    operation: yinz_types::cli::PluginOperation,
    target: Option<&str>,
    configuration: Option<&str>,
    config: Option<&yinz_config::config::Config>,
    directory: Option<&Path>,
    caller: Option<TerminalId>,
) -> anyhow::Result<yinz_types::cli::PluginRequest> {
    use anyhow::Context;
    use yinz_types::input::layout::{RunPlugin, RunPluginLocation};
    let cwd = std::env::current_dir().context("resolve plugin working directory")?;
    let aliases = config
        .map(|config| config.plugins.clone())
        .unwrap_or_default();
    let fallback = if let Some(target) = target.filter(|target| {
        !target
            .strip_prefix("plugin_")
            .is_some_and(|id| id.parse::<u32>().is_ok())
    }) {
        Some(if let Some(run) = aliases.aliases.get(target) {
            run.clone()
        } else if target.starts_with("file:") {
            yinz_runtime::plugin_location::run_plugin_from_url_with_cwd(target, Some(cwd.clone()))?
        } else if let Some(path) =
            yinz_server::plugin_authoring::resolve_explicit_plugin_path(target, &cwd)?
        {
            RunPlugin {
                location: RunPluginLocation::File(path),
                ..Default::default()
            }
        } else if target.contains(':') {
            RunPlugin {
                location: yinz_runtime::plugin_location::parse_plugin_location(
                    target,
                    Some(cwd.clone()),
                )?,
                ..Default::default()
            }
        } else {
            let path = yinz_plugin_api::authoring::resolve_plugin_file(
                directory.context("no plugin directory configured")?,
                target,
            )
            .map_err(anyhow::Error::msg)?;
            RunPlugin {
                location: RunPluginLocation::File(path),
                ..Default::default()
            }
        })
    } else {
        None
    };
    let mut candidates = vec![];
    if let Some(config) = config {
        for declaration in &config.background_plugins {
            let mut declaration = declaration.clone();
            declaration.populate_run_plugin_if_needed(&aliases);
            candidates.push(
                declaration
                    .get_run_plugin()
                    .context("unknown load_plugins alias")?,
            );
        }
    }
    Ok(yinz_types::cli::PluginRequest {
        operation,
        target: target.map(str::to_owned),
        configuration: configuration
            .map(serde_json::from_str)
            .transpose()
            .context("--configuration-json must be an object of string values")?,
        fallback,
        candidates,
        caller,
    })
}

/// What the source is going to do wrong at run time, on stderr, under whatever the command
/// printed. Compiling is the verdict; these are the traps a compile cannot see.
fn print_plugin_findings(findings: &[String]) {
    for finding in findings {
        eprintln!("warning: {finding}");
    }
}

pub(crate) fn read_source(file: Option<&Path>) -> anyhow::Result<String> {
    match file {
        Some(file) if file != Path::new("-") => std::fs::read_to_string(file)
            .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", file.display())),
        _ => {
            use std::io::Read;
            let mut source = String::new();
            std::io::stdin()
                .read_to_string(&mut source)
                .map_err(|e| anyhow::anyhow!("failed to read the plugin source from stdin: {e}"))?;
            Ok(source)
        },
    }
}

#[cfg(test)]
#[path = "unit/plugin_lifecycle.rs"]
mod plugin_lifecycle_tests;

pub(crate) fn data(command: yinz_cli::args::Data, opts: &CliArgs) -> anyhow::Result<()> {
    use anyhow::Context;
    use yinz_cli::args::Data;
    use yinz_types::cli::DataRequest;
    let request = match command {
        Data::Catalog { namespace } => DataRequest::Catalog { namespace },
        Data::Apply { namespace, file } => DataRequest::Apply {
            namespace,
            mutations: serde_json::from_str(&read_source(Some(&file))?)
                .context("expected a Mutation JSON array")?,
        },
        command @ (Data::Fetch { .. } | Data::Count { .. }) => {
            let count = matches!(command, Data::Count { .. });
            let (Data::Fetch { namespace, file } | Data::Count { namespace, file }) = command
            else {
                unreachable!()
            };
            let mut request: yinz_types::plugin_data::FetchRequest =
                serde_json::from_str(&read_source(Some(&file))?)
                    .context("expected FetchRequest JSON")?;
            if request
                .namespace
                .as_ref()
                .is_some_and(|value| value != &namespace)
            {
                anyhow::bail!("--namespace conflicts with the request namespace");
            }
            request.namespace = Some(namespace);
            if count {
                DataRequest::Count(request)
            } else {
                DataRequest::Fetch(request)
            }
        },
    };
    let session = resolve_session_for_stream(opts.session.clone(), "query data in");
    println!(
        "{}",
        yinz_client::store_client::data_request(&session, request)?
    );
    Ok(())
}

/// Select the only active session after explicit and caller session selection.
pub(crate) fn resolve_session(
    active: ActiveSession,
) -> yinz_runtime::errors::prelude::Result<String> {
    use yinz_runtime::errors::prelude::*;

    match active {
        ActiveSession::One(name) => Some(name.into_string()),
        ActiveSession::None | ActiveSession::Many => None,
    }
    .context("no session selected; pass --session SESSION")
}

#[cfg(test)]
mod tests {
    use super::{
        kill_outcome, named_session_exists_or_error, sessions_to_delete, sessions_to_kill,
    };
    use std::{cell::Cell, io, time::Duration};
    use {
        yinz_cli::sessions::session_listing_error_message,
        yinz_cli::sessions::unresponsive_session_message, yinz_ipc::probe::SessionProbe,
    };

    fn session(name: &str) -> (yinz_types::internal_ids::SessionName, Duration) {
        (name.into(), Duration::default())
    }

    #[test]
    fn session_selection_requires_one_active_session() {
        use yinz_cli::sessions::ActiveSession;

        assert_eq!(
            super::resolve_session(ActiveSession::One("work".into())).unwrap(),
            "work"
        );
        for active in [ActiveSession::None, ActiveSession::Many] {
            assert_eq!(
                super::resolve_session(active).unwrap_err().to_string(),
                "no session selected; pass --session SESSION"
            );
        }
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
                (
                    "alive".to_owned().into(),
                    Duration::default(),
                    SessionProbe::Alive,
                ),
                (
                    "wedged".to_owned().into(),
                    Duration::default(),
                    SessionProbe::Unresponsive,
                ),
                (
                    "absent".to_owned().into(),
                    Duration::default(),
                    SessionProbe::Absent,
                ),
            ])
        })
        .unwrap();

        assert_eq!(sessions, vec![session("alive"), session("wedged")]);
    }

    #[test]
    fn a_kill_requires_completion_and_always_cleans_up_unconfirmed_shutdowns() {
        use std::cell::Cell;
        for probe in [SessionProbe::Alive, SessionProbe::Unresponsive] {
            assert!(kill_outcome(Ok(probe), || Ok(()), || panic!("already complete")).is_ok());
            for kind in [
                io::ErrorKind::TimedOut,
                io::ErrorKind::UnexpectedEof,
                io::ErrorKind::ConnectionRefused,
                io::ErrorKind::BrokenPipe,
                io::ErrorKind::InvalidData,
                io::ErrorKind::Other,
            ] {
                let swept = Cell::new(false);
                let result = kill_outcome(
                    Ok(probe),
                    || Err(kind.into()),
                    || {
                        swept.set(true);
                        Ok(1)
                    },
                );
                assert!(swept.get());
                assert!(result.is_err(), "fallback must not confirm server shutdown");
            }
        }
        assert!(kill_outcome(Ok(SessionProbe::Absent), || panic!("absent"), || Ok(1)).is_ok());
        assert!(kill_outcome(Ok(SessionProbe::Absent), || panic!("absent"), || Ok(0)).is_err());
        for probe in [
            Ok(SessionProbe::Absent),
            Err(io::ErrorKind::PermissionDenied),
        ] {
            let result = kill_outcome(
                probe,
                || panic!("not deliverable"),
                || Err("cleanup failed".into()),
            );
            assert!(result.unwrap_err().contains("cleanup failed"));
        }
    }

    #[test]
    fn only_an_absent_named_session_allows_creation() {
        let creation_allowed = |probe| {
            named_session_exists_or_error(
                &yinz_types::internal_ids::SessionName::new("named"),
                probe,
            )
            .map(|session_exists| !session_exists)
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
