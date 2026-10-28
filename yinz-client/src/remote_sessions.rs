//! SSH discovery and forwarding shared by startup and the attached session picker.
use interprocess::local_socket::prelude::*;
use serde::{Deserialize, Serialize};
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use yinz_ipc::ipc::{ClientToServerMsg, IpcRecv, IpcSenderWithContext, ServerToClientMsg};
use yinz_process::ssh::{self, HostForward, SshOptions};
use yinz_runtime::{
    consts::{CLIENT_SERVER_CONTRACT_VERSION, REMOTE_BOOTSTRAP_VERSION, VERSION, YINZ_SOCK_DIR},
    errors::prelude::*,
};
use yinz_session::sessions::validate_session_name;
use yinz_types::envs::EnvironmentVariables;
use yinz_types::internal_ids::SessionName;
pub struct StartupCancellation {
    pub cancelled: Arc<AtomicBool>,
    handlers: Vec<signal_hook::SigId>,
}

impl StartupCancellation {
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn new() -> Result<Self> {
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

const ENDPOINT_LIMIT: usize = 8192;
#[derive(Debug, Serialize, Deserialize)]
pub struct Endpoint {
    pub session_name: String,
    pub socket: PathBuf,
    pub yinz_version: String,
    pub contract_version: usize,
    /// Absent, and so 0, from remotes that predate bootstrap versioning.
    #[serde(default)]
    pub bootstrap_version: usize,
}

fn versions(version: &str, contract: usize, bootstrap: usize) -> String {
    let bootstrap = match bootstrap {
        0 => "no remote bootstrap version".to_owned(),
        bootstrap => format!("remote bootstrap {bootstrap}"),
    };
    format!("Yinz {version}, client/server contract {contract}, {bootstrap}")
}

fn mismatch(peer: &str) -> anyhow::Error {
    anyhow!(
        "Yinz compatibility mismatch: this machine has {}; peer has {peer}. Install matching Yinz versions on both machines.",
        versions(VERSION, CLIENT_SERVER_CONTRACT_VERSION, REMOTE_BOOTSTRAP_VERSION)
    )
}

pub fn check_versions(version: &str, contract: usize, bootstrap: usize) -> Result<()> {
    if version == VERSION
        && contract == CLIENT_SERVER_CONTRACT_VERSION
        && bootstrap == REMOTE_BOOTSTRAP_VERSION
    {
        return Ok(());
    }
    Err(mismatch(&versions(version, contract, bootstrap)))
}

/// Explain a failed remote bootstrap or discovery from its stderr. A remote that rejects
/// `--bootstrap-version` or the subcommand predates [`REMOTE_BOOTSTRAP_VERSION`]; one that
/// rejects another flag is older but versioned; one that parsed it but disagrees says so.
/// A missing or already existing session is reported in the remote's own words.
pub fn bootstrap_failure(host: &str, stderr: &[u8]) -> Option<anyhow::Error> {
    use crate::remote_failure::{reported_error, BootstrapCategory, BootstrapError};
    let stderr = String::from_utf8_lossy(stderr);
    let mut lines = stderr.lines().map(str::trim);
    if let Some((
        category @ (BootstrapCategory::SessionMissing | BootstrapCategory::SessionExists),
        reported,
    )) = reported_error(&stderr)
    {
        return Some(
            BootstrapError {
                category,
                diagnostic: anyhow!("{host} reported: {reported}"),
            }
            .into(),
        );
    }
    if let Some(reported) = lines
        .clone()
        .find(|line| line.contains("Yinz compatibility mismatch"))
    {
        return Some(anyhow!(
            "{host} reported: {}",
            reported.trim_start_matches("Error: ")
        ));
    }
    let rejected = lines.find(|line| {
        line.contains("unexpected argument") || line.contains("unrecognized subcommand")
    })?;
    let unversioned = line_names_bootstrap_flag_or_subcommand(rejected);
    Some(mismatch(&format!(
        "an older Yinz on {host}{} (it reported: {})",
        if unversioned {
            " with no remote bootstrap version"
        } else {
            ""
        },
        rejected.trim_start_matches("error: ")
    )))
}

/// Whether clap's rejection is of `--bootstrap-version` itself or of the whole subcommand, the
/// only rejections that prove the remote predates bootstrap versioning.
fn line_names_bootstrap_flag_or_subcommand(line: &str) -> bool {
    line.contains("unrecognized subcommand") || line.contains("'--bootstrap-version'")
}

pub fn validate_name(name: &str) -> Result<()> {
    validate_session_name(name).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(
        !name.chars().any(char::is_control),
        "session name contains control characters"
    );
    Ok(())
}

/// `name` is the session asked for; without one, the remote named it.
pub fn parse_endpoint(bytes: &[u8], name: Option<&str>) -> Result<Endpoint> {
    anyhow::ensure!(
        bytes.len() <= ENDPOINT_LIMIT,
        "remote endpoint response exceeds {ENDPOINT_LIMIT} bytes"
    );
    let endpoint: Endpoint = serde_json::from_slice(bytes).context(
        "invalid remote endpoint response; the SSH command must print only Yinz's JSON response",
    )?;
    check_versions(
        &endpoint.yinz_version,
        endpoint.contract_version,
        endpoint.bootstrap_version,
    )?;
    match name {
        Some(name) => anyhow::ensure!(
            endpoint.session_name == name,
            "remote endpoint returned a different session name"
        ),
        None => validate_name(&endpoint.session_name)
            .context("remote endpoint returned an invalid session name")?,
    }
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

pub fn probe(socket: &Path) -> Result<()> {
    probe_until(
        socket,
        &AtomicBool::new(false),
        Instant::now() + Duration::from_secs(5),
    )
}

fn probe_until(socket: &Path, cancelled: &AtomicBool, deadline: Instant) -> Result<()> {
    let stream = yinz_ipc::consts::ipc_connect(socket).context("connect to session socket")?;
    stream
        .set_recv_timeout(Some(remaining(deadline)?))
        .context("bound session probe reply")?;
    stream
        .set_send_timeout(Some(remaining(deadline)?))
        .context("bound session probe request")?;
    let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(stream);
    let mut receiver = sender
        .get_receiver::<ServerToClientMsg>()
        .context("read session probe")?;
    sender
        .send_client_msg_until(ClientToServerMsg::ConnStatus, deadline)
        .context("send session probe")?;
    loop {
        ssh::check_cancelled(Some(cancelled))?;
        remaining(deadline)?;
        match receiver.recv_server_msg_until(deadline.min(Instant::now() + Duration::from_millis(100))) {
            IpcRecv::Msg(ServerToClientMsg::Connected { .. }, _) => break,
            IpcRecv::Idle => {},
            _ => bail!("session did not answer through the socket; check SSH Unix-socket forwarding permissions and server responsiveness"),
        }
    }
    Ok(())
}

/// One budget for a first attachment's bootstrap, forwarding and probing.
const CONNECT_BUDGET: Duration = Duration::from_secs(85);

pub fn connect(
    host: &str,
    name: &str,
    create: bool,
    cancelled: &AtomicBool,
    env: &EnvironmentVariables,
) -> Result<HostForward> {
    connect_until(
        host,
        name,
        create,
        cancelled,
        Instant::now() + CONNECT_BUDGET,
        env,
    )
}

/// Create a new session on `host`, named by the remote when `name` is absent, and return
/// its forward with the session's name.
pub fn connect_new(
    host: &str,
    name: Option<&str>,
    cancelled: &AtomicBool,
    env: &EnvironmentVariables,
) -> Result<(HostForward, String)> {
    bootstrap_until(
        host,
        Request::New(name),
        cancelled,
        Instant::now() + CONNECT_BUDGET,
        env,
    )
}

/// One budget covers bootstrap, forwarding and probing during recovery.
pub(crate) fn connect_until(
    host: &str,
    name: &str,
    create: bool,
    cancelled: &AtomicBool,
    deadline: Instant,
    env: &EnvironmentVariables,
) -> Result<HostForward> {
    let request = if create {
        Request::Create(name)
    } else {
        Request::Attach(name)
    };
    bootstrap_until(host, request, cancelled, deadline, env).map(|(forward, _)| forward)
}

/// What the remote bootstrap is asked to do with the session.
#[derive(Clone, Copy)]
enum Request<'a> {
    Attach(&'a str),
    /// Attach, creating or resurrecting the session when it is absent.
    Create(&'a str),
    /// A session under an untaken name, which the remote picks when none is given.
    New(Option<&'a str>),
}

impl<'a> Request<'a> {
    fn flag(self) -> Option<&'static str> {
        match self {
            Request::Attach(_) => None,
            Request::Create(_) => Some("--create"),
            Request::New(_) => Some("--new"),
        }
    }

    fn name(self) -> Option<&'a str> {
        match self {
            Request::Attach(name) | Request::Create(name) => Some(name),
            Request::New(name) => name,
        }
    }
}

fn bootstrap_until(
    host: &str,
    request: Request,
    cancelled: &AtomicBool,
    deadline: Instant,
    env: &EnvironmentVariables,
) -> Result<(HostForward, String)> {
    ssh::validate_ssh_host(host)?;
    let name = request.name();
    if let Some(name) = name {
        validate_name(name)?;
    }
    let mut args = vec![
        "yinz".to_owned(),
        "remote-session".to_owned(),
        "--client-version".to_owned(),
        VERSION.to_owned(),
        "--contract-version".to_owned(),
        CLIENT_SERVER_CONTRACT_VERSION.to_string(),
        "--bootstrap-version".to_owned(),
        REMOTE_BOOTSTRAP_VERSION.to_string(),
    ];
    args.extend(request.flag().map(str::to_owned));
    if let Some(name) = name {
        args.extend(["--".to_owned(), name.to_owned()]);
    }
    let remote_command = args
        .iter()
        .map(|s| ssh::shell_argument(s))
        .collect::<Result<Vec<_>>>()?
        .join(" ");
    let options = SshOptions::default();
    let output = ssh::bootstrap_output(
        Command::new("ssh")
            .envs(env.inner())
            .args(ssh::ssh_forward_args(&options))
            .args(["-T", "--", host, &remote_command]),
        remaining(deadline)?.min(Duration::from_secs(60)),
        Some(cancelled),
    )
    .with_context(|| format!("run remote session bootstrap on {host}"))?;
    if !output.status.success()
        && let Some(error) = bootstrap_failure(host, &output.stderr)
    {
        return Err(error);
    }
    anyhow::ensure!(output.status.success(), "SSH bootstrap on {host} failed ({}). Check key/agent authentication and that compatible Yinz is on the remote PATH: {}", output.status, String::from_utf8_lossy(&output.stderr).trim());
    let endpoint = parse_endpoint(&output.stdout, name)
        .map_err(|diagnostic| crate::remote_failure::BootstrapError {
            category: crate::remote_failure::BootstrapCategory::Configuration,
            diagnostic,
        })
        .with_context(|| format!("read endpoint from {host}"))?;
    let name = endpoint.session_name;
    let socket = client_socket(&YINZ_SOCK_DIR)?;
    let mut forward = HostForward::start_until(
        host,
        &options,
        &socket,
        endpoint
            .socket
            .to_str()
            .context("remote socket is not UTF-8")?,
        Some(cancelled),
        deadline,
        env,
    )
    // A session the remote just created and named is otherwise left behind unnamed.
    .with_context(|| format!("forward remote session {name:?} on {host}"))?;
    ssh::check_cancelled(Some(cancelled))?;
    let probed = probe_until(forward.socket(), cancelled, deadline);
    ssh::check_cancelled(Some(cancelled))?;
    if let Err(error) = probed {
        bail!(
            "remote session {name:?} on {host} is unreachable through SSH: {error:#}. {}",
            forward.diagnostics()?
        );
    }
    Ok((forward, name))
}

pub(crate) fn remaining(deadline: Instant) -> Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    anyhow::ensure!(!remaining.is_zero(), "Remote attachment deadline expired");
    Ok(remaining)
}

pub fn client_socket(session_directory: &Path) -> Result<PathBuf> {
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

#[derive(Debug, Serialize, Deserialize)]
struct SessionList {
    pub yinz_version: String,
    pub contract_version: usize,
    #[serde(default)]
    pub bootstrap_version: usize,
    sessions: Vec<(SessionName, Duration, yinz_cli::sessions::SessionListing)>,
}

pub fn discover(
    host: &str,
    cancelled: &AtomicBool,
    env: &EnvironmentVariables,
) -> Result<Vec<yinz_cli::session_picker::PickerRow>> {
    ssh::validate_ssh_host(host)?;
    let args = [
        "yinz",
        "remote-sessions",
        "--client-version",
        VERSION,
        "--contract-version",
        &CLIENT_SERVER_CONTRACT_VERSION.to_string(),
        "--bootstrap-version",
        &REMOTE_BOOTSTRAP_VERSION.to_string(),
    ];
    let command = args
        .iter()
        .map(|arg| ssh::shell_argument(arg))
        .collect::<Result<Vec<_>>>()?
        .join(" ");
    let output = ssh::bootstrap_output(
        Command::new("ssh")
            .envs(env.inner())
            .args(ssh::ssh_forward_args(&SshOptions::default()))
            .args(["-T", "--", host, &command]),
        Duration::from_secs(10),
        Some(cancelled),
    )
    .with_context(|| format!("discover sessions on {host}"))?;
    if !output.status.success()
        && let Some(error) = bootstrap_failure(host, &output.stderr)
    {
        return Err(error);
    }
    anyhow::ensure!(
        output.status.success(),
        "SSH discovery failed ({})",
        output.status
    );
    parse_sessions(host, &output.stdout)
}

pub fn parse_sessions(
    host: &str,
    bytes: &[u8],
) -> Result<Vec<yinz_cli::session_picker::PickerRow>> {
    use yinz_cli::session_picker::{Destination, PickerRow};
    let response: SessionList = serde_json::from_slice(bytes).context("decode remote sessions")?;
    check_versions(
        &response.yinz_version,
        response.contract_version,
        response.bootstrap_version,
    )?;
    response
        .sessions
        .into_iter()
        .filter(|(_, _, listing)| *listing != yinz_cli::sessions::SessionListing::Exited)
        .map(|(name, age, listing)| {
            validate_name(name.as_str())?;
            Ok(PickerRow {
                label: format!("{host} / {name}"),
                destination: Some(Destination::Remote {
                    host: host.to_owned(),
                    name,
                }),
                age,
                listing,
                current: false,
                status: None,
            })
        })
        .collect()
}

// Fatal terminal exits skip destructors, so keep weak references to every owned tunnel.
type ForwardSlot = std::sync::Arc<std::sync::Mutex<Option<HostForward>>>;
static FORWARDS: std::sync::Mutex<Vec<std::sync::Weak<std::sync::Mutex<Option<HostForward>>>>> =
    std::sync::Mutex::new(Vec::new());
static CLIENT_FORWARD: std::sync::Mutex<Option<ClientForward>> = std::sync::Mutex::new(None);
pub(crate) struct ClientForward {
    _forward: ForwardSlot,
    socket: PathBuf,
}
impl ClientForward {
    pub(crate) fn socket(&self) -> &Path {
        &self.socket
    }
}
extern "C" fn cleanup_forwards() {
    for forward in FORWARDS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
        .filter_map(std::sync::Weak::upgrade)
    {
        forward
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
    }
}
pub(crate) fn own_forward(forward: HostForward) -> ClientForward {
    static REGISTERED: std::sync::Once = std::sync::Once::new();
    REGISTERED.call_once(|| {
        unsafe { nix::libc::atexit(cleanup_forwards) };
    });
    let socket = forward.socket().to_owned();
    let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(forward)));
    let mut forwards = FORWARDS.lock().unwrap_or_else(|error| error.into_inner());
    forwards.retain(|forward| forward.strong_count() > 0);
    forwards.push(std::sync::Arc::downgrade(&slot));
    ClientForward {
        _forward: slot,
        socket,
    }
}
pub(crate) fn set_owned_forward(forward: Option<ClientForward>) {
    *CLIENT_FORWARD
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = forward;
}
pub fn set_forward(forward: Option<HostForward>) {
    set_owned_forward(forward.map(own_forward));
}

/// Collect the old SSH diagnostic before releasing its private socket and process.
pub(crate) fn release_forward() -> String {
    let forward = CLIENT_FORWARD
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take();
    forward
        .and_then(|forward| {
            forward
                ._forward
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
                .and_then(|mut owned| owned.diagnostics().ok())
        })
        .unwrap_or_default()
}

pub(crate) fn attachment_rejected(reason: yinz_ipc::ipc::ExitReason) -> anyhow::Error {
    crate::remote_failure::BootstrapError {
        category: crate::remote_failure::BootstrapCategory::AttachmentRejected,
        diagnostic: anyhow!("Destination rejected attachment: {reason}"),
    }
    .into()
}
