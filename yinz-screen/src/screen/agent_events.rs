use super::Screen;
use crate::ClientId;
use std::time::Instant;
use yinz_ipc::ipc::EventSubscription;
#[cfg(test)]
use yinz_types::agent::{AgentEvent, HookEvent};
use yinz_types::pane_id::PaneId;
#[cfg(test)]
use {serde_json::Value, yinz_types::agent::control::AgentOutcome};
impl Screen {
    pub(super) fn subscribe_agent_events(&mut self, client: ClientId, options: EventSubscription) {
        self.agents
            .subscribe_agent_events(&self.core, client, options)
    }

    #[cfg(test)]
    pub(super) fn publish_agent_event(&mut self, pane: PaneId, event_type: &str, extra: Value) {
        self.agents
            .publish_agent_event(&self.core, pane, event_type, extra)
    }

    pub(super) fn flush_agent_events(&mut self) {
        self.agents.flush_agent_events(&self.core)
    }

    pub(super) fn refresh_agent_events(&mut self) {
        self.agents.refresh_agent_events(&self.core)
    }

    pub(super) fn retire_agent_event_pane(&mut self, pane: PaneId, event_type: &str) {
        self.agents
            .retire_agent_event_pane(&self.core, pane, event_type)
    }

    #[cfg(test)]
    pub(super) fn observe_agent_event(
        &mut self,
        event: &AgentEvent,
        hook: Option<&HookEvent>,
        submission: Option<String>,
    ) {
        self.agents
            .observe_agent_event(&self.core, event, hook, submission)
    }
}

impl Screen {
    #[cfg(test)]
    pub(super) fn agent_observation_uncertain(
        &mut self,
        pane: PaneId,
        submission: Option<String>,
        outcome: AgentOutcome,
    ) {
        self.agents
            .agent_observation_uncertain(&self.core, pane, submission, outcome)
    }

    pub(super) fn invalidate_agent_event_projects(&mut self) {
        self.palette.invalidate_palette_agent_labels();
        self.agents.invalidate_agent_event_projects(&self.core);
    }
}

impl Screen {
    pub(super) fn retry_agent_event_projects(&mut self, now: Instant) {
        self.agents.retry_agent_event_projects(&self.core, now)
    }

    pub(super) fn refresh_agent_event_projects(&mut self) {
        self.agents.refresh_agent_event_projects(&self.core)
    }

    pub(super) fn apply_agent_event_projects(
        &mut self,
        version: u64,
        result: yinz_bus::screen::AgentEventProjectMemberships,
    ) {
        self.agents
            .apply_agent_event_projects(&self.core, version, result)
    }
}
