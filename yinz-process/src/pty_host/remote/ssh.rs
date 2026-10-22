//! Getting a socket to a broker on another machine.
//!
//! Two ssh invocations per host and option set, with different jobs:
//!
//!   1. **Bootstrap** — `ssh <host> yinz --pty-broker ensure` runs to
//!      completion and prints where the broker is listening. The remote decides
//!      that path, never this side: `YINZ_SOCK_DIR` resolves differently on
//!      Linux and macOS (`$XDG_RUNTIME_DIR` vs the `~/.yinz/run` fallback), and the
//!      `sun_path` limit differs too (108 vs 104), so a locally-computed guess
//!      would be wrong in exactly the cases that are hardest to diagnose.
//!   2. **Forward** — one long-lived `ssh -T -L <local>:<remote> <host> 'exec cat'` per target
//!      that *owns* the forward, with every matching pane connecting through it.
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
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use yinz_types::internal_ids::TerminalId;

use serde::Deserialize;
use yinz_runtime::errors::prelude::*;
use yinz_runtime::shared::ensure_secure_dir;
use {yinz_runtime::consts::YINZ_SOCK_DIR, yinz_types::input::command::RunCommand};

pub use crate::ssh::SshOptions;
use crate::ssh::{
    bootstrap_output, ssh_option_value, validate_ssh_host, HostForward, BOOTSTRAP_TIMEOUT,
};
#[cfg(test)]
use crate::ssh::{SshOutputCapture, SSH_OUTPUT_CAPACITY};

use super::conn::{Reconnected, Reconnector, RemoteConn};
use super::env;
use super::protocol::{
    self, BrokerToServerMsg, Frame, RemotePaneEntry, ResumeRefusal, ServerToBrokerMsg,
    REMOTE_PROTOCOL_VERSION,
};
use crate::pty_host::protocol::{
    check_socket_path_length, SpawnFailureKind, WireWinsize, SSH_CONTROL_PATH_RENAME_MARGIN,
};

/// How long to keep retrying a resume the broker refuses because someone is
/// still attached. Deliberately the same as the supervisor's
/// `ATTACH_RETRY_WINDOW`: it is the same situation, and the thing being waited
/// for — a peer noticing its connection is over — is on the same timescale.
const RESUME_RETRY_WINDOW: Duration = Duration::from_millis(1000);

/// How long one reply in the resume handshake may take to arrive.
///
/// Two round trips over a forward that is already up, so seconds is generous —
/// and a bound is the point rather than the number. Before there was one, a
/// forward whose ssh was alive locally but dead on the wire held the first
/// resume attempt until TCP itself gave up, and the fallback that rebuilds the
/// forward was never reached. See `RemoteHosts::resume_through_forward`.
const RESUME_REPLY_TIMEOUT: Duration = Duration::from_secs(5);
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

/// What `yinz --pty-broker ensure` prints on the remote.
#[derive(Debug, Clone, Deserialize)]
pub struct BrokerEndpoint {
    pub socket: String,
    pub protocol_version: u32,
    pub yinz_version: String,
}

fn local_socket_dir() -> PathBuf {
    YINZ_SOCK_DIR.join("r")
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
    // Owner only: a control socket is an authenticated connection to another
    // machine, and anything that can reach it can use it.
    ensure_secure_dir(&dir)?;
    Ok(dir)
}

/// One local socket per host and option set *per server*; every pane on that
/// target multiplexes through it as a separate connection, and therefore a
/// separate ssh channel.
///
/// Keyed by the server as well as the host because `YINZ_SOCK_DIR` is per
/// user, not per session. On a host-only key two yinz sessions belonging to
/// the same person land on the same path, and then: the second `start_forward`
/// unlinks and rebinds the socket the first is using, so the first silently
/// starts routing its panes through the second's ssh; `ensure_forward`'s
/// liveness probe connects happily to that foreign socket and reports all is
/// well; and whichever session closes first deletes the other's socket on its
/// way out.
///
/// Folded into the digest rather than appended to it, to keep the path well
/// inside `sockaddr_un` — 104 bytes on macOS.
fn local_socket_path(instance: &str, host: &str, options: &SshOptions) -> PathBuf {
    // A host may be an alias, a user@host, or contain characters a path cannot
    // hold, so key by a digest rather than the name itself.
    let digest = socket_key(&format!("{host}\0{options:?}\0{instance}"));
    local_socket_dir().join(format!("{digest}.sock"))
}

fn socket_key(input: &str) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use sha2::{Digest, Sha256};
    // A collision can reuse an authenticated connection to the wrong host.
    // 96 bits fit in 16 path characters, leaving room for the socket directory
    // and OpenSSH's extra control-socket suffix. Do not fall back to shorter keys.
    URL_SAFE_NO_PAD.encode(&Sha256::digest(input.as_bytes())[..12])
}

/// The ControlMaster socket, keyed by host and authentication options.
///
/// Unlike the forward this is shared across servers: `ControlMaster=auto`
/// lets several processes reuse one authenticated connection to the same target.
fn control_path(host: &str, options: &SshOptions) -> PathBuf {
    local_socket_dir().join(format!(
        "{}.ctl",
        socket_key(&format!("{host}\0{options:?}"))
    ))
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
        if !entry
            .file_type()
            .map(|file_type| yinz_ipc::consts::is_ipc_socket(&file_type))
            .unwrap_or(false)
        {
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
        if crate::pty_host::client::socket_is_dead(&path) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Every host this session has panes on.
pub struct RemoteHosts {
    forwards: Mutex<HashMap<SshTarget, Arc<HostForward>>>,
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
            instance: uuid::Uuid::new_v4().simple().to_string()[..12].to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SshTarget {
    host: String,
    options: SshOptions,
}

impl SshTarget {
    fn new(host: &str, options: SshOptions) -> Self {
        Self {
            host: host.to_owned(),
            options,
        }
    }
}

struct SshReconnector {
    hosts: Arc<RemoteHosts>,
    target: SshTarget,
}

impl RemoteHosts {
    pub fn new() -> Self {
        Self::default()
    }

    /// Where this server's forward to `host` listens.
    pub fn forward_socket_path(&self, host: &str) -> PathBuf {
        local_socket_path(&self.instance, host, &SshOptions::default())
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
        terminal_id: TerminalId,
    ) -> Result<(Arc<RemoteConn>, UnixStream)> {
        self.open_pane_with_ssh_options(host, cmd, initial_size, terminal_id, SshOptions::default())
    }

    pub fn open_pane_with_ssh_options(
        self: &Arc<Self>,
        host: &str,
        cmd: &RunCommand,
        initial_size: Option<WireWinsize>,
        terminal_id: TerminalId,
        ssh_options: SshOptions,
    ) -> Result<(Arc<RemoteConn>, UnixStream)> {
        // Sampled here, per pane, rather than once when the host table was built:
        // sampling it once would freeze this side's terminal and locale for the
        // life of the session, which is the far side's bug seen from this end.
        self.open_pane_with_target_and_env(
            SshTarget::new(host, ssh_options),
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
        terminal_id: TerminalId,
        env: Vec<(String, String)>,
    ) -> Result<(Arc<RemoteConn>, UnixStream)> {
        self.open_pane_with_target_and_env(
            SshTarget::new(host, SshOptions::default()),
            cmd,
            initial_size,
            terminal_id,
            env,
        )
    }

    fn open_pane_with_target_and_env(
        self: &Arc<Self>,
        target: SshTarget,
        cmd: &RunCommand,
        initial_size: Option<WireWinsize>,
        terminal_id: TerminalId,
        env: Vec<(String, String)>,
    ) -> Result<(Arc<RemoteConn>, UnixStream)> {
        // Sampled before the attempt: a forward this call built itself failing is
        // not something rebuilding it again would fix, and only a reused one has
        // gone stale behind our back.
        let was_cached = self.has_cached_forward(&target);
        match self.try_open_pane(&target, cmd, initial_size, terminal_id, &env) {
            Err(e) if was_cached && !is_a_real_answer(&e) => {
                log::warn!(
                    "retrying a pane on {} through a fresh forward ({e:#})",
                    target.host
                );
                self.forget_forward(&target);
                self.try_open_pane(&target, cmd, initial_size, terminal_id, &env)
            },
            result => result,
        }
    }

    fn try_open_pane(
        self: &Arc<Self>,
        target: &SshTarget,
        cmd: &RunCommand,
        initial_size: Option<WireWinsize>,
        terminal_id: TerminalId,
        env: &[(String, String)],
    ) -> Result<(Arc<RemoteConn>, UnixStream)> {
        let socket = self.connect(target)?;
        let reader_socket = socket
            .try_clone()
            .context("failed to clone a remote pane's socket")?;

        let pane_token =
            handshake_and_spawn(&socket, &target.host, cmd, initial_size, terminal_id, env)?;

        let conn = Arc::new(RemoteConn::new(
            socket,
            target.host.clone(),
            pane_token,
            cmd.clone(),
            Some(Arc::new(SshReconnector {
                hosts: self.clone(),
                target: target.clone(),
            }) as Arc<dyn Reconnector>),
        )?);
        Ok((conn, reader_socket))
    }

    fn has_cached_forward(&self, target: &SshTarget) -> bool {
        self.forwards
            .lock()
            .map(|forwards| forwards.contains_key(target))
            .unwrap_or(false)
    }

    /// Drop the cached forward to `target`, so the next connect rebuilds it.
    fn forget_forward(&self, target: &SshTarget) {
        if let Ok(mut forwards) = self.forwards.lock() {
            forwards.remove(target);
        }
    }

    fn connect(&self, target: &SshTarget) -> Result<UnixStream> {
        let forward = self.ensure_forward(target)?;
        UnixStream::connect(&forward.local_socket).with_context(|| {
            format!(
                "failed to reach the yinz broker on {} through {}",
                target.host,
                forward.local_socket.display()
            )
        })
    }

    fn ensure_forward(&self, target: &SshTarget) -> Result<Arc<HostForward>> {
        let mut forwards = self
            .forwards
            .lock()
            .to_anyhow()
            .context("failed to lock the remote host table")?;
        if let Some(existing) = forwards.get(target) {
            // A master whose ssh has died leaves a socket that accepts nothing;
            // rebuild rather than handing out a dead forward.
            if UnixStream::connect(&existing.local_socket).is_ok() {
                return Ok(existing.clone());
            }
            forwards.remove(target);
        }
        let forward = Arc::new(start_forward(
            &self.instance,
            &target.host,
            &target.options,
        )?);
        forwards.insert(target.clone(), forward.clone());
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
        let socket = self.connect(&SshTarget::new(host, SshOptions::default()))?;
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
    ///
    /// The forward this host already has is tried first, and only rebuilt if
    /// that fails. Rebuilding unconditionally — which this used to do, on the
    /// grounds that a pane's connection usually dies because its forward did —
    /// is wrong for the case that is not a reconnect at all: adopting a warped
    /// pane resumes through a forward that is perfectly healthy, and often busy
    /// carrying other panes on the same host. Throwing it away bought a full
    /// ssh handshake, unmultiplexed by design, in the middle of a warp whose
    /// budget is counted in seconds.
    ///
    /// Nothing is lost by trying: `ensure_forward` already probes a cached
    /// forward and rebuilds one that no longer accepts connections, and a
    /// forward that is up but whose broker is not still reaches the rebuild
    /// below, which restarts it.
    fn try_resume_target(
        &self,
        target: &SshTarget,
        pane_token: &str,
        received: u64,
    ) -> Result<Reconnected> {
        let first = match self.resume_through_forward(target, pane_token, received) {
            Ok(resumed) => return Ok(resumed),
            // The broker answered, and said no. A fresh forward reaches the same
            // broker and gets the same answer, so retrying wastes the caller's
            // budget — and `reconnect` above needs this error to arrive intact
            // to decide whether waiting is worth it.
            Err(e) if e.downcast_ref::<ResumeRefusedError>().is_some() => return Err(e),
            Err(e) => e,
        };
        self.forget_forward(target);
        self.resume_through_forward(target, pane_token, received)
            .with_context(|| {
                format!("the forward this host already had did not work either: {first:#}")
            })
    }

    /// Resume a pane through whatever forward this host currently has,
    /// building one if it has none.
    fn resume_through_forward(
        &self,
        target: &SshTarget,
        pane_token: &str,
        received: u64,
    ) -> Result<Reconnected> {
        let socket = self.connect(target)?;
        // Bounded, because `connect` does not prove anything past the forward's
        // local end. An ssh whose TCP session has silently died — the laptop
        // slept, a NAT mapping expired — keeps accepting here, and the first
        // thing to notice is this handshake, reading a reply that is never
        // coming. Unbounded, that read lasts as long as the kernel's own
        // retransmit patience, which is measured in minutes; on the adopting
        // side of a warp it is the pty thread doing the waiting. The timeout is
        // lifted again once the pane is resumed: the reader that takes the
        // socket over drives it non-blocking, and a release later sets bounds
        // of its own.
        socket
            .set_read_timeout(Some(RESUME_REPLY_TIMEOUT))
            .context("failed to bound a remote pane's resume handshake")?;
        let read_socket = socket
            .try_clone()
            .context("failed to clone a reconnected pane's socket")?;
        let dropped = handshake_and_resume(&socket, &target.host, pane_token, received)?;
        socket
            .set_read_timeout(None)
            .context("failed to unbound a resumed pane's socket")?;
        Ok(Reconnected {
            write_socket: socket,
            read_socket,
            dropped,
        })
    }

    fn reconnect_target(
        &self,
        target: &SshTarget,
        pane_token: &str,
        received: u64,
    ) -> Result<Reconnected> {
        let deadline = Instant::now() + RESUME_RETRY_WINDOW;
        loop {
            match self.try_resume_target(target, pane_token, received) {
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
        self.reconnect_target(
            &SshTarget::new(host, SshOptions::default()),
            pane_token,
            received,
        )
    }
}

impl Reconnector for SshReconnector {
    fn reconnect(&self, _host: &str, pane_token: &str, received: u64) -> Result<Reconnected> {
        self.hosts
            .reconnect_target(&self.target, pane_token, received)
    }
}

/// Ask the remote where its broker listens, starting one if needed.
pub fn bootstrap(host: &str) -> Result<BrokerEndpoint> {
    bootstrap_with_options(host, &SshOptions::default())
}

fn bootstrap_with_options(host: &str, options: &SshOptions) -> Result<BrokerEndpoint> {
    validate_ssh_host(host)?;
    ensure_local_socket_dir()?;
    // ssh's ControlPath is bound as a unix socket too, and overflowing
    // `sun_path` there fails deep inside ssh with a message about multiplexing
    // rather than about the length.
    check_socket_path_length(&control_path(host, options), SSH_CONTROL_PATH_RENAME_MARGIN)?;
    let mut command = Command::new("ssh");
    command
        .args(ssh_common_args(host, options))
        // End of options: see `validate_ssh_host`.
        .arg("--")
        .arg(host)
        .arg("yinz")
        .arg("--pty-broker")
        .arg("ensure");
    let output = bootstrap_output(&mut command, BOOTSTRAP_TIMEOUT, None)
        .with_context(|| format!("failed to run ssh to bootstrap a broker on {host}"))?;
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !output.status.success() {
        return Err(anyhow!(
            "could not start a yinz broker on {host}: {}",
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
                "{host} did not report a broker endpoint. Is yinz on its PATH? \
                 (ssh said: {stderr})"
            )
        })?;
    let endpoint: BrokerEndpoint = serde_json::from_str(line)
        .with_context(|| format!("could not parse the broker endpoint from {host}: {line}"))?;

    if endpoint.protocol_version != REMOTE_PROTOCOL_VERSION {
        // Name both sides; a version gap is the most likely failure here and the
        // least likely to be guessed from a connection error.
        return Err(anyhow!(
            "remote pane protocol mismatch with {host}: it speaks v{} (yinz {}), this \
             yinz speaks v{REMOTE_PROTOCOL_VERSION} (yinz {}). Install matching yinz \
             versions on both machines.",
            endpoint.protocol_version,
            endpoint.yinz_version,
            yinz_runtime::consts::VERSION,
        ));
    }
    Ok(endpoint)
}

/// Options for the short-lived invocations: bootstrap, and nothing else.
///
/// Multiplexed, because bootstrapping happens once per host per session and
/// again on every reconnect, and a persistent master turns each repeat into a
/// channel on an existing connection rather than a fresh handshake.
fn ssh_common_args(host: &str, options: &SshOptions) -> Vec<String> {
    let mut args = vec![
        "-o".to_owned(),
        "BatchMode=yes".to_owned(),
        "-o".to_owned(),
        "ControlMaster=auto".to_owned(),
        "-o".to_owned(),
        format!(
            "ControlPath={}",
            ssh_option_value(&control_path(host, options))
        ),
        "-o".to_owned(),
        "ControlPersist=60".to_owned(),
    ];
    args.extend(options.args());
    args
}

fn start_forward(instance: &str, host: &str, options: &SshOptions) -> Result<HostForward> {
    // `bootstrap` validates the host before anything is spawned.
    let endpoint = bootstrap_with_options(host, options)?;

    let dir = ensure_local_socket_dir()?
        .canonicalize()
        .context("failed to resolve the local SSH socket directory")?;
    let local_socket = dir.join(
        local_socket_path(instance, host, options)
            .file_name()
            .context("local SSH socket path has no filename")?,
    );
    check_socket_path_length(&local_socket, 0)?;
    sweep_stale_sockets(&dir, &local_socket);
    // ssh refuses to bind over an existing file, and a stale one from a crashed
    // session is the common case. The failure it produces otherwise names only
    // the bind, not the reason.
    let _ = std::fs::remove_file(&local_socket);

    HostForward::start(host, options, &local_socket, &endpoint.socket, None)
}

/// Say hello, ask for the pane, and wait for the far side to confirm it started.
fn handshake_and_spawn(
    socket: &UnixStream,
    host: &str,
    cmd: &RunCommand,
    initial_size: Option<WireWinsize>,
    terminal_id: TerminalId,
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
            terminal_id: terminal_id.get(),
        },
    )?;
    match read_broker_msg(&mut read_half)? {
        BrokerToServerMsg::Spawned { pane_token } => Ok(pane_token),
        BrokerToServerMsg::SpawnFailed { kind, message } => match kind {
            // Rebuilt on this side so a remote pane behaves exactly as a local
            // one — a mistyped command is a mistyped command either way. The id
            // is a placeholder the far machine could not know; `spawn_terminal`
            // rewrites it with the one it reserved before asking.
            SpawnFailureKind::CommandNotFound => Err(YinzError::CommandNotFound {
                terminal_id: yinz_types::internal_ids::TerminalId::new(0),
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
            yinz_version: yinz_runtime::consts::VERSION.to_owned(),
        },
    )?;
    match read_broker_msg(read_half)? {
        BrokerToServerMsg::HelloAck {
            protocol_version, ..
        } if protocol_version == REMOTE_PROTOCOL_VERSION => Ok(()),
        BrokerToServerMsg::HelloAck {
            protocol_version,
            yinz_version,
        } => Err(anyhow!(
            "remote pane protocol mismatch with {host}: it speaks v{protocol_version} \
             (yinz {yinz_version}), this yinz speaks v{REMOTE_PROTOCOL_VERSION}"
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
/// A `YinzError` at this point was rebuilt from a `SpawnFailed` the broker
/// sent, so a healthy broker is on the other end and a mistyped command is its
/// answer. Rebuilding the forward under it would spend an ssh to be told the
/// same thing.
fn is_a_real_answer(err: &anyhow::Error) -> bool {
    err.downcast_ref::<YinzError>().is_some()
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
