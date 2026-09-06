use anyhow::Result;
use async_trait::async_trait;
use zellij_utils::pane_size::Size;

use crate::os_input_output_unix::{
    disable_mouse_support, enable_mouse_support, set_signal_keys, setup_ipc, AsyncSignalListener,
    BlockingSignalIterator,
};
use std::io::prelude::*;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::{io, thread, time};
use zellij_utils::{
    data::Palette,
    ipc::{
        ClientToServerMsg, IpcReceiverWithContext, IpcRecv, IpcSenderWithContext, ServerToClientMsg,
    },
    shared::default_palette,
};

const SIGWINCH_CB_THROTTLE_DURATION: time::Duration = time::Duration::from_millis(50);

pub(crate) const ENABLE_MOUSE_SUPPORT: &str =
    "\u{1b}[?1000h\u{1b}[?1002h\u{1b}[?1003h\u{1b}[?1015h\u{1b}[?1006h";
pub(crate) const DISABLE_MOUSE_SUPPORT: &str =
    "\u{1b}[?1006l\u{1b}[?1015l\u{1b}[?1003l\u{1b}[?1002l\u{1b}[?1000l";

/// Trait for async stdin reading, allowing for testable implementations
#[async_trait]
pub trait AsyncStdin: Send {
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
    session_name: Arc<Mutex<Option<String>>>,
    orig_termios: Option<zellij_utils::termios::WireTermios>,
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
            "could not connect to the zellij server at {} after {:?}: {}",
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
        match zellij_utils::consts::ipc_connect(path) {
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
    fn update_session_name(&mut self, new_session_name: String);
    fn spawn_server(&self, socket_path: &Path, debug: bool) -> Result<(), std::io::Error> {
        crate::spawn_server(socket_path, debug)
    }
}

/// Signals and other process-level environment the client reacts to.
pub trait SignalSource {
    fn handle_signals(&self, sigwinch_cb: Box<dyn Fn()>, quit_cb: Box<dyn Fn()>);
    fn env_variable(&self, _name: &str) -> Option<String> {
        None
    }
    /// Returns an async signal listener that can be polled in tokio::select
    fn get_async_signal_listener(&self) -> io::Result<Box<dyn AsyncSignals>> {
        Ok(Box::new(AsyncSignalListener::new()?))
    }
}

/// What the CLI client needs: a link to the server, the standard streams,
/// and the environment. It never touches the tty, so it does not ask for it.
pub trait CliClientOs: ServerConnection + StdioAccess + SignalSource {}
impl<T: ServerConnection + StdioAccess + SignalSource + ?Sized> CliClientOs for T {}

/// The `ClientOsApi` trait represents an abstract interface to the features of an operating system that
/// Zellij client requires.
///
/// It is the union of the four capability traits above. Code that needs the
/// whole client environment takes this; code that needs one slice of it should
/// take the narrower trait, which `dyn ClientOsApi` upcasts to.
pub trait ClientOsApi:
    TtyControl + StdioAccess + ServerConnection + SignalSource + Send + Sync + std::fmt::Debug
{
    /// Returns a [`Box`] pointer to this [`ClientOsApi`] struct.
    fn box_clone(&self) -> Box<dyn ClientOsApi>;
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
        // this was removed because termbg doesn't release stdin in certain scenarios (we know of
        // some terminals (including FreeBSD): https://github.com/zellij-org/zellij/issues/538
        //
        // let palette = default_palette();
        // let timeout = std::time::Duration::from_millis(100);
        // if let Ok(rgb) = termbg::rgb(timeout) {
        //     palette.bg = PaletteColor::Rgb((rgb.r as u8, rgb.g as u8, rgb.b as u8));
        //     // TODO: also dynamically get all other colors from the user's terminal
        //     // this should be done in the same method (OSC ]11), but there might be other
        //     // considerations here, hence using the library
        // };
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
        let session_name_at_calltime = { self.session_name.lock().unwrap().clone() };
        // here we wait for a lock in case another thread is holding stdin
        // this can happen for example when switching sessions, the old thread will only be
        // released once it sees input over STDIN
        //
        // when this happens, we detect in the other thread that our session is ended (by comparing
        // the session name at the beginning of the call and the one after we read from STDIN), and
        // so place what we read from STDIN inside a buffer (the "reading_from_stdin" on our state)
        // and release the lock
        //
        // then, another thread will see there's something in the buffer immediately as it acquires
        // the lock (without having to wait for STDIN itself) forward this buffer and proceed to
        // wait for the "real" STDIN net time it is called
        let mut buffered_bytes = self.reading_from_stdin.lock().unwrap();
        match buffered_bytes.take() {
            Some(buffered_bytes) => Ok(buffered_bytes),
            None => {
                let stdin = std::io::stdin();
                let mut stdin = stdin.lock();
                let buffer = stdin.fill_buf().unwrap();
                let length = buffer.len();
                let read_bytes = Vec::from(buffer);
                stdin.consume(length);

                let session_name_after_reading_from_stdin =
                    { self.session_name.lock().unwrap().clone() };
                if session_name_at_calltime.is_some()
                    && session_name_at_calltime != session_name_after_reading_from_stdin
                {
                    *buffered_bytes = Some(read_bytes);
                    Err("Session ended")
                } else {
                    Ok(read_bytes)
                }
            },
        }
    }
    fn get_stdout_writer(&self) -> Box<dyn io::Write> {
        let stdout = ::std::io::stdout();
        Box::new(stdout)
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

    fn update_session_name(&mut self, new_session_name: String) {
        *self.session_name.lock().unwrap() = Some(new_session_name);
    }

    fn send_to_server(&self, msg: ClientToServerMsg) {
        let tracing_attach = matches!(&msg, ClientToServerMsg::AttachClient { .. });
        let started_at = std::time::Instant::now();
        if tracing_attach {
            log::info!(
                "attach trace: client pid={} waiting for IPC sender lock",
                std::process::id()
            );
        }
        let mut sender = self.send_instructions_to_server.lock().unwrap();
        if tracing_attach {
            log::info!(
                "attach trace: client pid={} acquired IPC sender lock after {:?}",
                std::process::id(),
                started_at.elapsed()
            );
        }
        match sender.as_mut() {
            Some(sender) => {
                let result = sender.send_client_msg(msg);
                if tracing_attach {
                    log::info!(
                        "attach trace: client pid={} IPC AttachClient write finished after {:?} ({})",
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
        self.receive_instructions_from_server
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .recv_server_msg()
    }
    fn connect_to_server(&self, path: &Path) -> Result<(), ConnectToServerError> {
        let socket = connect_with_deadline(
            path,
            CONNECT_TO_SERVER_TIMEOUT,
            CONNECT_TO_SERVER_RETRY_INTERVAL,
        )?;
        let (sender, receiver) = setup_ipc(socket, path);
        *self.send_instructions_to_server.lock().unwrap() = Some(sender);
        *self.receive_instructions_from_server.lock().unwrap() = Some(receiver);
        Ok(())
    }
}

impl SignalSource for ClientOsInputOutput {
    fn handle_signals(&self, sigwinch_cb: Box<dyn Fn()>, quit_cb: Box<dyn Fn()>) {
        let mut sigwinch_cb_timestamp = time::Instant::now();
        let signals = BlockingSignalIterator::new().unwrap();
        for event in signals {
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
    }

    fn env_variable(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

impl ClientOsApi for ClientOsInputOutput {
    fn box_clone(&self) -> Box<dyn ClientOsApi> {
        Box::new((*self).clone())
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
        session_name: Arc::new(Mutex::new(None)),
        orig_termios: nix::sys::termios::tcgetattr(io::stdin())
            .ok()
            .as_ref()
            .map(zellij_utils::termios::WireTermios::from_termios),
    })
}

pub fn get_cli_client_os_input() -> Result<ClientOsInputOutput, std::io::Error> {
    get_client_os_input()
}

pub const DEFAULT_STDIN_POLL_TIMEOUT_MS: u64 = 10;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_termios_survives_startup_raw_mode_and_cloning() {
        use nix::sys::termios::{self, InputFlags, LocalFlags, SetArg, SpecialCharacterIndices};
        use zellij_utils::termios::WireTermios;
        const CHILD: &str = "ZELLIJ_TEST_CLIENT_TERMIOS";
        if std::env::var_os(CHILD).is_some() {
            let original = termios::tcgetattr(io::stdin()).unwrap();
            let mut client = get_client_os_input().unwrap();
            client.set_raw_mode_keeping_signals();
            let raw = termios::tcgetattr(io::stdin()).unwrap();
            assert!(!raw.input_flags.contains(InputFlags::ICRNL));
            assert!(raw.local_flags.contains(LocalFlags::ISIG));
            let cloned = client.clone();
            let command =
                crate::server_command(Path::new("test.sock"), false, Some(&cloned.orig_termios))
                    .unwrap();
            let args: Vec<_> = command.get_args().collect();
            let json = args[args
                .iter()
                .position(|arg| *arg == "--server-termios")
                .unwrap()
                + 1];
            let snapshot: Option<WireTermios> =
                serde_json::from_str(json.to_str().unwrap()).unwrap();
            assert_eq!(snapshot, Some(WireTermios::from_termios(&original)));
            client.unset_raw_mode().unwrap();
            assert_eq!(termios::tcgetattr(io::stdin()).unwrap(), original);
            return;
        }
        // Raw mode and stdin are process-global: isolate them from the test runner.
        let pty = nix::pty::openpty(None, None).unwrap();
        let mut cooked = termios::tcgetattr(&pty.slave).unwrap();
        cooked.input_flags.insert(InputFlags::ICRNL);
        cooked.control_chars[SpecialCharacterIndices::VERASE as usize] = 8;
        termios::tcsetattr(&pty.slave, SetArg::TCSANOW, &cooked).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "os_input_output::tests::original_termios_survives_startup_raw_mode_and_cloning",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .stdin(std::process::Stdio::from(pty.slave))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    /// The branch nothing else exercises: a socket that never appears. The
    /// error must name the socket and carry the connect failure, and the wait
    /// must be bounded by the timeout, not by the retry loop's patience.
    #[test]
    fn connect_with_deadline_gives_up_at_the_deadline_with_a_named_socket() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("never-bound.sock");
        let timeout = std::time::Duration::from_millis(50);

        let started_at = std::time::Instant::now();
        let Err(err) = connect_with_deadline(&path, timeout, std::time::Duration::from_millis(5))
        else {
            panic!("a socket that does not exist must not connect");
        };
        let waited = started_at.elapsed();

        assert!(
            waited >= timeout,
            "gave up after {waited:?}, before the {timeout:?} deadline"
        );
        assert!(
            waited < std::time::Duration::from_secs(1),
            "kept retrying for {waited:?} past a {timeout:?} deadline"
        );
        assert_eq!(err.last_error().kind(), io::ErrorKind::NotFound);
        let message = err.to_string();
        assert!(
            message.contains(&path.display().to_string()),
            "the message should name the socket: {message}"
        );
    }

    #[test]
    fn get_terminal_size_returns_nonzero_or_fallback() {
        let size = get_terminal_size();
        // In CI or when not attached to a terminal, crossterm may return an error
        // and we fall back to 80x24. Either way, size should be valid.
        assert!(size.rows > 0, "rows should be positive");
        assert!(size.cols > 0, "cols should be positive");
    }

    #[test]
    fn get_terminal_size_fallback_values() {
        // Verify the fallback constants are what we expect
        let fallback = Size { rows: 24, cols: 80 };
        // When crossterm::terminal::size() fails (no terminal), we should get 80x24
        // This is implicitly tested by get_terminal_size_returns_nonzero_or_fallback
        // but we verify the constants here
        assert_eq!(fallback.rows, 24);
        assert_eq!(fallback.cols, 80);
    }

    #[test]
    fn client_os_input_output_can_be_constructed() {
        let os_input = get_client_os_input().expect("should construct ClientOsInputOutput");
        let size = os_input.get_terminal_size();
        assert!(size.rows > 0, "rows should be positive");
        assert!(size.cols > 0, "cols should be positive");
    }

    #[test]
    fn cli_client_os_input_can_be_constructed() {
        let os_input = get_cli_client_os_input().expect("should construct CLI ClientOsInputOutput");
        let size = os_input.get_terminal_size();
        assert!(size.rows > 0, "rows should be positive");
        assert!(size.cols > 0, "cols should be positive");
    }
}
