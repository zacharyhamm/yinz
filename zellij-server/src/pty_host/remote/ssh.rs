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
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use zellij_utils::{consts::ZELLIJ_SOCK_DIR, errors::prelude::*, input::command::RunCommand};

use super::conn::RemoteConn;
use super::protocol::{self, BrokerToServerMsg, Frame, ServerToBrokerMsg, REMOTE_PROTOCOL_VERSION};
use crate::pty_host::protocol::{check_socket_path_length, SpawnFailureKind, WireWinsize};

/// How long to wait for the forwarded socket to become connectable.
const FORWARD_READY_TIMEOUT: Duration = Duration::from_secs(15);
const FORWARD_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// Bootstrapping runs a remote command; a cold ssh handshake plus a broker start
/// is comfortably under this, and a hang here would otherwise wedge a pane.
const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(30);

/// What `zellij --pty-broker ensure` prints on the remote.
#[derive(Debug, Clone, Deserialize)]
pub struct BrokerEndpoint {
    pub socket: String,
    pub protocol_version: u32,
    pub zellij_version: String,
}

fn local_socket_dir() -> PathBuf {
    ZELLIJ_SOCK_DIR.join("remote")
}

/// One local socket per host; every pane on that host multiplexes through it as
/// a separate connection, and therefore a separate ssh channel.
fn local_socket_path(host: &str) -> PathBuf {
    // A host may be an alias, a user@host, or contain characters a path cannot
    // hold, so key by a digest rather than the name itself.
    let digest = blake3_short(host);
    local_socket_dir().join(format!("{digest}.sock"))
}

fn blake3_short(input: &str) -> String {
    // A short, stable, filesystem-safe key. Collisions only cost a shared
    // forward between two hosts, so 8 bytes is ample.
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    input.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn control_path(host: &str) -> PathBuf {
    local_socket_dir().join(format!("{}.ctl", blake3_short(host)))
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
#[derive(Default)]
pub struct RemoteHosts {
    forwards: Mutex<HashMap<String, Arc<HostForward>>>,
}

impl RemoteHosts {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a pane on `host`, returning its connection and the reader's socket.
    ///
    /// The handshake and spawn both complete here, so a pane never reaches the
    /// screen in a state where the far side has not confirmed its child is
    /// running.
    pub fn open_pane(
        &self,
        host: &str,
        cmd: &RunCommand,
        initial_size: Option<WireWinsize>,
    ) -> Result<(Arc<RemoteConn>, UnixStream)> {
        let forward = self.ensure_forward(host)?;
        let socket = UnixStream::connect(&forward.local_socket).with_context(|| {
            format!(
                "failed to reach the zellij broker on {host} through {}",
                forward.local_socket.display()
            )
        })?;
        let reader_socket = socket
            .try_clone()
            .context("failed to clone a remote pane's socket")?;

        handshake_and_spawn(&socket, host, cmd, initial_size)?;

        let conn = Arc::new(RemoteConn::new(socket, host.to_owned())?);
        Ok((conn, reader_socket))
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
        let forward = Arc::new(start_forward(host)?);
        forwards.insert(host.to_owned(), forward.clone());
        Ok(forward)
    }

    /// Drop every forward, on the way out.
    pub fn shutdown(&self) {
        if let Ok(mut forwards) = self.forwards.lock() {
            forwards.clear();
        }
    }
}

/// Ask the remote where its broker listens, starting one if needed.
pub fn bootstrap(host: &str) -> Result<BrokerEndpoint> {
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

fn start_forward(host: &str) -> Result<HostForward> {
    let endpoint = bootstrap(host)?;

    let dir = local_socket_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let local_socket = local_socket_path(host);
    check_socket_path_length(&local_socket)?;
    // ssh refuses to bind over an existing file, and a stale one from a crashed
    // session is the common case. The failure it produces otherwise names only
    // the bind, not the reason.
    let _ = std::fs::remove_file(&local_socket);

    let ssh = Command::new("ssh")
        .args(ssh_common_args(host))
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
) -> Result<()> {
    let mut write_half = socket
        .try_clone()
        .context("failed to clone a remote pane's socket")?;
    let mut read_half = socket
        .try_clone()
        .context("failed to clone a remote pane's socket")?;

    protocol::write_control(
        &mut write_half,
        &ServerToBrokerMsg::Hello {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            zellij_version: zellij_utils::consts::VERSION.to_owned(),
        },
    )?;
    match read_broker_msg(&mut read_half)? {
        BrokerToServerMsg::HelloAck {
            protocol_version, ..
        } if protocol_version == REMOTE_PROTOCOL_VERSION => {},
        BrokerToServerMsg::HelloAck {
            protocol_version,
            zellij_version,
        } => {
            return Err(anyhow!(
                "remote pane protocol mismatch with {host}: it speaks v{protocol_version} \
                 (zellij {zellij_version}), this zellij speaks v{REMOTE_PROTOCOL_VERSION}"
            ))
        },
        other => return Err(anyhow!("expected a hello ack from {host}, got {other:?}")),
    }

    protocol::write_control(
        &mut write_half,
        &ServerToBrokerMsg::Spawn {
            cmd: cmd.clone(),
            failover_cmd: None,
            initial_size,
        },
    )?;
    match read_broker_msg(&mut read_half)? {
        BrokerToServerMsg::Spawned => Ok(()),
        BrokerToServerMsg::SpawnFailed { kind, message } => match kind {
            // Rebuilt on this side so the eight `downcast_ref::<ZellijError>()`
            // sites in pty.rs behave for a remote pane exactly as for a local
            // one — a mistyped command is a mistyped command either way.
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
