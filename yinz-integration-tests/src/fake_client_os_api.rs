use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use yinz_client::os_input_output::{
    connect_with_deadline, ClientOsApi, ConnectToServerError, ServerConnection, SignalEvent,
    SignalSource, StdioAccess, TtyControl,
};
use yinz_types::input::actions::Action;
use yinz_types::pane_size::Size;
use yinz_types::style::default_palette;
use yinz_types::style::Palette;
use {
    yinz_ipc::ipc::ClientToServerMsg, yinz_ipc::ipc::IpcReceiverWithContext,
    yinz_ipc::ipc::IpcRecv, yinz_ipc::ipc::IpcSenderWithContext, yinz_ipc::ipc::ServerToClientMsg,
};

use crate::client_screen::ClientScreen;

type ServerSpawner = Box<dyn FnOnce(PathBuf) + Send>;

fn server_message_name(msg: &ServerToClientMsg) -> String {
    match msg {
        ServerToClientMsg::AgentControl(_) => "AgentControl",
        ServerToClientMsg::DataResponse { .. } => "DataResponse",
        ServerToClientMsg::PluginResponse { .. } => "PluginResponse",
        ServerToClientMsg::AgentReportAccepted { .. } => "AgentReportAccepted",
        ServerToClientMsg::KillSessionComplete { .. } => "KillSessionComplete",
        ServerToClientMsg::KillSessionProgress => "KillSessionProgress",
        ServerToClientMsg::Render { .. } => "Render",
        ServerToClientMsg::UnblockInputThread => "UnblockInputThread",
        ServerToClientMsg::Exit { .. } => "Exit",
        ServerToClientMsg::Connected { .. } => "Connected",
        ServerToClientMsg::Log { .. } => "Log",
        ServerToClientMsg::LogError { .. } => "LogError",
        ServerToClientMsg::SwitchSession { .. } => "SwitchSession",
        ServerToClientMsg::UnblockCliPipeInput { .. } => "UnblockCliPipeInput",
        ServerToClientMsg::CliPipeOutput { .. } => "CliPipeOutput",
        ServerToClientMsg::QueryTerminalSize => "QueryTerminalSize",
        ServerToClientMsg::RenamedSession { .. } => "RenamedSession",
        ServerToClientMsg::ConfigFileUpdated => "ConfigFileUpdated",
        ServerToClientMsg::PaneRenderUpdate { .. } => "PaneRenderUpdate",
        ServerToClientMsg::SubscribedPaneClosed { .. } => "SubscribedPaneClosed",
        ServerToClientMsg::EventUpdate { .. } => "EventUpdate",
        ServerToClientMsg::ForwardQueryToHost { .. } => "ForwardQueryToHost",
        ServerToClientMsg::EmitNestedSessionFrame { .. } => "EmitNestedSessionFrame",
        ServerToClientMsg::SetMouseMode { .. } => "SetMouseMode",
    }
    .to_string()
}

pub struct FakeClientOsApi {
    client_screen: ClientScreen,
    size: Arc<Mutex<Size>>,
    stdin_rx: crossbeam::channel::Receiver<Vec<u8>>,
    cli_stdin: Option<String>,
    signal_rx: crossbeam::channel::Receiver<SignalEvent>,
    send_instructions_to_server: Arc<Mutex<Option<IpcSenderWithContext<ClientToServerMsg>>>>,
    receive_instructions_from_server: Arc<Mutex<Option<IpcReceiverWithContext<ServerToClientMsg>>>>,
    session_name: Arc<Mutex<Option<String>>>,
    server_spawner: Arc<Mutex<Option<ServerSpawner>>>,
    env: Arc<Mutex<HashMap<String, String>>>,
    received_server_messages: Arc<Mutex<Vec<String>>>,
}

impl Clone for FakeClientOsApi {
    fn clone(&self) -> Self {
        FakeClientOsApi {
            client_screen: self.client_screen.clone(),
            size: self.size.clone(),
            stdin_rx: self.stdin_rx.clone(),
            cli_stdin: self.cli_stdin.clone(),
            signal_rx: self.signal_rx.clone(),
            send_instructions_to_server: self.send_instructions_to_server.clone(),
            receive_instructions_from_server: self.receive_instructions_from_server.clone(),
            session_name: self.session_name.clone(),
            server_spawner: self.server_spawner.clone(),
            env: self.env.clone(),
            received_server_messages: self.received_server_messages.clone(),
        }
    }
}

impl std::fmt::Debug for FakeClientOsApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeClientOsApi").finish()
    }
}

pub struct FakeClientHandle {
    pub client_screen: ClientScreen,
    pub size: Arc<Mutex<Size>>,
    pub stdin_tx: crossbeam::channel::Sender<Vec<u8>>,
    pub signal_tx: crossbeam::channel::Sender<SignalEvent>,
    pub received_server_messages: Arc<Mutex<Vec<String>>>,
    send_instructions_to_server: Arc<Mutex<Option<IpcSenderWithContext<ClientToServerMsg>>>>,
}

impl FakeClientHandle {
    pub fn received_server_messages(&self) -> Vec<String> {
        self.received_server_messages.lock().unwrap().clone()
    }

    pub fn kill_session(&self) {
        self.send_instructions_to_server
            .lock()
            .unwrap()
            .as_mut()
            .expect("client is not connected")
            .send_client_msg(ClientToServerMsg::KillSession)
            .expect("failed to kill test session");
    }

    pub fn quit_client(&self) {
        if let Some(sender) = self.send_instructions_to_server.lock().unwrap().as_mut() {
            let _ = sender.send_client_msg(ClientToServerMsg::Action {
                action: Action::Detach,
                terminal_id: None,
                client_id: None,
                is_cli_client: false,
            });
        }
    }
}

impl FakeClientOsApi {
    pub fn new(
        initial_size: Size,
        server_spawner: Option<ServerSpawner>,
    ) -> (Self, FakeClientHandle) {
        Self::new_with_env(initial_size, server_spawner, HashMap::new())
    }

    pub fn new_with_env(
        initial_size: Size,
        server_spawner: Option<ServerSpawner>,
        env: HashMap<String, String>,
    ) -> (Self, FakeClientHandle) {
        let size = Arc::new(Mutex::new(initial_size));
        let client_screen = ClientScreen::new(size.clone());
        let (stdin_tx, stdin_rx) = crossbeam::channel::unbounded();
        let (signal_tx, signal_rx) = crossbeam::channel::unbounded();
        // the screen owns this so that a `wait_until` that times out can print the tail of it
        let received_server_messages = client_screen.server_messages();
        let fake_client_os_api = FakeClientOsApi {
            client_screen: client_screen.clone(),
            size: size.clone(),
            stdin_rx,
            cli_stdin: None,
            signal_rx,
            send_instructions_to_server: Arc::new(Mutex::new(None)),
            receive_instructions_from_server: Arc::new(Mutex::new(None)),
            session_name: Arc::new(Mutex::new(None)),
            server_spawner: Arc::new(Mutex::new(server_spawner)),
            env: Arc::new(Mutex::new(env)),
            received_server_messages: received_server_messages.clone(),
        };
        let fake_client_handle = FakeClientHandle {
            client_screen,
            size,
            stdin_tx,
            signal_tx,
            received_server_messages,
            send_instructions_to_server: fake_client_os_api.send_instructions_to_server.clone(),
        };
        (fake_client_os_api, fake_client_handle)
    }

    pub fn with_cli_stdin(mut self, stdin: Option<&str>) -> Self {
        self.cli_stdin = stdin.map(str::to_owned);
        self
    }
}

impl TtyControl for FakeClientOsApi {
    fn get_terminal_size(&self) -> Size {
        *self.size.lock().unwrap()
    }
    fn set_raw_mode(&mut self) {}
    fn unset_raw_mode(&self) -> Result<(), std::io::Error> {
        Ok(())
    }
    fn load_palette(&self) -> Palette {
        default_palette()
    }
    fn enable_mouse(&self) -> anyhow::Result<()> {
        use std::io::Write;
        const ENABLE_MOUSE_SUPPORT: &str =
            "\u{1b}[?1000h\u{1b}[?1002h\u{1b}[?1003h\u{1b}[?1015h\u{1b}[?1006h";
        let mut writer = self.client_screen.writer();
        writer.write_all(ENABLE_MOUSE_SUPPORT.as_bytes())?;
        writer.flush()?;
        Ok(())
    }
    fn disable_mouse(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

impl StdioAccess for FakeClientOsApi {
    fn get_stdout_writer(&self) -> Box<dyn std::io::Write> {
        self.client_screen.writer()
    }
    fn get_stdin_reader(&self) -> Box<dyn std::io::BufRead> {
        Box::new(std::io::Cursor::new(
            self.cli_stdin.clone().unwrap_or_default(),
        ))
    }
    fn stdin_is_terminal(&self) -> bool {
        self.cli_stdin.is_none()
    }
    fn read_from_stdin(&mut self) -> Result<Vec<u8>, &'static str> {
        self.stdin_rx.recv().map_err(|_| "fake client stdin closed")
    }
}

impl ServerConnection for FakeClientOsApi {
    fn update_session_name(&mut self, new_session_name: yinz_types::internal_ids::SessionName) {
        *self.session_name.lock().unwrap() = Some(new_session_name.to_string());
    }
    fn send_to_server(&self, msg: ClientToServerMsg) {
        match self.send_instructions_to_server.lock().unwrap().as_mut() {
            Some(ipc_sender) => {
                let _ = ipc_sender.send_client_msg(msg);
            },
            None => {
                log::warn!("Server not ready, dropping message.");
            },
        }
    }
    fn recv_from_server(&self) -> IpcRecv<ServerToClientMsg> {
        let received = self
            .receive_instructions_from_server
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .recv_server_msg();
        if let IpcRecv::Msg(msg, _) = &received {
            // `Log`/`LogError` carry their lines rather than just their name:
            // they are how an action answers a cli client in prose, and for
            // anything whose whole outcome *is* the prose — a warp explaining why
            // it will not move a pane — the name says nothing. A real cli client
            // prints these to its own stderr, which a test running the client on
            // a thread of its own cannot read.
            let recorded = match msg {
                ServerToClientMsg::Log { lines } | ServerToClientMsg::LogError { lines } => {
                    format!("{}: {}", server_message_name(msg), lines.join("\n"))
                },
                ServerToClientMsg::CliPipeOutput { output, .. } => {
                    format!("CliPipeOutput: {output}")
                },
                // The type and then the JSON, one space apart: `streamed_event_types` reads
                // the first word, `streamed_events` the rest, and a test that wants to know
                // what an `AgentInfo` said can read it off the payload.
                ServerToClientMsg::EventUpdate {
                    event_type,
                    event_json,
                } => {
                    format!(
                        "{}: {} {}",
                        server_message_name(msg),
                        event_type,
                        event_json
                    )
                },
                _ => server_message_name(msg),
            };
            self.received_server_messages.lock().unwrap().push(recorded);
        }
        received
    }
    fn connect_to_server(&self, path: &Path) -> Result<(), ConnectToServerError> {
        // Bounded for the same reason as the real one, and doubly so here: a
        // test whose server never binds should fail with a message naming the
        // socket, not hang until nextest's kill timer notices. Tighter than the
        // real client's 30s/50ms because an in-process server binds at once.
        let socket = connect_with_deadline(
            path,
            std::time::Duration::from_secs(10),
            std::time::Duration::from_millis(10),
        )?;
        let ipc_sender = IpcSenderWithContext::new(socket);
        let ipc_receiver = ipc_sender
            .get_receiver()
            .expect("failed to clone client socket");
        *self.send_instructions_to_server.lock().unwrap() = Some(ipc_sender);
        *self.receive_instructions_from_server.lock().unwrap() = Some(ipc_receiver);
        Ok(())
    }
    fn spawn_server(&self, socket_path: &Path, _debug: bool) -> Result<(), std::io::Error> {
        if let Some(server_spawner) = self.server_spawner.lock().unwrap().take() {
            server_spawner(socket_path.to_path_buf());
        }
        Ok(())
    }
}

impl SignalSource for FakeClientOsApi {
    fn handle_signals(&self, sigwinch_cb: Box<dyn Fn()>, quit_cb: Box<dyn Fn()>) {
        loop {
            match self.signal_rx.recv() {
                Ok(SignalEvent::Resize) => sigwinch_cb(),
                Ok(SignalEvent::Quit) => {
                    quit_cb();
                    break;
                },
                Err(_) => break,
            }
        }
    }
    fn env_variable(&self, name: &str) -> Option<String> {
        self.env.lock().unwrap().get(name).cloned()
    }
}

impl ClientOsApi for FakeClientOsApi {
    fn box_clone(&self) -> Box<dyn ClientOsApi> {
        Box::new(self.clone())
    }
    fn should_install_panic_hook(&self) -> bool {
        false
    }
}
