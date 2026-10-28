use crate::os_input_output::{
    AsyncSignals, AsyncStdin, ClientOsApi, ServerConnection, SignalEvent, SignalListener,
    SignalSource, StdioAccess, TtyControl,
};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use yinz_test_support::Watched;
use yinz_types::diagnostics::ErrorContext;
use yinz_types::pane_size::Size;
use yinz_types::style::Palette;
use {yinz_ipc::ipc::ClientToServerMsg, yinz_ipc::ipc::IpcRecv, yinz_ipc::ipc::ServerToClientMsg};

/// A `ClientOsApi` that does nothing until you ask it to.
///
/// Terminal operations are no-ops, the terminal is 24x80, and nothing is
/// recorded. Each builder method switches on one capability, so a test only
/// opts into the behaviour it actually asserts on.
#[derive(Clone)]
pub struct FakeClientOsApi {
    raw_mode: Arc<AtomicBool>,
    recovery: Arc<Mutex<Option<crate::remote_recovery::RecoveryControl>>>,
    terminal_size: Size,
    stdout: Option<Arc<Mutex<Vec<u8>>>>,
    pub stdout_attempts: Watched<Vec<Vec<u8>>>,
    stdout_error: Option<io::ErrorKind>,
    stdout_error_on: Option<Vec<u8>>,
    stdin: Option<Vec<u8>>,
    pub sent_to_server: Watched<Vec<ClientToServerMsg>>,
    from_server: Arc<Mutex<VecDeque<IpcRecv<ServerToClientMsg>>>>,
    stdin_rx: Option<Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<Vec<u8>>>>>,
    signal_rx: Option<Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<SignalEvent>>>>,
    blocking_signal_rx: Option<yinz_runtime::channels::Receiver<SignalEvent>>,
    signal_setup_error: bool,
}

impl Default for FakeClientOsApi {
    fn default() -> Self {
        Self {
            raw_mode: Arc::new(AtomicBool::new(false)),
            recovery: Arc::new(Mutex::new(None)),
            terminal_size: Size { rows: 24, cols: 80 },
            stdout: None,
            stdout_attempts: Watched::default(),
            stdout_error: None,
            stdout_error_on: None,
            stdin: None,
            sent_to_server: Watched::default(),
            from_server: Arc::new(Mutex::new(VecDeque::new())),
            stdin_rx: None,
            signal_rx: None,
            blocking_signal_rx: None,
            signal_setup_error: false,
        }
    }
}

impl std::fmt::Debug for FakeClientOsApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeClientOsApi").finish()
    }
}

// A test helper offers the whole builder surface whether or not every test uses every part of it;
// a constructor going unused for a while is the toolkit being complete, not code going stale.
#[allow(dead_code)]
impl FakeClientOsApi {
    pub fn raw_mode(&self) -> bool {
        self.raw_mode.load(Ordering::SeqCst)
    }

    pub fn with_terminal_size(mut self, size: Size) -> Self {
        self.terminal_size = size;
        self
    }

    pub fn new() -> Self {
        Self::default()
    }

    /// Keep everything written to stdout so a test can assert on the escape
    /// sequences the client emitted.
    pub fn capturing_stdout(mut self) -> Self {
        self.stdout = Some(Arc::new(Mutex::new(Vec::new())));
        self
    }

    pub fn with_stdin(mut self, bytes: Vec<u8>) -> Self {
        self.stdin = Some(bytes);
        self
    }

    pub fn with_stdout_error(mut self, error: io::ErrorKind) -> Self {
        self.stdout_error = Some(error);
        self
    }

    /// Fail only the selected write, allowing startup and restoration to complete.
    pub fn with_stdout_error_on(mut self, error: io::ErrorKind, bytes: Vec<u8>) -> Self {
        self.stdout_error = Some(error);
        self.stdout_error_on = Some(bytes);
        self
    }

    /// Feed the async stdin reader from `rx`, for tests that drive the
    /// remote-client loop.
    pub fn with_async_stdin(mut self, rx: mpsc::UnboundedReceiver<Vec<u8>>) -> Self {
        self.stdin_rx = Some(Arc::new(tokio::sync::Mutex::new(rx)));
        self
    }

    /// Deliver signals from `rx` to the async signal listener.
    pub fn with_async_signals(mut self, rx: mpsc::UnboundedReceiver<SignalEvent>) -> Self {
        self.signal_rx = Some(Arc::new(tokio::sync::Mutex::new(rx)));
        self
    }

    pub fn with_signals(mut self, rx: yinz_runtime::channels::Receiver<SignalEvent>) -> Self {
        self.blocking_signal_rx = Some(rx);
        self
    }

    pub fn with_signal_setup_error(mut self) -> Self {
        self.signal_setup_error = true;
        self
    }

    /// Everything the client has written to stdout so far. Empty unless
    /// `capturing_stdout` was used.
    pub fn stdout_bytes(&self) -> Vec<u8> {
        self.stdout
            .as_ref()
            .map(|buf| buf.lock().unwrap().clone())
            .unwrap_or_default()
    }

    /// Every message the client sent to the server, in order.
    pub fn sent_messages(&self) -> Vec<ClientToServerMsg> {
        self.sent_to_server.get()
    }

    /// Queue a message for `recv_from_server` to hand back.
    pub fn queue_from_server(&self, msg: ServerToClientMsg) {
        self.from_server
            .lock()
            .unwrap()
            .push_back(IpcRecv::Msg(msg, ErrorContext::default()));
    }

    /// Queue a hangup, as if the server process had gone away.
    pub fn queue_server_hangup(&self) {
        self.from_server.lock().unwrap().push_back(IpcRecv::Eof);
    }

    /// Queue an unreadable frame, as if the stream had desynchronised.
    pub fn queue_malformed_from_server(&self) {
        self.from_server
            .lock()
            .unwrap()
            .push_back(IpcRecv::Malformed);
    }
}

/// Writes into the fake's stdout buffer, or discards when not capturing.
struct FakeStdout(
    Option<Arc<Mutex<Vec<u8>>>>,
    Option<io::ErrorKind>,
    Option<Vec<u8>>,
    Watched<Vec<Vec<u8>>>,
);

impl Write for FakeStdout {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.0.is_some() {
            self.3.lock().push(buf.to_vec());
        }
        if let Some(error) = self.1
            && self.2.as_deref().is_none_or(|bytes| bytes == buf)
        {
            return Err(error.into());
        }
        if let Some(sink) = &self.0 {
            sink.lock().unwrap().extend_from_slice(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct FakeAsyncStdin(Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<Vec<u8>>>>);

#[async_trait]
impl AsyncStdin for FakeAsyncStdin {
    async fn read(&mut self) -> io::Result<Vec<u8>> {
        // A closed channel reads as EOF, which is how the loop is asked to
        // finish.
        Ok(self.0.lock().await.recv().await.unwrap_or_default())
    }
}

struct FakeAsyncSignals(Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<SignalEvent>>>);

#[async_trait]
impl AsyncSignals for FakeAsyncSignals {
    async fn recv(&mut self) -> Option<SignalEvent> {
        self.0.lock().await.recv().await
    }
}

/// Never-ready signal source, for fakes that were not given a signal channel.
struct NoSignals;

#[async_trait]
impl AsyncSignals for NoSignals {
    async fn recv(&mut self) -> Option<SignalEvent> {
        None
    }
}

impl TtyControl for FakeClientOsApi {
    fn get_terminal_size(&self) -> Size {
        self.terminal_size
    }
    fn set_raw_mode(&mut self) {
        self.raw_mode.store(true, Ordering::SeqCst);
    }
    fn unset_raw_mode(&self) -> Result<(), std::io::Error> {
        self.raw_mode.store(false, Ordering::SeqCst);
        Ok(())
    }
    fn load_palette(&self) -> Palette {
        Palette::default()
    }
    fn enable_mouse(&self) -> anyhow::Result<()> {
        Ok(())
    }
    fn disable_mouse(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

impl StdioAccess for FakeClientOsApi {
    fn get_stdout_writer(&self) -> Box<dyn Write> {
        Box::new(crate::terminal_escapes::HostWriter::with_stall_policy(
            FakeStdout(
                self.stdout.clone(),
                self.stdout_error,
                self.stdout_error_on.clone(),
                self.stdout_attempts.clone(),
            ),
            |_| Ok(()),
            crate::terminal_escapes::stalled_output_abandoned,
            std::time::Duration::ZERO,
        ))
    }
    fn get_stdin_reader(&self) -> Box<dyn io::BufRead> {
        Box::new(io::Cursor::new(self.stdin.clone().unwrap_or_default()))
    }
    fn stdin_is_terminal(&self) -> bool {
        self.stdin.is_none()
    }
    fn read_from_stdin(&mut self) -> Result<Vec<u8>, &'static str> {
        Err("Session ended")
    }
    fn get_async_stdin_reader(&self) -> Box<dyn AsyncStdin> {
        match &self.stdin_rx {
            Some(rx) => Box::new(FakeAsyncStdin(rx.clone())),
            None => Box::new(FakeAsyncStdin(Arc::new(tokio::sync::Mutex::new(
                mpsc::unbounded_channel().1,
            )))),
        }
    }
}

impl ServerConnection for FakeClientOsApi {
    fn set_remote_recovery(
        &self,
        control: Option<crate::remote_recovery::RecoveryControl>,
    ) -> io::Result<()> {
        *self.recovery.lock().unwrap() = control;
        Ok(())
    }
    fn remote_recovery(&self) -> Option<crate::remote_recovery::RecoveryControl> {
        self.recovery.lock().unwrap().clone()
    }

    fn spawn_server(
        &self,
        _socket_path: &Path,
        _debug: bool,
        _session_name: &yinz_types::internal_ids::SessionName,
        _env: &yinz_types::envs::EnvironmentVariables,
    ) -> io::Result<()> {
        Ok(())
    }

    fn send_to_server(&self, msg: ClientToServerMsg) {
        if let Some(control) = self.remote_recovery() {
            control.cache(&msg);
            if !control.can_write() {
                return;
            }
        }
        self.sent_to_server.lock().push(msg);
    }
    fn recv_from_server(&self) -> IpcRecv<ServerToClientMsg> {
        match self.from_server.lock().unwrap().pop_front() {
            Some(received) => received,
            None => {
                // The real implementation blocks on a socket. Callers poll this
                // in a loop, so yield briefly rather than spinning a core and
                // starving the threads under test. `Idle` rather than a hangup:
                // an empty queue means "not yet", not "gone".
                std::thread::sleep(std::time::Duration::from_millis(1));
                IpcRecv::Idle
            },
        }
    }
    fn connect_to_server(
        &self,
        _path: &Path,
    ) -> Result<(), crate::os_input_output::ConnectToServerError> {
        Ok(())
    }
    fn update_session_name(&mut self, _new_session_name: yinz_types::internal_ids::SessionName) {}
}

impl SignalSource for FakeClientOsApi {
    fn start_signal_listener(
        &self,
        sigwinch_cb: Box<dyn Fn() + Send>,
        quit_cb: Box<dyn Fn() + Send>,
    ) -> anyhow::Result<SignalListener> {
        if self.signal_setup_error {
            anyhow::bail!("signal registration failed");
        }
        let signals = self.blocking_signal_rx.clone();
        let (stop, cancelled) = yinz_runtime::channels::bounded::<()>(0);
        SignalListener::spawn(
            move || drop(stop),
            move || {
                let Some(signals) = signals else {
                    let _ = cancelled.recv();
                    return;
                };
                let mut select = yinz_runtime::channels::Select::new_biased();
                select.recv(&cancelled);
                select.recv(&signals);
                loop {
                    let operation = select.select();
                    if operation.index() == 0 {
                        let _ = operation.recv(&cancelled);
                        break;
                    }
                    match operation.recv(&signals) {
                        Ok(SignalEvent::Resize) => sigwinch_cb(),
                        Ok(SignalEvent::Quit) => {
                            quit_cb();
                            break;
                        },
                        Err(_) => break,
                    }
                }
            },
        )
    }
    fn get_async_signal_listener(&self) -> io::Result<Box<dyn AsyncSignals>> {
        Ok(match &self.signal_rx {
            Some(rx) => Box::new(FakeAsyncSignals(rx.clone())),
            None => Box::new(NoSignals),
        })
    }
}

impl ClientOsApi for FakeClientOsApi {
    fn original_termios(&self) -> Option<yinz_runtime::termios::WireTermios> {
        None
    }
    fn should_install_panic_hook(&self) -> bool {
        false
    }
    fn box_clone(&self) -> Box<dyn ClientOsApi> {
        Box::new(self.clone())
    }
}
