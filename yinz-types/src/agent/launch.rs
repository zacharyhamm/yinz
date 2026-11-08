//! Version two managed launches. Prompt acknowledgement is deliberately independent.
use super::{control::AgentPlacement, AgentLaunchResources, AgentSpawnSpec, SpawnedAgent};
use crate::{internal_ids::TabId, pane_id::PaneId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub const CAPABILITY: &str = "agent-launch-transaction-v2";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LaunchRequest {
    pub placement: AgentPlacement,
    pub spec: AgentSpawnSpec,
    pub cwd: Option<PathBuf>,
    pub project: Option<String>,
    pub remote_host: Option<String>,
    #[serde(default)]
    pub new_project: Option<(String, PathBuf)>,
}

impl LaunchRequest {
    /// Struct fields and environment maps have deterministic serialization order. Never store
    /// the serialized request: environment values can contain credentials.
    pub fn digest(&self) -> Result<String, serde_json::Error> {
        serde_json::to_vec(self).map(|bytes| {
            Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchOutcome {
    Pending,
    Rejected,
    Started,
    Failed,
    Cancelled,
    Unknown,
    Conflict,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchPhase {
    Admits,
    Prepares,
    Starts,
    Registers,
    Aborts,
    Finished,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessState {
    NotStarted,
    Starting,
    Running,
    Exited,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchReason {
    InsufficientRoom,
    PaneLimit,
    ControllerMoved,
    ControllerClosed,
    DestinationClosed,
    PlacementRevoked,
    PrepareFailed,
    ExecFailed,
    RegistrationFailed,
    HistoryUnavailable,
    RequestConflict,
    InvalidRequest,
    TransportUnavailable,
    ProcessExited,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerDisposition {
    Reserved,
    Available,
    Closed,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchSlot {
    pub pane_id: PaneId,
    /// Terminal allocations are never reused within a server incarnation.
    pub process_generation: u64,
    pub tab_id: TabId,
    pub tab_incarnation: String,
    pub controller: Option<PaneId>,
    pub worker_region: Option<String>,
    pub placement_generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceOwnership {
    Borrowed,
    Created,
    RetainedAttempt,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitResourceKind {
    Branch,
    Worktree,
    ProjectDirectory,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitResource {
    pub kind: GitResourceKind,
    pub ownership: ResourceOwnership,
    pub path: PathBuf,
    pub branch: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LaunchManifest {
    pub slot: Option<LaunchSlot>,
    pub terminal_allocated: bool,
    pub placeholder_owned: bool,
    pub process_owned: bool,
    pub created_tab: bool,
    pub checkout: Option<AgentLaunchResources>,
    #[serde(default)]
    pub git: Vec<GitResource>,
    pub cleanup_pending: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LaunchReceipt {
    pub version: u8,
    pub launch_id: String,
    pub request_digest: String,
    pub server_instance: String,
    pub outcome: LaunchOutcome,
    pub phase: LaunchPhase,
    pub reason: Option<LaunchReason>,
    pub process: ProcessState,
    pub resources: LaunchManifest,
    pub spawned: Option<SpawnedAgent>,
    pub disposition: WorkerDisposition,
}
