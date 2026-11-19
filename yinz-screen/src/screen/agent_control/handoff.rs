//! Warp snapshots contain observations, never live clients or partially delivered requests.
use super::*;
use yinz_types::agent::tracking::AgentTrackingHandoff;

impl super::AgentController {
    fn restore_tracking(
        &mut self,
        host: &impl AgentHost,
        pane: PaneId,
        mut tracking: AgentTrackingHandoff,
    ) {
        let valid = host
            .pane_info(pane)
            .is_some_and(|info| info.agent.as_ref() == Some(&tracking.agent))
            && host.agent_process_matches(pane, &tracking.agent.kind)
            && tracking.lifecycle.as_ref().is_none_or(|l| {
                l.kind == tracking.agent.kind
                    && if l.conversation.is_empty() {
                        // Only Muse's verified bootstrap establishes a provisional lifecycle.
                        l.kind == "muse" && tracking.agent.session_id.is_none()
                    } else {
                        tracking.agent.session_id.as_ref() == Some(&l.conversation)
                    }
            });
        if !valid {
            log::debug!("agent handoff rejected pane={pane:?} kind={} conversation={:?} reason=identity_or_process_mismatch", tracking.agent.kind, tracking.agent.session_id);
            return;
        }
        let live = host.pane_info(pane).is_some_and(|info| {
            !info.is_held && !info.exited && tracking.agent.state != AgentState::Exited
        });
        let destination = host.agent_input_revision(pane);
        yinz_agent_control::handoff::remap_input(&mut tracking, destination);
        if live && tracking.lifecycle.is_none() {
            self.warped_agent_inputs.insert(
                pane,
                TransferredInput {
                    kind: tracking.agent.kind.clone(),
                    conversation: tracking.agent.session_id.clone().unwrap_or_default(),
                    input_revision: destination,
                    consumed: tracking.input_consumed,
                },
            );
        }
        if let Some(lifecycle) = tracking.lifecycle.filter(|_| live) {
            self.agent_lifecycle.insert(pane, lifecycle.into());
        }
        if let Some(dialog) = tracking.dialog_input.filter(|_| live) {
            self.agent_dialog_inputs.insert(pane, dialog);
        }
        if let Some(workers) = tracking.worker_waits.filter(|_| live) {
            self.agent_worker_waits.insert(pane, workers);
        }
        if let Some(permission) = tracking.permission_wait.filter(|_| live) {
            self.claude.permission_waits.insert(pane, permission);
        }
        if let Some(mut launch) = tracking.launch.take() {
            launch.pane_id = pane;
            launch.tab_id = host.tab_for_pane(pane);
            launch.created_tab_id = None;
            launch.occupied_by.clear();
            self.agent_launches.insert(pane, launch);
        }
        self.agent_submissions.insert(pane, tracking.submissions);
        if live {
            self.restore_agent_event_tracking(host, pane, tracking.events);
        }
        log::debug!(
            "agent handoff restored session={:?} pane={pane:?} kind={} conversation={:?}",
            host.session_name(),
            tracking.agent.kind,
            tracking.agent.session_id
        );
    }
}

impl super::AgentController {
    pub(in crate::screen) fn capture_agent_tracking(
        &mut self,
        host: &impl AgentHost,
        pane: PaneId,
    ) -> Option<AgentTrackingHandoff> {
        let agent = host.pane_info(pane)?.agent?;
        // Finish requests while the source pane still exists, so retained results and replies
        // describe the original target. A partially delivered prompt must remain unconsumed.
        let partial = self.agent_draft_clears.contains_key(&pane)
            || self.agent_clear_inputs.contains_key(&pane)
            || self.agent_controls.iter().any(|p| {
                p.pane == pane && (p.delivery.is_some() || p.enter_at.is_some() || p.clear)
            });
        let mut keep = Vec::new();
        for pending in std::mem::take(&mut self.agent_controls) {
            if pending.pane == pane {
                self.finish_agent_control(host, &pending, AgentOutcome::TargetChanged, None, None);
            } else {
                keep.push(pending);
            }
        }
        self.agent_controls = keep;
        if let Some(mut operation) = self.agent_draft_clears.remove(&pane) {
            if let Some((pending, _, _)) = operation.prompt.take() {
                self.finish_agent_control(host, &pending, AgentOutcome::TargetChanged, None, None);
            } else if let Some(client) = operation.client.take() {
                self.agent_reply(
                    host,
                    &client,
                    self.agent_result(host, pane, AgentOutcome::TargetChanged, None),
                );
            }
        }
        let mut lifecycle = self.agent_lifecycle.get(&pane).map(Lifecycle::handoff);
        if partial && let Some(lifecycle) = &mut lifecycle {
            lifecycle.input_revision = None;
        }
        let input_consumed = if partial {
            None
        } else {
            lifecycle
                .as_ref()
                .and_then(|l| {
                    l.input_revision
                        .map(|revision| revision == host.agent_input_revision(pane))
                })
                .or_else(|| {
                    self.warped_agent_inputs.get(&pane).and_then(|input| {
                        input.consumed.map(|consumed| {
                            consumed && input.input_revision == host.agent_input_revision(pane)
                        })
                    })
                })
        };
        Some(AgentTrackingHandoff {
            events: self.agent_event_tracking(pane),
            input_consumed,
            agent,
            input_revision: host.agent_input_revision(pane),
            lifecycle,
            dialog_input: self.agent_dialog_inputs.get(&pane).cloned(),
            worker_waits: self.agent_worker_waits.get(&pane).cloned(),
            permission_wait: self.claude.permission_waits.get(&pane).cloned(),
            launch: self.agent_launches.get(&pane).cloned(),
            submissions: self
                .agent_submissions
                .get(&pane)
                .cloned()
                .unwrap_or_default(),
        })
    }
}

impl super::AgentController {
    pub(in crate::screen) fn warped_agent_input(
        &self,
        host: &impl AgentHost,
        pane: PaneId,
    ) -> Option<yinz_types::data::WarpedAgentInput> {
        let info = host.pane_info(pane)?;
        let agent = info.agent?;
        let conversation = agent.session_id?;
        if info.is_held
            || info.exited
            || agent.state == AgentState::Exited
            || agent.kind != "codex"
            || agent.source != AgentSource::Reported
            || !host.agent_process_matches(pane, &agent.kind)
        {
            return None;
        }
        let revision = host.agent_input_revision(pane);
        let consumed = if let Some(lifecycle) = self.agent_lifecycle.get(&pane) {
            if lifecycle.kind != agent.kind || lifecycle.conversation != conversation {
                return None;
            }
            lifecycle.input_revision == Some(revision)
        } else {
            let input = self.warped_agent_inputs.get(&pane)?;
            if input.kind != agent.kind || input.conversation != conversation {
                return None;
            }
            input.consumed == Some(true) && input.input_revision == revision
        };
        Some(yinz_types::data::WarpedAgentInput {
            kind: agent.kind,
            conversation,
            consumed: consumed
                && !self.agent_draft_clears.contains_key(&pane)
                && !self
                    .agent_controls
                    .iter()
                    .any(|pending| pending.pane == pane && pending.delivery.is_some()),
        })
    }

    pub(in crate::screen) fn restore_warped_agent_input(
        &mut self,
        host: &impl AgentHost,
        pane: PaneId,
        input: yinz_types::data::WarpedAgentInput,
    ) {
        if input.kind == "codex"
            && host.pane_info(pane).is_some_and(|info| {
                !info.is_held
                    && !info.exited
                    && info.agent.is_some_and(|agent| {
                        agent.source == AgentSource::Reported
                            && agent.kind == input.kind
                            && agent.session_id.as_ref() == Some(&input.conversation)
                    })
            })
            && host.agent_process_matches(pane, &input.kind)
        {
            let revision = host.agent_input_revision(pane);
            // Adopted panes start at revision zero; a parent hook can beat this restoration.
            if let Some(lifecycle) = self.agent_lifecycle.get_mut(&pane).filter(|lifecycle| {
                lifecycle.kind == input.kind && lifecycle.conversation == input.conversation
            }) {
                if revision == 0 {
                    lifecycle.input_revision = input.consumed.then_some(0);
                }
            } else {
                self.warped_agent_inputs.insert(
                    pane,
                    TransferredInput {
                        kind: input.kind,
                        conversation: input.conversation,
                        input_revision: 0,
                        consumed: Some(input.consumed),
                    },
                );
            }
        }
    }
}

impl super::AgentController {
    pub(in crate::screen) fn stage_transfer_identity(
        &mut self,
        pane: PaneId,
        tracking: super::PendingAgentTracking,
    ) {
        self.pending_agent_tracking.insert(pane, tracking);
    }
    pub(in crate::screen) fn stage_terminal_state(&mut self, pane: PaneId, state: String) {
        self.pending_terminal_states.insert(pane, state);
    }
    pub(in crate::screen) fn has_staged_terminal(&self, pane: PaneId) -> bool {
        self.pending_terminal_states.contains_key(&pane)
    }
    pub(in crate::screen) fn discard_staged_transfer(&mut self, pane: PaneId) {
        self.pending_terminal_states.remove(&pane);
        self.pending_agent_tracking.remove(&pane);
    }
    pub(in crate::screen) fn register_spawn(&mut self, agent: yinz_types::agent::SpawnedAgent) {
        self.agent_launches.insert(agent.pane_id, agent);
    }
    pub(in crate::screen) fn forget_launch(&mut self, pane: PaneId) {
        self.agent_launches.remove(&pane);
    }
    pub(in crate::screen) fn relocate_launch(
        &mut self,
        pane: PaneId,
        tab: Option<yinz_types::internal_ids::TabId>,
    ) {
        if let Some(launch) = self.agent_launches.get_mut(&pane) {
            launch.tab_id = tab;
        }
    }
    pub(in crate::screen) fn launch_matches(
        &self,
        agent: &yinz_types::agent::SpawnedAgent,
    ) -> bool {
        self.agent_launches.get(&agent.pane_id) == Some(agent)
    }
}

impl super::AgentController {
    pub(in crate::screen) fn restore_transfer_state(
        &mut self,
        host: &mut impl AgentHost,
        pane: PaneId,
    ) -> bool {
        let restored = self
            .pending_agent_tracking
            .remove(&pane)
            .and_then(|(agent, tracking)| {
                // Apply identity first: changing it retires the destination's old OSC status.
                // Absence is part of the snapshot too: SessionEnd may have cleared the source
                // record while the destination command synthesized a new Busy record.
                self.claude.permission_waits.remove(&pane);
                let staged = self.stage_agent(host, pane, agent, false, false);
                (!matches!(staged, super::reports::AgentUpdate::Missing)).then_some(tracking)
            });
        // Restore independently of identity/tracking availability or acceptance.
        if let Some(state) = self.pending_terminal_states.remove(&pane) {
            // PTY preflight validated the complete snapshot before attaching any supervisor.
            host.restore_terminal_state(pane, &state);
        }
        let Some(tracking) = restored else {
            return false;
        };
        if let Some(tracking) = tracking {
            self.restore_tracking(host, pane, tracking);
        }
        true
    }
}

impl super::AgentController {
    pub(in crate::screen) fn observe_restored_claude(
        &mut self,
        host: &impl AgentHost,
        pane: PaneId,
    ) {
        if !self.agent_lifecycle.contains_key(&pane) {
            self.claude.restored_observers.enroll(host, pane);
        }
    }

    pub(in crate::screen) fn restored_claude_activity(
        &mut self,
        host: &impl AgentHost,
        pane: PaneId,
        generation: Instant,
        expected: &AgentInfo,
        input_revision: u64,
        last_hook: Option<i64>,
        state: Option<AgentState>,
    ) -> Option<Option<AgentInfo>> {
        let orphan = !self.agent_lifecycle.contains_key(&pane)
            && !self.agent_worker_waits.contains_key(&pane)
            && !self.claude.permission_waits.contains_key(&pane)
            && self
                .agent_submissions
                .get(&pane)
                .is_none_or(|submissions| submissions.iter().all(|s| s.result.is_some()));
        self.claude.restored_observers.activity(
            host,
            pane,
            generation,
            expected,
            input_revision,
            last_hook,
            state,
            orphan,
        )
    }
}

impl super::AgentController {
    pub(in crate::screen) fn staged_transfers(&self) -> impl Iterator<Item = PaneId> + '_ {
        self.pending_agent_tracking.keys().copied()
    }
}
