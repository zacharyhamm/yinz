use crate::internal_ids::TabId;
use crate::internal_ids::{RecordingId, TerminalId};
use crate::units::{ElapsedMicros, UnixMicros};
use serde::{Deserialize, Serialize};

/// A recorded pane, as listed by `zellij_storage::session_recording::list_recordings`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct RecordingSummary {
    pub id: RecordingId,
    pub session_name: String,
    pub tab_name: String,
    pub terminal_id: TerminalId,
    pub pane_title: String,
    pub started_at_us: UnixMicros,
    pub duration_us: ElapsedMicros,
    pub is_running: bool,
    pub capture_input: bool,
    pub error: Option<String>,
}

/// A match from `zellij_storage::session_recording::search_recordings`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SearchHit {
    /// Same id `list_recordings` and `export_recording` use (a `recording_panes` row).
    pub recording_id: RecordingId,
    pub session_name: String,
    pub tab_name: String,
    pub pane_title: String,
    /// Offset into the recording, for seeking within the exported asciicast.
    pub elapsed_us: ElapsedMicros,
    pub started_at_us: UnixMicros,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Output,
    Input,
    Resize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedEvent {
    pub terminal_id: TerminalId,
    pub elapsed_us: ElapsedMicros,
    pub kind: EventKind,
    pub data: Vec<u8>,
}

impl RecordedEvent {
    pub fn output(terminal_id: TerminalId, elapsed_us: ElapsedMicros, data: Vec<u8>) -> Self {
        Self {
            terminal_id,
            elapsed_us,
            kind: EventKind::Output,
            data,
        }
    }

    pub fn input(terminal_id: TerminalId, elapsed_us: ElapsedMicros, data: Vec<u8>) -> Self {
        Self {
            terminal_id,
            elapsed_us,
            kind: EventKind::Input,
            data,
        }
    }

    pub fn resize(
        terminal_id: TerminalId,
        elapsed_us: ElapsedMicros,
        cols: u16,
        rows: u16,
    ) -> Self {
        Self {
            terminal_id,
            elapsed_us,
            kind: EventKind::Resize,
            data: format!("{}x{}", cols, rows).into_bytes(),
        }
    }

    pub fn asciicast_code(&self) -> &'static str {
        match self.kind {
            EventKind::Output => "o",
            EventKind::Input => "i",
            EventKind::Resize => "r",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneMetadata {
    pub terminal_id: TerminalId,
    pub tab_id: TabId,
    pub tab_position: usize,
    pub tab_name: String,
    pub title: String,
    pub cols: usize,
    pub rows: usize,
}
