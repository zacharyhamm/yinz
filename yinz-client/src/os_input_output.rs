use async_trait::async_trait;
use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::SessionName;
use yinz_types::pane_size::Size;
use yinz_types::style::default_palette;

use crate::os_input_output_unix::{
    disable_mouse_support, enable_mouse_support, set_signal_keys, setup_ipc, AsyncSignalListener,
    BlockingSignalIterator,
};
use std::io::prelude::*;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::{io, thread, time};
use {
    yinz_ipc::ipc::ClientToServerMsg, yinz_ipc::ipc::IpcReceiverWithContext,
    yinz_ipc::ipc::IpcRecv, yinz_ipc::ipc::IpcSenderWithContext, yinz_ipc::ipc::ServerToClientMsg,
    yinz_types::data::Palette,
};

const SIGWINCH_CB_THROTTLE_DURATION: time::Duration = time::Duration::from_millis(50);

pub(crate) const ENABLE_MOUSE_SUPPORT: &str =
    "\u{1b}[?1000h\u{1b}[?1002h\u{1b}[?1003h\u{1b}[?1015h\u{1b}[?1006h";
pub(crate) const DISABLE_MOUSE_SUPPORT: &str =
    "\u{1b}[?1006l\u{1b}[?1015l\u{1b}[?1003l\u{1b}[?1002l\u{1b}[?1000l";

/// Trait for async stdin reading, allowing for testable implementations
#[async_trait]
pub trait AsyncStdin: Send {
    #[expect(
        clippy::double_must_use,
        reason = "async_trait adds must_use to an already must-use future"
    )]
    async fn read(&mut self) -> io::Result<Vec<u8>>;
}

pub struct AsyncStdinReader {
    stdin: tokio::io::Stdin,
    buffer: Vec<u8>,
}

impl AsyncStdinReader {
    pub fn new() -> Self {
        Self {
            stdin: tokio::io::stdin(),
            buffer: vec![0u8; 10 * 1024],
        }
    }
}

impl Default for AsyncStdinReader {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AsyncStdin for AsyncStdinReader {
    async fn read(&mut self) -> io::Result<Vec<u8>> {
        use tokio::io::AsyncReadExt;
        let n = self.stdin.read(&mut self.buffer).await?;
        Ok(self.buffer[..n].to_vec())
    }
}

pub enum SignalEvent {
    Resize,
    Quit,
}

/// Trait for async signal listening, allowing for testable implementations
#[async_trait]
pub trait AsyncSignals: Send {
    #[expect(
        clippy::double_must_use,
        reason = "async_trait adds must_use to an already must-use future"
    )]
    async fn recv(&mut self) -> Option<SignalEvent>;
}

pub(crate) fn get_terminal_size() -> Size {
    match crossterm::terminal::size() {
        Ok((cols, rows)) => {
            // fallback to default values when rows/cols == 0: https://github.com/zellij-org/zellij/issues/1551
            let rows = if rows != 0 { rows as usize } else { 24 };
            let cols = if cols != 0 { cols as usize } else { 80 };
            Size { rows, cols }
        },
        Err(_) => Size { rows: 24, cols: 80 },
    }
}

#[derive(Clone)]
pub struct ClientOsInputOutput {
    send_instructions_to_server: Arc<Mutex<Option<IpcSenderWithContext<ClientToServerMsg>>>>,
    receive_instructions_from_server: Arc<Mutex<Option<IpcReceiverWithContext<ServerToClientMsg>>>>,
    reading_from_stdin: Arc<Mutex<Option<Vec<u8>>>>,
    connection_generation: Arc<std::sync::atomic::AtomicUsize>,
    generation: usize,
    orig_termios: Option<yinz_runtime::termios::WireTermios>,
}

impl ClientOsInputOutput {
    fn is_current_connection(&self) -> bool {
        self.generation
            == self
                .connection_generation
                .load(std::sync::atomic::Ordering::Acquire)
    }
}

impl std::fmt::Debug for ClientOsInputOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientOsInputOutput").finish()
    }
}

/// Control over the controlling terminal's state.
pub trait TtyControl {
    /// Returns the size of the terminal.
    fn get_terminal_size(&self) -> Size;
    /// Set the terminal to
    /// [raw mode](https://en.wikipedia.org/wiki/Terminal_mode).
    fn set_raw_mode(&mut self);
    /// Raw mode, except that the signal keys (Ctrl-C and friends) keep working.
    ///
    /// Used for the startup window, where the terminal must stop echoing before we query the host
    /// but nothing is reading stdin yet, so Ctrl-C has to reach us as a signal or not at all. The
    /// default is plain raw mode, for the fake terminals under test that have no signals to keep.
    fn set_raw_mode_keeping_signals(&mut self) {
        self.set_raw_mode();
    }
    /// Set the terminal to
    /// [cooked mode](https://en.wikipedia.org/wiki/Terminal_mode).
    fn unset_raw_mode(&self) -> Result<(), std::io::Error>;
    fn load_palette(&self) -> Palette;
    fn enable_mouse(&self) -> Result<()>;
    fn disable_mouse(&self) -> Result<()>;
}

/// Access to the process's standard streams.
pub trait StdioAccess {
    /// Returns the writer that allows writing to standard output.
    fn get_stdout_writer(&self) -> Box<dyn io::Write>;
    /// Returns a writer that bounds stalls during terminal restoration.
    fn get_stdout_writer_for_teardown(&self) -> Box<dyn io::Write> {
        self.get_stdout_writer()
    }
    /// Returns a BufReader that allows to read from STDIN line by line, also locks STDIN
    fn get_stdin_reader(&self) -> Box<dyn io::BufRead>;
    /// Returns the raw contents of standard input.
    fn read_from_stdin(&mut self) -> Result<Vec<u8>, &'static str>;
    fn stdin_is_terminal(&self) -> bool {
        true
    }
    fn stdout_is_terminal(&self) -> bool {
        true
    }
    /// Returns an async stdin reader that can be polled in tokio::select
    fn get_async_stdin_reader(&self) -> Box<dyn AsyncStdin> {
        Box::new(AsyncStdinReader::new())
    }
}

/// How long a client waits for the server's socket to appear before giving up.
///
/// The wait is normally a few milliseconds — the server has usually just been
/// spawned by this same client and only has to bind. The margin is for a loaded
/// machine starting a debug build, not for a server that is in trouble: one that
/// has not bound in this long is not going to.
const CONNECT_TO_SERVER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const CONNECT_TO_SERVER_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// The server socket never became connectable.
#[derive(Debug)]
pub struct ConnectToServerError {
    path: PathBuf,
    waited: std::time::Duration,
    last_error: io::Error,
}

impl ConnectToServerError {
    /// The error the final connect attempt failed with.
    pub fn last_error(&self) -> &io::Error {
        &self.last_error
    }
}

impl std::fmt::Display for ConnectToServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "could not connect to the yinz server at {} after {:?}: {}",
            self.path.display(),
            self.waited,
            self.last_error,
        )
    }
}

impl std::error::Error for ConnectToServerError {}

/// Connect to the server socket at `path`, retrying every `retry_interval`
/// until `timeout` has passed.
///
/// The one retry policy for every client, real or test double: the server is
/// usually spawned by the caller a moment earlier and only has to bind, so a
/// connect that keeps failing past the deadline means a server that is not
/// coming, and the caller gets an error naming the socket rather than a hang.
pub fn connect_with_deadline(
    path: &Path,
    timeout: std::time::Duration,
    retry_interval: std::time::Duration,
) -> Result<interprocess::local_socket::Stream, ConnectToServerError> {
    let started_at = std::time::Instant::now();
    let mut attempts = 0;
    log::info!(
        "attach trace: client pid={} IPC connect loop begin path={}",
        std::process::id(),
        path.display()
    );
    let deadline = std::time::Instant::now() + timeout;
    loop {
        attempts += 1;
        match yinz_ipc::consts::ipc_connect(path) {
            Ok(socket) => {
                log::info!(
                    "attach trace: client pid={} IPC connect succeeded after {:?} attempts={attempts}",
                    std::process::id(),
                    started_at.elapsed()
                );
                return Ok(socket);
            },
            Err(last_error) => {
                if std::time::Instant::now() >= deadline {
                    log::warn!(
                        "attach trace: client pid={} IPC connect timed out after {:?} attempts={attempts}: {last_error}",
                        std::process::id(),
                        started_at.elapsed()
                    );
                    return Err(ConnectToServerError {
                        path: path.to_path_buf(),
                        waited: timeout,
                        last_error,
                    });
                }
                std::thread::sleep(retry_interval);
            },
        }
    }
}

/// The IPC link to the session's server process.
pub trait ServerConnection {
    /// Sends a message to the server.
    fn send_to_server(&self, msg: ClientToServerMsg);
    /// Receives a message on client-side IPC channel
    // This should be called from the client-side router thread only.
    ///
    /// Returns `IpcRecv` rather than an `Option` so that "the server hung up"
    /// is distinguishable from "that frame was garbage": the first is terminal
    /// and must not be retried, the second may be.
    fn recv_from_server(&self) -> IpcRecv<ServerToClientMsg>;
    /// Establish a connection with the server socket.
    ///
    /// Fails rather than waiting forever: the server this is waiting on may
    /// never arrive (it failed to start, or it is wedged and never binds), and
    /// a client that spins on that gives the user a hang with nothing on screen
    /// to explain it.
    fn connect_to_server(&self, path: &Path) -> Result<(), ConnectToServerError>;
    fn install_session_connection(
        &self,
        _sender: IpcSenderWithContext<ClientToServerMsg>,
        _receiver: IpcReceiverWithContext<ServerToClientMsg>,
    ) -> io::Result<()> {
        Err(io::Error::other(
            "This client cannot switch destination connections",
        ))
    }
    fn update_session_name(&mut self, new_session_name: SessionName);
    fn spawn_server(&self, socket_path: &Path, debug: bool) -> Result<(), std::io::Error> {
        crate::spawn_server(socket_path, debug)
    }
}

/// Signals and other process-level environment the client reacts to.
pub trait SignalSource {
    /// Registers signals before returning; stopping or dropping the guard joins the listener.
    fn start_signal_listener(
        &self,
        sigwinch_cb: Box<dyn Fn() + Send>,
        quit_cb: Box<dyn Fn() + Send>,
    ) -> Result<SignalListener>;
    fn env_variable(&self, _name: &str) -> Option<String> {
        None
    }
    /// Returns an async signal listener that can be polled in tokio::select
    fn get_async_signal_listener(&self) -> io::Result<Box<dyn AsyncSignals>> {
        Ok(Box::new(AsyncSignalListener::new()?))
    }
}

/// Owns a signal worker and the operation that wakes it for shutdown.
pub struct SignalListener {
    stop: Option<Box<dyn FnOnce() + Send>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl SignalListener {
    /// `stop` must wake an idle `run` so it can return without another signal.
    pub fn spawn(
        stop: impl FnOnce() + Send + 'static,
        run: impl FnOnce() + Send + 'static,
    ) -> Result<Self> {
        match thread::Builder::new()
            .name("signal_listener".to_string())
            .spawn(run)
        {
            Ok(thread) => Ok(Self {
                stop: Some(Box::new(stop)),
                thread: Some(thread),
            }),
            Err(error) => {
                stop();
                Err(error).context("failed to spawn the signal listener")
            },
        }
    }

    /// Cancel the listener and wait until its callbacks have been released.
    pub fn stop(mut self) -> Result<()> {
        self.stop_and_join()
    }

    fn stop_and_join(&mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            stop();
        }
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| anyhow!("signal listener panicked"))?;
        }
        Ok(())
    }
}

impl Drop for SignalListener {
    fn drop(&mut self) {
        self.stop_and_join().non_fatal();
    }
}

/// What the CLI client needs: a link to the server, the standard streams,
/// and the environment. It never touches the tty, so it does not ask for it.
pub trait CliClientOs: ServerConnection + StdioAccess + SignalSource {}
impl<T: ServerConnection + StdioAccess + SignalSource + ?Sized> CliClientOs for T {}

/// The `ClientOsApi` trait represents an abstract interface to the features of an operating system that
/// Yinz client requires.
///
/// It is the union of the four capability traits above. Code that needs the
/// whole client environment takes this; code that needs one slice of it should
/// take the narrower trait, which `dyn ClientOsApi` upcasts to.
pub trait ClientOsApi:
    TtyControl + StdioAccess + ServerConnection + SignalSource + Send + Sync + std::fmt::Debug
{
    /// Returns a [`Box`] pointer to this [`ClientOsApi`] struct.
    fn box_clone(&self) -> Box<dyn ClientOsApi>;
    /// Terminal settings captured before the client entered raw mode.
    fn original_termios(&self) -> Option<yinz_runtime::termios::WireTermios>;
    fn should_install_panic_hook(&self) -> bool {
        true
    }
}

impl TtyControl for ClientOsInputOutput {
    fn get_terminal_size(&self) -> Size {
        get_terminal_size()
    }
    fn set_raw_mode(&mut self) {
        crossterm::terminal::enable_raw_mode().expect("could not enable raw mode");
        // Explicitly, rather than leaning on `enable_raw_mode`: it early-returns once it has
        // stashed the terminal's original attributes, so the call above is a no-op when we are
        // being promoted from `set_raw_mode_keeping_signals` - which is exactly the case that
        // needs ISIG cleared.
        set_signal_keys(false);
    }
    fn set_raw_mode_keeping_signals(&mut self) {
        crossterm::terminal::enable_raw_mode().expect("could not enable raw mode");
        set_signal_keys(true);
    }
    fn unset_raw_mode(&self) -> Result<(), std::io::Error> {
        crossterm::terminal::disable_raw_mode()
    }
    fn load_palette(&self) -> Palette {
        // The terminal's own colours are not probed here: querying them over stdin
        // deadlocked some terminals (zellij-org/zellij#538). Light/dark detection now
        // happens through `theme_dark`/`theme_light` (see `yinz-cli/src/options.rs`).
        default_palette()
    }
    fn enable_mouse(&self) -> Result<()> {
        let mut stdout = self.get_stdout_writer();
        enable_mouse_support(&mut *stdout)
    }

    fn disable_mouse(&self) -> Result<()> {
        let mut stdout = self.get_stdout_writer();
        disable_mouse_support(&mut *stdout)
    }
}

impl StdioAccess for ClientOsInputOutput {
    fn read_from_stdin(&mut self) -> Result<Vec<u8>, &'static str> {
        // Readers from an old connection leave bytes for the new reader, even for equal names.
        let mut buffered_bytes = self.reading_from_stdin.lock().unwrap();
        if !self.is_current_connection() {
            return Err("Session ended");
        }
        match buffered_bytes.take() {
            Some(buffered_bytes) => Ok(buffered_bytes),
            None => {
                let stdin = std::io::stdin();
                let mut stdin = stdin.lock();
                let buffer = stdin.fill_buf().unwrap();
                let length = buffer.len();
                let read_bytes = Vec::from(buffer);
                stdin.consume(length);

                if !self.is_current_connection() {
                    *buffered_bytes = Some(read_bytes);
                    Err("Session ended")
                } else {
                    Ok(read_bytes)
                }
            },
        }
    }
    fn get_stdout_writer(&self) -> Box<dyn io::Write> {
        stdout_writer(false)
    }

    fn get_stdout_writer_for_teardown(&self) -> Box<dyn io::Write> {
        stdout_writer(true)
    }

    fn get_stdin_reader(&self) -> Box<dyn io::BufRead> {
        let stdin = ::std::io::stdin();
        Box::new(stdin.lock())
    }

    fn stdin_is_terminal(&self) -> bool {
        let stdin = ::std::io::stdin();
        stdin.is_terminal()
    }

    fn stdout_is_terminal(&self) -> bool {
        let stdout = ::std::io::stdout();
        stdout.is_terminal()
    }
}

impl ServerConnection for ClientOsInputOutput {
    fn spawn_server(&self, socket_path: &Path, debug: bool) -> io::Result<()> {
        crate::spawn_server_with_termios(socket_path, debug, Some(&self.orig_termios))
    }

    fn update_session_name(&mut self, _new_session_name: SessionName) {
        self.generation = self
            .connection_generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            + 1;
    }

    fn send_to_server(&self, msg: ClientToServerMsg) {
        if !self.is_current_connection() {
            return;
        }
        let startup_message = match &msg {
            ClientToServerMsg::AttachClient { .. } => Some("AttachClient"),
            ClientToServerMsg::FirstClientConnected { .. } => Some("FirstClientConnected"),
            _ => None,
        };
        let started_at = std::time::Instant::now();
        if let Some(kind) = startup_message {
            log::info!(
                "startup trace: client pid={} message={kind} waiting for IPC sender lock",
                std::process::id()
            );
        }
        let mut sender = self.send_instructions_to_server.lock().unwrap();
        if let Some(kind) = startup_message {
            log::info!(
                "startup trace: client pid={} message={kind} acquired IPC sender lock after {:?}",
                std::process::id(),
                started_at.elapsed()
            );
        }
        match sender.as_mut() {
            Some(sender) => {
                let result = sender.send_client_msg(msg);
                if let Some(kind) = startup_message {
                    log::info!(
                        "startup trace: client pid={} IPC {kind} write finished after {:?} ({})",
                        std::process::id(),
                        started_at.elapsed(),
                        if result.is_ok() { "ok" } else { "error" }
                    );
                }
                if let Err(error) = result {
                    log::warn!("failed to send message to server: {error:#}");
                }
            },
            None => {
                log::warn!("Server not ready, dropping message.");
            },
        }
    }
    fn recv_from_server(&self) -> IpcRecv<ServerToClientMsg> {
        let Ok(mut receiver) = self.receive_instructions_from_server.lock() else {
            log::error!("client receiver lock poisoned");
            return IpcRecv::Eof;
        };
        match receiver.as_mut() {
            Some(receiver) => receiver.recv_server_msg(),
            None => IpcRecv::Eof,
        }
    }
    fn install_session_connection(
        &self,
        sender: IpcSenderWithContext<ClientToServerMsg>,
        receiver: IpcReceiverWithContext<ServerToClientMsg>,
    ) -> io::Result<()> {
        *self
            .send_instructions_to_server
            .lock()
            .map_err(|_| io::Error::other("client sender lock poisoned"))? = Some(sender);
        *self
            .receive_instructions_from_server
            .lock()
            .map_err(|_| io::Error::other("client receiver lock poisoned"))? = Some(receiver);
        Ok(())
    }
    fn connect_to_server(&self, path: &Path) -> Result<(), ConnectToServerError> {
        let socket = connect_with_deadline(
            path,
            CONNECT_TO_SERVER_TIMEOUT,
            CONNECT_TO_SERVER_RETRY_INTERVAL,
        )?;
        let (sender, receiver) =
            setup_ipc(socket, path).map_err(|last_error| ConnectToServerError {
                path: path.to_owned(),
                waited: std::time::Duration::ZERO,
                last_error,
            })?;
        *self.send_instructions_to_server.lock().unwrap() = Some(sender);
        *self.receive_instructions_from_server.lock().unwrap() = Some(receiver);
        Ok(())
    }
}

impl SignalSource for ClientOsInputOutput {
    fn start_signal_listener(
        &self,
        sigwinch_cb: Box<dyn Fn() + Send>,
        quit_cb: Box<dyn Fn() + Send>,
    ) -> Result<SignalListener> {
        let signals = BlockingSignalIterator::new().context("failed to register client signals")?;
        let handle = signals.handle();
        let connection = self.clone();
        SignalListener::spawn(
            move || handle.close(),
            move || {
                let mut sigwinch_cb_timestamp = time::Instant::now();
                for event in signals {
                    if !connection.is_current_connection() {
                        break;
                    }
                    match event {
                        SignalEvent::Resize => {
                            // throttle sigwinch_cb calls, reduce excessive renders while resizing
                            if sigwinch_cb_timestamp.elapsed() < SIGWINCH_CB_THROTTLE_DURATION {
                                thread::sleep(SIGWINCH_CB_THROTTLE_DURATION);
                            }
                            sigwinch_cb_timestamp = time::Instant::now();
                            sigwinch_cb();
                        },
                        SignalEvent::Quit => {
                            quit_cb();
                            break;
                        },
                    }
                }
            },
        )
    }

    fn env_variable(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

impl ClientOsApi for ClientOsInputOutput {
    fn box_clone(&self) -> Box<dyn ClientOsApi> {
        Box::new((*self).clone())
    }
    fn original_termios(&self) -> Option<yinz_runtime::termios::WireTermios> {
        self.orig_termios.clone()
    }
}

impl Clone for Box<dyn ClientOsApi> {
    fn clone(&self) -> Box<dyn ClientOsApi> {
        self.box_clone()
    }
}

pub fn get_client_os_input() -> Result<ClientOsInputOutput, std::io::Error> {
    let reading_from_stdin = Arc::new(Mutex::new(None));
    Ok(ClientOsInputOutput {
        send_instructions_to_server: Arc::new(Mutex::new(None)),
        receive_instructions_from_server: Arc::new(Mutex::new(None)),
        reading_from_stdin,
        connection_generation: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        generation: 0,
        orig_termios: nix::sys::termios::tcgetattr(io::stdin())
            .ok()
            .as_ref()
            .map(yinz_runtime::termios::WireTermios::from_termios),
    })
}

pub fn get_cli_client_os_input() -> Result<ClientOsInputOutput, std::io::Error> {
    get_client_os_input()
}

pub const DEFAULT_STDIN_POLL_TIMEOUT_MS: u64 = 10;

/// A serialized stdout writer that waits out transient terminal backpressure.
pub(crate) fn stdout_writer(for_teardown: bool) -> Box<dyn io::Write> {
    // Hold the lock for the writer's whole life, including its waits on a full
    // terminal, so another thread cannot write into the middle of a frame.
    let stdout = ::std::io::stdout().lock();
    // Reopen the tty instead of changing stdout's shared file description: stdin
    // and the invoking shell can share it, and must retain their blocking mode.
    let tty = STDOUT_TTY
        .get_or_init(|| {
            if !stdout.is_terminal() {
                return None;
            }
            reopen_stdout_tty(&stdout)
                .context("writing frames through the shared, possibly blocking, stdout")
                .to_log()
                .ok()
        })
        .as_ref();
    let writer = StdoutWriter { stdout, tty };
    let wait = |writer: &StdoutWriter| match &writer.tty {
        Some(tty) => wait_until_stdout_is_writable(tty),
        _ => wait_until_stdout_is_writable(&writer.stdout),
    };
    if for_teardown {
        Box::new(crate::terminal_escapes::HostWriter::with_stall_policy(
            writer,
            wait,
            || true,
            crate::terminal_escapes::STALLED_OUTPUT_LIMIT,
        ))
    } else {
        Box::new(crate::terminal_escapes::HostWriter::new(writer, wait))
    }
}

/// A private, non-blocking file description for the terminal behind stdout.
fn reopen_stdout_tty(stdout: &io::StdoutLock<'_>) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = nix::unistd::ttyname(stdout).context("failed to name stdout's terminal")?;
    std::fs::OpenOptions::new()
        .write(true)
        .custom_flags((nix::fcntl::OFlag::O_NONBLOCK | nix::fcntl::OFlag::O_NOCTTY).bits())
        .open(&path)
        .with_context(|| {
            format!(
                "failed to reopen {} for non-blocking writes",
                path.display()
            )
        })
}

// Cache both success and failure across startup, frames, and teardown.
static STDOUT_TTY: OnceLock<Option<std::fs::File>> = OnceLock::new();

struct StdoutWriter {
    stdout: io::StdoutLock<'static>,
    tty: Option<&'static std::fs::File>,
}

impl StdoutWriter {
    fn writer(&mut self) -> io::Result<&mut dyn Write> {
        match &mut self.tty {
            Some(tty) => {
                // Standard stdout may hold newline-free text written before this frame.
                self.stdout.flush()?;
                Ok(tty)
            },
            None => Ok(&mut self.stdout),
        }
    }
}

impl Write for StdoutWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.writer()?.write(bytes)
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        self.writer()?.write_vectored(bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stdout.flush()
    }
}

/// Pause while the terminal is full, then let HostWriter retry. A timeout is temporary
/// backpressure, not an error.
fn wait_until_stdout_is_writable(stdout: &impl std::os::fd::AsFd) -> io::Result<()> {
    wait_until_writable(stdout, time::Duration::from_millis(100)).map(|_| ())
}

/// Wait until `fd` can take output, returning false if `timeout` passes.
///
/// `select`, not `poll`: macOS `poll` does not support ttys and answers `POLLNVAL`, which
/// would turn ordinary backpressure into a failed write. tmux waits on ttys the same way.
/// A closed terminal still reports ready, and the retried write then returns its error.
fn wait_until_writable(fd: &impl std::os::fd::AsFd, timeout: time::Duration) -> io::Result<bool> {
    use nix::sys::select::{select, FdSet};
    use nix::sys::time::{TimeVal, TimeValLike};
    let mut writable = FdSet::new();
    writable.insert(fd.as_fd());
    let micros = i64::try_from(timeout.as_micros()).unwrap_or(i64::MAX);
    let mut timeout = TimeVal::microseconds(micros);
    select(None, None, Some(&mut writable), None, Some(&mut timeout))
        .map(|ready| ready > 0)
        .map_err(io::Error::from)
}

#[cfg(test)]
#[path = "unit/os_input_output_tests.rs"]
mod tests;
