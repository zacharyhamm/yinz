//! Correlated, capability-gated commands used by the out-of-process update coordinator.
use crate::{data::ClientId, input::cli_assets::CliAssets, update_client};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const PROTOCOL_VERSION: u32 = 1;
pub const COORDINATOR_ARG: &str = "--session-update";
pub const CAPABILITIES_ARG: &str = "--update-capabilities";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub token: String,
    pub operation: String,
    pub command: Command,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Command {
    Identity,
    Inventory,
    Start,
    Status,
    Cancel,
    PrepareMigration,
    ReleaseMigration {
        snapshot: PathBuf,
    },
    AdoptMigration {
        snapshot: PathBuf,
    },
    AbortMigration,
    FinishMigration,
    RenameSession {
        name: String,
    },
    Notify {
        text: String,
    },
    LegacyReconnect {
        client: ClientId,
        target: crate::data::SessionConnection,
    },
    RestoreClients {
        clients: Vec<Client>,
    },
    Client {
        client: ClientId,
        request: update_client::Request,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerIdentity {
    pub server_pid: u32,
    pub server_instance: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    pub token: String,
    pub operation: String,
    pub result: Result<serde_json::Value, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Client {
    pub id: ClientId,
    pub supports_update: bool,
    pub resumed: Option<(String, ClientId)>,
    pub configuration: String,
    pub mode: crate::data::InputMode,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Inventory {
    pub session: String,
    pub server_pid: u32,
    /// Process incarnation, independent of the logical name and reused OS PIDs.
    pub server_instance: String,
    pub executable: PathBuf,
    pub running_version: String,
    pub assets: CliAssets,
    pub configuration: serde_json::Value,
    pub clients: Vec<Client>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub build_version: String,
    pub client_server: usize,
    pub ssh_bootstrap: usize,
    pub update: u32,
    pub migration: u32,
}

/// All generations on the same host and canonical installation share this identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installation {
    pub host: String,
    pub executable: PathBuf,
    pub data_dir: PathBuf,
    pub running_version: String,
    pub installed_version: String,
    pub target: String,
    pub capabilities: Capabilities,
}
