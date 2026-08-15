use axum::extract::ws::Message;
use std::collections::{HashMap, VecDeque};
use std::sync::{atomic::AtomicBool, Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::os_input_output::ClientOsApi;
use crate::web_client::session_management::spawn_new_session;
use crate::web_protocol::http::WebSessionInfo;
use std::path::{Path, PathBuf};
use zellij_utils::input::actions::Action;
use zellij_utils::{
    input::{config::Config, options::Options},
    ipc::ClientToServerMsg,
};

pub trait ClientOsApiFactory: Send + Sync + std::fmt::Debug {
    fn create_client_os_api(&self) -> Result<Box<dyn ClientOsApi>, Box<dyn std::error::Error>>;
}

#[derive(Debug, Clone)]
pub struct RealClientOsApiFactory;

impl ClientOsApiFactory for RealClientOsApiFactory {
    fn create_client_os_api(&self) -> Result<Box<dyn ClientOsApi>, Box<dyn std::error::Error>> {
        crate::os_input_output::get_client_os_input()
            .map(|os_input| Box::new(os_input) as Box<dyn ClientOsApi>)
            .map_err(|e| format!("Failed to create client OS API: {:?}", e).into())
    }
}

pub trait SessionManager: Send + Sync + std::fmt::Debug {
    fn session_exists(&self, session_name: &str) -> Result<bool, Box<dyn std::error::Error>>;
    fn list_sessions(&self) -> Vec<WebSessionInfo>;
    fn get_resurrection_layout(
        &self,
        session_name: &str,
    ) -> Option<zellij_utils::input::layout::Layout>;
    fn spawn_session_if_needed(
        &self,
        session_name: &str,
        os_input: Box<dyn ClientOsApi>,
        session_exists: bool,
        zellij_ipc_pipe: &Path,
        first_message: ClientToServerMsg,
    );
}

#[derive(Debug, Clone)]
pub struct RealSessionManager;

impl SessionManager for RealSessionManager {
    fn session_exists(&self, session_name: &str) -> Result<bool, Box<dyn std::error::Error>> {
        zellij_utils::sessions::session_exists(session_name)
            .map_err(|e| format!("Session check failed: {:?}", e).into())
    }

    fn list_sessions(&self) -> Vec<WebSessionInfo> {
        zellij_utils::sessions::read_live_session_states_default_dirs("")
            .into_values()
            .map(|info| WebSessionInfo {
                name: info.name,
                web_clients_allowed: info.web_clients_allowed,
                tab_count: info.tabs.len(),
                pane_count: info.panes.panes.values().map(|panes| panes.len()).sum(),
                connected_clients: info.connected_clients,
                creation_secs_ago: info.creation_time.as_secs(),
            })
            .collect()
    }

    fn get_resurrection_layout(
        &self,
        session_name: &str,
    ) -> Option<zellij_utils::input::layout::Layout> {
        zellij_utils::sessions::resurrection_layout(session_name)
            .ok()
            .flatten()
    }

    fn spawn_session_if_needed(
        &self,
        session_name: &str,
        os_input: Box<dyn ClientOsApi>,
        session_exists: bool,
        zellij_ipc_pipe: &Path,
        first_message: ClientToServerMsg,
    ) {
        if !session_exists {
            spawn_new_session(session_name, os_input.clone(), zellij_ipc_pipe);
        }
        os_input.connect_to_server(zellij_ipc_pipe);
        os_input.send_to_server(first_message);
    }
}

#[derive(Debug, Default, Clone)]
pub struct ConnectionTable {
    pub client_id_to_channels: HashMap<String, ClientChannels>,
    pub client_read_only_status: HashMap<String, bool>,
    pub client_session_token_hash: HashMap<String, String>,
}

const MAX_PENDING_CONTROL_MESSAGES: usize = 64;

#[derive(Debug, Clone)]
pub struct ClientChannels {
    pub os_api: Box<dyn ClientOsApi>,
    pub control_channel_tx: Option<UnboundedSender<Message>>,
    pub terminal_channel_tx: Option<UnboundedSender<String>>,
    terminal_channel_cancellation_token: Option<CancellationToken>,
    pub should_not_reconnect: Arc<AtomicBool>,
    pending_control_messages: Vec<Message>,
}

impl ClientChannels {
    pub fn new(os_api: Box<dyn ClientOsApi>) -> Self {
        ClientChannels {
            os_api,
            control_channel_tx: None,
            terminal_channel_tx: None,
            terminal_channel_cancellation_token: None,
            should_not_reconnect: Arc::new(AtomicBool::new(false)),
            pending_control_messages: Vec::new(),
        }
    }

    pub fn add_control_tx(&mut self, control_channel_tx: UnboundedSender<Message>) {
        for message in self.pending_control_messages.drain(..) {
            let _ = control_channel_tx.send(message);
        }
        self.control_channel_tx = Some(control_channel_tx);
    }

    pub fn queue_control_message(&mut self, message: Message) {
        if self.pending_control_messages.len() >= MAX_PENDING_CONTROL_MESSAGES {
            self.pending_control_messages.remove(0);
        }
        self.pending_control_messages.push(message);
    }

    pub fn add_terminal_tx(&mut self, terminal_channel_tx: UnboundedSender<String>) {
        self.terminal_channel_tx = Some(terminal_channel_tx);
    }

    pub fn add_terminal_channel_cancellation_token(
        &mut self,
        terminal_channel_cancellation_token: CancellationToken,
    ) {
        self.terminal_channel_cancellation_token = Some(terminal_channel_cancellation_token);
    }
    pub fn cleanup(&mut self) {
        if let Some(terminal_channel_cancellation_token) =
            self.terminal_channel_cancellation_token.take()
        {
            terminal_channel_cancellation_token.cancel();
        }
    }
}

#[derive(Debug)]
pub struct ClientConnectionBus {
    pub connection_table: Arc<Mutex<ConnectionTable>>,
    pub stdout_channel_tx: Option<UnboundedSender<String>>,
    pub control_channel_tx: Option<UnboundedSender<Message>>,
    pub web_client_id: String,
}

impl ClientConnectionBus {
    pub fn new(web_client_id: &str, connection_table: &Arc<Mutex<ConnectionTable>>) -> Self {
        let connection_table = connection_table.clone();
        let web_client_id = web_client_id.to_owned();
        let (stdout_channel_tx, control_channel_tx) = {
            let connection_table = connection_table.lock().unwrap();
            (
                connection_table.get_client_terminal_tx(&web_client_id),
                connection_table.get_client_control_tx(&web_client_id),
            )
        };
        ClientConnectionBus {
            connection_table,
            stdout_channel_tx,
            control_channel_tx,
            web_client_id,
        }
    }
}

pub const MAX_PENDING_WELCOME_SESSIONS: usize = 256;
pub const PENDING_WELCOME_SESSION_TTL: Duration = Duration::from_secs(300);

pub type PendingWelcomeSessions = Arc<Mutex<VecDeque<(String, Instant)>>>;

pub fn record_pending_welcome_session(pending: &PendingWelcomeSessions, session_name: &str) {
    let mut pending = pending.lock().unwrap();
    let now = Instant::now();
    pending.retain(|(_, created_at)| now.duration_since(*created_at) < PENDING_WELCOME_SESSION_TTL);
    while pending.len() >= MAX_PENDING_WELCOME_SESSIONS {
        pending.pop_front();
    }
    pending.push_back((session_name.to_owned(), now));
}

pub fn take_pending_welcome_session(pending: &PendingWelcomeSessions, session_name: &str) -> bool {
    let mut pending = pending.lock().unwrap();
    let now = Instant::now();
    pending.retain(|(_, created_at)| now.duration_since(*created_at) < PENDING_WELCOME_SESSION_TTL);
    match pending.iter().position(|(name, _)| name == session_name) {
        Some(index) => {
            pending.remove(index);
            true
        },
        None => false,
    }
}

#[derive(Clone)]
pub struct AppState {
    pub connection_table: Arc<Mutex<ConnectionTable>>,
    pub config: Arc<Mutex<Config>>,
    pub config_options: Options,
    pub config_file_path: PathBuf,
    pub session_manager: Arc<dyn SessionManager>,
    pub client_os_api_factory: Arc<dyn ClientOsApiFactory>,
    pub is_https: bool,
    pub pending_welcome_sessions: PendingWelcomeSessions,
}

impl AppState {
    /// The client's own view of the OS, cloned out from under the table lock.
    ///
    /// Every websocket frame needs this, and holding the lock while handling
    /// the frame would serialise all connections, so it is always cloned.
    pub fn os_api_for(&self, web_client_id: &str) -> Option<Box<dyn ClientOsApi>> {
        self.connection_table
            .lock()
            .unwrap()
            .get_client_os_api(web_client_id)
            .cloned()
    }

    /// Whether `session_token_hash` is the token that created `web_client_id`.
    pub fn client_is_owned_by(&self, web_client_id: &str, session_token_hash: &str) -> bool {
        self.connection_table
            .lock()
            .unwrap()
            .verify_client_ownership(web_client_id, session_token_hash)
    }
}

pub const BRACKETED_PASTE_START: [u8; 6] = [27, 91, 50, 48, 48, 126]; // \u{1b}[200~
pub const BRACKETED_PASTE_END: [u8; 6] = [27, 91, 50, 48, 49, 126]; // \u{1b}[201~

/// A plain action from a web client: not tied to a terminal, not attributed
/// to a particular client id, and never from the CLI.
pub fn client_action(action: Action) -> ClientToServerMsg {
    ClientToServerMsg::Action {
        action,
        terminal_id: None,
        client_id: None,
        is_cli_client: false,
    }
}
