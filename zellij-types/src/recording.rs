use serde::{Deserialize, Serialize};

/// A recorded pane, as listed by `zellij_storage::session_recording::list_recordings`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct RecordingSummary {
    pub id: i64,
    pub session_name: String,
    pub tab_name: String,
    pub terminal_id: u32,
    pub pane_title: String,
    pub started_at_us: i64,
    pub duration_us: i64,
    pub is_running: bool,
    pub capture_input: bool,
    pub error: Option<String>,
}

/// A match from `zellij_storage::session_recording::search_recordings`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SearchHit {
    /// Same id `list_recordings` and `export_recording` use (a `recording_panes` row).
    pub recording_id: i64,
    pub session_name: String,
    pub tab_name: String,
    pub pane_title: String,
    /// Offset into the recording, for seeking within the exported asciicast.
    pub elapsed_us: i64,
    pub started_at_us: i64,
    pub text: String,
}
