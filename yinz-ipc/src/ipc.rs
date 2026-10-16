//! IPC stuff for starting to split things into a client and server model.
use interprocess::local_socket::Stream as LocalSocketStream;
use log::warn;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fmt::{Display, Error, Formatter},
    io::{self, Read, Write},
    marker::PhantomData,
};
use yinz_runtime::errors::prelude::*;
use yinz_types::data::ClientId;
use yinz_types::data::PaneId;
use yinz_types::data::SessionConnection;
use yinz_types::diagnostics::ErrorContext;
use yinz_types::input::actions::Action;
use yinz_types::input::cli_assets::CliAssets;
use yinz_types::internal_ids::SessionName;
use yinz_types::internal_ids::TabNumber;
use yinz_types::keys::KeyModifier;
use yinz_types::keys::KeyWithModifier;
use yinz_types::pane_size::Size;
use yinz_types::pane_size::SizeInPixels;
use yinz_types::style::HostTerminalThemeMode;
use yinz_types::style::Style;

// Protobuf imports
use crate::client_server_contract::client_server_contract::{
    ClientToServerMsg as ProtoClientToServerMsg, ServerToClientMsg as ProtoServerToClientMsg,
};
use prost::Message;

mod enum_conversions;
mod protobuf_conversion;

#[cfg(test)]
mod tests;

type SessionId = u64;

/// A bidirectional byte stream that supports cloning for simultaneous read/write.
pub trait IpcStream: Read + Write + Send + 'static {
    fn try_clone_stream(&self) -> io::Result<Box<dyn IpcStream>>;

    /// Bound every read with `timeout`, or remove the bound with `None`.
    ///
    /// A read that runs out of time fails with `WouldBlock` (Linux) or
    /// `TimedOut` (elsewhere), which `classify_read_error` reports as
    /// [`IpcRecv::Idle`] if no frame bytes arrived. A deadline that cuts a frame
    /// short makes the receiver terminal, since its next boundary is unknown.
    fn set_read_timeout(&self, timeout: Option<std::time::Duration>) -> io::Result<()>;
}

impl IpcStream for LocalSocketStream {
    fn try_clone_stream(&self) -> io::Result<Box<dyn IpcStream>> {
        use interprocess::TryClone;
        Ok(Box::new(self.try_clone()?))
    }

    fn set_read_timeout(&self, timeout: Option<std::time::Duration>) -> io::Result<()> {
        use interprocess::local_socket::traits::Stream as _;
        self.set_recv_timeout(timeout)
    }
}

#[derive(PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct Session {
    // Unique ID for this session
    id: SessionId,
    // Identifier for the underlying IPC primitive (socket, pipe)
    conn_name: String,
    // User configured alias for the session
    alias: String,
}

// How do we want to connect to a session?
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientType {
    Reader,
    Writer,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct ClientAttributes {
    pub size: Size,
    pub style: Style,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelDimensions {
    pub text_area_size: Option<SizeInPixels>,
    pub character_cell_size: Option<SizeInPixels>,
    /// The grid observed with this text-area report, independent of resize-message ordering.
    #[serde(default)]
    pub text_area_grid_size: Option<Size>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct ColorRegister {
    pub index: usize,
    pub color: String,
}

impl PixelDimensions {
    pub fn merge(&mut self, other: PixelDimensions) {
        if let Some(text_area_size) = other.text_area_size {
            self.text_area_size = Some(text_area_size);
            self.text_area_grid_size = other.text_area_grid_size;
        }
        if let Some(character_cell_size) = other.character_cell_size {
            self.character_cell_size = Some(character_cell_size);
        }
    }
}

/// Filters for the explicitly selected normalized agent lifecycle stream.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventSubscription {
    pub panes: Vec<String>,
    pub projects: Vec<String>,
    pub kinds: Vec<String>,
    pub lifecycle_types: Vec<String>,
    pub cursor: Option<String>,
}

// Types of messages sent from the client to the server
#[allow(clippy::large_enum_variant)]
#[derive(
    Serialize,
    Deserialize,
    Debug,
    Clone,
    PartialEq,
    strum_macros::IntoStaticStr,
    strum_macros::VariantNames,
)]
pub enum ClientToServerMsg {
    AgentControl(yinz_types::agent::control::AgentRequest),
    PluginRequest {
        request_id: String,
        request: yinz_types::cli::PluginRequest,
    },
    DataRequest {
        request_id: String,
        request: yinz_types::cli::DataRequest,
    },
    AgentReport(yinz_types::agent::report::AgentReport),
    DetachSession {
        client_ids: Vec<ClientId>,
    },
    TerminalPixelDimensions {
        pixel_dimensions: PixelDimensions,
    },
    BackgroundColor {
        color: String,
    },
    ForegroundColor {
        color: String,
    },
    ColorRegisters {
        color_registers: Vec<ColorRegister>,
    },
    TerminalResize {
        new_size: Size,
    },
    FirstClientConnected {
        cli_assets: CliAssets,
    },
    AttachClient {
        cli_assets: CliAssets,
        tab_number_to_focus: Option<TabNumber>,
        #[serde(default, with = "yinz_types::pane_id::legacy_reference")]
        pane_to_focus: Option<PaneId>,
    },
    AttachWatcherClient {
        terminal_size: Size,
    },
    Action {
        action: Action,
        terminal_id: Option<yinz_types::internal_ids::TerminalId>,
        client_id: Option<ClientId>,
        is_cli_client: bool,
    },
    Key {
        key: KeyWithModifier,
        raw_bytes: Vec<u8>,
        is_kitty_keyboard_protocol: bool,
    },
    Paste(String),
    ClientExited,
    KillSession,
    KillSessionWithCompletion,
    ConnStatus,
    SubscribeToPaneRenders {
        pane_ids: Vec<PaneId>,
        scrollback: Option<usize>,
        ansi: bool,
    },
    /// `yinz events`: stream the session's state events, as plugins receive them. Event types
    /// travel by name (`EventType::from_cli_name`); an empty list is every streamable one.
    SubscribeToEvents {
        event_types: Vec<String>,
        #[serde(default)]
        options: EventSubscription,
    },
    DesktopNotificationResponse {
        raw_bytes: Vec<u8>,
    },
    ForwardedReplyFromHost {
        token: u32,
        reply_bytes: Vec<u8>,
    },
    HostTerminalThemeChanged {
        mode: HostTerminalThemeMode,
    },
    NestedSessionFrameFromHost {
        payload_bytes: Vec<u8>,
    },
    KittyGraphicsSupport {
        supported: bool,
    },
    SixelSupport {
        supported: bool,
    },
    /// Whether this client's host terminal presents a frame atomically (DECSET 2026
    /// synchronised output). The server uses it to decide whether the cursor has to be hidden
    /// while a frame paints: inside an atomic frame nothing intermediate is ever displayed, so
    /// the hide/show pair is churn that restarts the host's cursor blink phase on every frame.
    SynchronizedOutputSupport {
        supported: bool,
    },
    RequestSessionList,
    /// The host terminal gained or lost keyboard focus (DECSET 1004 report).
    HostTerminalFocusChanged {
        focused: bool,
    },
    /// Which modifier keys the host terminal says are physically down, sent when the set
    /// changes. The set is complete each time, so an empty one means everything came back up.
    ModifiersHeld {
        modifiers: BTreeSet<KeyModifier>,
    },
}

// Types of messages sent from the server to the client
#[derive(
    Serialize,
    Deserialize,
    Debug,
    Clone,
    PartialEq,
    strum_macros::IntoStaticStr,
    strum_macros::VariantNames,
)]
pub enum ServerToClientMsg {
    AgentControl(yinz_types::agent::control::AgentResponse),
    PluginResponse {
        request_id: String,
        result: Result<yinz_types::cli::PluginResponse, String>,
    },
    DataResponse {
        request_id: String,
        result: Result<String, String>,
    },
    KillSessionComplete {
        error: Option<String>,
    },
    AgentReportAccepted {
        report_id: String,
    },
    Render {
        content: String,
    },
    UnblockInputThread,
    Exit {
        exit_reason: ExitReason,
    },
    Connected {
        warp_companions: bool,
        #[serde(default)]
        warp_held_terminals: bool,
    },
    Log {
        lines: Vec<String>,
    },
    LogError {
        lines: Vec<String>,
    },
    SwitchSession {
        connect_to_session: SessionConnection,
    },
    UnblockCliPipeInput {
        pipe_name: String,
    },
    /// Built with [`ServerToClientMsg::cli_pipe_output`], which holds it to the frame limit.
    CliPipeOutput {
        pipe_name: String,
        output: String,
    },
    QueryTerminalSize,
    RenamedSession {
        name: SessionName,
    },
    ConfigFileUpdated,
    SetMouseMode {
        enabled: bool,
    },
    PaneRenderUpdate {
        pane_id: PaneId,
        viewport: Vec<String>,
        scrollback: Option<Vec<String>>,
        is_initial: bool,
    },
    SubscribedPaneClosed {
        pane_id: PaneId,
    },
    /// One event for a `SubscribeToEvents` client: its type by name, and the `Event` as JSON.
    /// JSON rather than a second protobuf mirror of `Event`, which already has one for plugins.
    EventUpdate {
        event_type: String,
        event_json: String,
    },
    ForwardQueryToHost {
        token: u32,
        query_bytes: Vec<u8>,
        resolve_async: bool,
    },
    EmitNestedSessionFrame {
        payload_bytes: Vec<u8>,
    },
}

impl ServerToClientMsg {
    /// A CLI pipe's output, refused when its frame would be over [`MAX_IPC_FRAME_LEN`].
    ///
    /// [`IpcSenderWithContext::send_server_msg`] refuses that frame too, but on the thread that
    /// writes to the client, where the refusal can only be logged; a CLI then takes the unblock
    /// that follows for a reply with no output. Refused here, the server can answer the pipe.
    pub fn cli_pipe_output(pipe_name: String, output: String) -> Result<Self> {
        // Measured on the wire message itself, which takes both strings without copying them.
        let frame = ProtoServerToClientMsg::from(Self::CliPipeOutput { pipe_name, output });
        check_frame_len(frame.encoded_len())?;
        Self::try_from(frame)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum ExitReason {
    Normal,
    NormalDetached,
    ForceDetached,
    CannotAttach,
    Disconnect,
    KickedByHost,
    CustomExitStatus(i32),
    Error(String),
}

impl Display for ExitReason {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
        match self {
            Self::Normal => write!(f, "Bye from Yinz!"),
            Self::NormalDetached => write!(f, "Session detached"),
            Self::ForceDetached => write!(
                f,
                "Session was detached from this client (possibly because another client connected)"
            ),
            Self::CannotAttach => write!(
                f,
                "Session attached to another client. Use --force flag to force connect."
            ),
            Self::Disconnect => {
                let session_tip = match yinz_runtime::envs::get_session_name() {
                    Ok(name) => format!("`yinz attach {}`", name),
                    Err(_) => "see `yinz ls` and `yinz attach`".to_string(),
                };
                write!(
                    f,
                    "
Your yinz client lost connection to the yinz server.

As a safety measure, you have been disconnected from the current yinz session.
However, the session should still exist and none of your data should be lost.

This usually means that your terminal didn't process server messages quick
enough. Maybe your system is currently under high load, or your terminal
isn't performant enough.

There are a few things you can try now:
    - Reattach to your previous session and see if it works out better this
      time: {session_tip}
    - Try using a faster (maybe GPU-accelerated) terminal emulator
    "
                )
            },
            Self::KickedByHost => write!(f, "Disconnected by host"),
            Self::CustomExitStatus(exit_status) => write!(f, "Exit {}", exit_status),
            Self::Error(e) => write!(f, "Error occurred in server:\n{}", e),
        }
    }
}

/// Sends messages on a stream socket, along with an [`ErrorContext`].
pub struct IpcSenderWithContext<T: Serialize> {
    sender: Box<dyn IpcStream>,
    closed: bool,
    _phantom: PhantomData<T>,
}

impl<T: Serialize> IpcSenderWithContext<T> {
    /// Returns a sender to the given [LocalSocketStream](interprocess::local_socket::LocalSocketStream).
    pub fn new(sender: LocalSocketStream) -> Self {
        Self::from_boxed(Box::new(sender))
    }

    fn from_boxed(sender: Box<dyn IpcStream>) -> Self {
        Self {
            sender,
            closed: false,
            _phantom: PhantomData,
        }
    }

    pub fn send_client_msg(&mut self, msg: ClientToServerMsg) -> Result<()> {
        let proto_msg: ProtoClientToServerMsg = msg.try_into()?;
        self.send_message(&proto_msg)
    }

    pub fn send_server_msg(&mut self, msg: ServerToClientMsg) -> Result<()> {
        let proto_msg: ProtoServerToClientMsg = msg.into();
        self.send_message(&proto_msg)
    }

    fn send_message(&mut self, msg: &impl Message) -> Result<()> {
        if self.closed {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "IPC sender is closed").into());
        }
        let result = write_protobuf_message(&mut self.sender, msg)
            .and_then(|()| self.sender.flush().map_err(Into::into));
        // A partial write loses the frame boundary. Retrying on this stream would
        // make the next message part of the failed one. Encoding/size errors occur
        // before any bytes are written, so those leave the sender usable.
        if result.as_ref().is_err_and(|error| error.is::<io::Error>()) {
            self.closed = true;
        }
        result
    }

    /// Returns an [`IpcReceiverWithContext`] with the same socket as this sender.
    pub fn get_receiver<F>(&self) -> io::Result<IpcReceiverWithContext<F>>
    where
        F: for<'de> Deserialize<'de> + Serialize,
    {
        let socket = self.sender.try_clone_stream()?;
        Ok(IpcReceiverWithContext::from_boxed(socket))
    }
}

/// The outcome of one read on an IPC socket.
///
/// The distinction that matters is `Eof` versus `Malformed`. A malformed frame
/// is a single bad message on a stream that may well carry good ones after it,
/// so retrying is reasonable. `Eof` means the peer is gone: it is terminal, it
/// never blocks, and every subsequent read returns it immediately — so a caller
/// that retries `Eof` is a busy loop, not a recovery.
pub enum IpcRecv<T> {
    Msg(T, ErrorContext),
    /// The peer closed the socket, or framing is irrecoverably lost. Terminal.
    Eof,
    /// A frame arrived but could not be read as a message. Possibly survivable.
    Malformed,
    /// Nothing has arrived yet. A blocking socket with no deadline never says
    /// this — it waits — so this comes from non-blocking transports, test
    /// doubles, and sockets carrying a receive timeout (see `assert_socket` in
    /// `sessions.rs`, which probes a possibly-wedged server). Without a name for
    /// it "nothing yet" gets counted as "garbage", which is the conflation this
    /// enum exists to stop.
    Idle,
}

impl<T> IpcRecv<T> {
    /// The message, if this read produced one.
    pub fn msg(self) -> Option<(T, ErrorContext)> {
        match self {
            IpcRecv::Msg(msg, err_ctx) => Some((msg, err_ctx)),
            _ => None,
        }
    }

    pub fn is_eof(&self) -> bool {
        matches!(self, IpcRecv::Eof)
    }
}

/// Decide whether a failed read means "the peer hung up" or "that frame was bad".
///
/// Only a complete frame with an invalid protobuf can be skipped safely.
/// Transport and framing failures are terminal: the unread bytes are not
/// necessarily the start of another frame.
fn classify_read_error<T>(e: anyhow::Error, msg_type: &str) -> IpcRecv<T> {
    if let Some(io_err) = e.downcast_ref::<io::Error>() {
        // A receive timeout expiring is not a bad frame: nothing arrived at all,
        // and the stream is still open and still in sync. Linux reports it as
        // `WouldBlock`, other platforms as `TimedOut`.
        if matches!(
            io_err.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ) {
            return IpcRecv::Idle;
        }
        if io_err.kind() != io::ErrorKind::UnexpectedEof {
            warn!("Error reading {}: {:?}", msg_type, e);
        }
        return IpcRecv::Eof;
    }
    // Previously discarded outright, which is what made a desynchronised stream
    // impossible to tell apart from a hangup after the fact.
    warn!("Error reading {}: {:?}", msg_type, e);
    IpcRecv::Malformed
}

/// Receives messages on a stream socket, along with an [`ErrorContext`].
pub struct IpcReceiverWithContext<T> {
    receiver: io::BufReader<Box<dyn IpcStream>>,
    closed: bool,
    _phantom: PhantomData<T>,
}

impl<T> IpcReceiverWithContext<T>
where
    T: for<'de> Deserialize<'de> + Serialize,
{
    /// Returns a receiver to the given [LocalSocketStream](interprocess::local_socket::LocalSocketStream).
    pub fn new(receiver: LocalSocketStream) -> Self {
        Self {
            receiver: io::BufReader::new(Box::new(receiver)),
            closed: false,
            _phantom: PhantomData,
        }
    }

    fn from_boxed(receiver: Box<dyn IpcStream>) -> Self {
        Self {
            receiver: io::BufReader::new(receiver),
            closed: false,
            _phantom: PhantomData,
        }
    }

    /// Bound every following read with `timeout`, or lift the bound with `None`.
    ///
    /// See [`IpcStream::set_read_timeout`] for what a read that runs out of time
    /// returns, including when the deadline interrupts a frame.
    pub fn set_recv_timeout(&self, timeout: Option<std::time::Duration>) -> io::Result<()> {
        self.receiver.get_ref().set_read_timeout(timeout)
    }

    pub fn recv_client_msg(&mut self) -> IpcRecv<ClientToServerMsg> {
        if self.closed {
            return IpcRecv::Eof;
        }
        match read_protobuf_message::<ProtoClientToServerMsg>(&mut self.receiver) {
            Ok(proto_msg) => match proto_msg.try_into() {
                Ok(rust_msg) => IpcRecv::Msg(rust_msg, ErrorContext::default()),
                Err(e) => {
                    warn!("Error converting protobuf to ClientToServerMsg: {:?}", e);
                    IpcRecv::Malformed
                },
            },
            Err(e) => {
                let received = classify_read_error(e, "ClientToServerMsg");
                self.closed = received.is_eof();
                received
            },
        }
    }

    pub fn recv_server_msg(&mut self) -> IpcRecv<ServerToClientMsg> {
        if self.closed {
            return IpcRecv::Eof;
        }
        match read_protobuf_message::<ProtoServerToClientMsg>(&mut self.receiver) {
            Ok(proto_msg) => match proto_msg.try_into() {
                Ok(rust_msg) => IpcRecv::Msg(rust_msg, ErrorContext::default()),
                Err(e) => {
                    warn!("Error converting protobuf to ServerToClientMsg: {:?}", e);
                    IpcRecv::Malformed
                },
            },
            Err(e) => {
                let received = classify_read_error(e, "ServerToClientMsg");
                self.closed = received.is_eof();
                received
            },
        }
    }

    /// Returns an [`IpcSenderWithContext`] with the same socket as this receiver.
    pub fn get_sender<F: Serialize>(&self) -> io::Result<IpcSenderWithContext<F>> {
        let socket = self.receiver.get_ref().try_clone_stream()?;
        Ok(IpcSenderWithContext::from_boxed(socket))
    }
}

/// Refuse absurd frames rather than allocating whatever a peer claims.
///
/// The length prefix below is read straight off the wire and used as an
/// allocation size, so without this a peer — or a corrupted stream — asking for
/// 4 GiB gets it. Generous rather than tight, because real messages can be large:
/// moving a pane between sessions carries its whole scrollback as an ANSI dump.
/// The same guard exists on the pty-host's own wire format, which sizes it far
/// smaller because it only ever carries control messages.
pub use crate::MAX_IPC_FRAME_LEN;

// Protobuf wire format utilities
/// The frame reader, reachable from the tests in this crate so the length guard
/// can be exercised without standing up a socket pair.
#[cfg(test)]
pub(crate) fn read_protobuf_message_for_test(
    reader: &mut impl Read,
) -> Result<ProtoClientToServerMsg> {
    read_protobuf_message(reader)
}

fn read_protobuf_message<T: Message + Default>(reader: &mut impl Read) -> Result<T> {
    // Read length-prefixed protobuf message
    let mut len_bytes = [0u8; 4];
    // An idle read is retryable only before the first byte of a frame. Once
    // that byte arrives, a failed read must never restart inside its payload.
    reader.read_exact(&mut len_bytes[..1])?;
    reader
        .read_exact(&mut len_bytes[1..])
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let len = u32::from_le_bytes(len_bytes) as usize;
    if len > MAX_IPC_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("refusing a {len} byte IPC frame, over the {MAX_IPC_FRAME_LEN} byte limit"),
        )
        .into());
    }

    let mut buf = vec![0u8; len];
    reader
        .read_exact(&mut buf)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    T::decode(&buf[..]).map_err(Into::into)
}

/// Refuse a frame over [`MAX_IPC_FRAME_LEN`] before any of it is written.
fn check_frame_len(len: usize) -> Result<()> {
    if len > MAX_IPC_FRAME_LEN {
        return Err(anyhow!(
            "refusing to send a {len} byte IPC frame, over the {MAX_IPC_FRAME_LEN} byte limit"
        ));
    }
    Ok(())
}

fn write_protobuf_message<T: Message>(writer: &mut impl Write, msg: &T) -> Result<()> {
    let len = msg.encoded_len();
    check_frame_len(len)?;
    // Build one complete frame before touching the socket. There is no buffered
    // writer to retry an undelivered tail when it is dropped after a write error.
    let mut frame = Vec::with_capacity(4 + len);
    frame.extend_from_slice(&(len as u32).to_le_bytes());
    msg.encode(&mut frame)?;
    writer.write_all(&frame)?;
    Ok(())
}

// Protobuf helper functions
pub fn send_protobuf_client_to_server(
    sender: &mut IpcSenderWithContext<ClientToServerMsg>,
    msg: ClientToServerMsg,
) -> Result<()> {
    sender.send_client_msg(msg)
}

pub fn send_protobuf_server_to_client(
    sender: &mut IpcSenderWithContext<ServerToClientMsg>,
    msg: ServerToClientMsg,
) -> Result<()> {
    sender.send_server_msg(msg)
}

pub fn recv_protobuf_client_to_server(
    receiver: &mut IpcReceiverWithContext<ClientToServerMsg>,
) -> Option<(ClientToServerMsg, ErrorContext)> {
    receiver.recv_client_msg().msg()
}

pub fn recv_protobuf_server_to_client(
    receiver: &mut IpcReceiverWithContext<ServerToClientMsg>,
) -> Option<(ServerToClientMsg, ErrorContext)> {
    receiver.recv_server_msg().msg()
}

/// Asynchronously send `ClientToServerMsg::KillSession` to the peer at `path`
/// and wait until the peer's existing shutdown path replies (or its socket
/// closes). Either of those outcomes confirms the kill landed; the caller
/// wraps this in `tokio::time::timeout` to bound the wait against a wedged
/// peer.
///
/// The local socket is bidirectional, so the same async stream is used for
/// both send and receive.
#[cfg(unix)]
pub async fn async_send_kill_and_await(path: &std::path::Path) -> io::Result<()> {
    use interprocess::local_socket::traits::tokio::Stream as _;
    use interprocess::local_socket::{prelude::*, GenericFilePath};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let fs_name = path.to_fs_name::<GenericFilePath>()?;
    let mut stream: interprocess::local_socket::tokio::Stream =
        interprocess::local_socket::tokio::Stream::connect(fs_name).await?;

    let proto_msg: ProtoClientToServerMsg =
        crate::ipc::ClientToServerMsg::KillSession
            .try_into()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let encoded = proto_msg.encode_to_vec();
    let len_bytes = (encoded.len() as u32).to_le_bytes();

    stream.write_all(&len_bytes).await?;
    stream.write_all(&encoded).await?;
    // Best-effort flush; failing here doesn't mean the kill failed.
    let _ = stream.flush().await;

    // The peer's shutdown path sends `ServerToClientMsg::Exit { Normal }`
    // (yinz-server/src/lib.rs ServerInstruction::KillSession) over this
    // same socket before exiting; if it dies without ACKing, the stream
    // closes. Either outcome -- a successful 4-byte length-prefix read or a
    // read error/EOF -- confirms the kill is no longer in flight.
    let mut len_buf = [0u8; 4];
    let _ = stream.read_exact(&mut len_buf).await;
    Ok(())
}

/// Wait up to ten seconds for confirmed server teardown.
#[cfg(unix)]
pub fn kill_session_with_completion(path: &std::path::Path) -> io::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            async_kill_session_with_completion(path),
        )
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "waiting for shutdown completion"))?
    })
}

/// Unlike the legacy kill, only a post-teardown reply confirms completion.
#[cfg(unix)]
pub async fn async_kill_session_with_completion(path: &std::path::Path) -> io::Result<()> {
    use interprocess::local_socket::traits::tokio::Stream as _;
    use interprocess::local_socket::{prelude::*, GenericFilePath};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream: interprocess::local_socket::tokio::Stream =
        interprocess::local_socket::tokio::Stream::connect(path.to_fs_name::<GenericFilePath>()?)
            .await?;
    let request: ProtoClientToServerMsg =
        ClientToServerMsg::KillSessionWithCompletion
            .try_into()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let body = request.encode_to_vec();
    stream.write_all(&(body.len() as u32).to_le_bytes()).await?;
    stream.write_all(&body).await?;
    stream.flush().await?;
    loop {
        let len = stream.read_u32_le().await? as usize;
        if len > MAX_IPC_FRAME_LEN {
            return Err(io::Error::other("oversized kill completion response"));
        }
        let mut body = vec![0; len];
        stream.read_exact(&mut body).await?;
        let response = ProtoServerToClientMsg::decode(body.as_slice())?;
        match ServerToClientMsg::try_from(response).map_err(io::Error::other)? {
            ServerToClientMsg::KillSessionComplete { error: None } => return Ok(()),
            ServerToClientMsg::KillSessionComplete { error: Some(error) } => {
                return Err(io::Error::other(error))
            },
            ServerToClientMsg::Exit { .. } => {
                return Err(io::Error::other(
                    "server exited without confirming shutdown completion",
                ))
            },
            // Broadcasts queued before the kill can still arrive on this socket.
            _ => {},
        }
    }
}
