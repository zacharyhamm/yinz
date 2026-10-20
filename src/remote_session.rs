//! A local interactive client with a remote session and an owned SSH tunnel.
use clap::Parser;
use interprocess::local_socket::prelude::*;
use serde::{Deserialize, Serialize};
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use yinz_cli::args::{CliArgs, Sessions};
use yinz_cli::setup::Setup;
use yinz_client::os_input_output::{connect_with_deadline, get_client_os_input};
use yinz_client::{start_client, ClientInfo};
use yinz_ipc::ipc::{ClientToServerMsg, IpcRecv, IpcSenderWithContext, ServerToClientMsg};
use yinz_ipc::probe::SessionProbe;
use yinz_process::ssh::{self, HostForward, SshOptions};
use yinz_runtime::consts::{CLIENT_SERVER_CONTRACT_VERSION, VERSION, YINZ_SOCK_DIR};
use yinz_runtime::errors::prelude::*;
use yinz_session::sessions::{resurrection_layout, session_status, validate_session_name};
use yinz_types::data::{LayoutInfo, LayoutMetadata, SessionConnection};
use yinz_types::input::cli_assets::CliAssets;
use yinz_types::internal_ids::SessionName;

const ENDPOINT_LIMIT: usize = 8192;
const READY_TIMEOUT: Duration = Duration::from_secs(25);

struct StartupCancellation {
    cancelled: Arc<AtomicBool>,
    handlers: Vec<signal_hook::SigId>,
}

impl StartupCancellation {
    fn new() -> Result<Self> {
        let mut guard = Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            handlers: Vec::new(),
        };
        for signal in [SIGINT, SIGTERM, SIGQUIT, SIGHUP] {
            guard.handlers.push(
                signal_hook::flag::register(signal, guard.cancelled.clone())
                    .context("register remote startup cancellation")?,
            );
        }
        Ok(guard)
    }
}

impl Drop for StartupCancellation {
    fn drop(&mut self) {
        for handler in self.handlers.drain(..) {
            signal_hook::low_level::unregister(handler);
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Endpoint {
    session_name: String,
    socket: PathBuf,
    yinz_version: String,
    contract_version: usize,
}

fn check_versions(version: &str, contract: usize) -> Result<()> {
    anyhow::ensure!(version == VERSION && contract == CLIENT_SERVER_CONTRACT_VERSION,
        "Yinz compatibility mismatch: this machine has Yinz {VERSION}, client/server contract {CLIENT_SERVER_CONTRACT_VERSION}; peer has Yinz {version}, client/server contract {contract}. Install matching Yinz versions on both machines.");
    Ok(())
}

fn validate_name(name: &str) -> Result<()> {
    validate_session_name(name).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(
        !name.chars().any(char::is_control),
        "session name contains control characters"
    );
    Ok(())
}

fn parse_endpoint(bytes: &[u8], name: &str) -> Result<Endpoint> {
    anyhow::ensure!(
        bytes.len() <= ENDPOINT_LIMIT,
        "remote endpoint response exceeds {ENDPOINT_LIMIT} bytes"
    );
    let endpoint: Endpoint = serde_json::from_slice(bytes).context(
        "invalid remote endpoint response; the SSH command must print only Yinz's JSON response",
    )?;
    check_versions(&endpoint.yinz_version, endpoint.contract_version)?;
    anyhow::ensure!(
        endpoint.session_name == name,
        "remote endpoint returned a different session name"
    );
    let socket = endpoint
        .socket
        .to_str()
        .context("remote socket path is not UTF-8")?;
    anyhow::ensure!(
        endpoint.socket.is_absolute() && !socket.chars().any(char::is_control),
        "invalid remote socket path"
    );
    Ok(endpoint)
}

/// Runs only on the remote host. Compatibility is checked before any startup side effects.
pub(crate) fn bootstrap(
    name: &str,
    create: bool,
    client_version: &str,
    contract: usize,
) -> Result<()> {
    check_versions(client_version, contract)?;
    validate_name(name)?;
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
        SessionProbe::Absent if create => create_session(&name, &socket)?,
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

fn create_session(name: &SessionName, socket: &Path) -> Result<()> {
    // Parse here so remote config environment variables retain their normal CLI semantics.
    let opts =
        CliArgs::try_parse_from(["yinz"]).context("resolve remote configuration arguments")?;
    let (config, layout, options, _, _) = Setup::from_cli_args(&opts)
        .map_err(|e| anyhow!("{e}"))
        .context("load remote configuration and layout")?;
    let layout = if resurrection_layout(name)
        .map_err(anyhow::Error::msg)
        .context("read remote session checkpoint")?
        .is_some()
    {
        Some(LayoutInfo::File(
            yinz_runtime::consts::session_layout_cache_file_name(name)
                .display()
                .to_string(),
            LayoutMetadata::default(),
        ))
    } else {
        layout
    };
    yinz_runtime::envs::set_vars(&config.env);
    // SSH without a PTY does not supply TERM for the server's pane processes.
    if std::env::var_os("TERM").is_none_or(|term| term.is_empty()) {
        unsafe { std::env::set_var("TERM", "xterm-256color") };
    }
    yinz_runtime::envs::set_session_name(name.clone());
    yinz_runtime::envs::set_yinz("0".to_owned());
    // No client termios are supplied: the remote server uses its own platform defaults.
    yinz_client::spawn_server(socket, false).context("start remote session server")?;
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
                config_dir: opts.config_dir,
                configuration_options: Some(options),
                layout,
                terminal_window_size: yinz_types::pane_size::Size { rows: 24, cols: 80 },
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

fn probe(socket: &Path) -> Result<()> {
    let stream = yinz_ipc::consts::ipc_connect(socket).context("connect to session socket")?;
    stream
        .set_recv_timeout(Some(Duration::from_secs(5)))
        .context("bound session probe reply")?;
    stream
        .set_send_timeout(Some(Duration::from_secs(5)))
        .context("bound session probe request")?;
    let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(stream);
    let mut receiver = sender
        .get_receiver::<ServerToClientMsg>()
        .context("read session probe")?;
    sender
        .send_client_msg(ClientToServerMsg::ConnStatus)
        .context("send session probe")?;
    anyhow::ensure!(matches!(receiver.recv_server_msg(), IpcRecv::Msg(ServerToClientMsg::Connected { .. }, _)), "session did not answer through the socket; check SSH Unix-socket forwarding permissions and server responsiveness");
    Ok(())
}

fn connect(host: &str, name: &str, create: bool, cancelled: &AtomicBool) -> Result<HostForward> {
    ssh::validate_ssh_host(host)?;
    validate_name(name)?;
    let mut args = vec![
        "yinz".to_owned(),
        "remote-session".to_owned(),
        "--client-version".to_owned(),
        VERSION.to_owned(),
        "--contract-version".to_owned(),
        CLIENT_SERVER_CONTRACT_VERSION.to_string(),
    ];
    if create {
        args.push("--create".to_owned());
    }
    args.extend(["--".to_owned(), name.to_owned()]);
    let remote_command = args
        .iter()
        .map(|s| ssh::shell_argument(s))
        .collect::<Result<Vec<_>>>()?
        .join(" ");
    let options = SshOptions::default();
    let output = ssh::bootstrap_output(
        Command::new("ssh")
            .args(ssh::ssh_forward_args(&options))
            .args(["-T", "--", host, &remote_command]),
        Duration::from_secs(60),
        Some(cancelled),
    )
    .with_context(|| format!("run remote session bootstrap on {host}"))?;
    anyhow::ensure!(output.status.success(), "SSH bootstrap on {host} failed ({}). Check key/agent authentication and that compatible Yinz is on the remote PATH: {}", output.status, String::from_utf8_lossy(&output.stderr).trim());
    let endpoint = parse_endpoint(&output.stdout, name)
        .with_context(|| format!("read endpoint from {host}"))?;
    let socket = client_socket(&YINZ_SOCK_DIR)?;
    let mut forward = HostForward::start(
        host,
        &options,
        &socket,
        endpoint
            .socket
            .to_str()
            .context("remote socket is not UTF-8")?,
        Some(cancelled),
    )?;
    ssh::check_cancelled(Some(cancelled))?;
    let probed = probe(forward.socket());
    ssh::check_cancelled(Some(cancelled))?;
    if let Err(error) = probed {
        bail!(
            "remote session {name:?} on {host} is unreachable through SSH: {error:#}. {}",
            forward.diagnostics()?
        );
    }
    Ok(forward)
}

fn client_socket(session_directory: &Path) -> Result<PathBuf> {
    let directory = session_directory
        .parent()
        .context("session socket directory has no parent")?
        .join("client");
    yinz_runtime::shared::ensure_secure_dir(&directory)?;
    let directory = directory
        .canonicalize()
        .context("resolve client tunnel directory")?;
    Ok(directory.join(format!(
        "{}.sock",
        &uuid::Uuid::new_v4().simple().to_string()[..24]
    )))
}

// Client fatal-error paths call process::exit, which skips Rust destructors.
static CLIENT_FORWARD: Mutex<Option<HostForward>> = Mutex::new(None);
extern "C" fn cleanup_forward() {
    CLIENT_FORWARD
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
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
    yinz_runtime::logging::configure_logger();
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
    // The callback owns no borrowed data and is registered before a tunnel can be installed.
    anyhow::ensure!(
        unsafe { nix::libc::atexit(cleanup_forward) } == 0,
        "register SSH cleanup"
    );
    let mut name = name.to_owned();
    let mut tab = None;
    let mut pane = None;
    let cancellation = StartupCancellation::new()?;
    loop {
        let forward = connect(host, &name, create, &cancellation.cancelled);
        if cancellation.cancelled.load(Ordering::Acquire) {
            return Ok(yinz_client::INTERRUPTED_EXIT_STATUS);
        }
        let forward = forward?;
        let socket = forward.socket().to_owned();
        // The client's startup signal guard can exit immediately, so publish ownership first.
        *CLIENT_FORWARD.lock().unwrap_or_else(|e| e.into_inner()) = Some(forward);
        let outcome = start_client(
            Box::new(os.clone()),
            opts.clone(),
            config.clone(),
            options.clone(),
            ClientInfo::Remote {
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
