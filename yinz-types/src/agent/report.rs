use super::{AgentEvent, AgentInfo};
use crate::internal_ids::ProcessId;
use crate::pane_id::PaneId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentReportState {
    Unchanged,
    Clear,
    Set(AgentInfo),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentReport {
    pub report_id: String,
    pub pane_id: PaneId,
    pub state: AgentReportState,
    pub event: Option<AgentEvent>,
    pub trace: bool,
    pub process_id: ProcessId,
    pub kind: String,
}
