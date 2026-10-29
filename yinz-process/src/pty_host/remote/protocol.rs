//! The wire format between a server and a *remote* broker.
//!
//! Deliberately separate from `pty_host::protocol` rather than an extension of
//! it, for three reasons that all point the same way:
//!
//!   * **Opposite traffic shapes.** The supervisor protocol carries a handful of
//!     messages per pane lifetime; this one carries every byte the pane ever
//!     prints. Framing decisions that are free there are hot here.
//!   * **Opposite upgrade constraints.** A supervisor outlives the server that
//!     spawned it, so `PROTOCOL_VERSION` must tolerate an orphan left by the
//!     previous binary — which is why bumping it is expensive (see the note on
//!     that constant). A broker, by contrast, is reached by `ssh`-ing to a
//!     binary that either matches or does not; there is nothing to orphan.
//!   * **Opposite peers.** One end of this connection is on another machine, so
//!     nothing platform-shaped may cross it. See `Spawn` on why `WireTermios`
//!     does not appear here.
//!
//! Bumping `PROTOCOL_VERSION` to cover remote panes would therefore make every
//! already-running detachable pane un-adoptable across the upgrade that shipped
//! the feature, to buy nothing.

use std::io::{self, Read, Write};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use yinz_runtime::errors::prelude::*;
use yinz_types::input::command::RunCommand;

use crate::pty_host::protocol::{SpawnFailureKind, WireWinsize};

/// Bumped whenever the message set changes incompatibly. Both ends check it
/// during the handshake and refuse with a message naming both versions — a
/// remote box running a different yinz build is the normal case, not the
/// exception, so the failure has to be legible.
/// v2 lets a `Spawn` carry no command at all — `RunCommand::remote_default_shell`
/// asks the broker to resolve this machine's login shell instead. A v1 broker
/// would drop the unknown field, try to exec an empty path, and answer
/// `CommandNotFound` with a blank command name; the bump turns that into the
/// handshake's "install matching yinz versions" message, which is the whole
/// point of checking.
/// v3 adds `Release`/`Released`, so a pane can be handed to another *server*
/// rather than only picked back up by the one that lost it, and `ListPanes`, so
/// a pane stranded by a failed handoff can be found again. A v2 broker frees a
/// pane only when its connection's blocking read returns, which a warp cannot
/// wait on; without the bump the adopting side would race that and be told the
/// pane already has a server attached.
/// v4 carries the pane's `PaneTarget` on `Spawn` in place of the loose
/// `remote_host` field; a v3 broker would read every pane as
/// local and refuse a `remote_default_shell` request it cannot see the host for.
/// v5 adds `AgentReport`: a coding agent's hook inside a remote pane hands its
/// event to the broker over its socket, and the broker relays it on the pane's
/// control channel, so a report is authenticated by the transport rather than
/// parsed out of the pane's bytes where any program could forge it.
/// v6 requests an explicit Git probe directory, independently of the process cwd.
/// v7 correlates explicit refresh requests and completed probes, including unchanged results.
/// Both machines must run this protocol; there is no process-cwd fallback.
/// v8 reports additional replay-buffer losses during catch-up with `OutputGap`, so later
/// resumes include every lost byte in their stream offset.
pub const REMOTE_PROTOCOL_VERSION: u32 = 8;

/// Frame kind tags. The tag is what lets pty bytes stay raw while everything
/// else stays JSON.
pub const KIND_CONTROL: u8 = 0x00;
pub const KIND_OUTPUT: u8 = 0x01;
pub const KIND_INPUT: u8 = 0x02;

/// Refuse absurd frames rather than allocating whatever a peer claims.
pub const MAX_FRAME_LEN: u32 = 4 * 1024 * 1024;

/// The `[u32 LE total_len][u8 kind]` prefix every frame carries.
pub const FRAME_HEADER_LEN: usize = 5;

/// The largest pty payload we will put in one frame.
///
/// Matched to the reader's buffer in `terminal_bytes.rs` so a frame is a natural
/// read rather than a coalesced batch. Capping well below `MAX_FRAME_LEN` also
/// means one noisy pane cannot monopolise the ssh channel for as long as it
/// takes to push 4 MiB.
pub const MAX_STREAM_PAYLOAD: usize = 64 * 1024;

pub use yinz_process_api::ChildSignal;

/// Why a `Resume` was refused, and so whether waiting could help.
///
/// The remote counterpart of `AttachRefusal`, and split for the same reason: a
/// supervisor holding a socket for a server that has died has not noticed yet,
/// and neither has a broker whose old connection has not returned from its read.
/// Both become free within a scheduling quantum. A pane that was never here, or
/// has been reclaimed, never does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResumeRefusal {
    /// Some connection still holds the pane. Worth a brief retry: an explicit
    /// `Release` makes this impossible, but a server that vanished without one
    /// leaves the broker to notice in its own time.
    AlreadyAttached,
    /// This broker does not have that pane, and will not come to have it.
    Unknown,
}

/// One pane in a broker's registry, as `ListPanes` describes it.
///
/// Enough to recognise and recover a pane. In particular no pid: a remote
/// pid is not merely useless to the asking side, it names an unrelated local
/// process, which is the hazard `ChildSignal` exists to avoid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemotePaneEntry {
    pub pane_token: String,
    /// How the pane's command reads, already resolved — a pane that asked for
    /// the login shell shows the shell it actually got.
    pub command: String,
    /// Resolved executable followed by its exact arguments; absent on older brokers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    /// Whether some server is speaking for this pane right now. A pane stranded
    /// by a failed handoff is exactly one where this is false.
    pub attached: bool,
    /// How long it has had no server, which is what the orphan reaper measures.
    /// `None` while one is attached.
    pub unattached_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ServerToBrokerMsg {
    GetGitProbe {
        refresh_token: Option<String>,
        cwd: String,
    },
    Hello {
        protocol_version: u32,
        yinz_version: String,
    },
    /// Start the pane's process.
    ///
    /// Carries no `WireTermios`, unlike the supervisor's `Spawn`. That type is
    /// portable across *compilation*, not across machines: it ships raw termios
    /// flag words and control characters by index, and those constants differ
    /// between Linux and the BSDs — `ICANON` and `ECHO` have different values,
    /// `speed_t` is an encoded index on Linux but the literal baud number on
    /// BSD, `NCCS` is 32 vs 20, and the `c_cc` indices are reordered outright
    /// (`VINTR` is 0 on Linux and 8 on BSD, where 0 is `VEOF`). Sending them
    /// across would not fail loudly; it would set a plausible-looking garbage
    /// terminal mode. The remote `openpty` uses its own platform's defaults
    /// instead, and every real application calls `tcsetattr` itself anyway.
    ///
    /// `cmd` may name no command at all: `RunCommand::remote_default_shell` is
    /// the pane asking for *this* machine's login shell, which the asking side
    /// has no way to name. `broker::resolve_default_shell` fills it in.
    Spawn {
        cmd: RunCommand,
        initial_size: Option<WireWinsize>,
        /// What the *asking* machine is entitled to say about this pane: its
        /// terminal type and its locale, sampled per pane. Nothing else crosses
        /// — a `PATH` or a `HOME` from over here names a filesystem that does not
        /// exist over there, so the far side recomputes those from its own passwd
        /// entry instead. See `remote::env` for the full policy and for why
        /// `SSH_CLIENT`/`SSH_CONNECTION` are deliberately absent.
        ///
        /// A `Vec`, not a map: the order is the precedence when the broker
        /// concatenates it after the login environment it built.
        env: Vec<(String, String)>,
        /// This pane's id in the asking session, so the far side can set a
        /// truthful `YINZ_PANE_ID`.
        terminal_id: u32,
    },
    Resize(WireWinsize),
    SignalChild(ChildSignal),
    Terminate,
    /// Pick a pane back up on a fresh connection after the transport dropped.
    ///
    /// `received` counts delivered output bytes plus reported gaps. The broker replays
    /// from exactly there, so a reconnect is a
    /// continuation of the stream rather than a guess at one — the local grid is
    /// a byte-stream consumer that already holds the pre-outage state.
    Resume {
        pane_token: String,
        received: u64,
    },
    /// Give this pane up without ending it, so another server can `Resume` it.
    ///
    /// The remote counterpart of `ServerToHostMsg::Release`, and there for the
    /// same reason: a pane moving between sessions must be detached at a moment
    /// the releasing side can observe. Merely closing the socket does detach it
    /// eventually — the broker notices when its blocking read returns — but
    /// "eventually" is not a point in time a warp can hand the token over
    /// after, and resuming before it arrives is refused as a double attach.
    ///
    /// Emphatically not `Terminate`: the child keeps running and the broker
    /// keeps buffering its output, exactly as when a link drops.
    Release,
    /// What panes this broker is holding, for finding one that a failed handoff
    /// left with no server.
    ///
    /// Answered from the registry rather than from any connection's state,
    /// because the panes worth asking about are precisely the ones no
    /// connection is speaking for.
    ListPanes,
    /// A coding agent's hook event, from a hook running *inside* the pane
    /// named by `pane_token` (the token is in that pane's environment). Sent on
    /// a connection of its own by `yinz --pty-broker report`; the broker
    /// relays it to the server on the pane's own channel and answers
    /// `AgentReported`.
    AgentReport {
        pane_token: String,
        kind: String,
        json: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BrokerToServerMsg {
    HelloAck {
        protocol_version: u32,
        yinz_version: String,
    },
    /// The child is running.
    ///
    /// Carries no pid, deliberately — see `ChildSignal`. The token is how the
    /// pane is named on a later `Resume`; it is meaningful only to the broker
    /// that issued it.
    Spawned { pane_token: String },
    /// The pane has been picked back up.
    ///
    /// `dropped` is how many output bytes were lost before the buffer could hold
    /// them — non-zero means the replay has a hole in it, and the stream cannot
    /// simply be continued.
    Resumed { dropped: u64 },
    /// Additional output lost while the initial replay was in flight. Sent before the
    /// surviving bytes; these missing bytes still advance the peer's resume offset.
    OutputGap { dropped: u64 },
    /// The named pane is not one this broker still holds: it was never known, or
    /// it was reclaimed while nobody was connected.
    ///
    /// `kind` is what makes the difference between waiting and giving up, and it
    /// is a discriminant rather than a matter of reading `reason` because the
    /// two outcomes are opposite: one is worth retrying for a moment, the other
    /// never becomes true.
    ResumeRefused { kind: ResumeRefusal, reason: String },
    /// The pane has been let go and is free for another server to `Resume`.
    ///
    /// Sent *after* the detach has happened, never before: the whole value of
    /// this message is that receiving it means the next `Resume` cannot be
    /// refused as a double attach.
    Released,
    /// The panes this broker is holding, in answer to `ListPanes`.
    PaneList { entries: Vec<RemotePaneEntry> },
    SpawnFailed {
        kind: SpawnFailureKind,
        message: String,
    },
    /// The child is gone. This is the *only* thing that means "this pane is
    /// over"; a transport failure must never be read that way, or a dropped
    /// packet closes a pane whose process is still perfectly alive.
    ChildExited { status: Option<i32> },
    /// What git says about the pane's working directory, unparsed.
    ///
    /// Base64 because git's `--null` config output is not text and would not
    /// survive a JSON string. Sent when the cwd changes rather than on every
    /// status tick: a checkout's identity does not change while you sit in it,
    /// and forking two git processes per second per pane would be absurd.
    GitProbe {
        refresh_token: Option<String>,
        cwd: String,
        exit_code: Option<i32>,
        metadata_base64: String,
        config_base64: String,
    },
    /// What the broker can see about the pane from its own machine: the things
    /// the local server would otherwise get from `sysinfo` and `tcgetpgrp`, and
    /// cannot, because the process is not local.
    StatusReport {
        /// Absent on older brokers; only true verifies cancellation.
        #[serde(default)]
        child_group_terminated: bool,
        cwd: Option<String>,
        /// The argv of the pane's own process.
        shell_argv: Option<Vec<String>>,
        /// The argv of whatever currently owns the terminal, when that is
        /// something other than the pane's own process. Kept distinct because
        /// the server reports the two differently — an `is_foreground` flag
        /// rides along to the plugins that classify agents.
        foreground_argv: Option<Vec<String>>,
    },
    /// A hook event from a coding agent in this pane, relayed from the hook's
    /// own connection to the broker. `json` is the event as the agent wrote
    /// it; `kind` names the agent.
    AgentReport { kind: String, json: String },
    /// The answer to a hook's `AgentReport`: whether a pane with that token was
    /// attached to a server to relay it to.
    AgentReported { delivered: bool },
}

/// A frame as it comes off the wire, before any control body is interpreted.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// A JSON control body. The caller decodes it into whichever direction's
    /// message type it expects.
    Control(Vec<u8>),
    Output(Vec<u8>),
    Input(Vec<u8>),
}

impl Frame {
    fn kind(&self) -> u8 {
        match self {
            Frame::Control(_) => KIND_CONTROL,
            Frame::Output(_) => KIND_OUTPUT,
            Frame::Input(_) => KIND_INPUT,
        }
    }

    fn body(&self) -> &[u8] {
        match self {
            Frame::Control(b) | Frame::Output(b) | Frame::Input(b) => b,
        }
    }

    /// Decode a control frame into a concrete message. Returns an error for a
    /// stream frame, since those carry no JSON.
    pub fn decode_control<T: DeserializeOwned>(&self) -> Result<T> {
        match self {
            Frame::Control(body) => {
                serde_json::from_slice(body).context("failed to decode a remote control frame")
            },
            Frame::Output(_) | Frame::Input(_) => {
                Err(anyhow!("expected a control frame, got a pty stream frame"))
            },
        }
    }
}

// --- framing ---

/// `[u32 LE total_len][u8 kind][body]`, where `total_len` counts the kind byte.
fn write_frame<W: Write>(writer: &mut W, frame: &Frame) -> Result<()> {
    let err_context = || "failed to write a remote pty frame".to_string();
    let body = frame.body();
    let len: u32 = (body.len() + 1)
        .try_into()
        .map_err(|_| anyhow!("remote pty frame too large: {} bytes", body.len()))
        .with_context(err_context)?;
    if len > MAX_FRAME_LEN {
        return Err(anyhow!("remote pty frame too large: {len} bytes")).with_context(err_context);
    }
    writer
        .write_all(&len.to_le_bytes())
        .with_context(err_context)?;
    writer
        .write_all(&[frame.kind()])
        .with_context(err_context)?;
    writer.write_all(body).with_context(err_context)?;
    writer.flush().with_context(err_context)
}

/// Reads one frame. Returns `Ok(None)` on a clean EOF at a frame boundary.
///
/// Note what that does *not* mean here: for a remote pane, EOF is a transport
/// event, not a child exit. Only `ChildExited` means the pane is over.
pub fn read_frame<R: Read>(reader: &mut R) -> Result<Option<Frame>> {
    let err_context = || "failed to read a remote pty frame".to_string();
    let mut len_bytes = [0u8; 4];
    match reader.read_exact(&mut len_bytes) {
        Ok(()) => {},
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e).with_context(err_context),
    }
    let len = u32::from_le_bytes(len_bytes);
    if len > MAX_FRAME_LEN {
        return Err(anyhow!("remote pty frame claims {len} bytes")).with_context(err_context);
    }
    if len == 0 {
        return Err(anyhow!("remote pty frame is missing its kind byte")).with_context(err_context);
    }
    let mut buf = vec![0u8; len as usize];
    reader.read_exact(&mut buf).with_context(err_context)?;
    let body = buf.split_off(1);
    match buf[0] {
        KIND_CONTROL => Ok(Some(Frame::Control(body))),
        KIND_OUTPUT => Ok(Some(Frame::Output(body))),
        KIND_INPUT => Ok(Some(Frame::Input(body))),
        other => {
            Err(anyhow!("unknown remote pty frame kind {other:#04x}")).with_context(err_context)
        },
    }
}

/// Try to decode one frame from the front of `buf`, returning it and how many
/// bytes it consumed.
///
/// The blocking `read_frame` is no use to the async reader: bytes arrive from an
/// ssh channel in whatever sizes the network produced, so a frame is routinely
/// split across reads and several frames routinely arrive in one. Returns
/// `Ok(None)` when `buf` does not yet hold a whole frame, which is the ordinary
/// case rather than an error.
pub fn try_decode_frame(buf: &[u8]) -> Result<Option<(Frame, usize)>> {
    if buf.len() < 4 {
        return Ok(None);
    }
    let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if len > MAX_FRAME_LEN {
        return Err(anyhow!("remote pty frame claims {len} bytes"));
    }
    if len == 0 {
        return Err(anyhow!("remote pty frame is missing its kind byte"));
    }
    let total = 4 + len as usize;
    if buf.len() < total {
        return Ok(None);
    }
    let kind = buf[4];
    let body = buf[5..total].to_vec();
    let frame = match kind {
        KIND_CONTROL => Frame::Control(body),
        KIND_OUTPUT => Frame::Output(body),
        KIND_INPUT => Frame::Input(body),
        other => return Err(anyhow!("unknown remote pty frame kind {other:#04x}")),
    };
    Ok(Some((frame, total)))
}

pub fn write_control<T: Serialize, W: Write>(writer: &mut W, msg: &T) -> Result<()> {
    let body = serde_json::to_vec(msg).context("failed to encode a remote control frame")?;
    write_frame(writer, &Frame::Control(body))
}

/// Write pty output, splitting anything oversized into `MAX_STREAM_PAYLOAD`
/// chunks rather than refusing it.
pub fn write_output<W: Write>(writer: &mut W, bytes: &[u8]) -> Result<()> {
    for chunk in bytes.chunks(MAX_STREAM_PAYLOAD) {
        write_frame(writer, &Frame::Output(chunk.to_vec()))?;
    }
    Ok(())
}

pub fn write_input<W: Write>(writer: &mut W, bytes: &[u8]) -> Result<()> {
    for chunk in bytes.chunks(MAX_STREAM_PAYLOAD) {
        write_frame(writer, &Frame::Input(chunk.to_vec()))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../unit/pty_host_remote_protocol_tests.rs"]
mod pty_host_remote_protocol_tests;
