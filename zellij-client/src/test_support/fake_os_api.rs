use crate::os_input_output::{
    AsyncSignals, AsyncStdin, ClientOsApi, ServerConnection, SignalEvent, SignalSource,
    StdioAccess, TtyControl,
};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use zellij_types::diagnostics::ErrorContext;
use zellij_types::pane_size::Size;
use zellij_types::style::Palette;
use {
    zellij_ipc::ipc::ClientToServerMsg, zellij_ipc::ipc::IpcRecv,
    zellij_ipc::ipc::ServerToClientMsg,
};

/// A `ClientOsApi` that does nothing until you ask it to.
///
/// Terminal operations are no-ops, the terminal is 24x80, and nothing is
/// recorded. Each builder method switches on one capability, so a test only
/// opts into the behaviour it actually asserts on.
#[derive(Clone)]
pub struct FakeClientOsApi {
    terminal_size: Size,
    stdout: Option<Arc<Mutex<Vec<u8>>>>,
    sent_to_server: Arc<Mutex<Vec<ClientToServerMsg>>>,
    from_server: Arc<Mutex<VecDeque<IpcRecv<ServerToClientMsg>>>>,
    stdin_rx: Option<Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<Vec<u8>>>>>,
    signal_rx: Option<Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<SignalEvent>>>>,
}

impl Default for FakeClientOsApi {
    fn default() -> Self {
        Self {
            terminal_size: Size { rows: 24, cols: 80 },
            stdout: None,
            sent_to_server: Arc::new(Mutex::new(Vec::new())),
            from_server: Arc::new(Mutex::new(VecDeque::new())),
            stdin_rx: None,
            signal_rx: None,
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
    pub fn new() -> Self {
        Self::default()
    }

    /// Keep everything written to stdout so a test can assert on the escape
    /// sequences the client emitted.
    pub fn capturing_stdout(mut self) -> Self {
        self.stdout = Some(Arc::new(Mutex::new(Vec::new())));
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
        self.sent_to_server.lock().unwrap().clone()
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
struct FakeStdout(Option<Arc<Mutex<Vec<u8>>>>);

impl Write for FakeStdout {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
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
    fn set_raw_mode(&mut self) {}
    fn unset_raw_mode(&self) -> Result<(), std::io::Error> {
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
        Box::new(FakeStdout(self.stdout.clone()))
    }
    fn get_stdin_reader(&self) -> Box<dyn io::BufRead> {
        Box::new(io::Cursor::new(Vec::new()))
    }
    fn read_from_stdin(&mut self) -> Result<Vec<u8>, &'static str> {
        Ok(Vec::new())
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
    fn send_to_server(&self, msg: ClientToServerMsg) {
        self.sent_to_server.lock().unwrap().push(msg);
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
    fn update_session_name(&mut self, _new_session_name: String) {}
}

impl SignalSource for FakeClientOsApi {
    fn handle_signals(&self, _sigwinch_cb: Box<dyn Fn()>, _quit_cb: Box<dyn Fn()>) {}
    fn get_async_signal_listener(&self) -> io::Result<Box<dyn AsyncSignals>> {
        Ok(match &self.signal_rx {
            Some(rx) => Box::new(FakeAsyncSignals(rx.clone())),
            None => Box::new(NoSignals),
        })
    }
}

impl ClientOsApi for FakeClientOsApi {
    fn box_clone(&self) -> Box<dyn ClientOsApi> {
        Box::new(self.clone())
    }
}
