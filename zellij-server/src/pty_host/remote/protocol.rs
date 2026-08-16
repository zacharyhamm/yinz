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
use zellij_utils::{errors::prelude::*, input::command::RunCommand};

use crate::pty_host::protocol::{SpawnFailureKind, WireWinsize};

/// Bumped whenever the message set changes incompatibly. Both ends check it
/// during the handshake and refuse with a message naming both versions — a
/// remote box running a different zellij build is the normal case, not the
/// exception, so the failure has to be legible.
pub const REMOTE_PROTOCOL_VERSION: u32 = 1;

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

/// A signal to deliver to the pane's child.
///
/// Symbolic on purpose. The local server never learns the remote child's pid —
/// a pid from another machine is not merely useless locally, it is *dangerous*,
/// because it will happily name an unrelated local process. The broker resolves
/// this against the child it actually owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChildSignal {
    Hup,
    Int,
    Kill,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ServerToBrokerMsg {
    Hello {
        protocol_version: u32,
        zellij_version: String,
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
    Spawn {
        cmd: RunCommand,
        failover_cmd: Option<RunCommand>,
        initial_size: Option<WireWinsize>,
    },
    Resize(WireWinsize),
    SignalChild(ChildSignal),
    Terminate,
    /// Pick a pane back up on a fresh connection after the transport dropped.
    ///
    /// `received` is how many output bytes this side has actually taken
    /// delivery of. The broker replays from exactly there, so a reconnect is a
    /// continuation of the stream rather than a guess at one — the local grid is
    /// a byte-stream consumer that already holds the pre-outage state.
    Resume {
        pane_token: String,
        received: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BrokerToServerMsg {
    HelloAck {
        protocol_version: u32,
        zellij_version: String,
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
    /// The named pane is not one this broker still holds: it was never known, or
    /// it was reclaimed while nobody was connected.
    ResumeRefused { reason: String },
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
        cwd: String,
        exit_code: Option<i32>,
        metadata_base64: String,
        config_base64: String,
    },
    /// What the broker can see about the pane from its own machine: the things
    /// the local server would otherwise get from `sysinfo` and `tcgetpgrp`, and
    /// cannot, because the process is not local.
    StatusReport {
        cwd: Option<String>,
        /// The argv of the pane's own process.
        shell_argv: Option<Vec<String>>,
        /// The argv of whatever currently owns the terminal, when that is
        /// something other than the pane's own process. Kept distinct because
        /// the server reports the two differently — an `is_foreground` flag
        /// rides along to the plugins that classify agents.
        foreground_argv: Option<Vec<String>>,
    },
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
