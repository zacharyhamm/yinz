//! Untrusted reported program status, independent of agent identity and completion.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AttentionStatus {
    Idle,
    Working,
    Done,
    Failed,
    NeedsYou,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlockedKind {
    Permission,
    Question,
    Auth,
}

impl BlockedKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            BlockedKind::Permission => "permission",
            BlockedKind::Question => "question",
            BlockedKind::Auth => "auth",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "permission" => Some(BlockedKind::Permission),
            "question" => Some(BlockedKind::Question),
            "auth" => Some(BlockedKind::Auth),
            _ => None,
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            BlockedKind::Permission => "needs approval",
            BlockedKind::Question => "has a question",
            BlockedKind::Auth => "needs login",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProgramState {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
}

impl ProgramState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProgramState::Idle => "idle",
            ProgramState::Working => "working",
            ProgramState::Done => "done",
            ProgramState::Blocked => "blocked",
            ProgramState::Error => "error",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "idle" => Some(ProgramState::Idle),
            "working" => Some(ProgramState::Working),
            "done" => Some(ProgramState::Done),
            "blocked" => Some(ProgramState::Blocked),
            "error" => Some(ProgramState::Error),
            _ => None,
        }
    }
    pub fn attention_status(&self) -> AttentionStatus {
        match self {
            ProgramState::Idle => AttentionStatus::Idle,
            ProgramState::Working => AttentionStatus::Working,
            ProgramState::Done => AttentionStatus::Done,
            ProgramState::Blocked => AttentionStatus::NeedsYou,
            ProgramState::Error => AttentionStatus::Failed,
        }
    }
    /// Basic display only: never use this to authorize input or complete a submission.
    pub fn agent_state(self, fallback: crate::agent::AgentState) -> crate::agent::AgentState {
        use crate::agent::AgentState;
        if fallback == AgentState::Exited {
            return fallback;
        }
        match self {
            Self::Working => AgentState::Busy,
            Self::Blocked => AgentState::Waiting,
            Self::Idle | Self::Done | Self::Error => AgentState::Idle,
        }
    }
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            ProgramState::Idle | ProgramState::Working | ProgramState::Blocked
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct ProgramStatusRecord {
    pub id: Option<String>,
    pub state: Option<ProgramState>,
    pub kind: Option<BlockedKind>,
    pub progress: Option<u8>,
    pub app: Option<String>,
    pub title: Option<String>,
    pub msg: Option<String>,
    pub updated_ms: u64,
}

impl ProgramStatusRecord {
    pub fn is_root(&self) -> bool {
        self.id.is_none()
    }
    pub fn state(&self) -> ProgramState {
        self.state.unwrap_or(ProgramState::Idle)
    }
}

/// Pane-local presentation. `app` and `id` never establish agent identity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProgramStatusView {
    pub records: Vec<ProgramStatusRecord>,
    pub current: Option<ProgramStatusRecord>,
}
impl ProgramStatusView {
    pub fn is_empty(&self) -> bool {
        self.records.is_empty() && self.current.is_none()
    }
    /// Basic display only: never use this to authorize input or complete a submission.
    pub fn agent_state(&self, fallback: crate::agent::AgentState) -> crate::agent::AgentState {
        self.current
            .as_ref()
            .map_or(fallback, |record| record.state().agent_state(fallback))
    }
}
