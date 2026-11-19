//! Observations for mixed Screen/agent integration fixtures. Enabled by the
//! screen crate's dev-dependency only. Mutable controller storage stays private.
use super::*;
use std::collections::VecDeque;
use yinz_types::agent::control::RetainedSubmission;

pub const RECOVERY_QUIET: Duration = RECOVER_QUIET;
pub const HISTORY_LIMIT: usize = SUBMISSION_HISTORY_LIMIT;

pub struct PaneTracking<'a> {
    agents: &'a AgentController,
    pane: PaneId,
}

impl AgentController {
    pub fn inspect_pane(&self, pane: PaneId) -> PaneTracking<'_> {
        PaneTracking { agents: self, pane }
    }

    pub fn pending_control_count(&self) -> usize {
        self.agent_controls.len()
    }

    pub fn closed_history(
        &self,
    ) -> VecDeque<(
        Instant,
        RetainedSubmission,
        u64,
        Option<yinz_types::agent::claude_notifications::Source>,
    )> {
        self.closed_agent_submissions.clone()
    }

    pub fn set_fixture_lifecycle(&mut self, pane: PaneId, state: LifecycleState) {
        self.agent_lifecycle.insert(pane, state.into());
    }
}

impl PaneTracking<'_> {
    pub fn lifecycle(&self) -> Option<LifecycleState> {
        self.agents
            .agent_lifecycle
            .get(&self.pane)
            .map(Lifecycle::handoff)
    }
    pub fn last_report(&self) -> Instant {
        self.agents.agent_lifecycle[&self.pane].last_report
    }
    pub fn background(&self) -> bool {
        self.agents.claude_background_pending(self.pane)
    }
    pub fn launch(&self) -> bool {
        self.agents.agent_launches.contains_key(&self.pane)
    }
    pub fn has_clear_input(&self) -> bool {
        self.agents.agent_clear_inputs.contains_key(&self.pane)
    }
    pub fn rejects_agy_confirmation(&self, hook: &HookEvent) -> bool {
        self.agents.ignore_agy_confirmation(self.pane, hook)
    }
}

impl<'a> PaneTracking<'a> {
    pub fn submissions(&self) -> Option<&'a VecDeque<Submission>> {
        self.agents.agent_submissions.get(&self.pane)
    }
    pub fn workers(&self) -> Option<&'a WorkerWaits> {
        self.agents.agent_worker_waits.get(&self.pane)
    }
    pub fn permission(&self) -> Option<&'a PermissionWait> {
        self.agents.claude.permission_waits.get(&self.pane)
    }
    pub fn dialog(&self) -> Option<&'a DialogInput> {
        self.agents.agent_dialog_inputs.get(&self.pane)
    }
}
