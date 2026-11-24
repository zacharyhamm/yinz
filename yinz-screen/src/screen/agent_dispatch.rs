//! Coordinate agent operations that publish pane state or change the layout.
use super::{
    agent_control::{AgentClose, Reply},
    Screen,
};
use std::time::Instant;
use yinz_runtime::errors::prelude::*;
#[cfg(test)]
use yinz_types::agent::control::{AgentStatus, NotReadyReason};
use yinz_types::agent::tracking::AgentTrackingHandoff;
use yinz_types::{
    agent::{control::AgentRequest, AgentEvent, AgentInfo, AgentState},
    internal_ids::ClientId,
    pane_id::PaneId,
};
impl Screen {
    #[cfg(test)]
    pub(crate) fn agent_status_for_test(&self) -> AgentStatus {
        self.agent_status(
            PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7)),
            None,
        )
        .expect("the test pane has an agent")
    }

    /// Sample positive quiescence while the pane still exists. Cleanup after removal
    /// cannot reconstruct it from absence.
    pub(super) fn retain_closed_agent(&mut self, pane: PaneId, now: Instant) {
        self.agents.retain_closed_agent(&self.core, pane, now)
    }

    pub(super) fn invalidate_agent_lifecycle(&mut self, pane: PaneId, reason: &str) {
        self.agents
            .invalidate_agent_lifecycle(&self.core, pane, reason)
    }

    pub(super) fn warped_agent_input(
        &self,
        pane: PaneId,
    ) -> Option<yinz_types::data::WarpedAgentInput> {
        self.agents.warped_agent_input(&self.core, pane)
    }

    pub(super) fn restore_warped_agent_input(
        &mut self,
        pane: PaneId,
        input: yinz_types::data::WarpedAgentInput,
    ) {
        self.agents
            .restore_warped_agent_input(&self.core, pane, input)
    }

    #[cfg(test)]
    pub(super) fn agent_status(
        &self,
        pane_id: PaneId,
        pending_claim: Option<NotReadyReason>,
    ) -> Option<AgentStatus> {
        self.agents.agent_status(&self.core, pane_id, pending_claim)
    }

    fn complete_agent_close(&mut self, close: AgentClose) {
        self.close_pane_explicitly(close.pane, None);
        let survived = self.core.tab_containing_pane(close.pane).is_some();
        if !survived {
            self.log_and_report_session_state().non_fatal();
            self.render().non_fatal();
        }
        close.finish(&self.agents, &self.core, survived);
    }

    pub(super) fn agent_control(&mut self, request: AgentRequest, client: ClientId) {
        self.agent_control_with_reply(request, Reply::Client(client));
    }

    pub(super) fn agent_control_with_reply(&mut self, request: AgentRequest, client: Reply) {
        self.agent_control_at(request, client, Instant::now());
    }

    pub(super) fn agent_control_at(&mut self, request: AgentRequest, client: Reply, now: Instant) {
        yinz_agent_control::controller::dispatch::request(self, request, client, now);
    }

    pub(super) fn refresh_agent_controls(&mut self) {
        self.refresh_agent_controls_at(Instant::now());
    }

    pub(crate) fn refresh_agent_controls_at(&mut self, now: Instant) {
        yinz_agent_control::controller::dispatch::tick(self, now);
    }

    pub(super) fn detach_agent_controls(&mut self, client: ClientId) {
        self.agents.detach_agent_controls(&self.core, client)
    }
}

impl Screen {
    pub(in crate::screen) fn capture_agent_tracking(
        &mut self,
        pane: PaneId,
    ) -> Option<AgentTrackingHandoff> {
        self.agents.capture_agent_tracking(&self.core, pane)
    }

    /// Called immediately after placement, before queued reports or terminal bytes are drained.
    /// Returns whether a staged identity was applied, even without a tracking payload.
    pub(in crate::screen) fn restore_staged_transfer_state(&mut self, pane: PaneId) -> bool {
        let restored = self.agents.restore_transfer_state(&mut self.core, pane);
        if restored {
            self.agents.observe_restored_claude(&self.core, pane);
        }
        restored
    }
}

impl Screen {
    pub(in crate::screen) fn reconcile_restored_claude(
        &mut self,
        pane: PaneId,
        generation: Instant,
        expected: &AgentInfo,
        input_revision: u64,
        last_hook: Option<i64>,
        state: Option<AgentState>,
    ) -> Result<bool> {
        let Some(change) = self.agents.restored_claude_activity(
            &self.core,
            pane,
            generation,
            expected,
            input_revision,
            last_hook,
            state,
        ) else {
            return Ok(false);
        };
        if let Some(current) = change {
            super::agent_reports::publish_pane_agent_activity(self, pane, current)
                .context("publish restored Claude state")?;
        }
        Ok(true)
    }
}

impl yinz_agent_control::controller::dispatch::AgentSession for Screen {
    fn agents_and_host(
        &mut self,
    ) -> (
        &mut super::AgentController,
        &mut impl yinz_agent_control::host::AgentHost,
    ) {
        (&mut self.agents, &mut self.core)
    }
    fn complete_close(&mut self, close: AgentClose) {
        self.complete_agent_close(close);
    }
    fn publish_session(&mut self) {
        self.log_and_report_session_state().non_fatal();
        self.render().non_fatal();
    }
    fn publish_if_tab_names_changed(&mut self) -> Result<()> {
        if self.update_plugin_tab_names() {
            self.log_and_report_session_state()
                .context("report session state after agent tab names changed")?;
        }
        Ok(())
    }
    fn publish_panes(&mut self) -> Result<()> {
        self.report_pane_state()
            .context("report pane state after an agent report")?;
        self.render().context("render after an agent report")
    }
    fn publish_activity(&mut self, pane: PaneId, agent: AgentInfo) -> Result<()> {
        super::agent_reports::publish_pane_agent_activity(self, pane, agent).map(|_| ())
    }
    fn publish_report(&mut self, pane: PaneId, agent: AgentInfo) -> Result<()> {
        super::agent_reports::apply_pane_agent_report(self, pane, Some(agent)).map(|_| ())
    }
    fn enqueue_event(&mut self, event: AgentEvent) -> Result<()> {
        self.core
            .bus
            .senders
            .send_to_plugin(crate::plugins::PluginInstruction::Update(vec![(
                None,
                None,
                yinz_types::data::Event::AgentEvent(event),
            )]))
            .context("enqueue agent event for plugins")
    }
    fn frame_now(&self) -> Instant {
        self.frame_now()
    }
    fn retire_program_status(&mut self, pane: PaneId) {
        self.with_pane(pane, |p| {
            if let Some(term) = p.terminal_mut() {
                term.retire_program_status();
            }
        });
    }
    fn set_last_hook(&mut self, pane: PaneId, at: i64) {
        self.with_pane(pane, |p| {
            if let Some(term) = p.terminal_mut() {
                term.set_last_hook_at_ms(at);
            }
        });
    }
}
