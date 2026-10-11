//! Session-local Mother supervision contracts.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MotherRequest {
    Status,
    Inbox,
    Ack {
        batch: String,
    },
    Checkpoint {
        expect_rev: Option<u64>,
        value: Option<serde_json::Value>,
    },
    Configure {
        panes: Vec<String>,
        projects: Vec<String>,
        kinds: Vec<String>,
        lifecycle_types: Vec<String>,
    },
    Pause,
    Resume,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MotherStatus {
    pub state: String,
    pub pending: usize,
    pub blockers: usize,
}

impl MotherStatus {
    pub fn label(&self) -> String {
        let state = match self.state.as_str() {
            "watching" => "watching",
            "processing" => "working",
            "queued" => "queued",
            "paused" => "paused",
            "needs_attention" => "attention",
            _ => "degraded",
        };
        format!(" Mother: {state} ({}/{}) ", self.pending, self.blockers)
    }
}
