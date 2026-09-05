//! IPC stuff for starting to split things into a client and server model.
use crate::{
    data::{ClientId, ConnectToSession, HostTerminalThemeMode, KeyWithModifier, PaneId, Style},
    errors::{prelude::*, ErrorContext},
    input::{actions::Action, cli_assets::CliAssets},
    pane_size::{Size, SizeInPixels},
};
use interprocess::local_socket::Stream as LocalSocketStream;
use log::warn;
use serde::{Deserialize, Serialize};
use std::{
    fmt::{Display, Error, Formatter},
    io::{self, Read, Write},
    marker::PhantomData,
};

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
    /// [`IpcRecv::Idle`]. Note that a frame cut short by the deadline leaves
    /// the stream desynchronised — the bytes already read are gone — so a
    /// deadline is only safe while nothing is expected mid-frame.
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
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct PaneReference {
    pub pane_id: u32,
    pub is_plugin: bool,
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
        }
        if let Some(character_cell_size) = other.character_cell_size {
            self.character_cell_size = Some(character_cell_size);
        }
    }
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
    AgentReport(crate::agent::report::AgentReport),
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
        tab_position_to_focus: Option<usize>,
        pane_to_focus: Option<PaneReference>,
    },
    AttachWatcherClient {
        terminal_size: Size,
    },
    Action {
        action: Action,
        terminal_id: Option<u32>,
        client_id: Option<ClientId>,
        is_cli_client: bool,
    },
    Key {
        key: KeyWithModifier,
        raw_bytes: Vec<u8>,
        is_kitty_keyboard_protocol: bool,
    },
    ClientExited,
    KillSession,
    ConnStatus,
    SubscribeToPaneRenders {
        pane_ids: Vec<PaneId>,
        scrollback: Option<usize>,
        ansi: bool,
    },
    /// `zellij events`: stream the session's state events, as plugins receive them. Event types
    /// travel by name (`EventType::from_cli_name`); an empty list is every streamable one.
    SubscribeToEvents {
        event_types: Vec<String>,
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
    RequestSessionList,
    /// The host terminal gained or lost keyboard focus (DECSET 1004 report).
    HostTerminalFocusChanged {
        focused: bool,
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
    Connected,
    Log {
        lines: Vec<String>,
    },
    LogError {
        lines: Vec<String>,
    },
    SwitchSession {
        connect_to_session: ConnectToSession,
    },
    UnblockCliPipeInput {
        pipe_name: String,
    },
    CliPipeOutput {
        pipe_name: String,
        output: String,
    },
    QueryTerminalSize,
    RenamedSession {
        name: String,
    },
    ConfigFileUpdated,
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
            Self::Normal => write!(f, "Bye from Zellij!"),
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
                let session_tip = match crate::envs::get_session_name() {
                    Ok(name) => format!("`zellij attach {}`", name),
                    Err(_) => "see `zellij ls` and `zellij attach`".to_string(),
                };
                write!(
                    f,
                    "
Your zellij client lost connection to the zellij server.

As a safety measure, you have been disconnected from the current zellij session.
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
    sender: io::BufWriter<Box<dyn IpcStream>>,
    _phantom: PhantomData<T>,
}

impl<T: Serialize> IpcSenderWithContext<T> {
    /// Returns a sender to the given [LocalSocketStream](interprocess::local_socket::LocalSocketStream).
    pub fn new(sender: LocalSocketStream) -> Self {
        Self {
            sender: io::BufWriter::new(Box::new(sender)),
            _phantom: PhantomData,
        }
    }

    fn from_boxed(sender: Box<dyn IpcStream>) -> Self {
        Self {
            sender: io::BufWriter::new(sender),
            _phantom: PhantomData,
        }
    }

    pub fn send_client_msg(&mut self, msg: ClientToServerMsg) -> Result<()> {
        let proto_msg: ProtoClientToServerMsg = msg.try_into()?;
        write_protobuf_message(&mut self.sender, &proto_msg)?;
        self.sender.flush()?;
        Ok(())
    }

    pub fn send_server_msg(&mut self, msg: ServerToClientMsg) -> Result<()> {
        let proto_msg: ProtoServerToClientMsg = msg.into();
        write_protobuf_message(&mut self.sender, &proto_msg)?;
        self.sender.flush()?;
        Ok(())
    }

    /// Returns an [`IpcReceiverWithContext`] with the same socket as this sender.
    pub fn get_receiver<F>(&self) -> IpcReceiverWithContext<F>
    where
        F: for<'de> Deserialize<'de> + Serialize,
    {
        let socket = self.sender.get_ref().try_clone_stream().unwrap();
        IpcReceiverWithContext::from_boxed(socket)
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
    /// The peer closed the socket, or the connection was reset. Terminal.
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
/// The length prefix is the only place a clean hangup can show up: `read_exact`
/// on a socket whose peer has closed returns `UnexpectedEof` with nothing
/// consumed. A reset or a broken pipe is the same situation reached less
/// politely. Anything else — a decode failure, an over-long frame, a truncated
/// body — is a bad message on a stream that is still open.
fn classify_read_error<T>(e: anyhow::Error, msg_type: &str) -> IpcRecv<T> {
    if let Some(io_err) = e.downcast_ref::<io::Error>() {
        if matches!(
            io_err.kind(),
            io::ErrorKind::UnexpectedEof
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::ConnectionAborted
                | io::ErrorKind::BrokenPipe
        ) {
            return IpcRecv::Eof;
        }
        // A receive timeout expiring is not a bad frame: nothing arrived at all,
        // and the stream is still open and still in sync. Linux reports it as
        // `WouldBlock`, other platforms as `TimedOut`.
        if matches!(
            io_err.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ) {
            return IpcRecv::Idle;
        }
    }
    // Previously discarded outright, which is what made a desynchronised stream
    // impossible to tell apart from a hangup after the fact.
    warn!("Error reading {}: {:?}", msg_type, e);
    IpcRecv::Malformed
}

/// Receives messages on a stream socket, along with an [`ErrorContext`].
pub struct IpcReceiverWithContext<T> {
    receiver: io::BufReader<Box<dyn IpcStream>>,
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
            _phantom: PhantomData,
        }
    }

    fn from_boxed(receiver: Box<dyn IpcStream>) -> Self {
        Self {
            receiver: io::BufReader::new(receiver),
            _phantom: PhantomData,
        }
    }

    /// Bound every following read with `timeout`, or lift the bound with `None`.
    ///
    /// See [`IpcStream::set_read_timeout`] for what a read that runs out of
    /// time returns, and for why this must not be set while a frame may be
    /// in flight.
    pub fn set_recv_timeout(&self, timeout: Option<std::time::Duration>) -> io::Result<()> {
        self.receiver.get_ref().set_read_timeout(timeout)
    }

    pub fn recv_client_msg(&mut self) -> IpcRecv<ClientToServerMsg> {
        match read_protobuf_message::<ProtoClientToServerMsg>(&mut self.receiver) {
            Ok(proto_msg) => match proto_msg.try_into() {
                Ok(rust_msg) => IpcRecv::Msg(rust_msg, ErrorContext::default()),
                Err(e) => {
                    warn!("Error converting protobuf to ClientToServerMsg: {:?}", e);
                    IpcRecv::Malformed
                },
            },
            Err(e) => classify_read_error(e, "ClientToServerMsg"),
        }
    }

    pub fn recv_server_msg(&mut self) -> IpcRecv<ServerToClientMsg> {
        match read_protobuf_message::<ProtoServerToClientMsg>(&mut self.receiver) {
            Ok(proto_msg) => match proto_msg.try_into() {
                Ok(rust_msg) => IpcRecv::Msg(rust_msg, ErrorContext::default()),
                Err(e) => {
                    warn!("Error converting protobuf to ServerToClientMsg: {:?}", e);
                    IpcRecv::Malformed
                },
            },
            Err(e) => classify_read_error(e, "ServerToClientMsg"),
        }
    }

    /// Returns an [`IpcSenderWithContext`] with the same socket as this receiver.
    pub fn get_sender<F: Serialize>(&self) -> IpcSenderWithContext<F> {
        let socket = self.receiver.get_ref().try_clone_stream().unwrap();
        IpcSenderWithContext::from_boxed(socket)
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
pub const MAX_IPC_FRAME_LEN: usize = 64 * 1024 * 1024;

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
    reader.read_exact(&mut len_bytes)?;
    let len = u32::from_le_bytes(len_bytes) as usize;
    if len > MAX_IPC_FRAME_LEN {
        return Err(anyhow!(
            "refusing a {len} byte IPC frame, over the {MAX_IPC_FRAME_LEN} byte limit"
        ));
    }

    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;

    T::decode(&buf[..]).map_err(Into::into)
}

fn write_protobuf_message<T: Message>(writer: &mut impl Write, msg: &T) -> Result<()> {
    let encoded = msg.encode_to_vec();
    if encoded.len() > MAX_IPC_FRAME_LEN {
        return Err(anyhow!(
            "refusing to send a {} byte IPC frame, over the {MAX_IPC_FRAME_LEN} byte limit",
            encoded.len()
        ));
    }
    let len = encoded.len() as u32;

    // we measure the length of the message and transmit it first so that the reader will be able
    // to first read exactly 4 bytes (representing this length) and then read that amount of bytes
    // as the actual message - this is so that we are able to distinct whole messages over the wire
    // stream
    writer.write_all(&len.to_le_bytes())?;
    writer.write_all(&encoded)?;
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
    match read_protobuf_message::<ProtoClientToServerMsg>(&mut receiver.receiver) {
        Ok(proto_msg) => match proto_msg.try_into() {
            Ok(rust_msg) => Some((rust_msg, ErrorContext::default())),
            Err(e) => {
                warn!("Error converting protobuf message: {:?}", e);
                None
            },
        },
        Err(_e) => None,
    }
}

pub fn recv_protobuf_server_to_client(
    receiver: &mut IpcReceiverWithContext<ServerToClientMsg>,
) -> Option<(ServerToClientMsg, ErrorContext)> {
    match read_protobuf_message::<ProtoServerToClientMsg>(&mut receiver.receiver) {
        Ok(proto_msg) => match proto_msg.try_into() {
            Ok(rust_msg) => Some((rust_msg, ErrorContext::default())),
            Err(e) => {
                warn!("Error converting protobuf message: {:?}", e);
                None
            },
        },
        Err(_e) => None,
    }
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
    // (zellij-server/src/lib.rs ServerInstruction::KillSession) over this
    // same socket before exiting; if it dies without ACKing, the stream
    // closes. Either outcome -- a successful 4-byte length-prefix read or a
    // read error/EOF -- confirms the kill is no longer in flight.
    let mut len_buf = [0u8; 4];
    let _ = stream.read_exact(&mut len_buf).await;
    Ok(())
}
