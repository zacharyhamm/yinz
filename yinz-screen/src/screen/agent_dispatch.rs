//! Coordinate agent operations that publish pane state or change the layout.
use super::{
    agent_control::{AgentClose, Completion, Reply},
    Screen,
};
use std::time::Instant;
use yinz_runtime::errors::prelude::*;
#[cfg(test)]
use yinz_types::agent::control::{AgentStatus, NotReadyReason};
use yinz_types::agent::tracking::AgentTrackingHandoff;
use yinz_types::{
    agent::{
        control::{AgentOutcome, AgentRequest, AgentResponse},
        AgentEvent, AgentInfo, AgentState, HookEvent,
    },
    internal_ids::ClientId,
    pane_id::PaneId,
};
impl Screen {
    fn reconcile_claude_activity(
        &mut self,
        pane: PaneId,
        event: &AgentEvent,
        hook: Option<&HookEvent>,
    ) {
        let Some(hook) = hook.filter(|hook| event.kind == "claude" && !hook.archive_only) else {
            return;
        };
        if !self
            .agents
            .has_agent_lifecycle(pane, "claude", &hook.session_id)
        {
            return;
        }
        self.reconcile_claude_parent(pane);
    }

    fn reconcile_claude_parent(&mut self, pane: PaneId) {
        self.agents.reconcile_claude_parent(&mut self.core, pane)
    }

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

    pub(super) fn apply_agent_report_state(
        &mut self,
        report: &yinz_types::agent::report::AgentReport,
        hook: Option<&HookEvent>,
    ) -> Result<bool> {
        self.agents
            .apply_agent_report_state(&mut self.core, report, hook)
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

    /// Tell plugins that an interrupted turn ended. Claude has no hook for it, so this is the
    /// `Interrupt` Codex reports for a cancelled turn.
    fn broadcast_interrupt(&self, pane: PaneId, agent: &AgentInfo) -> Result<()> {
        let PaneId::Terminal(terminal) = pane else {
            return Ok(());
        };
        let hook = HookEvent {
            hook_event_name: "Interrupt".into(),
            session_id: agent.session_id.clone().unwrap_or_default(),
            cwd: agent.cwd.clone(),
            ..Default::default()
        };
        let json = serde_json::to_string(&hook).context("serialize an interrupt event")?;
        let event = AgentEvent::from_hook(&agent.kind, terminal, &hook, json);
        self.core
            .bus
            .senders
            .send_to_plugin(crate::plugins::PluginInstruction::Update(vec![(
                None,
                None,
                yinz_types::data::Event::AgentEvent(event),
            )]))
    }

    #[cfg(test)]
    pub(super) fn agent_status(
        &self,
        pane_id: PaneId,
        pending_claim: Option<NotReadyReason>,
    ) -> Option<AgentStatus> {
        self.agents.agent_status(&self.core, pane_id, pending_claim)
    }

    // A stale hook record in a shell is insufficient authority to type into that shell.
    pub(super) fn agent_reply(&self, reply: &Reply, response: AgentResponse) {
        self.agents.agent_reply(&self.core, reply, response)
    }

    fn agent_result(
        &self,
        pane: PaneId,
        outcome: AgentOutcome,
        message: Option<String>,
    ) -> AgentResponse {
        self.agents.agent_result(&self.core, pane, outcome, message)
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
        if let Some(completion) = self.agents.handle(&mut self.core, request, client, now) {
            match completion {
                Completion::Close(close) => self.complete_agent_close(close),
                Completion::Refresh => self.refresh_agent_controls_at(now),
                Completion::Recover {
                    pane,
                    agent,
                    revision,
                    client,
                } => {
                    self.finalize_agent_recovery(pane, &agent, revision);
                    self.agent_reply(&client, self.agent_result(pane, AgentOutcome::Ok, None));
                },
                Completion::Visibility { pane, client } => {
                    self.log_and_report_session_state().non_fatal();
                    self.render().non_fatal();
                    self.agent_reply(&client, self.agent_result(pane, AgentOutcome::Ok, None));
                },
                Completion::Restarted { client, response } => {
                    self.refresh_agent_worktrees();
                    self.log_and_report_session_state().non_fatal();
                    self.render().non_fatal();
                    self.agent_reply(&client, *response);
                },
            }
        }
    }

    fn finalize_agent_recovery(&mut self, pane: PaneId, agent: &AgentInfo, revision: u64) {
        self.agents
            .begin_recovery(pane, revision, agent.state != AgentState::Idle);
        let state = agent.state;
        if state != AgentState::Idle {
            let mut idle = agent.clone();
            idle.state = AgentState::Idle;
            idle.detail = None;
            idle.waiting_on = None;
            idle.wait_needs_a_person = false;

            super::agent_reports::apply_pane_agent_report(self, pane, Some(idle))
                .context("reset an interrupted agent to idle")
                .non_fatal();
            self.broadcast_interrupt(pane, agent)
                .context("announce an interrupted turn to plugins")
                .non_fatal();
        }
        self.agents.finish_recovery(pane);
    }

    pub(super) fn refresh_agent_controls(&mut self) {
        self.refresh_agent_controls_at(Instant::now());
    }

    pub(crate) fn refresh_agent_controls_at(&mut self, now: Instant) {
        self.expire_claude_observations(now);
        self.refresh_claude_permission_waits();
        self.refresh_draft_clears(now);
        self.agents.advance_pending_controls(&mut self.core, now);
    }

    fn refresh_claude_permission_waits(&mut self) {
        for pane in self.agents.notification_permission_panes() {
            if let Some(parent) = self
                .agents
                .advance_notification_permission(&self.core, pane)
            {
                self.publish_claude_permission_activity(pane, parent);
            }
        }
        for (pane, wait) in self.agents.take_permission_waits() {
            if let Some(agent) = self.agents.advance_permission_wait(&self.core, pane, wait) {
                self.publish_claude_permission_activity(pane, agent);
            }
        }
    }

    fn publish_claude_permission_activity(&mut self, pane: PaneId, visible: AgentInfo) {
        let visible = self.agents.permission_activity(pane, visible);
        super::agent_reports::publish_pane_agent_activity(self, pane, visible)
            .context("clear Claude permission wait after Enter")
            .non_fatal();
    }

    /// Whether this event completed a correlated pending prompt.
    pub(super) fn agent_control_event(
        &mut self,
        event: &AgentEvent,
        hook: Option<&HookEvent>,
    ) -> bool {
        if !self.agents.observe_control_hook(&self.core, event, hook) {
            return false;
        }
        let pane = PaneId::Terminal(event.pane_id);
        if event.kind == "claude"
            && let Some(hook) = hook
        {
            self.retain_claude_evidence(pane, hook);
        }
        self.reconcile_claude_activity(pane, event, hook);
        self.correlate_agent_submissions(event, hook)
    }

    /// A recovered turn already has its lifecycle. Native evidence may still acknowledge
    /// input, even after completion, without reopening that turn or publishing it twice.
    pub(super) fn correlate_recovered_claude_submit(
        &mut self,
        event: &AgentEvent,
        hook: &HookEvent,
    ) {
        self.agents
            .correlate_recovered_claude_submit(&self.core, event, hook)
    }

    fn correlate_agent_submissions(
        &mut self,
        event: &AgentEvent,
        hook: Option<&HookEvent>,
    ) -> bool {
        self.agents
            .correlate_agent_submissions(&self.core, event, hook)
    }

    pub(super) fn detach_agent_controls(&mut self, client: ClientId) {
        self.agents.detach_agent_controls(&self.core, client)
    }
}

impl Screen {
    fn refresh_draft_clears(&mut self, now: Instant) {
        for pane in self.agents.clearing_panes() {
            if let Some((agent, revision, client)) =
                self.agents.advance_draft_clear(&mut self.core, pane, now)
            {
                self.finalize_agent_recovery(pane, &agent, revision);
                if let Some(client) = client {
                    self.agent_reply(
                        &client,
                        self.agent_result(
                            pane,
                            AgentOutcome::Ok,
                            Some(
                                "Composer verified empty; task completion remains unverified."
                                    .into(),
                            ),
                        ),
                    );
                }
            }
        }
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

impl Screen {
    pub(in crate::screen) fn accept_claude_permission(
        &mut self,
        pane: PaneId,
        hook: &mut HookEvent,
    ) -> bool {
        self.agents.accept_claude_permission(&self.core, pane, hook)
    }
}

impl Screen {
    /// Only native startup or a positive launch can establish a replacement provider.
    /// Reject other incarnations before they can affect turns, input, waits or presentation.
    pub(in crate::screen) fn accept_claude_source(
        &mut self,
        pane: PaneId,
        hook: &HookEvent,
    ) -> bool {
        self.agents.accept_claude_source(&self.core, pane, hook)
    }

    /// Called before confirmation filters/state application and again after lifecycle creation.
    pub(in crate::screen) fn retain_claude_evidence(&mut self, pane: PaneId, hook: &HookEvent) {
        if self.agents.retain_claude_evidence(pane, hook) {
            self.publish_claude_observation_activity(pane);
        }
    }

    pub(super) fn expire_claude_observations(&mut self, now: Instant) {
        for pane in self.agents.expire_claude_observations(now) {
            self.publish_claude_observation_activity(pane);
        }
    }

    fn publish_claude_observation_activity(&mut self, pane: PaneId) {
        let previous = self.get_pane_info(pane).and_then(|p| p.agent);
        self.reconcile_claude_parent(pane);
        let current = self.get_pane_info(pane).and_then(|p| p.agent);
        if previous != current
            && let Some(previous) = previous
            && let Some(current) = current
        {
            if let Some(tab) = self.tab_containing_pane_mut(pane) {
                tab.set_pane_agent(pane, Some(previous));
            }
            super::agent_reports::publish_pane_agent_activity(self, pane, current)
                .context("publish Claude notification observation activity")
                .non_fatal();
        }
    }
}
