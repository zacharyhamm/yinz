//! A local interactive client with a remote session and an owned SSH tunnel.
#[cfg(test)]
use clap::Parser;
use interprocess::local_socket::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{Duration, Instant};
use yinz_cli::args::{CliArgs, Sessions};
use yinz_cli::setup::Setup;
use yinz_client::os_input_output::{connect_with_deadline, get_client_os_input};
use yinz_client::remote_sessions::{
    check_versions, connect, probe, validate_name, Endpoint, StartupCancellation,
};
#[cfg(test)]
use yinz_client::remote_sessions::{client_socket, discover, parse_endpoint, parse_sessions};
use yinz_client::{start_client, ClientInfo};
use yinz_ipc::ipc::{ClientToServerMsg, IpcRecv, IpcSenderWithContext, ServerToClientMsg};
use yinz_ipc::probe::SessionProbe;
use yinz_process::ssh;
use yinz_runtime::consts::{CLIENT_SERVER_CONTRACT_VERSION, VERSION, YINZ_SOCK_DIR};
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

/// Compatibility is checked before any local or remote startup side effects.
pub(crate) fn bootstrap(
    name: &str,
    create: bool,
    client_version: &str,
    contract: usize,
    configuration_options: Option<&str>,
    client_termios: Option<&str>,
    terminal_window_size: Option<&str>,
    opts: &CliArgs,
) -> Result<()> {
    check_versions(client_version, contract)?;
    validate_name(name)?;
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
    let name = SessionName::new(name);
    let socket = YINZ_SOCK_DIR
        .canonicalize()
        .context("resolve remote socket directory")?
        .join(&name);
    yinz_process::pty_host::protocol::check_socket_path_length(&socket, 0)?;
    match session_status(&name)
        .map_err(std::io::Error::from)
        .context("probe remote session")?
    {
        SessionProbe::Alive => wait_until_ready(&socket)?,
        SessionProbe::Absent if create => create_session(
            &name,
            &socket,
            opts,
            configuration_options,
            client_termios,
            terminal_window_size,
        )?,
        SessionProbe::Absent => {
            bail!("remote session {name:?} does not exist; use --create to create or resurrect it")
        },
        SessionProbe::Unresponsive => {
            bail!("remote session {name:?} is unresponsive; refusing to replace it")
        },
        SessionProbe::Denied => bail!("permission denied probing remote session {name:?}"),
    }
    probe(&socket).context("remote session is not ready")?;
    let endpoint = Endpoint {
        session_name: name.into_string(),
        socket,
        yinz_version: VERSION.to_owned(),
        contract_version: CLIENT_SERVER_CONTRACT_VERSION,
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

fn create_session(
    name: &SessionName,
    socket: &Path,
    opts: &CliArgs,
    configuration_options: Option<Options>,
    client_termios: Option<Option<WireTermios>>,
    terminal_window_size: Size,
) -> Result<()> {
    let (config, layout, options) = if resurrection_layout(name)
        .map_err(anyhow::Error::msg)
        .context("read remote session checkpoint")?
        .is_some()
    {
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
    yinz_runtime::envs::set_vars(&config.env);
    // SSH without a PTY does not supply TERM for the server's pane processes.
    if std::env::var_os("TERM").is_none_or(|term| term.is_empty()) {
        unsafe { std::env::set_var("TERM", "xterm-256color") };
    }
    yinz_runtime::envs::set_session_name(name.clone());
    yinz_runtime::envs::set_yinz("0".to_owned());
    yinz_client::spawn_server_with_termios(socket, opts.debug, client_termios.as_ref())
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

fn cleanup_forward() {
    yinz_client::remote_sessions::set_forward(None);
}

fn validate_options(opts: &CliArgs) -> Result<()> {
    let Some(yinz_cli::args::Command::Sessions(Sessions::Attach {
        remote: Some(_),
        index,
        create_background,
        force_run_commands,
        options,
        ..
    })) = &opts.command
    else {
        bail!("expected remote attach arguments")
    };
    anyhow::ensure!(index.is_none() && !create_background && !force_run_commands && options.is_none()
        && opts.layout.is_none() && opts.layout_string.is_none() && opts.new_session_with_layout.is_none()
        && opts.session.is_none() && opts.max_panes.is_none() && opts.data_dir.is_none(),
        "remote attach does not support local session selection, layout, data-directory, or session option overrides; configure the session on the remote host");
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

pub(crate) fn attach(opts: &CliArgs, host: &str, name: &str, mut create: bool) -> Result<i32> {
    validate_options(opts)?;
    validate_name(name)?;
    ssh::validate_ssh_host(host)?;
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
    let os = get_client_os_input().context("open local terminal")?;
    let mut name = name.to_owned();
    let mut tab = None;
    let mut pane = None;
    let cancellation = StartupCancellation::new()?;
    loop {
        let forward = connect(host, &name, create, &cancellation.cancelled);
        if cancellation.is_cancelled() {
            return Ok(yinz_client::INTERRUPTED_EXIT_STATUS);
        }
        let forward = forward?;
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
        let Some(target) = outcome.reconnect_to_session else {
            return Ok(0);
        };
        (name, tab, pane) = switch_target(target)?;
        create = true;
    }
}

#[cfg(test)]
#[path = "unit/remote_session.rs"]
mod tests;

#[derive(Debug, Serialize, Deserialize)]
struct SessionList {
    yinz_version: String,
    contract_version: usize,
    sessions: Vec<(SessionName, Duration, yinz_cli::sessions::SessionListing)>,
}

pub(crate) fn list(client_version: &str, contract: usize) -> Result<()> {
    check_versions(client_version, contract)?;
    let mut sessions = yinz_cli::sessions::collect_sessions(false)
        .map_err(std::io::Error::from)
        .context("collect remote sessions")?;
    yinz_cli::sessions::sort_sessions(&mut sessions, true);
    let response = SessionList {
        yinz_version: VERSION.to_owned(),
        contract_version: CLIENT_SERVER_CONTRACT_VERSION,
        sessions,
    };
    println!(
        "{}",
        serde_json::to_string(&response).context("encode remote sessions")?
    );
    Ok(())
}
