//! Remote CLI routing. Inputs stay local; session requests use an owned SSH forward.
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    process::{Command as ProcessCommand, Stdio},
    sync::OnceLock,
    time::Duration,
};
use yinz_cli::args::{Agent, CliArgs, Command, Plugins, Sessions};
use yinz_client::{
    cli_client::StreamEnd,
    remote_sessions::{self, StartupCancellation},
    session_target::{SessionEndpoint, SessionTarget},
};
use yinz_config::config::Config;
use yinz_process::ssh;
use yinz_runtime::{
    consts::{CLIENT_SERVER_CONTRACT_VERSION, REMOTE_BOOTSTRAP_VERSION, VERSION},
    errors::prelude::*,
};
use yinz_types::internal_ids::{SessionName, TerminalId};

#[derive(Debug, PartialEq, Eq)]
enum Route {
    New,
    Attach,
    Session,
    Host,
    Local,
}

// Deliberately exhaustive: adding a command requires deciding which machine owns it.
fn route(command: Option<&Command>) -> Route {
    match command {
        None => Route::New,
        Some(Command::Sessions(command)) => match command {
            Sessions::Attach { .. } => Route::Attach,
            Sessions::Watch { .. }
            | Sessions::Run { .. }
            | Sessions::Edit { .. }
            | Sessions::Plugin { .. }
            | Sessions::Pipe { .. } => Route::Session,
            Sessions::ListSessions { .. }
            | Sessions::KillSession { .. }
            | Sessions::DeleteSession { .. }
            | Sessions::KillAllSessions { .. }
            | Sessions::DeleteAllSessions { .. } => Route::Host,
            Sessions::ListAliases => Route::Local,
        },
        Some(
            Command::Action(_)
            | Command::Repl { .. }
            | Command::Subscribe(_)
            | Command::Events(_)
            | Command::Data(_),
        ) => Route::Session,
        Some(Command::Agent(command)) => match command {
            Agent::Claude { .. }
            | Agent::Codex { .. }
            | Agent::CodexUntracked { .. }
            | Agent::Muse { .. }
            | Agent::CodexReport
            | Agent::MuseReport => Route::Local,
            Agent::Watch { .. }
            | Agent::Archive(_)
            | Agent::Register { .. }
            | Agent::List { .. }
            | Agent::Status { .. }
            | Agent::Spawn { .. }
            | Agent::Show { .. }
            | Agent::Hide { .. }
            | Agent::Restart { .. }
            | Agent::Prompt { .. }
            | Agent::Wait { .. }
            | Agent::Read { .. }
            | Agent::ClearDraft { .. }
            | Agent::Recover { .. }
            | Agent::SendKeys { .. } => Route::Session,
        },
        Some(Command::Plugins(command)) => match command {
            Plugins::Start { .. }
            | Plugins::Stop { .. }
            | Plugins::Reload { .. }
            | Plugins::Log { .. }
            | Plugins::List { running: true, .. } => Route::Host,
            Plugins::Install { .. }
            | Plugins::Update { .. }
            | Plugins::Status { .. }
            | Plugins::Docs { .. }
            | Plugins::List { running: false, .. }
            | Plugins::Edit { .. }
            | Plugins::Test { .. }
            | Plugins::Write { .. }
            | Plugins::Check { .. }
            | Plugins::Format { .. }
            | Plugins::Dir => Route::Local,
        },
        #[cfg(unix)]
        Some(Command::PtyHosts(command)) => match command {
            yinz_cli::args::PtyHosts::Adopt { .. } => Route::Host,
            yinz_cli::args::PtyHosts::List { .. }
            | yinz_cli::args::PtyHosts::Kill { .. }
            | yinz_cli::args::PtyHosts::KillAll => Route::Local,
        },
        Some(
            Command::RemoteCommand { .. }
            | Command::RemoteSessions { .. }
            | Command::RemoteSession { .. }
            | Command::Bubblewrap(_)
            | Command::Skills(_)
            | Command::Options(_)
            | Command::Setup(_)
            | Command::Hooks(_),
        ) => Route::Local,
    }
}

#[derive(Serialize, Deserialize)]
struct Request {
    version: String,
    contract: usize,
    bootstrap: usize,
    command: Option<Command>, // None reads remote configuration; no user input on stdin.
    session: Option<String>,
}
/// Decoded first: a peer's `Command` may not decode here, which is the mismatch to report.
#[derive(Deserialize)]
struct RequestVersions {
    version: String,
    contract: usize,
    bootstrap: usize,
}
struct RemoteContext {
    target: SessionTarget,
    config: Option<Config>,
}
static CONTEXT: OnceLock<RemoteContext> = OnceLock::new();
// Publish termination before closing SSH: EOF can wake the main thread first.
static EXIT_SIGNAL: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static EXIT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
pub(crate) fn exit(status: i32) -> ! {
    // SSH's EOF wakes the main thread while the signal thread is still dropping the
    // forward. Only the thread that finishes cleanup may terminate the process.
    let _exit = EXIT_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    if selected().is_some() {
        remote_sessions::set_forward(None);
    }
    let interrupted = EXIT_SIGNAL.load(std::sync::atomic::Ordering::Acquire);
    std::process::exit(if interrupted == 0 {
        status
    } else {
        interrupted
    });
}

pub(crate) fn selected() -> Option<&'static SessionTarget> {
    CONTEXT.get().map(|c| &c.target)
}
pub(crate) fn config() -> Option<&'static Config> {
    CONTEXT.get().and_then(|c| c.config.as_ref())
}
pub(crate) fn local_cwd() -> Option<PathBuf> {
    if selected().is_some() {
        None
    } else {
        std::env::current_dir().ok()
    }
}
pub(crate) fn target(name: &SessionName) -> SessionTarget {
    selected().cloned().unwrap_or_else(|| SessionTarget {
        name: name.clone(),
        socket: name.socket(),
    })
}
/// The session a command addresses and the pane calling it. A local shell's session and pane
/// identity never apply to a selected remote session.
pub(crate) fn pane_target(
    explicit: Option<SessionName>,
) -> Result<(Option<SessionName>, Option<TerminalId>)> {
    match selected() {
        Some(target) => Ok((Some(target.name.clone()), None)),
        None => yinz_process::pane_context::target(explicit),
    }
}
/// Local streams end when the server closes them. Through SSH, a close without the server's
/// `Exit` means the link dropped, which a caller must be able to tell from a finished stream.
pub(crate) fn finish_stream(end: StreamEnd) {
    if end == StreamEnd::Disconnected && EXIT_SIGNAL.load(std::sync::atomic::Ordering::Acquire) == 0
    {
        eprintln!("Lost the connection to the remote session");
        exit(1);
    }
}

fn ssh_command(
    host: &str,
    request: &Request,
    env: &yinz_types::envs::EnvironmentVariables,
) -> Result<ProcessCommand> {
    ssh::validate_ssh_host(host)?;
    let json = serde_json::to_string(request).context("encode remote CLI request")?;
    let command = ["yinz", "remote-command", &json]
        .into_iter()
        .map(ssh::shell_argument)
        .collect::<Result<Vec<_>>>()?
        .join(" ");
    let mut process = ProcessCommand::new("ssh");
    process
        .envs(env.inner())
        .args(ssh::ssh_forward_args(&Default::default()))
        .args(["-T", "--", host, &command]);
    Ok(process)
}

fn run_host(mut command: ProcessCommand, cancellation: &StartupCancellation) -> Result<i32> {
    // No host command reads input, and an inherited stdin would be consumed by SSH: a caller's
    // `while read host; do yinz list-sessions --remote "$host"; done` would stop after one host.
    let mut child = command
        .stdin(Stdio::null())
        .spawn()
        .context("start remote host command")?;
    let result = (|| loop {
        if cancellation.is_cancelled() {
            return Ok(yinz_client::INTERRUPTED_EXIT_STATUS);
        }
        if let Some(status) = child.try_wait().context("wait for remote host command")? {
            return Ok(status.code().unwrap_or(1));
        }
        std::thread::sleep(Duration::from_millis(25));
    })();
    // Also reap SSH when waiting failed or a signal interrupted it.
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

/// SSH runs host commands without a terminal, so an all-session prompt is answered here and
/// the host receives the answer. Exits when the person declines or interrupts the question.
fn confirm_all_sessions(
    command: &mut Option<Command>,
    host: &str,
    cancellation: &StartupCancellation,
) -> Result<()> {
    let (yes, action) = match command {
        Some(Command::Sessions(Sessions::KillAllSessions { yes })) => (yes, "kill all sessions"),
        Some(Command::Sessions(Sessions::DeleteAllSessions { yes, .. })) => {
            (yes, "delete all resurrectable sessions")
        },
        _ => return Ok(()),
    };
    if *yes {
        return Ok(());
    }
    let answer = crate::remote_session::confirm(
        format!("WARNING: this action will {action} on {host}. Do you want to continue?"),
        false,
    );
    // A signal can end the question with an error as well as an answer.
    if cancellation.is_cancelled() {
        std::process::exit(yinz_client::INTERRUPTED_EXIT_STATUS);
    }
    match answer.context("confirm on a terminal, or pass --yes")? {
        Some(true) => *yes = true,
        Some(false) => {
            println!("Abort.");
            std::process::exit(1);
        },
        None => std::process::exit(yinz_client::INTERRUPTED_EXIT_STATUS),
    }
    Ok(())
}

fn handle(request: &str, opts: &mut CliArgs) -> Result<()> {
    let versions: RequestVersions =
        serde_json::from_str(request).context("decode remote CLI request versions")?;
    remote_sessions::check_versions(&versions.version, versions.contract, versions.bootstrap)?;
    let request: Request = serde_json::from_str(request).context("decode remote CLI request")?;
    let Some(command) = request.command else {
        let config = yinz_cli::setup::load_config(opts).map_err(|e| anyhow!("{e}"))?;
        println!(
            "{}",
            serde_json::to_string(&config.to_string(false).map_err(|e| anyhow!("{e}"))?)
                .context("encode remote configuration")?
        );
        std::process::exit(0);
    };
    anyhow::ensure!(
        route(Some(&command)) == Route::Host,
        "unsupported remote host command"
    );
    // This helper runs before logging and worker threads are initialized. SSH may
    // forward shell context through SendEnv; a host request has no caller pane.
    for key in ["YINZ", "YINZ_SESSION_NAME", "YINZ_PANE_ID", "YINZ_PANE_REF"] {
        // SAFETY: dispatch runs at process startup, before any threads are spawned.
        unsafe { std::env::remove_var(key) };
    }
    *opts = host_options(command, request.session, opts);
    Ok(())
}

/// The request selects the command and session. The configuration location comes from this
/// host's own YINZ_CONFIG_* environment, which clap has already read into `parsed`.
fn host_options(command: Command, session: Option<String>, parsed: &CliArgs) -> CliArgs {
    CliArgs {
        command: Some(command),
        session,
        config: parsed.config.clone(),
        config_dir: parsed.config_dir.clone(),
        ..Default::default()
    }
}

/// An unresponsive or denied session cannot serve a command, so it never counts as the only one.
fn running_names(rows: Vec<yinz_cli::session_picker::PickerRow>) -> Vec<SessionName> {
    rows.into_iter()
        .filter(|row| row.listing == yinz_cli::sessions::SessionListing::Running)
        .filter_map(|row| match row.destination {
            Some(yinz_cli::session_picker::Destination::Remote { name, .. }) => Some(name),
            _ => None,
        })
        .collect()
}

/// Layouts are input files read on this machine, so the host's `layout_dir` must not name
/// where to find them here.
fn parse_remote_config(stdout: &[u8]) -> Result<Config> {
    let kdl: String = serde_json::from_slice(stdout).context("decode remote configuration")?;
    let mut config = Config::from_kdl(&kdl, None).map_err(|e| anyhow!("{e}"))?;
    config.options.layout_dir = None;
    Ok(config)
}

fn read_remote_config(
    host: &str,
    request: &Request,
    env: &yinz_types::envs::EnvironmentVariables,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<Config> {
    let output = ssh::bootstrap_output(
        &mut ssh_command(host, request, env)?,
        Duration::from_secs(25),
        Some(cancelled),
    )?;
    anyhow::ensure!(
        output.status.success(),
        "read remote configuration: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    parse_remote_config(&output.stdout)
}

fn select_name(explicit: Option<&str>, names: Vec<SessionName>, host: &str) -> Result<SessionName> {
    if let Some(name) = explicit {
        remote_sessions::validate_name(name)?;
        return Ok(SessionName::new(name));
    }
    match names.as_slice() {
        [name] => Ok(name.clone()),
        [] => bail!("No active yinz sessions found on {host}"),
        _ => bail!(
            "Specify --session NAME on {host}; active sessions: {}",
            names
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

pub(crate) fn dispatch(opts: &mut CliArgs) -> Result<()> {
    if let Some(Command::RemoteCommand { request }) = &opts.command {
        return handle(&request.clone(), opts);
    }
    let Some(host) = opts.remote.clone() else {
        return Ok(());
    };
    match route(opts.command.as_ref()) {
        Route::New => {
            yinz_runtime::logging::configure_logger();
            std::process::exit(crate::remote_session::start(opts, &host)?);
        },
        Route::Attach => {
            yinz_runtime::logging::configure_logger();
            let Some(Command::Sessions(Sessions::Attach {
                session_name,
                create,
                ..
            })) = &opts.command
            else {
                unreachable!()
            };
            std::process::exit(crate::remote_session::attach(
                opts,
                &host,
                session_name.as_deref(),
                *create,
            )?);
        },
        Route::Local => bail!("this command operates locally and does not support --remote"),
        Route::Host | Route::Session => {},
    }
    anyhow::ensure!(
        opts.server.is_none()
            && opts.pty_host.is_none()
            && opts.server_termios.is_none()
            && opts.max_panes.is_none()
            && opts.new_session_with_layout.is_none()
            && opts.layout.is_none()
            && opts.layout_string.is_none()
            && opts.data_dir.is_none(),
        "remote commands do not support local server, layout, or data-directory overrides"
    );
    let (local_config, _) = crate::remote_session::load_local_config(opts)?;
    let mut request = Request {
        version: VERSION.into(),
        contract: CLIENT_SERVER_CONTRACT_VERSION,
        bootstrap: REMOTE_BOOTSTRAP_VERSION,
        command: None,
        session: opts.session.clone(),
    };
    if route(opts.command.as_ref()) == Route::Host {
        request.command = opts.command.clone();
        // Session-targeting host commands should be as explicit as local commands run outside a pane.
        if let Some(Command::Sessions(
            Sessions::KillSession { target_session }
            | Sessions::DeleteSession { target_session, .. },
        )) = &mut request.command
        {
            *target_session = target_session.clone().or(opts.session.clone());
        }
        let cancellation = StartupCancellation::new()?;
        confirm_all_sessions(&mut request.command, &host, &cancellation)?;
        std::process::exit(run_host(
            ssh_command(&host, &request, &local_config.env)?,
            &cancellation,
        )?);
    }
    let cancellation = StartupCancellation::new()?;
    let setup = (|| -> Result<()> {
        let explicit = match &opts.command {
            Some(Command::Sessions(Sessions::Watch { session_name })) => {
                session_name.as_deref().or(opts.session.as_deref())
            },
            _ => opts.session.as_deref(),
        };
        let names = if explicit.is_none() {
            running_names(remote_sessions::discover(
                &host,
                &cancellation.cancelled,
                &local_config.env,
            )?)
        } else {
            vec![]
        };
        let name = select_name(explicit, names, &host)?;
        // Only actions consume the session's configuration. As locally, one that cannot be read
        // does not stop them, and connecting below reports any SSH or compatibility failure.
        let config = matches!(opts.command, Some(Command::Action(_)))
            .then(|| {
                read_remote_config(&host, &request, &local_config.env, &cancellation.cancelled)
            })
            .and_then(Result::ok);
        let forward = remote_sessions::connect(
            &host,
            name.as_str(),
            false,
            &cancellation.cancelled,
            &local_config.env,
        )?;
        let target = SessionTarget {
            name: name.clone(),
            socket: forward.socket().to_owned(),
        };
        remote_sessions::set_forward(Some(forward));
        CONTEXT
            .set(RemoteContext { target, config })
            .map_err(|_| anyhow!("remote target already selected"))?;
        opts.session = Some(name.into_string());
        opts.remote = None;
        Ok(())
    })();
    if cancellation.is_cancelled() {
        std::process::exit(yinz_client::INTERRUPTED_EXIT_STATUS);
    }
    setup?;
    drop(cancellation);
    // Interactive watchers already install terminal-aware signal handling.
    if !matches!(
        opts.command,
        Some(Command::Sessions(Sessions::Watch { .. }))
    ) {
        let mut signals = signal_hook::iterator::Signals::new([
            signal_hook::consts::SIGINT,
            signal_hook::consts::SIGTERM,
            signal_hook::consts::SIGHUP,
            signal_hook::consts::SIGQUIT,
        ])
        .context("register remote command signals")?;
        std::thread::Builder::new()
            .name("remote-cli-signals".into())
            .spawn(move || {
                if let Some(signal) = signals.forever().next() {
                    EXIT_SIGNAL.store(128 + signal, std::sync::atomic::Ordering::Release);
                    exit(128 + signal);
                }
            })
            .context("start remote command signal handler")?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "unit/remote_commands.rs"]
mod tests;
