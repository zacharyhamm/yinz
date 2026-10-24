//! Client-relative session destinations. Session names alone are not machine identities.
use serde::{Deserialize, Serialize};

/// The server's reply to a session request for a client that did not advertise
/// `CliAssets::supports_session_requests`. Such a client would discard the request unread.
pub const UNSUPPORTED_CLIENT: &str = "This Yinz client cannot switch sessions across machines. \
     Reattach with a current Yinz build to use the session switcher";

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Destination {
    CurrentServer,
    Local,
    Remote(String),
}

impl Destination {
    pub fn label(&self) -> &str {
        match self {
            Self::CurrentServer => "Current server",
            Self::Local => "Local",
            Self::Remote(host) => host,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub destination: Destination,
    pub name: Option<String>,
    pub status: String,
    pub current: bool,
    pub saved: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Inventory {
    pub current_machine: Destination,
    pub entries: Vec<Entry>,
    /// The acting client predates session requests: only current-server sessions are listed,
    /// and switching to one uses the server's own session switch.
    #[serde(default)]
    pub current_server_only: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SwitchTarget {
    pub destination: Destination,
    pub name: String,
    /// Only a current-server saved session may be resurrected.
    pub saved: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SwitchRequest {
    Prepare { id: String, target: SwitchTarget },
    Status(String),
    Commit(String),
    Cancel(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClientRequest {
    Inventory(String),
    Switch(SwitchRequest),
    CancelAll,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClientReply {
    Inventory(Inventory),
    Pending,
    Ready,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub token: String,
    pub request: ClientRequest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    pub token: String,
    pub result: Result<ClientReply, String>,
}
