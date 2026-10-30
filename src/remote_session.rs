//! A local interactive client with a remote session and an owned SSH tunnel.
#[cfg(test)]
use clap::Parser;
use interprocess::local_socket::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{Duration, Instant};
use yinz_cli::args::{CliArgs, Sessions};
use yinz_cli::session_picker::{Destination, Outcome, PickerRow};
use yinz_cli::setup::Setup;
use yinz_client::os_input_output::{connect_with_deadline, get_client_os_input};
use yinz_client::remote_sessions::{
    check_versions, connect, connect_new, discover, probe, validate_name, Endpoint,
    StartupCancellation,
};
#[cfg(test)]
use yinz_client::remote_sessions::{client_socket, parse_endpoint, parse_sessions};
use yinz_client::{start_client, ClientInfo};
use yinz_ipc::ipc::{ClientToServerMsg, IpcRecv, IpcSenderWithContext, ServerToClientMsg};
use yinz_ipc::probe::SessionProbe;
use yinz_process::ssh;
use yinz_runtime::consts::{
    CLIENT_SERVER_CONTRACT_VERSION, REMOTE_BOOTSTRAP_VERSION, VERSION, YINZ_SOCK_DIR,
};
use yinz_runtime::errors::prelude::*;
use yinz_runtime::termios::WireTermios;
use yinz_session::sessions::{resurrection_layout, session_status};
use yinz_types::data::{LayoutInfo, LayoutMetadata, SessionConnection};
use yinz_types::input::cli_assets::CliAssets;
use yinz_types::input::options::Options;
use yinz_types::internal_ids::SessionName;
use yinz_types::pane_size::Size;

const ENDPOINT_LIMIT: usize = 8192;
const READY_TIMEOUT: Duration = Duration::from_secs(25);
/// The client gives up on a bootstrap after this long, so waiting longer for another creator
/// serves nobody.
const CREATION_LOCK_TIMEOUT: Duration = Duration::from_secs(60);
/// Generated names that a concurrent creator took before giving up.
const GENERATED_NAME_ATTEMPTS: usize = 3;

/// Compatibility is checked before any local or remote startup side effects. `new` creates a
/// session under an untaken name, generating one when `name` is absent.
pub(crate) fn bootstrap(
    name: Option<&str>,
    create: bool,
    new: bool,
    client_version: &str,
    contract: usize,
    bootstrap: usize,
    configuration_options: Option<&str>,
    client_termios: Option<&str>,
    terminal_window_size: Option<&str>,
    opts: &CliArgs,
) -> Result<()> {
    use yinz_client::remote_failure::{BootstrapCategory, BootstrapError};
    check_versions(client_version, contract, bootstrap).map_err(|diagnostic| BootstrapError {
        category: BootstrapCategory::Compatibility,
        diagnostic,
    })?;
    if let Some(name) = name {
        validate_name(name)?;
    }
    let configuration_options = configuration_options
        .map(serde_json::from_str::<Options>)
        .transpose()
        .context("decode configuration options")?;
    let client_termios = client_termios
        .map(serde_json::from_str::<Option<WireTermios>>)
        .transpose()
        .context("decode client terminal settings")?;
    let terminal_window_size = terminal_window_size
        .map(serde_json::from_str::<Size>)
        .transpose()
        .context("decode terminal window size")?
        .unwrap_or(Size { rows: 24, cols: 80 });
    yinz_runtime::logging::configure_logger();
    yinz_runtime::consts::create_config_and_cache_folders();
    yinz_runtime::shared::ensure_secure_dir(&YINZ_SOCK_DIR)?;
    let sock_dir = YINZ_SOCK_DIR
        .canonicalize()
        .context("resolve remote socket directory")?;
    let requested = name;
    let mut attempts = 0;
    // `_creation` is held through creation and the final probe, so a concurrent creator sees
    // the result.
    let (name, socket, status, resurrectable, _creation) = loop {
        let name = match requested {
            Some(name) => SessionName::new(name),
            None => yinz_session::sessions::generate_unique_session_name()
                .map_err(std::io::Error::from)
                .context("list remote sessions")?
                .context("failed to generate a unique remote session name")?,
        };
        let socket = sock_dir.join(&name);
        yinz_process::pty_host::protocol::check_socket_path_length(&socket, 0)
            .context("check remote session socket path")?;
        let creation = (create || new)
            .then(|| {
                yinz_ipc::session_ownership::lock_creation(
                    &socket,
                    Instant::now() + CREATION_LOCK_TIMEOUT,
                )
            })
            .transpose()
            .context("wait for concurrent remote session creation")?;
        let status = session_status(&name)
            .map_err(std::io::Error::from)
            .context("probe remote session")?;
        // Read once, so the name `new` refuses and the session `create_session` starts agree.
        let resurrectable = status == SessionProbe::Absent
            && (create || new)
            && resurrection_layout(&name)
                .map_err(anyhow::Error::msg)
                .context("read remote session checkpoint")?
                .is_some();
        // A concurrent creator can take a generated name between generation and the lock;
        // the person named nothing, so pick another instead of refusing.
        if requested.is_none()
            && (status != SessionProbe::Absent || resurrectable)
            && attempts < GENERATED_NAME_ATTEMPTS
        {
            attempts += 1;
            continue;
        }
        break (name, socket, status, resurrectable, creation);
    };
    if new && (status == SessionProbe::Alive || resurrectable) {
        let next = if resurrectable {
            "resurrect it with `yinz attach --remote HOST NAME --create`"
        } else {
            "attach to it"
        };
        return Err(BootstrapError {
            category: BootstrapCategory::SessionExists,
            diagnostic: anyhow!(
                "remote session {:?} already exists; {next} or choose another name",
                name.as_str()
            ),
        }
        .into());
    }
    match status {
        SessionProbe::Alive => wait_until_ready(&socket)?,
        SessionProbe::Absent if create || new => create_session(
            &name,
            &socket,
            resurrectable,
            opts,
            configuration_options,
            client_termios,
            terminal_window_size,
        )?,
        SessionProbe::Absent => {
            return Err(BootstrapError {
                category: BootstrapCategory::SessionMissing,
                diagnostic: anyhow!(
                    "remote session {:?} does not exist; use --create to create or resurrect it",
                    name.as_str()
                ),
            }
            .into())
        },
        SessionProbe::Unresponsive => {
            return Err(BootstrapError {
                category: BootstrapCategory::Unresponsive,
                diagnostic: anyhow!(
                    "remote session {:?} is unresponsive; refusing to replace it",
                    name.as_str()
                ),
            }
            .into())
        },
        SessionProbe::Denied => {
            return Err(BootstrapError {
                category: BootstrapCategory::Permission,
                diagnostic: anyhow!(
                    "permission denied probing remote session {:?}",
                    name.as_str()
                ),
            }
            .into())
        },
    }
    probe(&socket).context("remote session is not ready")?;
    let endpoint = Endpoint {
        session_name: name.into_string(),
        socket,
        yinz_version: VERSION.to_owned(),
        contract_version: CLIENT_SERVER_CONTRACT_VERSION,
        bootstrap_version: REMOTE_BOOTSTRAP_VERSION,
    };
    let json = serde_json::to_string(&endpoint).context("encode remote endpoint")?;
    anyhow::ensure!(
        json.len() < ENDPOINT_LIMIT,
        "remote endpoint exceeds size limit"
    );
    println!("{json}");
    Ok(())
}

fn wait_until_ready(socket: &Path) -> Result<()> {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        anyhow::ensure!(
            !remaining.is_zero(),
            "remote session did not finish initialization"
        );
        let stream = yinz_ipc::consts::ipc_connect(socket).context("connect to remote session")?;
        stream
            .set_recv_timeout(Some(remaining.min(Duration::from_secs(1))))
            .context("bound readiness reply")?;
        stream
            .set_send_timeout(Some(remaining))
            .context("bound readiness request")?;
        let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(stream);
        let mut receiver = sender
            .get_receiver::<ServerToClientMsg>()
            .context("read session readiness")?;
        // ConnStatus answers before initialization; this read-only query needs a live screen.
        sender
            .send_client_msg(ClientToServerMsg::Action {
                action: yinz_types::input::actions::Action::ListPanes {
                    show_tab: false,
                    show_command: false,
                    show_state: false,
                    show_geometry: false,
                    show_all: false,
                    output_json: false,
                },
                terminal_id: None,
                client_id: None,
                is_cli_client: true,
            })
            .context("check session initialization")?;
        loop {
            match receiver.recv_server_msg() {
                IpcRecv::Msg(ServerToClientMsg::Log { .. }, _) => return Ok(()),
                IpcRecv::Msg(ServerToClientMsg::LogError { lines }, _) => {
                    bail!("remote session is not ready: {}", lines.join("\n"))
                },
                IpcRecv::Msg(ServerToClientMsg::Exit { exit_reason }, _) => {
                    bail!("remote session is not ready: {exit_reason}")
                },
                IpcRecv::Msg(_, _) if Instant::now() < deadline => {},
                _ => break,
            }
        }
        // Another bootstrap may own creation but not yet have sent FirstClientConnected.
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// `resurrect` restores the session from its checkpoint instead of the configured layout.
fn create_session(
    name: &SessionName,
    socket: &Path,
    resurrect: bool,
    opts: &CliArgs,
    configuration_options: Option<Options>,
    client_termios: Option<Option<WireTermios>>,
    terminal_window_size: Size,
) -> Result<()> {
    let (config, layout, options) = if resurrect {
        let (config, options) = Setup::config_without_layout(opts)
            .map_err(|e| anyhow!("{e}"))
            .context("load remote configuration without a session layout")?;
        let layout = Some(LayoutInfo::File(
            yinz_runtime::consts::session_layout_cache_file_name(name)
                .display()
                .to_string(),
            LayoutMetadata::default(),
        ));
        (config, layout, options)
    } else {
        let (config, layout, options, _, _) = Setup::from_cli_args(opts)
            .map_err(|e| anyhow!("{e}"))
            .context("load remote configuration and layout")?;
        (config, layout, options)
    };
    let env = remote_server_env(&config.env, std::env::var_os("TERM"));
    yinz_client::spawn_server_with_termios(socket, opts.debug, name, &env, client_termios.as_ref())
        .context("start remote session server")?;
    let stream = connect_with_deadline(socket, READY_TIMEOUT, Duration::from_millis(50))
        .context("connect to new remote session")?;
    stream
        .set_recv_timeout(Some(READY_TIMEOUT))
        .context("bound remote startup reply")?;
    stream
        .set_send_timeout(Some(READY_TIMEOUT))
        .context("bound remote startup request")?;
    let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(stream);
    let mut receiver = sender
        .get_receiver::<ServerToClientMsg>()
        .context("read remote startup response")?;
    sender
        .send_client_msg(ClientToServerMsg::FirstClientConnected {
            cli_assets: CliAssets {
                config_file_path: yinz_config::config::Config::config_file_path(
                    opts.config.as_deref(),
                    opts.config_dir.as_deref(),
                ),
                config_dir: opts.config_dir.clone(),
                data_dir: opts.data_dir.clone(),
                configuration_options: Some(configuration_options.unwrap_or(options)),
                layout,
                terminal_window_size,
                max_panes: opts.max_panes,
                is_debug: opts.debug,
                ..Default::default()
            },
        })
        .context("initialize remote session")?;
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        match receiver.recv_server_msg() {
            IpcRecv::Msg(ServerToClientMsg::Render { .. }, _) => break,
            IpcRecv::Msg(ServerToClientMsg::Exit { exit_reason }, _) => {
                bail!("remote session startup failed: {exit_reason}")
            },
            IpcRecv::Msg(_, _) if Instant::now() < deadline => {},
            _ => bail!("remote session did not finish initialization within {READY_TIMEOUT:?}"),
        }
    }
    sender
        .send_client_msg(ClientToServerMsg::ClientExited)
        .context("detach remote bootstrap client")?;
    Ok(())
}

fn remote_server_env(
    configured: &yinz_types::envs::EnvironmentVariables,
    inherited_term: Option<std::ffi::OsString>,
) -> yinz_types::envs::EnvironmentVariables {
    let mut env = configured.inner().clone();
    let term = env
        .get("TERM")
        .map(std::ffi::OsString::from)
        .or(inherited_term);
    // SSH without a PTY may omit TERM; set the fallback on the server child only.
    if term.is_none_or(|term| term.is_empty()) {
        env.insert("TERM".into(), "xterm-256color".into());
    }
    yinz_types::envs::EnvironmentVariables::from_data(env)
}

fn cleanup_forward() {
    yinz_client::remote_sessions::set_forward(None);
}

fn validate_options(opts: &CliArgs) -> Result<()> {
    let local_overrides = opts.layout.is_some()
        || opts.layout_string.is_some()
        || opts.new_session_with_layout.is_some()
        || opts.max_panes.is_some()
        || opts.data_dir.is_some();
    match &opts.command {
        Some(yinz_cli::args::Command::Sessions(Sessions::Attach {
            remote: Some(_),
            session_name,
            create,
            index,
            create_background,
            force_run_commands,
            options,
        })) if opts.remote.is_none() => {
            anyhow::ensure!(index.is_none() && !create_background && !force_run_commands && options.is_none()
                && !local_overrides && opts.session.is_none(),
                "remote attach does not support local session selection, layout, data-directory, or session option overrides; configure the session on the remote host");
            anyhow::ensure!(
                session_name.is_some() || !create,
                "remote attach with --create requires a session name"
            );
        },
        None if opts.remote.is_some() => anyhow::ensure!(
            !local_overrides,
            "a remote session does not support layout, data-directory, or session option overrides; configure the session on the remote host"
        ),
        Some(_) if opts.remote.is_some() => {
            bail!("--remote starts a new session and takes no subcommand; use `yinz attach --remote HOST` to attach")
        },
        _ => bail!("expected remote session arguments"),
    }
    Ok(())
}

fn switch_target(
    target: SessionConnection,
) -> Result<(
    String,
    Option<yinz_types::internal_ids::TabNumber>,
    Option<yinz_types::pane_id::PaneId>,
)> {
    anyhow::ensure!(
        target.layout.is_none() && target.cwd.is_none(),
        "remote session switching does not support layout or working-directory overrides"
    );
    let name = target.name.context("remote session switching requires an explicit session name; unnamed create/switch is unsupported")?;
    validate_name(name.as_str())?;
    Ok((name.into_string(), target.tab_position, target.pane_id))
}

/// `None` when the host has no sessions and a person at a terminal can be offered a new one.
fn select_discovered_session(
    host: &str,
    rows: Vec<PickerRow>,
    interactive: bool,
) -> Result<Option<Outcome>> {
    if rows.is_empty() {
        anyhow::ensure!(
            interactive,
            "No active yinz sessions found on {host}. Start one with `yinz --remote {host}`."
        );
        return Ok(None);
    }
    let mut candidates = rows
        .iter()
        .filter(|row| row.listing == yinz_cli::sessions::SessionListing::Running)
        .filter_map(|row| row.destination.as_ref());
    if let Some(destination) = candidates.next()
        && candidates.next().is_none()
    {
        return Ok(Some(Outcome::Picked(destination.clone())));
    }
    anyhow::ensure!(
        interactive,
        "Specify a session name with `yinz attach --remote {host} SESSION`, or use a terminal to choose. Sessions on {host}: {}",
        rows.iter().map(|row| row.label.as_str()).collect::<Vec<_>>().join(", ")
    );
    crate::session_menu::run(rows, "attach", Vec::new())
        .map(Some)
        .context("choose a remote session")
}

/// `None` when the person interrupted the question.
fn confirm_new_session(host: &str) -> Result<Option<bool>> {
    use nix::sys::termios::{tcgetattr, tcsetattr, SetArg};
    let stdin = std::io::stdin();
    let original = tcgetattr(&stdin).context("read terminal settings")?;
    let term = console::Term::stderr();
    let answer = dialoguer::Confirm::new()
        .with_prompt(format!(
            "No active yinz sessions found on {host}. Create a new session?"
        ))
        .default(true)
        .interact_on_opt(&term);
    if answer.is_err() {
        // console's key reader returns early on Ctrl-C or a signal, leaving the terminal in raw
        // mode with the cursor hidden (see `session_menu::KeyMode`).
        tcsetattr(&stdin, SetArg::TCSADRAIN, &original)
            .context("restore terminal settings")
            .non_fatal();
        term.show_cursor().context("show the cursor").non_fatal();
        term.write_line("").context("end the question").non_fatal();
    }
    match answer {
        Ok(answer) => Ok(Some(answer.unwrap_or(false))),
        Err(dialoguer::Error::IO(error)) if error.kind() == std::io::ErrorKind::Interrupted => {
            Ok(None)
        },
        Err(error) => Err(error).context("ask whether to create a remote session"),
    }
}

/// The session the first connection reaches.
enum Target {
    Named {
        name: String,
        create: bool,
    },
    /// A new session, named by the remote host when no name is given.
    New(Option<String>),
}

/// Checks shared by every remote entry point, before any SSH.
fn prepare(opts: &CliArgs, host: &str, name: Option<&str>) -> Result<StartupCancellation> {
    validate_options(opts)?;
    if let Some(name) = name {
        validate_name(name).context("validate remote session name")?;
    }
    ssh::validate_ssh_host(host).context("validate SSH host")?;
    StartupCancellation::new().context("watch for remote startup cancellation")
}

/// `yinz --remote HOST`: a new session on the host, named by `--session` or by the host.
pub(crate) fn start(opts: &CliArgs, host: &str) -> Result<i32> {
    let cancellation = prepare(opts, host, opts.session.as_deref())?;
    let local = load_local_config(opts)?;
    run(
        opts,
        host,
        Target::New(opts.session.clone()),
        &cancellation,
        local,
    )
}

pub(crate) fn attach(opts: &CliArgs, host: &str, name: Option<&str>, create: bool) -> Result<i32> {
    let cancellation = prepare(opts, host, name)?;
    // Discovery and attachment run the same SSH, so both use the configured environment.
    let local = load_local_config(opts)?;
    let target = match name {
        Some(name) => Target::Named {
            name: name.to_owned(),
            create,
        },
        None => {
            let rows = discover(host, &cancellation.cancelled, &local.0.env);
            if cancellation.is_cancelled() {
                return Ok(yinz_client::INTERRUPTED_EXIT_STATUS);
            }
            match select_discovered_session(host, rows?, crate::commands::can_pick_a_session())? {
                Some(Outcome::Picked(Destination::Remote { name, .. })) => Target::Named {
                    name: name.into_string(),
                    create,
                },
                Some(Outcome::Cancelled(_)) => return Ok(0),
                Some(Outcome::Interrupted) => return Ok(yinz_client::INTERRUPTED_EXIT_STATUS),
                Some(_) => bail!("remote session picker returned no remote destination"),
                None => {
                    let answer = confirm_new_session(host);
                    // A signal can end the question with an error as well as an answer.
                    if cancellation.is_cancelled() {
                        return Ok(yinz_client::INTERRUPTED_EXIT_STATUS);
                    }
                    match answer? {
                        Some(true) => Target::New(None),
                        Some(false) => return Ok(0),
                        None => return Ok(yinz_client::INTERRUPTED_EXIT_STATUS),
                    }
                },
            }
        },
    };
    run(opts, host, target, &cancellation, local)
}

/// The local configuration whose environment every SSH invocation for this attach uses.
fn load_local_config(opts: &CliArgs) -> Result<(yinz_config::config::Config, Options)> {
    let mut config = yinz_config::config::Config::load(
        opts.config.as_deref(),
        opts.config_dir.as_deref(),
        false,
    )
    .map_err(|e| anyhow!("{e}"))
    .context("load local terminal configuration")?;
    let options = yinz_config::config::apply_themes(&mut config, None, opts.config_dir.clone())
        .map_err(|e| anyhow!("{e}"))
        .context("load local terminal theme")?;
    Ok((config, options))
}

fn run(
    opts: &CliArgs,
    host: &str,
    mut target: Target,
    cancellation: &StartupCancellation,
    (config, options): (yinz_config::config::Config, Options),
) -> Result<i32> {
    let os = get_client_os_input().context("open local terminal")?;
    let mut tab = None;
    let mut pane = None;
    loop {
        let connected = match target {
            Target::Named { name, create } => {
                connect(host, &name, create, &cancellation.cancelled, &config.env)
                    .map(|forward| (forward, name))
            },
            Target::New(name) => {
                connect_new(host, name.as_deref(), &cancellation.cancelled, &config.env)
            },
        };
        if cancellation.is_cancelled() {
            return Ok(yinz_client::INTERRUPTED_EXIT_STATUS);
        }
        let (forward, name) = connected?;
        let socket = forward.socket().to_owned();
        // The client's startup signal guard can exit immediately, so publish ownership first.
        yinz_client::remote_sessions::set_forward(Some(forward));
        let outcome = start_client(
            Box::new(os.clone()),
            opts.clone(),
            config.clone(),
            options.clone(),
            ClientInfo::Remote {
                host: host.to_owned(),
                name: SessionName::new(&name),
                socket,
                cancelled: cancellation.cancelled.clone(),
            },
            tab,
            pane,
            false,
            false,
        );
        cleanup_forward();
        if outcome.exit_status != 0 {
            return Ok(outcome.exit_status);
        }
        let Some(switch) = outcome.reconnect_to_session else {
            return Ok(0);
        };
        let name;
        (name, tab, pane) = switch_target(switch)?;
        target = Target::Named { name, create: true };
    }
}

#[cfg(test)]
#[path = "unit/remote_session.rs"]
mod tests;

#[derive(Debug, Serialize, Deserialize)]
struct SessionList {
    yinz_version: String,
    contract_version: usize,
    bootstrap_version: usize,
    sessions: Vec<(SessionName, Duration, yinz_cli::sessions::SessionListing)>,
}

pub(crate) fn list(client_version: &str, contract: usize, bootstrap: usize) -> Result<()> {
    check_versions(client_version, contract, bootstrap)?;
    let mut sessions = yinz_cli::sessions::collect_sessions(false)
        .map_err(std::io::Error::from)
        .context("collect remote sessions")?;
    yinz_cli::sessions::sort_sessions(&mut sessions, true);
    let response = SessionList {
        yinz_version: VERSION.to_owned(),
        contract_version: CLIENT_SERVER_CONTRACT_VERSION,
        bootstrap_version: REMOTE_BOOTSTRAP_VERSION,
        sessions,
    };
    println!(
        "{}",
        serde_json::to_string(&response).context("encode remote sessions")?
    );
    Ok(())
}
