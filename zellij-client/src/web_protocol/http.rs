//! The HTTP request and response bodies exchanged over the routes in
//! [`super::endpoints`].

use serde::{Deserialize, Serialize};

use super::control_message::SetConfigPayload;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LoginRequest {
    pub auth_token: String,
    /// Absent when the caller did not say either way, which reads as "no".
    pub remember_me: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LoginResponse {
    pub success: bool,
    pub message: String,
}

/// The `/session` response.
///
/// A remote client only needs `web_client_id`; the rest is for the browser. The
/// defaults are what let the remote client parse a body that carries just the
/// id.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CreateClientIdResponse {
    pub web_client_id: String,
    #[serde(default)]
    pub is_read_only: bool,
    #[serde(default)]
    pub session_name: String,
    #[serde(default)]
    pub config: SetConfigPayload,
}

#[derive(Deserialize)]
pub struct SessionQuery {
    pub session: Option<String>,
    pub welcome: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct WebSessionInfo {
    pub name: String,
    pub web_clients_allowed: bool,
    pub tab_count: usize,
    pub pane_count: usize,
    pub connected_clients: usize,
    pub creation_secs_ago: u64,
}

#[derive(Serialize, Deserialize)]
pub struct SessionListResponse {
    pub sessions: Vec<WebSessionInfo>,
}

#[derive(Deserialize)]
pub struct TerminalParams {
    pub web_client_id: String,
    pub rows: Option<u16>,
    pub cols: Option<u16>,
    pub cell_width: Option<u16>,
    pub cell_height: Option<u16>,
}

#[derive(Deserialize)]
pub struct ControlParams {
    pub web_client_id: String,
}
