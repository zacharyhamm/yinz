//! Getting a socket to a broker on another machine.
//!
//! Two ssh invocations per host, with different jobs:
//!
//!   1. **Bootstrap** — `ssh <host> zellij --pty-broker ensure` runs to
//!      completion and prints where the broker is listening. The remote decides
//!      that path, never this side: `ZELLIJ_SOCK_DIR` resolves differently on
//!      Linux and macOS (`$XDG_RUNTIME_DIR` vs a `$TMPDIR` fallback), and the
//!      `sun_path` limit differs too (108 vs 104), so a locally-computed guess
//!      would be wrong in exactly the cases that are hardest to diagnose.
//!   2. **Forward** — one long-lived `ssh -N -L <local>:<remote>` per host that
//!      *owns* the forward, with every pane connecting through it.
//!
//! Why a dedicated master rather than per-pane `ssh -L`: a forward belongs to
//! the connection that created it and dies with it, so with `ControlPersist` a
//! per-pane forward would vanish the moment that pane's ssh exited, taking
//! other panes' traffic with it.
//!
//! `StreamLocal` forwarding needs OpenSSH at both ends — it is the
//! `direct-streamlocal@openssh.com` channel type, which Dropbear and most
//! embedded sshd's do not implement.

use std::collections::HashMap;
use std::io::Read;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use zellij_utils::{consts::ZELLIJ_SOCK_DIR, errors::prelude::*, input::command::RunCommand};

use super::conn::{Reconnected, Reconnector, RemoteConn};
use super::env;
use super::protocol::{
    self, BrokerToServerMsg, Frame, RemotePaneEntry, ResumeRefusal, ServerToBrokerMsg,
    REMOTE_PROTOCOL_VERSION,
};
use crate::pty_host::protocol::{
    check_socket_path_length, new_host_id, SpawnFailureKind, WireWinsize,
    SSH_CONTROL_PATH_RENAME_MARGIN,
};

/// How long to wait for the forwarded socket to become connectable.
const FORWARD_READY_TIMEOUT: Duration = Duration::from_secs(15);
const FORWARD_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// Bootstrapping runs a remote command; a cold ssh handshake plus a broker start
/// is comfortably under this, and a hang here would otherwise wedge a pane.
const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(30);

/// How long to keep retrying a resume the broker refuses because someone is
/// still attached. Deliberately the same as the supervisor's
/// `ATTACH_RETRY_WINDOW`: it is the same situation, and the thing being waited
/// for — a peer noticing its connection is over — is on the same timescale.
const RESUME_RETRY_WINDOW: Duration = Duration::from_millis(1000);
const RESUME_RETRY_INTERVAL: Duration = Duration::from_millis(50);

/// A refusal that says whether waiting could change the answer.
///
/// Its own error type so the retry above can ask, rather than reading the prose
/// a human is meant to see.
#[derive(Debug)]
struct ResumeRefusedError {
    kind: ResumeRefusal,
    message: String,
}

impl std::fmt::Display for ResumeRefusedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ResumeRefusedError {}

/// What `zellij --pty-broker ensure` prints on the remote.
#[derive(Debug, Clone, Deserialize)]
pub struct BrokerEndpoint {
    pub socket: String,
    pub protocol_version: u32,
    pub zellij_version: String,
}

fn local_socket_dir() -> PathBuf {
    ZELLIJ_SOCK_DIR.join("r")
}

/// Make sure the directory both the control socket and the forwarded socket live
/// in exists.
///
/// Called before *any* ssh, not just the one that binds the forward. The
/// `ControlMaster` in `ssh_common_args` binds its socket here too, and the first
/// ssh of a session is the bootstrap — which would otherwise die with
/// "unix_listener: cannot bind to path ...: No such file or directory" on any
/// machine that has not opened a remote pane before.
fn ensure_local_socket_dir() -> Result<PathBuf> {
    let dir = local_socket_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Owner only: a control socket is an authenticated connection to
        // another machine, and anything that can reach it can use it.
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(dir)
}

/// One local socket per host *per server*; every pane on that host multiplexes
/// through it as a separate connection, and therefore a separate ssh channel.
///
/// Keyed by the server as well as the host because `ZELLIJ_SOCK_DIR` is per
/// user, not per session. On a host-only key two zellij sessions belonging to
/// the same person land on the same path, and then: the second `start_forward`
/// unlinks and rebinds the socket the first is using, so the first silently
/// starts routing its panes through the second's ssh; `ensure_forward`'s
/// liveness probe connects happily to that foreign socket and reports all is
/// well; and whichever session closes first deletes the other's socket on its
/// way out.
///
/// Folded into the digest rather than appended to it, to keep the path well
/// inside `sockaddr_un` — 104 bytes on macOS, where the temp dir alone can be
/// nearly 50.
fn local_socket_path(instance: &str, host: &str) -> PathBuf {
    // A host may be an alias, a user@host, or contain characters a path cannot
    // hold, so key by a digest rather than the name itself.
    let digest = blake3_short(&format!("{host}\0{instance}"));
    local_socket_dir().join(format!("{digest}.sock"))
}

fn blake3_short(input: &str) -> String {
    // A short, stable, filesystem-safe key. Collisions only cost a shared
    // forward between two hosts, so 4 bytes (truncated from the 8-byte hash)
    // is ample — and every byte here is one the socket path can't spare.
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    input.hash(&mut hasher);
    format!("{:08x}", hasher.finish() as u32)
}

/// The ControlMaster socket, which stays keyed by host alone.
///
/// Unlike the forward there is no collision to fix here: nothing unlinks or
/// rebinds it, and `ControlMaster=auto` is built for exactly this — several
/// processes sharing one authenticated connection. Making it per-server would
/// cost a full handshake per host per reconnect and buy nothing.
fn control_path(host: &str) -> PathBuf {
    local_socket_dir().join(format!("{}.ctl", blake3_short(host)))
}

/// How long a socket nobody answers on has to have been there before it is
/// treated as abandoned.
///
/// `start_forward` polls for up to `FORWARD_READY_TIMEOUT` waiting for its ssh
/// to bind, so a socket younger than this may belong to a server that is still
/// starting up rather than to one that died.
const STALE_SOCKET_AGE: Duration = Duration::from_secs(60);

/// Remove forward sockets left behind by servers that are gone.
///
/// Per-server socket names cost the one bit of tidying the shared name gave for
/// free: a crashed server's socket used to be reclaimed by the next server that
/// wanted the same path. Nothing reclaims it now, so this does.
///
/// A live forward always accepts locally, even when the broker behind it is
/// unreachable, so failing to connect really does mean the ssh that bound it is
/// gone.
fn sweep_stale_sockets(dir: &Path, ours: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == ours || path.extension().and_then(|e| e.to_str()) != Some("sock") {
            continue;
        }
        let old_enough = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|m| {
                m.elapsed()
                    .map(|age| age > STALE_SOCKET_AGE)
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if !old_enough {
            continue;
        }
        if UnixStream::connect(&path).is_ok() {
            continue;
        }
        let _ = std::fs::remove_file(&path);
    }
}

/// A live ssh forward to one host. Dropping it tears the forward down.
struct HostForward {
    host: String,
    local_socket: PathBuf,
    ssh: Child,
}

impl Drop for HostForward {
    fn drop(&mut self) {
        let _ = self.ssh.kill();
        let _ = self.ssh.wait();
        let _ = std::fs::remove_file(&self.local_socket);
    }
}

/// Every host this session has panes on.
pub struct RemoteHosts {
    forwards: Mutex<HashMap<String, Arc<HostForward>>>,
    /// What makes this server's forward sockets its own. See
    /// `local_socket_path`.
    instance: String,
}

impl Default for RemoteHosts {
    // Written out rather than derived: a derived `String::default()` would give
    // every server the same empty instance id, which is the collision this
    // exists to prevent, and nothing would fail loudly enough to notice.
    fn default() -> Self {
        Self {
            forwards: Mutex::new(HashMap::new()),
            instance: new_host_id(),
        }
    }
}

impl RemoteHosts {
    pub fn new() -> Self {
        Self::default()
    }

    /// Where this server's forward to `host` listens.
    pub fn forward_socket_path(&self, host: &str) -> PathBuf {
        local_socket_path(&self.instance, host)
    }

    /// Open a pane on `host`, returning its connection and the reader's socket.
    ///
    /// The handshake and spawn both complete here, so a pane never reaches the
    /// screen in a state where the far side has not confirmed its child is
    /// running.
    ///
    /// A cached forward that fails buys one retry through a rebuilt one. The
    /// liveness probe in `ensure_forward` can only ask whether the *local* end
    /// accepts, and it always does while the ssh lives — a broker that died
    /// behind it looks healthy from here right up until the handshake reads EOF.
    /// Rebuilding re-runs `bootstrap`, which starts a broker again.
    pub fn open_pane(
        self: &Arc<Self>,
        host: &str,
        cmd: &RunCommand,
        initial_size: Option<WireWinsize>,
        terminal_id: u32,
    ) -> Result<(Arc<RemoteConn>, UnixStream)> {
        // Sampled here, per pane, rather than once when the host table was built:
        // sampling it once would freeze this side's terminal and locale for the
        // life of the session, which is the far side's bug seen from this end.
        self.open_pane_with_env(
            host,
            cmd,
            initial_size,
            terminal_id,
            env::carry_from_local(),
        )
    }

    /// `open_pane` with the carried environment supplied rather than sampled, so
    /// a test can say what this side's terminal is without mutating the process
    /// environment out from under every other test in the binary.
    pub fn open_pane_with_env(
        self: &Arc<Self>,
        host: &str,
        cmd: &RunCommand,
        initial_size: Option<WireWinsize>,
        terminal_id: u32,
        env: Vec<(String, String)>,
    ) -> Result<(Arc<RemoteConn>, UnixStream)> {
        // Sampled before the attempt: a forward this call built itself failing is
        // not something rebuilding it again would fix, and only a reused one has
        // gone stale behind our back.
        let was_cached = self.has_cached_forward(host);
        match self.try_open_pane(host, cmd, initial_size, terminal_id, &env) {
            Err(e) if was_cached && !is_a_real_answer(&e) => {
                log::warn!("retrying a pane on {host} through a fresh forward ({e:#})");
                self.forget_forward(host);
                self.try_open_pane(host, cmd, initial_size, terminal_id, &env)
            },
            result => result,
        }
    }

    fn try_open_pane(
        self: &Arc<Self>,
        host: &str,
        cmd: &RunCommand,
        initial_size: Option<WireWinsize>,
        terminal_id: u32,
        env: &[(String, String)],
    ) -> Result<(Arc<RemoteConn>, UnixStream)> {
        let socket = self.connect(host)?;
        let reader_socket = socket
            .try_clone()
            .context("failed to clone a remote pane's socket")?;

        let pane_token = handshake_and_spawn(&socket, host, cmd, initial_size, terminal_id, env)?;

        let conn = Arc::new(RemoteConn::new(
            socket,
            host.to_owned(),
            pane_token,
            cmd.clone(),
            Some(self.clone() as Arc<dyn Reconnector>),
        )?);
        Ok((conn, reader_socket))
    }

    fn has_cached_forward(&self, host: &str) -> bool {
        self.forwards
            .lock()
            .map(|forwards| forwards.contains_key(host))
            .unwrap_or(false)
    }

    /// Drop the cached forward to `host`, so the next connect rebuilds it.
    fn forget_forward(&self, host: &str) {
        if let Ok(mut forwards) = self.forwards.lock() {
            forwards.remove(host);
        }
    }

    fn connect(&self, host: &str) -> Result<UnixStream> {
        let forward = self.ensure_forward(host)?;
        UnixStream::connect(&forward.local_socket).with_context(|| {
            format!(
                "failed to reach the zellij broker on {host} through {}",
                forward.local_socket.display()
            )
        })
    }

    fn ensure_forward(&self, host: &str) -> Result<Arc<HostForward>> {
        let mut forwards = self
            .forwards
            .lock()
            .to_anyhow()
            .context("failed to lock the remote host table")?;
        if let Some(existing) = forwards.get(host) {
            // A master whose ssh has died leaves a socket that accepts nothing;
            // rebuild rather than handing out a dead forward.
            if UnixStream::connect(&existing.local_socket).is_ok() {
                return Ok(existing.clone());
            }
            forwards.remove(host);
        }
        let forward = Arc::new(start_forward(&self.instance, host)?);
        forwards.insert(host.to_owned(), forward.clone());
        Ok(forward)
    }

    /// Ask a host's broker what panes it is holding.
    ///
    /// For finding a pane a failed handoff stranded: it is running, and nothing
    /// on this machine records its token, because the only two places a token
    /// ever lived were the releasing server's memory and the broker's registry.
    /// The broker is therefore the only thing that can answer, which is why this
    /// is a question rather than a lookup.
    ///
    /// Starts a broker if none is running, exactly as opening a pane does — a
    /// host with no broker has no panes, and the empty answer is the true one.
    pub fn list_panes(self: &Arc<Self>, host: &str) -> Result<Vec<RemotePaneEntry>> {
        let socket = self.connect(host)?;
        let mut write_half = socket
            .try_clone()
            .context("failed to clone a broker socket")?;
        let mut read_half = socket
            .try_clone()
            .context("failed to clone a broker socket")?;

        say_hello(&mut write_half, &mut read_half, host)?;
        protocol::write_control(&mut write_half, &ServerToBrokerMsg::ListPanes)?;
        match read_broker_msg(&mut read_half)? {
            BrokerToServerMsg::PaneList { entries } => Ok(entries),
            other => Err(anyhow!("expected a pane list from {host}, got {other:?}")),
        }
    }

    /// Drop every forward, on the way out.
    pub fn shutdown(&self) {
        if let Ok(mut forwards) = self.forwards.lock() {
            forwards.clear();
        }
    }

    /// One attempt at picking a pane back up. See `Reconnector::reconnect`.
    fn try_resume(&self, host: &str, pane_token: &str, received: u64) -> Result<Reconnected> {
        // Drop the cached forward first. The usual reason a pane's connection
        // died is that the forward did, and reusing a dead one would fail in a
        // way that looks like the broker refusing us.
        self.forget_forward(host);
        let socket = self.connect(host)?;
        let read_socket = socket
            .try_clone()
            .context("failed to clone a reconnected pane's socket")?;
        let dropped = handshake_and_resume(&socket, host, pane_token, received)?;
        Ok(Reconnected {
            write_socket: socket,
            read_socket,
            dropped,
        })
    }
}

impl Reconnector for RemoteHosts {
    /// Pick a pane back up, retrying briefly if the broker still thinks someone
    /// has it.
    ///
    /// The retry is the same one `pty_host_client::attach` does for a local
    /// supervisor, for the same reason. A connection the broker has not noticed
    /// is over still holds the pane, and "not noticed yet" resolves in a
    /// scheduling quantum: after an outage, the old connection is a socket whose
    /// read has not returned; after a warp with no `Release` — a server that
    /// vanished mid-handoff — likewise.
    ///
    /// `Release` makes this unnecessary in the ordinary warp, which is the
    /// point of having it: the pane is provably free before the token is handed
    /// over. This is what stops the *un*ordinary one from losing a pane outright.
    fn reconnect(&self, host: &str, pane_token: &str, received: u64) -> Result<Reconnected> {
        let deadline = Instant::now() + RESUME_RETRY_WINDOW;
        loop {
            match self.try_resume(host, pane_token, received) {
                Ok(resumed) => return Ok(resumed),
                Err(e) => {
                    let worth_waiting_for = matches!(
                        e.downcast_ref::<ResumeRefusedError>(),
                        Some(ResumeRefusedError {
                            kind: ResumeRefusal::AlreadyAttached,
                            ..
                        })
                    );
                    if !worth_waiting_for || Instant::now() >= deadline {
                        return Err(e);
                    }
                    std::thread::sleep(RESUME_RETRY_INTERVAL);
                },
            }
        }
    }
}

/// Ask the remote where its broker listens, starting one if needed.
pub fn bootstrap(host: &str) -> Result<BrokerEndpoint> {
    ensure_local_socket_dir()?;
    // ssh's ControlPath is bound as a unix socket too, and overflowing
    // `sun_path` there fails deep inside ssh with a message about multiplexing
    // rather than about the length.
    check_socket_path_length(&control_path(host), SSH_CONTROL_PATH_RENAME_MARGIN)?;
    let mut child = Command::new("ssh")
        .args(ssh_common_args(host))
        .arg(host)
        .arg("zellij")
        .arg("--pty-broker")
        .arg("ensure")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to run ssh to bootstrap a broker on {host}"))?;

    let deadline = Instant::now() + BOOTSTRAP_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(FORWARD_POLL_INTERVAL),
            Ok(None) => {
                let _ = child.kill();
                return Err(anyhow!(
                    "timed out waiting for {host} to report where its zellij broker listens"
                ));
            },
            Err(e) => return Err(e).context("failed waiting for the bootstrap ssh"),
        }
    }
    let output = child
        .wait_with_output()
        .context("failed to collect the bootstrap ssh output")?;
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !output.status.success() {
        return Err(anyhow!(
            "could not start a zellij broker on {host}: {}",
            if stderr.is_empty() {
                "ssh failed with no message".to_owned()
            } else {
                stderr
            }
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find(|l| l.trim_start().starts_with('{'))
        .ok_or_else(|| {
            anyhow!(
                "{host} did not report a broker endpoint. Is zellij on its PATH? \
                 (ssh said: {stderr})"
            )
        })?;
    let endpoint: BrokerEndpoint = serde_json::from_str(line)
        .with_context(|| format!("could not parse the broker endpoint from {host}: {line}"))?;

    if endpoint.protocol_version != REMOTE_PROTOCOL_VERSION {
        // Name both sides; a version gap is the most likely failure here and the
        // least likely to be guessed from a connection error.
        return Err(anyhow!(
            "remote pane protocol mismatch with {host}: it speaks v{} (zellij {}), this \
             zellij speaks v{REMOTE_PROTOCOL_VERSION} (zellij {}). Install matching zellij \
             versions on both machines.",
            endpoint.protocol_version,
            endpoint.zellij_version,
            zellij_utils::consts::VERSION,
        ));
    }
    Ok(endpoint)
}

/// Options for the short-lived invocations: bootstrap, and nothing else.
///
/// Multiplexed, because bootstrapping happens once per host per session and
/// again on every reconnect, and a persistent master turns each repeat into a
/// channel on an existing connection rather than a fresh handshake.
fn ssh_common_args(host: &str) -> Vec<String> {
    vec![
        "-o".to_owned(),
        "BatchMode=yes".to_owned(),
        "-o".to_owned(),
        "ControlMaster=auto".to_owned(),
        "-o".to_owned(),
        format!("ControlPath={}", control_path(host).display()),
        "-o".to_owned(),
        "ControlPersist=60".to_owned(),
    ]
}

/// Options for the ssh that owns the forward.
///
/// Deliberately *not* multiplexed, which is the whole reason this exists
/// separately. Given a control master to use — and the bootstrap always leaves
/// one behind — `ssh -N -T -L` hands the forward to that master and exits
/// immediately with status 0. Two things break at once: the forward now belongs
/// to a master that outlives this `HostForward` and ignores its `Drop`, and the
/// wait below sees a process that has already exited and calls a working forward
/// a failure.
///
/// Its own connection means the forward really does belong to the process that
/// owns it, which is what the rest of this file assumes.
fn ssh_forward_args() -> Vec<String> {
    vec![
        "-o".to_owned(),
        "BatchMode=yes".to_owned(),
        "-o".to_owned(),
        "ControlMaster=no".to_owned(),
        "-o".to_owned(),
        "ControlPath=none".to_owned(),
    ]
}

fn start_forward(instance: &str, host: &str) -> Result<HostForward> {
    let endpoint = bootstrap(host)?;

    let dir = ensure_local_socket_dir()?;
    let local_socket = local_socket_path(instance, host);
    check_socket_path_length(&local_socket, 0)?;
    sweep_stale_sockets(&dir, &local_socket);
    // ssh refuses to bind over an existing file, and a stale one from a crashed
    // session is the common case. The failure it produces otherwise names only
    // the bind, not the reason.
    let _ = std::fs::remove_file(&local_socket);

    let ssh = Command::new("ssh")
        .args(ssh_forward_args())
        .arg("-N")
        .arg("-T")
        .arg("-L")
        .arg(format!("{}:{}", local_socket.display(), endpoint.socket))
        .arg(host)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start the ssh forward to {host}"))?;

    let mut forward = HostForward {
        host: host.to_owned(),
        local_socket,
        ssh,
    };

    // Wait for the forward to actually carry a connection. `ssh` returning
    // immediately does not mean the socket is bound yet.
    let deadline = Instant::now() + FORWARD_READY_TIMEOUT;
    loop {
        if UnixStream::connect(&forward.local_socket).is_ok() {
            return Ok(forward);
        }
        if let Ok(Some(status)) = forward.ssh.try_wait() {
            let mut stderr = String::new();
            if let Some(mut pipe) = forward.ssh.stderr.take() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            return Err(anyhow!(
                "the ssh forward to {} exited ({status}): {}",
                forward.host,
                stderr.trim()
            ));
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "timed out waiting for the ssh forward to {} to come up. Does its sshd allow \
                 unix-socket forwarding? (OpenSSH only; Dropbear and most embedded sshd's do \
                 not implement it)",
                forward.host
            ));
        }
        std::thread::sleep(FORWARD_POLL_INTERVAL);
    }
}

/// Say hello, ask for the pane, and wait for the far side to confirm it started.
fn handshake_and_spawn(
    socket: &UnixStream,
    host: &str,
    cmd: &RunCommand,
    initial_size: Option<WireWinsize>,
    terminal_id: u32,
    env: &[(String, String)],
) -> Result<String> {
    let mut write_half = socket
        .try_clone()
        .context("failed to clone a remote pane's socket")?;
    let mut read_half = socket
        .try_clone()
        .context("failed to clone a remote pane's socket")?;

    say_hello(&mut write_half, &mut read_half, host)?;

    protocol::write_control(
        &mut write_half,
        &ServerToBrokerMsg::Spawn {
            cmd: cmd.clone(),
            failover_cmd: None,
            initial_size,
            env: env.to_vec(),
            terminal_id,
        },
    )?;
    match read_broker_msg(&mut read_half)? {
        BrokerToServerMsg::Spawned { pane_token } => Ok(pane_token),
        BrokerToServerMsg::SpawnFailed { kind, message } => match kind {
            // Rebuilt on this side so a remote pane behaves exactly as a local
            // one — a mistyped command is a mistyped command either way. The id
            // is a placeholder the far machine could not know; `spawn_terminal`
            // rewrites it with the one it reserved before asking.
            SpawnFailureKind::CommandNotFound => Err(ZellijError::CommandNotFound {
                terminal_id: 0,
                command: cmd.command.to_string_lossy().to_string(),
            }
            .into()),
            SpawnFailureKind::Other => Err(anyhow!("{host} could not start the pane: {message}")),
        },
        other => Err(anyhow!(
            "expected a spawn result from {host}, got {other:?}"
        )),
    }
}

/// The version handshake, shared by opening a pane and picking one back up.
fn say_hello(write_half: &mut UnixStream, read_half: &mut UnixStream, host: &str) -> Result<()> {
    protocol::write_control(
        write_half,
        &ServerToBrokerMsg::Hello {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            zellij_version: zellij_utils::consts::VERSION.to_owned(),
        },
    )?;
    match read_broker_msg(read_half)? {
        BrokerToServerMsg::HelloAck {
            protocol_version, ..
        } if protocol_version == REMOTE_PROTOCOL_VERSION => Ok(()),
        BrokerToServerMsg::HelloAck {
            protocol_version,
            zellij_version,
        } => Err(anyhow!(
            "remote pane protocol mismatch with {host}: it speaks v{protocol_version} \
             (zellij {zellij_version}), this zellij speaks v{REMOTE_PROTOCOL_VERSION}"
        )),
        other => Err(anyhow!("expected a hello ack from {host}, got {other:?}")),
    }
}

/// Pick a pane back up on a fresh connection.
fn handshake_and_resume(
    socket: &UnixStream,
    host: &str,
    pane_token: &str,
    received: u64,
) -> Result<u64> {
    let mut write_half = socket
        .try_clone()
        .context("failed to clone a remote pane's socket")?;
    let mut read_half = socket
        .try_clone()
        .context("failed to clone a remote pane's socket")?;

    say_hello(&mut write_half, &mut read_half, host)?;
    protocol::write_control(
        &mut write_half,
        &ServerToBrokerMsg::Resume {
            pane_token: pane_token.to_owned(),
            received,
        },
    )?;
    match read_broker_msg(&mut read_half)? {
        BrokerToServerMsg::Resumed { dropped } => Ok(dropped),
        BrokerToServerMsg::ResumeRefused { kind, reason } => Err(ResumeRefusedError {
            kind,
            message: format!("{host} would not give this pane back: {reason}"),
        }
        .into()),
        other => Err(anyhow!(
            "expected a resume result from {host}, got {other:?}"
        )),
    }
}

/// Whether the far side answered rather than went missing.
///
/// A `ZellijError` at this point was rebuilt from a `SpawnFailed` the broker
/// sent, so a healthy broker is on the other end and a mistyped command is its
/// answer. Rebuilding the forward under it would spend an ssh to be told the
/// same thing.
fn is_a_real_answer(err: &anyhow::Error) -> bool {
    err.downcast_ref::<ZellijError>().is_some()
}

fn read_broker_msg(reader: &mut UnixStream) -> Result<BrokerToServerMsg> {
    match protocol::read_frame(reader)? {
        Some(frame @ Frame::Control(_)) => frame.decode_control(),
        Some(other) => Err(anyhow!("expected a control frame, got {other:?}")),
        None => Err(anyhow!("the broker closed the connection")),
    }
}

#[cfg(test)]
#[path = "../../unit/pty_host_remote_ssh_tests.rs"]
mod pty_host_remote_ssh_tests;
