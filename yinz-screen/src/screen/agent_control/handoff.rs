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
    pub(in crate::screen) fn restore_staged_transfer_state(&mut self, pane: PaneId) -> bool {
        let restored = self.restore_transfer_state(pane);
        if restored {
            self.observe_restored_claude(pane);
        }
        restored
    }

    fn restore_transfer_state(&mut self, pane: PaneId) -> bool {
        let restored = self
            .pending_agent_tracking
            .remove(&pane)
            .and_then(|(agent, tracking)| {
                // Apply identity first: changing it retires the destination's old OSC status.
                // Absence is part of the snapshot too: SessionEnd may have cleared the source
                // record while the destination command synthesized a new Busy record.
                match super::super::agent_reports::stage_pane_agent_report(self, pane, agent)
                    .context("restore adopted agent identity")
                {
                    Ok(true) => Some(tracking),
                    Ok(false) => None,
                    Err(error) => {
                        Err::<(), _>(error).non_fatal();
                        None
                    },
                }
            });
        // Restore independently of identity/tracking availability or acceptance.
        if let Some(state) = self.pending_terminal_states.remove(&pane) {
            // PTY preflight validated the complete snapshot before attaching any supervisor.
            for tab in self.tabs.values_mut() {
                if let Some(terminal) = tab.get_pane_with_id_mut(pane) {
                    terminal
                        .restore_terminal_state(&state)
                        .map_err(|e| anyhow!("{e}"))
                        .context("restore adopted terminal state")
                        .non_fatal();
                    break;
                }
            }
        }
        let Some(tracking) = restored else {
            return false;
        };
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

    fn observe_restored_claude(&mut self, pane: PaneId) {
        let PaneId::Terminal(terminal) = pane else {
            return;
        };
        let Some(info) = self.get_pane_info(pane) else {
            return;
        };
        let Some(expected) = info
            .agent
            .filter(|agent| agent.kind == "claude" && agent.source == AgentSource::Reported)
        else {
            return;
        };
        let Some(conversation) = expected.session_id.clone().filter(|id| !id.is_empty()) else {
            return;
        };
        if info.host.is_some()
            || info.is_held
            || info.exited
            || self.agent_lifecycle.contains_key(&pane)
        {
            return;
        }
        let Some(os) = &self.bus.os_input else { return };
        let Some(sender) = self.bus.senders.to_screen.clone() else {
            return;
        };
        let input_revision = self.agent_input_revision(pane);
        let generation = Instant::now();
        self.restored_claude_observers.insert(pane, generation);
        os.observe_restored_claude(
            terminal,
            conversation,
            Box::new(move |state| {
                let (reply, response) = std::sync::mpsc::channel();
                if sender
                    .send(
                        yinz_bus::screen::ScreenInstruction::ReconcileRestoredClaude {
                            pane,
                            generation,
                            expected: expected.clone(),
                            input_revision,
                            last_hook: info.last_hook_at_ms,
                            state,
                            reply,
                        },
                    )
                    .is_err()
                {
                    return false;
                }
                response
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap_or(false)
            }),
        )
        .context("observe adopted Claude state")
        .non_fatal();
    }

    pub(in crate::screen) fn reconcile_restored_claude(
        &mut self,
        pane: PaneId,
        generation: Instant,
        expected: &AgentInfo,
        input_revision: u64,
        last_hook: Option<i64>,
        state: Option<AgentState>,
    ) -> Result<bool> {
        if self.restored_claude_observers.get(&pane) != Some(&generation) {
            return Ok(false);
        }
        let current =
            self.get_pane_info(pane)
                .filter(|info| {
                    !info.is_held
                        && !info.exited
                        && info.host.is_none()
                        && info.last_hook_at_ms == last_hook
                        && self.agent_input_revision(pane) == input_revision
                        && !self.agent_lifecycle.contains_key(&pane)
                        && !self.agent_worker_waits.contains_key(&pane)
                        && !self.claude_permission_waits.contains_key(&pane)
                        && self.agent_submissions.get(&pane).is_none_or(|submissions| {
                            submissions.iter().all(|s| s.result.is_some())
                        })
                })
                .and_then(|info| info.agent)
                .filter(|agent| {
                    agent.kind == expected.kind
                        && agent.session_id == expected.session_id
                        && agent.run_id == expected.run_id
                        && agent.source == expected.source
                        // The OS observer validates the foreground group and Claude's
                        // process instance; invoked_with may still name the shell.
                        && agent.state != AgentState::Exited
                });
        let Some(mut current) = current else {
            self.restored_claude_observers.remove(&pane);
            return Ok(false);
        };
        if let Some(state) = state.filter(|state| {
            matches!(
                state,
                AgentState::Idle | AgentState::Busy | AgentState::Waiting
            )
        }) && current.state != state
        {
            current.state = state;
            current.detail = None;
            current.waiting_on =
                (state == AgentState::Waiting).then(|| "approve Claude permission request".into());
            current.wait_needs_a_person = state == AgentState::Waiting;
            super::super::agent_reports::publish_pane_agent_activity(self, pane, current)
                .context("publish restored Claude state")?;
        }
        Ok(true)
    }
}
