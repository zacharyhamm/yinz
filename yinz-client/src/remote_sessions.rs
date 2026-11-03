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
    /// Legacy bootstrap identity, echoed for peers that require exact release names.
    pub yinz_version: String,
    /// Actual remote build; absent on peers predating protocol-based compatibility.
    #[serde(default)]
    pub build_version: Option<String>,
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
        "Yinz compatibility mismatch: this machine has {}; peer has {peer}. Use Yinz builds with matching client/server and SSH bootstrap contracts.",
        versions(VERSION, CLIENT_SERVER_CONTRACT_VERSION, REMOTE_BOOTSTRAP_VERSION)
    )
}

pub fn check_versions(version: &str, contract: usize, bootstrap: usize) -> Result<()> {
    if contract == CLIENT_SERVER_CONTRACT_VERSION && bootstrap == REMOTE_BOOTSTRAP_VERSION {
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
        endpoint
            .build_version
            .as_deref()
            .unwrap_or(&endpoint.yinz_version),
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

/// SSH running `args` on `host`, quoted for its shell and without a remote terminal.
pub fn ssh_command(
    host: &str,
    args: &[impl AsRef<str>],
    env: &EnvironmentVariables,
) -> Result<Command> {
    ssh::validate_ssh_host(host).context("validate SSH host")?;
    let remote_command = args
        .iter()
        .map(|arg| ssh::shell_argument(arg.as_ref()))
        .collect::<Result<Vec<_>>>()
        .context("quote remote command")?
        .join(" ");
    let mut command = Command::new("ssh");
    command
        .envs(env.inner())
        .args(ssh::ssh_forward_args(&SshOptions::default()))
        .args(["-T", "--", host, &remote_command]);
    Ok(command)
}

/// Older SSH bootstraps checked the release string before checking either wire
/// contract. Retry only their explicit compatibility refusal, using their build
/// identity in that legacy field. Both protocol numbers still have to match;
/// ordinary SSH or startup failures must never cause a second startup attempt.
fn compatible_bootstrap(
    host: &str,
    args: &[impl AsRef<str>],
    env: &EnvironmentVariables,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<std::process::Output> {
    compatible_bootstrap_with_version(host, args, env, cancelled, deadline)
        .map(|(output, _)| output)
}

/// Negotiate using a read-only operation before streaming a command. Never retry
/// a mutating remote command: a lost response does not mean it failed to run.
pub fn negotiate_legacy_version(
    host: &str,
    env: &EnvironmentVariables,
    cancelled: &AtomicBool,
) -> Result<String> {
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
    let (output, version) = compatible_bootstrap_with_version(
        host,
        &args,
        env,
        cancelled,
        Instant::now() + Duration::from_secs(25),
    )?;
    anyhow::ensure!(
        output.status.success(),
        "negotiate remote wire contracts: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    parse_sessions(host, &output.stdout).context("validate remote wire contracts")?;
    Ok(version)
}

fn compatible_bootstrap_with_version(
    host: &str,
    args: &[impl AsRef<str>],
    env: &EnvironmentVariables,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<(std::process::Output, String)> {
    let mut args: Vec<String> = args.iter().map(|arg| arg.as_ref().to_owned()).collect();
    let index = args
        .iter()
        .position(|arg| arg == "--client-version")
        .context("bootstrap has no legacy version argument")?
        + 1;
    let original = args
        .get(index)
        .context("bootstrap has no version value")?
        .clone();
    let output = ssh::bootstrap_output(
        &mut ssh_command(host, &args, env)?,
        remaining(deadline)?,
        Some(cancelled),
    )
    .context("run versioned SSH bootstrap")?;
    if output.status.success()
        || !String::from_utf8_lossy(&output.stderr).contains("Yinz compatibility mismatch")
    {
        return Ok((output, original));
    }
    let peer = ssh::bootstrap_output(
        &mut ssh_command(host, &["yinz", "--version"], env)?,
        remaining(deadline)?,
        Some(cancelled),
    )
    .context("read legacy bootstrap build identity")?;
    if !peer.status.success() {
        return Ok((output, original));
    }
    let peer_version = legacy_build_identity(&peer.stdout)?;
    let advertised = args
        .get_mut(index)
        .context("bootstrap has no legacy version value")?;
    if *advertised == peer_version {
        return Ok((output, original));
    }
    *advertised = peer_version.to_owned();
    let output = ssh::bootstrap_output(
        &mut ssh_command(host, &args, env)?,
        remaining(deadline)?,
        Some(cancelled),
    )
    .context("retry legacy SSH bootstrap with unchanged wire contracts")?;
    Ok((output, peer_version.to_owned()))
}

fn legacy_build_identity(bytes: &[u8]) -> Result<&str> {
    anyhow::ensure!(bytes.len() <= 256, "oversized remote build identity");
    let version = std::str::from_utf8(bytes)
        .context("decode remote build identity")?
        .trim()
        .strip_prefix("yinz ")
        .context("unexpected remote build identity")?;
    anyhow::ensure!(
        !version.is_empty()
            && !version.chars().any(char::is_whitespace)
            && !version.chars().any(char::is_control),
        "invalid remote build identity"
    );
    Ok(version)
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
    let options = SshOptions::default();
    let output = compatible_bootstrap(
        host,
        &args,
        env,
        cancelled,
        deadline.min(Instant::now() + Duration::from_secs(60)),
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
    /// Legacy bootstrap identity, echoed for peers that require exact release names.
    pub yinz_version: String,
    /// Actual remote build; absent on peers predating protocol-based compatibility.
    #[serde(default)]
    pub build_version: Option<String>,
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
    let output = compatible_bootstrap(
        host,
        &args,
        env,
        cancelled,
        Instant::now() + Duration::from_secs(10),
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
        response
            .build_version
            .as_deref()
            .unwrap_or(&response.yinz_version),
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
    let forwards: Vec<_> = FORWARDS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
        .filter_map(std::sync::Weak::upgrade)
        .collect();
    for forward in forwards {
        let owned = forward
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        // Killing SSH and joining its output reader must not hold ownership locks.
        drop(owned);
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
    let previous = std::mem::replace(
        &mut *CLIENT_FORWARD
            .lock()
            .unwrap_or_else(|error| error.into_inner()),
        forward,
    );
    drop(previous);
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
            let owned = forward
                ._forward
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take();
            owned.and_then(|mut owned| owned.diagnostics().ok())
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "unit/remote_forward_cleanup.rs"]
mod cleanup_tests;

pub(crate) fn attachment_rejected(reason: yinz_ipc::ipc::ExitReason) -> anyhow::Error {
    crate::remote_failure::BootstrapError {
        category: crate::remote_failure::BootstrapCategory::AttachmentRejected,
        diagnostic: anyhow!("Destination rejected attachment: {reason}"),
    }
    .into()
}

/// Query or acknowledge remote recovery offers through the versioned SSH endpoint.
pub fn recovery_request(
    host: &str,
    cancelled: &AtomicBool,
    env: &EnvironmentVariables,
    dismiss: Option<&str>,
) -> Result<Vec<yinz_runtime::session_recovery::Candidate>> {
    let contract = CLIENT_SERVER_CONTRACT_VERSION.to_string();
    let bootstrap = REMOTE_BOOTSTRAP_VERSION.to_string();
    let mut args = vec![
        "yinz",
        "remote-sessions",
        "--client-version",
        VERSION,
        "--contract-version",
        &contract,
        "--bootstrap-version",
        &bootstrap,
        "--recovery",
    ];
    if let Some(dismiss) = dismiss {
        args.extend(["--dismiss-recovery", dismiss]);
    }
    let output = compatible_bootstrap(
        host,
        &args,
        env,
        cancelled,
        Instant::now() + Duration::from_secs(10),
    )
    .context("request remote recovery")?;
    anyhow::ensure!(
        output.status.success(),
        "Remote recovery failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    if dismiss.is_some() {
        return Ok(Vec::new());
    }
    let candidates: Vec<yinz_runtime::session_recovery::Candidate> =
        serde_json::from_slice(&output.stdout).context("decode remote recovery candidates")?;
    for candidate in &candidates {
        validate_name(&candidate.name)?;
    }
    Ok(candidates)
}

#[cfg(test)]
#[path = "unit/remote_compatibility.rs"]
mod compatibility_tests;
