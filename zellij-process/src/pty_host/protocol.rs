//! Wire format between zellij-server and a pane's pty-host supervisor.
//!
//! Framing is a 4-byte little-endian length prefix followed by a JSON body, the
//! same shape as the client/server contract in `zellij_ipc::ipc` but without
//! prost — the message volume here is a handful per pane lifetime, and staying
//! off the protobuf assets keeps `cargo x proto` / `assets --check` out of the
//! loop for every protocol tweak.

use std::io::{self, Read, Write};
use std::path::PathBuf;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use zellij_runtime::errors::prelude::*;
use {zellij_runtime::consts::ZELLIJ_SOCK_DIR, zellij_types::input::command::RunCommand};

/// Bumped whenever the message set changes incompatibly. A supervisor outlives
/// the server that spawned it, so a later zellij may well find itself talking to
/// an older host; both ends check this and refuse cleanly rather than
/// misinterpreting a payload.
///
/// v2 added `Release`/`Attach`. The practical cost of the bump is that a v1
/// orphan left over from before an upgrade cannot be adopted — `pty-host adopt`
/// says so rather than failing obscurely.
pub const PROTOCOL_VERSION: u32 = 2;

/// The descriptor the control channel is `dup2`'d onto before the supervisor
/// execs. Picked as the first free fd after stdio.
pub const CONTROL_FD: std::os::unix::io::RawFd = 3;

/// Refuse absurd frames rather than allocating whatever a peer claims.
const MAX_FRAME_LEN: u32 = 4 * 1024 * 1024;

/// Identifies one supervisor. 12 hex chars, short enough to keep the socket path
/// well inside `sockaddr_un.sun_path`.
pub type HostId = String;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerToHostMsg {
    Hello {
        protocol_version: u32,
        zellij_version: String,
    },
    Spawn {
        terminal_id: u32,
        session_name: Option<String>,
        cmd: RunCommand,
        failover_cmd: Option<RunCommand>,
        termios: Option<WireTermios>,
        initial_size: Option<WireWinsize>,
        /// What to pass as `argv[0]`, if not the command itself.
        ///
        /// A login shell is nothing more than a shell whose `argv[0]` begins
        /// with `-`; that single character is what makes bash read
        /// `~/.bash_profile` and ash read `~/.profile`. Only the remote broker
        /// sets it — see `remote::broker::resolve_default_shell` — and it is
        /// `serde(default)` rather than a version bump because an older
        /// supervisor simply spawning without it is the pre-existing behaviour.
        #[serde(default)]
        arg0: Option<String>,
        /// The child's environment, applied over the inherited one in order -
        /// later entries win, which is what makes this a `Vec` rather than a map.
        ///
        /// Empty for every local pane: a local pane inherits the server's
        /// environment and always has. Only the remote broker fills it, because
        /// only a remote pane has an environment it cannot inherit — see
        /// `remote::env`.
        env: Vec<(String, String)>,
        /// This child is a remote *login*: scrub the frozen `SSH_*`/`ZELLIJ_*`
        /// leftovers of whichever ssh session started this supervisor's broker,
        /// and set `SSH_TTY` to the pty this pane is about to get - a path only
        /// this process, after `openpty`, is in a position to name.
        login_session: bool,
        /// Give the pane real job control: the supervisor takes the pty as its
        /// controlling terminal and the child gets a process group, so `^Z`
        /// actually stops the command. See the module docs in `host.rs`.
        ///
        /// Asked for rather than assumed, because a suspended pane is only
        /// recoverable if somebody is listening for `ChildStopped` and can send
        /// `SIGCONT` back. The remote broker is not — it leaves this false, so a
        /// remote pane keeps the `login_tty` behaviour where `^Z` is discarded
        /// and no pane can be stopped with no way out.
        ///
        /// `serde(default)` so an older server driving a newer supervisor gets
        /// exactly what it got before, rather than a feature it cannot finish.
        #[serde(default)]
        job_control: bool,
    },
    // Note: there is deliberately no `Respawn`. `re_run_command_in_terminal`
    // terminates the supervisor and spawns a fresh one, which matches today's
    // behavior exactly (a new `openpty` and a new waiter bound to the *new*
    // `quit_cb`) and avoids a stale callback surviving a re-run.
    /// Kill the child and exit. Nothing else ends a supervisor — a dropped
    /// control connection means "detached", not "shut down".
    Terminate,
    Query,
    /// Give the pane up without killing it, so someone else can take it.
    ///
    /// A dropped control channel already means "detached", so this carries no
    /// information the supervisor could not eventually infer. What it buys is
    /// *ordering*: `event_loop` polls the control channel before the named
    /// socket, so a `Release` written before the new owner is told to connect is
    /// always processed before that owner's `Attach` is accepted. Relying on EOF
    /// instead would race two processes against each other.
    Release,
    /// Take a detached supervisor over: re-key it to the attaching server's pane
    /// and hand the pty master back.
    ///
    /// Only ever arrives on the named socket — the accepted connection *becomes*
    /// the new control channel, which is why it cannot be a control-channel
    /// message.
    Attach {
        protocol_version: u32,
        zellij_version: String,
        terminal_id: u32,
        session_name: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HostToServerMsg {
    HelloAck {
        protocol_version: u32,
        zellij_version: String,
        host_id: HostId,
        host_pid: u32,
        /// Where the supervisor bound its named socket, or `None` if binding
        /// failed. Binding is not fatal: it only powers `pty-host list|kill` and
        /// (later) re-attach, so a long `$TMPDIR` or an unwritable runtime dir
        /// degrades those rather than breaking pane creation.
        control_socket: Option<PathBuf>,
    },
    /// Sent with the pty master fd as `SCM_RIGHTS` ancillary data.
    Spawned {
        child_pid: u32,
    },
    SpawnFailed {
        kind: SpawnFailureKind,
        message: String,
    },
    ChildExited {
        status: Option<i32>,
    },
    /// The child stopped — `SIGTSTP` from `^Z`, almost always.
    ///
    /// Deliberately not modelled as a kind of exit: the pane is still alive and
    /// still owns its pty, it simply cannot be woken from inside itself. A
    /// command pane has no shell above the child to `fg` it, so without this the
    /// pane is unrecoverable.
    ///
    /// This needs no `PROTOCOL_VERSION` bump. The handshake already requires an
    /// exact match (`client.rs`), so a supervisor is never a different build
    /// than the server talking to it.
    ChildStopped {
        signal: i32,
    },
    /// The child is running again, whether or not we are the ones who continued
    /// it — an external `kill -CONT` clears the pane's suspended state too.
    ChildContinued,
    Status(HostStatus),
    /// Sent with the pty master fd as `SCM_RIGHTS` ancillary data, exactly like
    /// `Spawned`. The connection this arrives on is the new control channel.
    AttachAck {
        protocol_version: u32,
        zellij_version: String,
        host_id: HostId,
        host_pid: u32,
        child_pid: u32,
        /// The structured command. `HostStatus.command` only carries its display
        /// form, and the adopting server needs `hold_on_close` to rebuild a
        /// `quit_cb` that behaves like the original.
        ///
        /// `originating_plugin` is cleared before this is sent: its plugin and
        /// client ids belong to the session the pane came from, and would name
        /// something else entirely in the session adopting it.
        cmd: RunCommand,
        status: HostStatus,
        /// What accumulated in the ring while nobody was attached, so the gap
        /// between one server letting go and the next taking hold costs nothing.
        ///
        /// Base64 rather than `Vec<u8>` because serde_json renders a byte vector
        /// as a JSON array of decimals — up to 4 bytes on the wire per byte of
        /// output, which a full 256 KiB ring would push against `MAX_FRAME_LEN`.
        buffered_base64: String,
        /// Non-zero when the ring overflowed while detached. Reported rather than
        /// hidden: the adopting pane is missing the *middle* of its output, and
        /// that is worth a warning rather than a silently mangled screen.
        dropped_bytes: u64,
    },
    AttachRefused {
        reason: AttachRefusal,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachRefusal {
    /// Someone else is already holding this supervisor's control channel. Two
    /// servers reading the same master would each get half the output.
    AlreadyAttached,
    ProtocolMismatch,
    /// The supervisor never got as far as spawning a child, so there is no pty
    /// to hand over.
    NoSession,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpawnFailureKind {
    CommandNotFound,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostStatus {
    pub host_id: HostId,
    pub session_name: Option<String>,
    pub terminal_id: u32,
    pub command: String,
    pub cwd: Option<PathBuf>,
    pub child_pid: Option<u32>,
    pub host_pid: u32,
    pub started_at_unix: u64,
    pub attached: bool,
    /// `Some(status)` once the child has exited; `None` while it is running.
    pub child_exit_status: Option<Option<i32>>,
    pub buffered_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireWinsize {
    pub cols: u16,
    pub rows: u16,
    pub width_in_pixels: u16,
    pub height_in_pixels: u16,
}

// --- framing ---

pub fn write_msg<T: Serialize, W: Write>(writer: &mut W, msg: &T) -> Result<()> {
    let err_context = || "failed to write pty-host message".to_string();
    let body = serde_json::to_vec(msg).with_context(err_context)?;
    let len: u32 = body
        .len()
        .try_into()
        .map_err(|_| anyhow!("pty-host message too large: {} bytes", body.len()))
        .with_context(err_context)?;
    if len > MAX_FRAME_LEN {
        return Err(anyhow!("pty-host message too large: {len} bytes")).with_context(err_context);
    }
    writer
        .write_all(&len.to_le_bytes())
        .with_context(err_context)?;
    writer.write_all(&body).with_context(err_context)?;
    writer.flush().with_context(err_context)
}

/// Reads one framed message. Returns `Ok(None)` on a clean EOF at a frame
/// boundary, which is how the peer disconnecting is reported.
pub fn read_msg<T: DeserializeOwned, R: Read>(reader: &mut R) -> Result<Option<T>> {
    let err_context = || "failed to read pty-host message".to_string();
    let mut len_bytes = [0u8; 4];
    match reader.read_exact(&mut len_bytes) {
        Ok(()) => {},
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e).with_context(err_context),
    }
    let len = u32::from_le_bytes(len_bytes);
    if len > MAX_FRAME_LEN {
        return Err(anyhow!("pty-host frame claims {len} bytes")).with_context(err_context);
    }
    let mut body = vec![0u8; len as usize];
    reader.read_exact(&mut body).with_context(err_context)?;
    serde_json::from_slice(&body)
        .map(Some)
        .with_context(err_context)
}

// --- buffered output encoding ---

/// Encode the detached ring's contents for `AttachAck`.
///
/// Pty output is arbitrary bytes, not text — it is half-written UTF-8 and raw
/// escape sequences — so it cannot ride in a JSON string directly, and a JSON
/// array of decimals would cost up to 4 wire bytes per byte. Base64 is 4 bytes
/// per 3, which keeps a full `RING_CAPACITY` well inside `MAX_FRAME_LEN`.
pub fn encode_buffered(bytes: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;
    STANDARD.encode(bytes)
}

pub fn decode_buffered(encoded: &str) -> Result<Vec<u8>> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;
    STANDARD
        .decode(encoded)
        .context("failed to decode the buffered pty output in an AttachAck")
}

// --- socket layout ---

pub fn socket_dir() -> PathBuf {
    ZELLIJ_SOCK_DIR.join("pty")
}

pub fn socket_path(host_id: &str) -> PathBuf {
    socket_dir().join(format!("{host_id}.sock"))
}

pub fn metadata_path(host_id: &str) -> PathBuf {
    socket_dir().join(format!("{host_id}.json"))
}

/// OpenSSH's `mux.c` binds `ControlMaster` at `<ControlPath>.<random>` before renaming
/// it into place — observed suffixes are `.` + 16 random chars (17 bytes); round up for
/// safety. Pass this as `margin` to [`check_socket_path_length`] for any path ssh itself
/// binds through (i.e. a `ControlPath`), 0 for a path zellij binds directly.
pub const SSH_CONTROL_PATH_RENAME_MARGIN: usize = 20;

/// A `sockaddr_un.sun_path` is 104 bytes on macOS and 108 elsewhere. The default
/// `ZELLIJ_SOCK_DIR` leaves plenty of room, but a long `$TMPDIR` fallback does
/// not, so check before binding rather than failing obscurely inside `bind`.
///
/// `margin` reserves extra bytes beyond `path` itself — for a `ControlPath`, ssh binds
/// a longer temp name first (see [`SSH_CONTROL_PATH_RENAME_MARGIN`]), so `path` alone
/// fitting is not enough.
pub fn check_socket_path_length(path: &std::path::Path, margin: usize) -> Result<()> {
    use zellij_runtime::consts::ZELLIJ_SOCK_MAX_LENGTH;
    let len = path.as_os_str().len() + margin;
    if len >= ZELLIJ_SOCK_MAX_LENGTH {
        return Err(anyhow!(
            "pty-host socket path is {} bytes ({} margin reserved), over the {} byte limit: {}",
            len,
            margin,
            ZELLIJ_SOCK_MAX_LENGTH,
            path.display()
        ));
    }
    Ok(())
}

pub fn new_host_id() -> HostId {
    let uuid = uuid::Uuid::new_v4();
    uuid.simple().to_string()[..12].to_string()
}

pub use zellij_runtime::termios::WireTermios;

#[cfg(test)]
#[path = "../unit/pty_host_protocol_tests.rs"]
mod pty_host_protocol_tests;
