//! Capability-gated client participation in a session update.
use crate::data::SessionConnection;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const PROTOCOL_VERSION: u32 = 1;
pub const UNSUPPORTED_CLIENT: &str =
    "This legacy client requires a manual update/restart on its host after the session update";
pub const PARTICIPANT_ARG: &str = "--update-participant";
pub const RESUME_ARG: &str = "--resume-client";

/// Each helper invocation acts on one durable, operation-scoped installation journal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ParticipantRequest {
    Inventory,
    Prepare {
        operation: String,
        release: serde_json::Value,
    },
    Status {
        operation: String,
    },
    Publish {
        operation: String,
    },
    Rollback {
        operation: String,
    },
    Commit {
        operation: String,
    },
}

impl ParticipantRequest {
    pub fn operation(&self) -> Option<&str> {
        match self {
            Self::Inventory => None,
            Self::Prepare { operation, .. }
            | Self::Status { operation }
            | Self::Publish { operation }
            | Self::Rollback { operation }
            | Self::Commit { operation } => Some(operation),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParticipantInvocation {
    pub data_dir: Option<PathBuf>,
    /// Captured before publication, so replacing the executable cannot change its identity.
    pub executable: PathBuf,
    pub request: ParticipantRequest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClientRequest {
    Participant(ParticipantRequest),
    /// Poll the work started by the request with this token, without starting it again.
    Poll {
        token: String,
    },
    /// Sent after the replacement has restored this client's server-side state.
    Ready {
        operation: String,
        source_client: crate::data::ClientId,
    },
    Restart {
        operation: String,
        source_client: crate::data::ClientId,
        executable: PathBuf,
        target: SessionConnection,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClientReply {
    Pending,
    Participant(serde_json::Value),
    /// The old client has accepted restart; this is not an acknowledgement of the new client.
    Restarting,
    /// A replacement client has successfully written its first session render.
    Resumed {
        operation: String,
        source_client: crate::data::ClientId,
    },
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
