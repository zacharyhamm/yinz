//! Warp snapshots contain observations, never live clients or partially delivered requests.
use super::*;
use yinz_types::agent::tracking::AgentTrackingHandoff;

impl Screen {
    pub(in crate::screen) fn capture_agent_tracking(
        &mut self,
        pane: PaneId,
    ) -> Option<AgentTrackingHandoff> {
        let agent = self.get_pane_info(pane)?.agent?;
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
                self.finish_agent_control(&pending, AgentOutcome::TargetChanged, None, None);
            } else {
                keep.push(pending);
            }
        }
        self.agent_controls = keep;
        if let Some(mut operation) = self.agent_draft_clears.remove(&pane) {
            if let Some((pending, _, _)) = operation.prompt.take() {
                self.finish_agent_control(&pending, AgentOutcome::TargetChanged, None, None);
            } else if let Some(client) = operation.client.take() {
                self.agent_reply(
                    &client,
                    self.agent_result(pane, AgentOutcome::TargetChanged, None),
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
                        .map(|revision| revision == self.agent_input_revision(pane))
                })
                .or_else(|| {
                    self.warped_agent_inputs.get(&pane).and_then(|input| {
                        input.consumed.map(|consumed| {
                            consumed && input.input_revision == self.agent_input_revision(pane)
                        })
                    })
                })
        };
        Some(AgentTrackingHandoff {
            events: self.agent_event_tracking(pane),
            input_consumed,
            agent,
            input_revision: self.agent_input_revision(pane),
            lifecycle,
            dialog_input: self.agent_dialog_inputs.get(&pane).cloned(),
            worker_waits: self.agent_worker_waits.get(&pane).cloned(),
            permission_wait: self.claude_permission_waits.get(&pane).cloned(),
            launch: self.agent_launches.get(&pane).cloned(),
            submissions: self
                .agent_submissions
                .get(&pane)
                .cloned()
                .unwrap_or_default(),
        })
    }

    /// Called immediately after placement, before queued reports or terminal bytes are drained.
    /// Returns whether a staged identity was applied, even without a tracking payload.
    pub(in crate::screen) fn restore_staged_agent_tracking(&mut self, pane: PaneId) -> bool {
        let Some((agent, tracking)) = self.pending_agent_tracking.remove(&pane) else {
            return false;
        };
        // Absence is part of the snapshot too: SessionEnd may have cleared the source
        // record while the destination command synthesized a new Busy record.
        match super::super::agent_reports::stage_pane_agent_report(self, pane, agent)
            .context("restore adopted agent identity")
        {
            Ok(true) => {},
            Ok(false) => return false,
            Err(error) => {
                Err::<(), _>(error).non_fatal();
                return false;
            },
        }
        let Some(mut tracking) = tracking else {
            return true;
        };
        let valid = self
            .get_pane_info(pane)
            .is_some_and(|info| info.agent.as_ref() == Some(&tracking.agent))
            && self.agent_process_matches(pane, &tracking.agent.kind)
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
            return true;
        }
        let live = self.get_pane_info(pane).is_some_and(|info| {
            !info.is_held && !info.exited && tracking.agent.state != AgentState::Exited
        });
        let source = tracking.input_revision;
        let destination = self.agent_input_revision(pane);
        // A non-current revision is deliberately unequal even at revision zero. Unknown
        // evidence stays None. No input timestamps from another process are restored.
        let remap = |revision| {
            if revision == source {
                destination
            } else {
                destination.wrapping_sub(1)
            }
        };
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
        if let Some(mut lifecycle) = tracking.lifecycle.filter(|_| live) {
            lifecycle.input_revision = lifecycle.input_revision.map(remap);
            // Permission revisions are ordered thresholds, not consumption evidence:
            // input delivered after restoration must exceed them, while earlier
            // destination input must not clear a transferred wait.
            for notification in lifecycle.claude_notifications.values_mut() {
                notification.permission_revision =
                    notification.permission_revision.map(|_| destination);
            }
            self.agent_lifecycle.insert(pane, lifecycle.into());
        }
        if let Some(mut dialog) = tracking.dialog_input.filter(|_| live) {
            dialog.input_revision = remap(dialog.input_revision);
            self.agent_dialog_inputs.insert(pane, dialog);
        }
        if let Some(workers) = tracking.worker_waits.filter(|_| live) {
            self.agent_worker_waits.insert(pane, workers);
        }
        if let Some(mut permission) = tracking.permission_wait.filter(|_| live) {
            permission.input_revision = destination;
            self.claude_permission_waits.insert(pane, permission);
        }
        if let Some(mut launch) = tracking.launch.take() {
            launch.pane_id = pane;
            launch.tab_id = self
                .tabs
                .iter()
                .find(|(_, tab)| tab.has_pane_with_pid(&pane))
                .map(|(id, _)| *id);
            launch.created_tab_id = None;
            launch.occupied_by.clear();
            self.agent_launches.insert(pane, launch);
        }
        self.agent_submissions.insert(pane, tracking.submissions);
        if live {
            self.restore_agent_event_tracking(pane, tracking.events);
        }
        log::debug!(
            "agent handoff restored session={:?} pane={pane:?} kind={} conversation={:?}",
            self.session_name,
            tracking.agent.kind,
            tracking.agent.session_id
        );
        true
    }
}
