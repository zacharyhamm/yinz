//! Session-local Mother supervision contracts.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MotherRequest {
    Status,
    Overview,
    Journal {
        after: Option<u64>,
        through: Option<u64>,
        limit: Option<usize>,
    },
    Resolve {
        submission_id: String,
        expect_rev: u64,
    },
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
